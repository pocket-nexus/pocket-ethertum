// Runs a compiled interface bundle on the host: PocketJS's wasm core and
// software rasterizer, with a renderer that exists only as state (`Mock`).
// It answers commands the way a device's renderer does (`crates/ether-interface`
// `Session`), so a test can press buttons and touch the panel and read what the
// interface drew and asked for.
import { existsSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { createCanvas, ImageData, loadImage, type Canvas } from "../../vendor/pocketjs/node_modules/@napi-rs/canvas";
import { PROP } from "../../vendor/pocketjs/contracts/spec/spec.ts";
import { __packTouch, createTouchHitFacts } from "../../vendor/pocketjs/framework/src/touch.ts";
import { createWasmUi } from "../../vendor/pocketjs/hosts/web/wasm-ops.js";
import type { Command, HostState, Slot } from "../app/protocol.ts";

const root = resolve(import.meta.dir, "../..");
export type Device = "vita";

const slot = (kind: 0 | 1 | 2, id: number, count = 0): Slot => ({ kind, id, count });

/** A device's renderer, reduced to the state the interface sees. */
export class Mock {
  state: HostState = {
    mode: "title", message: "", progress: 100,
    materials: [
      { id: 22, name: "Stone", picture: 21 }, { id: 1, name: "Dirt", picture: 0 }, { id: 12, name: "Grass", picture: 11 },
      { id: 19, name: "Sand", picture: 18 }, { id: 13, name: "Log", picture: 12 }, { id: 10, name: "Cobble", picture: 9 },
    ],
    items: [{ id: 7, name: "Pickaxe", picture: 6 }, { id: 1, name: "Apple", picture: 0 }],
    hotbar: [slot(1, 22), slot(1, 1), slot(1, 12), slot(1, 19), slot(1, 13), slot(1, 10), slot(2, 7), slot(2, 1, 12), slot(0, 0)],
    slot: 0, health: 20, healthMax: 20, flying: false, sprinting: false,
    brush: { size: 4, strength: 80, shape: 0, material: 10 }, camera: 0, mods: 1,
    options: [
      { key: "view", value: 2, choices: ["2", "3", "4", "5", "6"] }, { key: "fov", value: 1, choices: ["70°", "85°", "100°"] },
      { key: "look", value: 1, choices: ["Slow", "Normal", "Fast"] }, { key: "invert", value: 0 },
      { key: "day", value: 1, choices: ["Stopped", "24 min", "6 min"] }, { key: "unfly", value: 1 }, { key: "stats", value: 0 },
    ],
    stats: "", prefs: "", woke: 0, t: [216, 125, 384, -77],
  };
  /** Every command received, oldest first. */
  log: Command[] = [];
  /** The interface said it has nothing scheduled. */
  idle = false;
  /** Seconds this renderer has run, and what the interface asked to be woken for. */
  private seconds = 0;
  private wakes: { id: number; due: number }[] = [];
  private sent: Partial<HostState> = {};

  /** One frame of the renderer's clock: a wake that is due is said back. */
  tick(dt: number) {
    this.seconds += dt;
    const at = this.wakes.findIndex((wake) => wake.due <= this.seconds);
    if (at >= 0) this.state.woke = this.wakes.splice(at, 1)[0].id;
  }

  receive(command: Command) {
    this.log.push(command);
    const s = this.state;
    switch (command.type) {
      case "start": if (s.mode === "title") s.mode = "play"; break;
      case "menu":
        if (s.mode === "play" && command.on) s.mode = "menu";
        else if (s.mode === "menu" && !command.on) s.mode = "play";
        break;
      case "title": if (s.mode === "play" || s.mode === "menu") s.mode = "title"; break;
      case "option":
        s.options = s.options.map((setting) => (setting.key === command.key ? { ...setting, value: command.value } : setting));
        s.stats = s.options.find((setting) => setting.key === "stats")?.value ? "59.9 fps · 16.7 ms · late 0 · 214 chunks · 96k tris" : "";
        break;
      case "brush": s.brush = { size: command.size, strength: command.strength, shape: command.shape, material: command.material }; break;
      case "slot": s.slot = Math.min(8, command.index); break;
      case "prefs": s.prefs = command.value; break;
      case "idle": this.idle = command.on; break;
      case "wake": this.wakes.push({ id: command.id, due: this.seconds + command.seconds }); break;
    }
  }

  /** The line of state members to deliver, or undefined while nothing changed. */
  poll(): string | undefined {
    const changed: Record<string, unknown> = {};
    for (const [key, value] of Object.entries(this.state)) {
      const text = JSON.stringify(value);
      if (JSON.stringify((this.sent as Record<string, unknown>)[key]) === text) continue;
      changed[key] = (this.sent as Record<string, unknown>)[key] = JSON.parse(text);
    }
    return Object.keys(changed).length ? JSON.stringify({ type: "state", value: changed }) : undefined;
  }
}

const VIEW: Record<Device, { w: number; h: number; density: number }> = {
  vita: { w: 480, h: 272, density: 2 },
};

export interface Rig {
  mock: Mock;
  view: (typeof VIEW)[Device];
  /** Advance `frames`, holding `buttons`; `touch` is the contacts on the panel. */
  step(frames?: number, input?: { buttons?: number; touch?: { x: number; y: number; id?: number }[] }): void;
  /** Press and release. */
  press(buttons: number): void;
  tap(x: number, y: number): void;
  /** The screen over `backdrop` (a picture, or drawn by the function), at the device's density. */
  shot(backdrop?: string | ((ctx: CanvasRenderingContext2D, w: number, h: number) => void)): Promise<Canvas>;
}

/** Boots the bundle `bun tools/ui.ts <device>` wrote. One per process: the bundle owns `globalThis.frame`. */
export async function boot(device: Device): Promise<Rig> {
  const view = VIEW[device];
  const directory = join(root, ".pocket-build/ui", device);
  const wasmPath = join(root, "vendor/pocketjs/hosts/web/pocketjs.wasm");
  if (!existsSync(wasmPath)) throw new Error("run `bun tools/wasm.ts` in vendor/pocketjs first");
  const wasm = await createWasmUi(readFileSync(wasmPath), { width: view.w, height: view.h, rasterDensity: view.density });
  const mock = new Mock();
  const globals = globalThis as Record<string, any>;
  const pending: string[] = [];
  Object.assign(wasm.ops, {
    svcOpen: (name: string) => name === "pocket.overlay",
    svcPoll: () => mock.poll(),
    svcSend: (line: string) => pending.push(line),
  });
  globals.ui = wasm.ops;
  globals.__pak = readFileSync(join(directory, "ether.pak")).buffer;
  (0, eval)(readFileSync(join(directory, "ether.js"), "utf8"));

  const ops = wasm.ops as any;
  const facts = createTouchHitFacts((x, y) => ops.hitTestBounds(x, y));
  const frame = (buttons: number, touch: { x: number; y: number; id?: number }[]) => {
    mock.tick(1 / 60);
    const packed = touch.map((t) => __packTouch(t.id ?? 1, t.x, t.y));
    globals.frame(buttons, undefined, packed, facts(packed), packed.map(() => 0));
    for (const line of pending.splice(0)) mock.receive(JSON.parse(line));
    wasm.tick();
  };

  // The rasterizer has no alpha; render over black and over white and take
  // the difference (the root's own colour is the only thing that changes).
  const matte = (render: () => Uint8Array, rootNode: number, w: number, h: number, scale: number): Canvas => {
    ops.setProp(rootNode, PROP.bgColor, 0xff000000);
    const black = render().slice();
    ops.setProp(rootNode, PROP.bgColor, 0xffffffff);
    const white = render().slice();
    ops.setProp(rootNode, PROP.bgColor, 0);
    const pixels = new Uint8ClampedArray(black.length);
    for (let i = 0; i < black.length; i += 4) {
      const alpha = 255 - (white[i + 1] - black[i + 1]);
      pixels[i + 3] = alpha;
      for (let c = 0; c < 3; c++) pixels[i + c] = alpha ? Math.min(255, (black[i + c] * 255) / alpha) : 0;
    }
    const canvas = createCanvas(w * scale, h * scale);
    canvas.getContext("2d").putImageData(new ImageData(pixels, w * scale, h * scale), 0, 0);
    return canvas;
  };

  return {
    mock, view,
    step(frames = 1, input = {}) {
      for (let i = 0; i < frames; i++) frame(input.buttons ?? 0, input.touch ?? []);
    },
    press(buttons) {
      frame(buttons, []);
      frame(buttons, []);
      frame(0, []);
    },
    tap(x, y) {
      frame(0, [{ x, y }]);
      frame(0, [{ x, y }]);
      frame(0, []);
      frame(0, []);
    },
    async shot(backdrop) {
      const scale = view.density;
      const top = matte(() => wasm.renderScaled(scale), 1, view.w, view.h, scale);
      const canvas = createCanvas(view.w * scale, view.h * scale);
      const ctx = canvas.getContext("2d");
      ctx.fillStyle = "#000000";
      ctx.fillRect(0, 0, canvas.width, canvas.height);
      if (typeof backdrop === "function") backdrop(ctx as unknown as CanvasRenderingContext2D, canvas.width, canvas.height);
      else if (backdrop && existsSync(backdrop)) {
        // The world behind the interface: the capture cropped to this screen's shape.
        const image = await loadImage(backdrop);
        const width = Math.min(image.width, (image.height * view.w) / view.h), height = (width * view.h) / view.w;
        ctx.drawImage(image, (image.width - width) / 2, (image.height - height) / 2, width, height, 0, 0, view.w * scale, view.h * scale);
      }
      ctx.drawImage(top, 0, 0);
      return canvas;
    },
  };
}
