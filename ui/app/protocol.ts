// What crosses between a device's renderer and the interface. The renderer
// owns the world and the player; the interface owns every 2D pixel that is
// not anchored to a point of the world, and what a button or a finger means
// while a list is up. Both sides speak JSON lines over PocketJS's in-process
// overlay service: the renderer sends the members of its state that changed,
// the interface sends commands. `crates/ether-interface` is the renderer's
// side.
//
// The renderers parse commands with a small scanner, so commands are flat
// objects of numbers, booleans and short strings.

export type Mode = "loading" | "error" | "title" | "play" | "menu";

/** A switch (0 or 1), or with `choices` one of several named values. */
export interface Setting {
  key: string;
  value: number;
  choices?: string[];
}

/** A material or an item: its id, its name, and its picture (a tile of the
 *  diffuse atlas for a material, an icon of `items.png` for an item). */
export interface Entry {
  id: number;
  name: string;
  picture: number;
}

/** A hotbar slot: `kind` 0 holds nothing, 1 a material, 2 an item; `count` 0 draws no count. */
export interface Slot {
  kind: 0 | 1 | 2;
  id: number;
  count: number;
}

/** The voxel brush: radius in voxels (0…20), strength in percent, shape (0 the
 *  isosurface, 1 cubes) and the material it places. */
export interface Brush {
  size: number;
  strength: number;
  shape: number;
  material: number;
}

export interface HostState {
  mode: Mode;
  /** The loading step, or why `mode` is "error". */
  message: string;
  /** How far loading is, 0…100. */
  progress: number;
  materials: Entry[];
  items: Entry[];
  /** Nine slots. */
  hotbar: Slot[];
  /** The selected slot. */
  slot: number;
  health: number;
  healthMax: number;
  flying: boolean;
  sprinting: boolean;
  brush: Brush;
  /** The camera's step behind the eye: 0 is first person. */
  camera: number;
  /** Mods the world was set up with. */
  mods: number;
  /** What the device lets the person set, in menu order. */
  options: Setting[];
  /** One line of renderer statistics while the `stats` setting is on. */
  stats: string;
  /** What the interface last stored with `prefs`. */
  prefs: string;
  /** The `wake` the interface asked for that last came due; 0 before any has. */
  woke: number;
  /** The numbers that change in play, at most once a turn (see `T`). */
  t: number[];
}

/** Indices into `HostState.t`. */
export const T = {
  /** The world's clock, minutes since midnight. */
  minutes: 0,
  /** The player's position, tenths of a voxel. */
  x: 1,
  y: 2,
  z: 3,
} as const;

export type Command =
  /** Leave the title for the world. */
  | { type: "start" }
  /** A list comes up over the world, or leaves it. */
  | { type: "menu"; on: boolean }
  /** Back to the title. */
  | { type: "title" }
  | { type: "option"; key: string; value: number }
  /** The brush as the menu set it. */
  | ({ type: "brush" } & Brush)
  /** A finger chose a hotbar slot. */
  | { type: "slot"; index: number }
  | { type: "prefs"; value: string }
  /** The interface has nothing scheduled (nothing fading). A turn of the
   *  guest costs a millisecond on the Vita, so while this is on the renderer
   *  turns it only when it has news or a button the interface listens to
   *  changes. */
  | { type: "idle"; on: boolean }
  /** Say `id` back in `HostState.woke` in this many seconds. The interface's
   *  own timers count its turns, and an idle guest takes none: what must
   *  happen later (a hint leaving) is timed by the renderer. */
  | { type: "wake"; id: number; seconds: number };
