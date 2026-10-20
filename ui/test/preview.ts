// Writes pictures of the interface on one device to .pocket-build/ui/preview/,
// and the Vita's in one contact sheet, .pocket-build/ui/sheet-vita.png.
//   bun ui/test/preview.ts vita     (after `bun tools/ui.ts vita`)
// Behind the interface stands `.pocket-build/ui/backdrops/scene.png` when there
// is one (a capture of the world), else a drawn sky over hills: what matters is
// the interface over light and dark ground.
import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { BTN } from "../../vendor/pocketjs/contracts/spec/spec.ts";
import { boot, type Device } from "./harness.ts";

const device = (process.argv[2] ?? "vita") as Device;
const build = resolve(import.meta.dir, "../../.pocket-build/ui");
const out = join(build, "preview");
mkdirSync(out, { recursive: true });
const rig = await boot(device);
const capture = join(build, "backdrops/scene.png");

function drawn(ctx: CanvasRenderingContext2D, w: number, h: number) {
  const sky = ctx.createLinearGradient(0, 0, 0, h * 0.6);
  sky.addColorStop(0, "#3d7fd1");
  sky.addColorStop(1, "#cfe6fb");
  ctx.fillStyle = sky;
  ctx.fillRect(0, 0, w, h);
  const hill = (base: number, amp: number, color: string, phase: number) => {
    ctx.fillStyle = color;
    ctx.beginPath();
    ctx.moveTo(0, h);
    for (let x = 0; x <= w; x += 8) ctx.lineTo(x, h * base + Math.sin(x / (w / 5) + phase) * amp + Math.sin(x / (w / 13) + phase * 2) * amp * 0.35);
    ctx.lineTo(w, h);
    ctx.fill();
  };
  hill(0.52, h * 0.06, "#8b9298", 0.4);
  hill(0.62, h * 0.07, "#5f7f3b", 1.7);
  hill(0.78, h * 0.05, "#4c6a2c", 3.1);
  ctx.fillStyle = "#e9edef";
  ctx.fillRect(w * 0.7, h * 0.66, w * 0.3, h * 0.34);
}
const backdrop = Bun.file(capture).size ? capture : drawn;

const names: string[] = [];
const save = async (name: string) => {
  const file = join(out, `${device}-${name}.png`);
  writeFileSync(file, (await rig.shot(backdrop)).toBuffer("image/png"));
  names.push(file);
};
const press = (index: number) => {
  for (let i = 0; i < index; i++) rig.press(BTN.DOWN);
  rig.press(BTN.CIRCLE);
};
const back = () => rig.press(BTN.CROSS);

// Loading, and a failure.
rig.mock.state.mode = "loading";
rig.mock.state.message = "Generating chunks: 412 of 1 089";
rig.mock.state.progress = 38;
rig.step(12);
await save("loading");
rig.mock.state.mode = "error";
rig.mock.state.message = "ether.pack is missing: run `bun tools/vita.ts sync`";
rig.step(8);
await save("error");
rig.mock.state.mode = "title";
rig.mock.state.message = "";
rig.step(20);
await save("title");
press(1);
rig.step(12);
await save("title-settings");
back();
rig.step(6);
press(2);
rig.step(10);
await save("controls");
back();
rig.step(6);
press(3);
rig.step(10);
await save("about");
back();
rig.step(6);
// Play.
press(0);
rig.step(20);
await save("play-start");
rig.step(420);
rig.mock.state.flying = true;
rig.mock.state.sprinting = true;
rig.mock.state.slot = 2;
rig.step(12);
await save("play-slot");
rig.step(150);
rig.mock.state.health = 5;
rig.mock.state.flying = false;
rig.mock.state.options = rig.mock.state.options.map((o) => (o.key === "stats" ? { ...o, value: 1 } : o));
rig.mock.state.stats = "59.9 fps · 16.7 ms · late 0 · 214 chunks · 96k tris";
rig.mock.state.t = [567, 1254, 384, -777];
rig.step(12);
await save("play-stats");
rig.mock.state.health = 20;
rig.press(BTN.START);
rig.step(14);
await save("menu");
press(1);
rig.step(10);
rig.press(BTN.DOWN);
rig.press(BTN.DOWN);
rig.press(BTN.DOWN);
rig.step(8);
await save("brush");
back();
rig.step(6);
press(2);
rig.step(10);
await save("settings");

const sheet = join(build, `sheet-${device}.png`);
Bun.spawnSync(["magick", "montage", ...names, "-tile", "3x", "-geometry", "50%x50%+4+4", "-background", "#333333", sheet]);
console.log(sheet);
console.log(JSON.stringify(rig.mock.log.filter((command) => !["idle", "wake"].includes(command.type))));
