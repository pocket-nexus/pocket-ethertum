// The game as the interface sees it, on any device: which list is up, what
// its rows do, the brush as the menu edits it, and what is kept between runs
// (the settings).
import { createEffect, createMemo, createSignal, on, onCleanup, untrack, type Accessor } from "solid-js";
import { after } from "@pocketjs/framework/clock";
import { glyph } from "@pocketjs/framework/modality";
import { ICONS, TILES, VERSION } from "./generated/pictures.ts";
import type { Host } from "./host.ts";
import type { Brush, Entry, Mode, Setting } from "./protocol.ts";

/** What the settings a renderer may offer are called. */
const LABELS: Record<string, string> = {
  view: "View distance",
  fov: "Field of view",
  look: "Look speed",
  invert: "Invert look",
  day: "Length of a day",
  unfly: "Landing ends flight",
  stats: "Statistics",
  msaa: "Multisampling",
};

/**
 * A row of a list. What can change while the row stands (a switch, a named
 * value, a label that says what a press would do) is a function: a list is
 * built once, kept, and its rows are written in place.
 */
export interface Row {
  label: string | (() => string);
  /** A switch's state, or undefined for a row with a named value or none. */
  on?: () => boolean;
  /** The named value of a choice, or what stands at the right of the row. */
  value?: string | (() => string);
  /** A picture at the left of the value (a material's tile). */
  picture?: () => string;
  /** A row that only informs takes no press and no focus mark. */
  press?: () => void;
  /** The d-pad's left and right on the focused row: one step down or up. */
  adjust?: (by: number) => void;
}

/** What a row's member reads now. */
export function read<T extends string | boolean>(member: T | (() => T)): T {
  return typeof member === "function" ? member() : member;
}

/** The list up: the mode's own, or one opened from it. */
export type Sheet = "main" | "brush" | "settings" | "controls" | "about";

export interface Game {
  host: Host;
  mode: Accessor<Mode>;
  /** True while a list is up and the pad is the interface's. */
  listing: Accessor<boolean>;
  sheet: Accessor<Sheet>;
  open(sheet: Sheet): void;
  heading: Accessor<string>;
  rows: Accessor<Row[]>;
  /** One level up: a list opened from another closes; the menu leaves for play. */
  back(): void;
  backLabel: Accessor<string>;
  menu(): void;
  /** What the strip under a list says at its left: the version on the title, the brush under the menu. */
  notice: Accessor<string>;
  /** The title's own rows and the menu's: a presentation may build them before either is first shown. */
  lists: { title: Row[]; menu: Row[] };
  /** A material by id, and its picture. */
  material(id: number): Entry | undefined;
}

/** How many fades are playing: while one is, a timer of the guest's own is pending. */
const [settling, setSettling] = createSignal(0);
/** Seconds a fade takes to play, and a turn more. */
const SETTLE = 0.4;
let wakes = 0;
const waking = new Map<number, () => void>();

/** The guest takes every turn it is offered until what just changed has faded in or out. */
function settle() {
  setSettling((count) => count + 1);
  after(SETTLE, () => setSettling((count) => count - 1));
}

/**
 * True for `seconds` after each `show()`. The seconds are counted by the
 * renderer (`wake`), not by a timer here: a timer of the guest's counts its
 * turns, so one that is pending makes the device take every turn it offers
 * for as long as a hint stands. Between the two fades nothing is scheduled.
 */
export function createPulse(host: Host, seconds: number): [Accessor<boolean>, () => void] {
  const [shown, setShown] = createSignal(false);
  let id = 0;
  onCleanup(() => waking.delete(id));
  return [shown, () => {
    waking.delete(id);
    id = ++wakes;
    waking.set(id, () => {
      setShown(false);
      settle();
    });
    host.send({ type: "wake", id, seconds });
    if (untrack(shown)) return;
    setShown(true);
    settle();
  }];
}

/** "09:36" */
export function clock(minutes: number): string {
  const h = Math.floor(minutes / 60) % 24, m = Math.floor(minutes) % 60;
  return `${h < 10 ? "0" : ""}${h}:${m < 10 ? "0" : ""}${m}`;
}

/** Tenths of a voxel as "-12.3". */
export function tenths(value: number): string {
  const sign = value < 0 ? "-" : "";
  const v = Math.abs(Math.round(value));
  return `${sign}${Math.floor(v / 10)}.${v % 10}`;
}

/** The picture of a diffuse atlas tile, or of an item icon. `tools/ui.ts` compiles both and lists
 *  their names as literals, which is how the compiler finds the pictures to bake. */
export function tilePicture(tile: number): string {
  return TILES[Math.max(0, Math.min(TILES.length - 1, tile))];
}
export function iconPicture(icon: number): string {
  return ICONS[Math.max(0, Math.min(ICONS.length - 1, icon))];
}

/** The pad in play, each control with what it does (`crates/ether-interface` docs). */
export function controls(): [string, string][] {
  return [
    ["Left stick", "Move"],
    ["Right stick", "Look"],
    [glyph("cross"), "Jump; twice: fly, land"],
    [glyph("circle"), "Sneak; descend in flight"],
    [glyph("triangle"), "Sprint"],
    [`${glyph("rtrigger")}  ${glyph("ltrigger")}`, "Break, place"],
    ["D-pad", "Left, right: hotbar · up, down: brush"],
    [glyph("square"), "Camera"],
    [glyph("select"), "Statistics"],
    [glyph("start"), "Menu"],
  ];
}

interface Prefs {
  options?: Record<string, number>;
}

const SHAPES = ["Isosurface", "Cube"];

export function createGame(host: Host): Game {
  const [sheet, setSheet] = createSignal<Sheet>("main");
  const [prefs, setPrefs] = createSignal<Prefs>({});
  const mode = host.mode;
  const listing = createMemo(() => mode() === "title" || mode() === "menu");

  // Each mode opens on its own list.
  createEffect(on(mode, () => setSheet("main")));
  // With no pulse showing nothing is scheduled here: the renderer may skip turns until it has news.
  createEffect(() => host.ready() && host.send({ type: "idle", on: settling() === 0 }));
  // A showing of a pulse is over when the renderer says its id.
  createEffect(on(host.woke, (id) => {
    const over = waking.get(id);
    waking.delete(id);
    over?.();
  }, { defer: true }));

  // What the renderer stored for us last time: the settings, which it is told again once it has listed them.
  let restored = false;
  createEffect(() => {
    if (!host.ready() || restored || !host.options().length) return;
    restored = true;
    let stored: Prefs = {};
    try {
      stored = JSON.parse(untrack(host.prefs) || "{}") as Prefs;
    } catch {
      // A damaged file starts over.
    }
    setPrefs(stored);
    for (const setting of untrack(host.options)) {
      const value = stored.options?.[setting.key];
      if (typeof value === "number" && value !== setting.value) host.send({ type: "option", key: setting.key, value });
    }
  });
  const set = (setting: Setting, value: number) => {
    if (value === setting.value) return;
    host.send({ type: "option", key: setting.key, value });
    const next = { ...prefs(), options: { ...prefs().options, [setting.key]: value } };
    setPrefs(next);
    host.send({ type: "prefs", value: JSON.stringify(next) });
  };
  // A setting's row stands while the renderer offers the setting: its state is read where it is shown.
  const row = (key: string, choice: boolean): Row => {
    const label = LABELS[key] ?? key;
    const now = () => host.options().find((setting) => setting.key === key);
    if (!choice) {
      const flip = () => {
        const setting = now();
        if (setting) set(setting, setting.value ? 0 : 1);
      };
      return { label, on: () => !!now()?.value, press: flip, adjust: (by) => {
        const setting = now();
        if (setting) set(setting, by > 0 ? 1 : 0);
      } };
    }
    return {
      label,
      value: () => now()?.choices?.[now()!.value] ?? "",
      press: () => {
        const setting = now();
        if (setting?.choices) set(setting, (setting.value + 1) % setting.choices.length);
      },
      adjust: (by) => {
        const setting = now();
        if (setting?.choices) set(setting, Math.max(0, Math.min(setting.choices.length - 1, setting.value + by)));
      },
    };
  };
  // Which settings the renderer offers, each with whether it is a choice: the list made of them is
  // built again only when this changes, not when a setting's value does.
  const offered = createMemo(() => host.options().map((setting) => `${setting.key}${setting.choices ? ":" : ""}`).join(" "));
  const settings = createMemo<Row[]>(() => offered().split(" ").filter(Boolean).map((key) => row(key.replace(":", ""), key.endsWith(":"))));

  // The brush: what the menu sets is sent whole.
  const material = (id: number) => host.materials().find((entry) => entry.id === id);
  const brush = (change: Partial<Brush>) => host.send({ type: "brush", ...host.brush(), ...change });
  const step = (list: number[], now: number, by: number) => list[(Math.max(0, list.indexOf(now)) + by + list.length) % list.length];
  const brushRows: Row[] = [
    {
      label: "Size",
      value: () => `${host.brush().size}`,
      press: () => brush({ size: (host.brush().size + 1) % 21 }),
      adjust: (by) => brush({ size: Math.max(0, Math.min(20, host.brush().size + by)) }),
    },
    {
      label: "Strength",
      value: () => `${host.brush().strength}%`,
      press: () => brush({ strength: host.brush().strength >= 100 ? 0 : Math.min(100, host.brush().strength + 10) }),
      adjust: (by) => brush({ strength: Math.max(0, Math.min(100, host.brush().strength + by * 10)) }),
    },
    {
      label: "Shape",
      value: () => SHAPES[host.brush().shape] ?? SHAPES[0],
      press: () => brush({ shape: host.brush().shape ? 0 : 1 }),
      adjust: (by) => brush({ shape: by > 0 ? 1 : 0 }),
    },
    {
      label: "Material",
      value: () => material(host.brush().material)?.name ?? `${host.brush().material}`,
      picture: () => tilePicture(material(host.brush().material)?.picture ?? host.brush().material - 1),
      press: () => brush({ material: step(host.materials().map((entry) => entry.id), host.brush().material, 1) }),
      adjust: (by) => brush({ material: step(host.materials().map((entry) => entry.id), host.brush().material, by) }),
    },
  ];

  const title: Row[] = [
    { label: "Play", press: () => host.send({ type: "start" }) },
    { label: "Settings", press: () => setSheet("settings") },
    { label: "Controls", press: () => setSheet("controls") },
    { label: "About", press: () => setSheet("about") },
  ];
  const leave = () => host.send({ type: "menu", on: false });
  const menu: Row[] = [
    { label: "Resume", press: leave },
    { label: "Brush", press: () => setSheet("brush") },
    { label: "Settings", press: () => setSheet("settings") },
    { label: "Controls", press: () => setSheet("controls") },
    { label: "Back to the title", press: () => host.send({ type: "title" }) },
  ];
  const explained: Row[] = controls().map(([label, value]) => ({ label, value }));
  const about: Row[] = [
    { label: "Game", value: "Ethertum, Dreamtowards" },
    { label: "License", value: "GPL-3.0" },
    { label: "Source", value: "github.com/Dreamtowards/Ethertum" },
    { label: "Engine", value: "Pocket3D" },
    { label: "Sky", value: "Poly Haven, CC0" },
    { label: "Version", value: VERSION },
  ];
  // During play no list is up: the rows stay what they were, so nothing is rebuilt under the hotbar.
  const rows = createMemo<Row[]>((before) => {
    if (!listing()) return before;
    if (sheet() === "brush") return brushRows;
    if (sheet() === "settings") return settings();
    if (sheet() === "controls") return explained;
    if (sheet() === "about") return about;
    return mode() === "title" ? title : menu;
  }, []);

  const mods = () => `${host.mods()} mod${host.mods() === 1 ? "" : "s"} loaded`;
  const notice = createMemo<string>((before) => {
    if (mode() === "title") return `v${VERSION} · ${mods()}`;
    if (mode() === "menu") {
      const b = host.brush();
      return `Brush ${b.size} · ${b.strength}% · ${material(b.material)?.name ?? b.material}`;
    }
    return before;
  }, `v${VERSION}`);

  return {
    host, mode, listing, sheet, rows, notice, material,
    lists: { title, menu },
    open: setSheet,
    heading: () => {
      if (sheet() === "brush") return "BRUSH";
      if (sheet() === "settings") return "SETTINGS";
      if (sheet() === "controls") return "CONTROLS";
      if (sheet() === "about") return "ABOUT";
      return mode() === "menu" ? "PAUSED" : "";
    },
    back() {
      if (sheet() !== "main") setSheet("main");
      else if (mode() === "menu") leave();
    },
    backLabel: () => (sheet() !== "main" ? "back" : mode() === "menu" ? "resume" : ""),
    menu: () => host.send({ type: "menu", on: true }),
  };
}
