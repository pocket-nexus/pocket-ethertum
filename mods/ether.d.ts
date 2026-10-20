// Types of the `ether` object a mod's main.js receives.
//
// A mod is `mods/<name>/main.js`. Its body runs once when the mods load, with
// `ether` in scope; it registers data and handlers. Handlers run later, at
// events. docs/MODDING.md describes loading and what each change redoes.

/** A material by its name or its id (1..255). */
type MaterialRef = string | number;

type Shape = "isosurface" | "cube" | "leaves" | "grass" | "slab_y_min" | "slab_y_max" | "slab_x_min" | "slab_x_max" | "slab_z_min" | "slab_z_max" | "fence";

type Rgb = [number, number, number];

interface MaterialDef {
  /** 1..255. Ethertum's own materials are 1..24. Registering an id again replaces it. */
  id: number;
  /** Lower-case: a-z, 0-9, _. */
  name: string;
  /** Tile of the terrain atlas; `id - 1` when left out. */
  tile?: number;
  /** sRGB 0..255, what the interface shows for it. */
  color?: Rgb;
  /** "terrain" (isosurface or cube, the default) or "liquid" (drawn as water). */
  kind?: "terrain" | "liquid";
  /** Light it gives, 0..15. */
  light?: number;
  /** Whether hotbar brushes may place it (terrain materials: yes). */
  hotbar?: boolean;
}

interface FoliageDef {
  /** The id a `grass` or `leaves` voxel carries. */
  id: number;
  name: string;
  /** Tile of the foliage atlas; `id - 1` when left out. */
  tile?: number;
  color?: Rgb;
}

interface ItemDef {
  name: string;
  /** Icon in the item atlas, 64 px squares from the left. */
  icon: number;
  /** Largest stack, 1..255 (64). */
  stack?: number;
}

/** Terrain generation: val = fbm(x / terrainScale, z / terrainScale) - y / heightDiv + caveAmp · fbm(p / caveScale). */
interface Worldgen {
  seed: number;
  octaves: number;
  terrainScale: number;
  caveScale: number;
  heightDiv: number;
  caveAmp: number;
  seaLevel: number;
  waterIso: number;
  stone: MaterialRef;
  water: MaterialRef;
}

/** Population: sand, grass, dirt, tall grass, vines, trees. */
interface Decor {
  seed: number;
  vinesPer256: number;
  vineMax: number;
  treesPer256: number;
  bush: number;
  fern: number;
  rose: number;
  sandBelow: number;
  sandNoise: number;
}

interface Controller {
  jumpImpulse: number;
  acceleration: number;
  gravity: number;
  gravityScale: number;
  sprint: number;
  sneak: number;
  airControl: number;
  /** Degrees. */
  maxSlope: number;
  groundDamping: number;
  airDamping: number;
  flyDamping: number;
  lookSensitivity: number;
  stickLook: number;
  /** Degrees. */
  fov: number;
  sprintFov: number;
  doubleTap: number;
  jumpCooldown: number;
  unflyOnGround: boolean;
  radius: number;
  halfHeight: number;
  eyeHeight: number;
}

interface Clock {
  /** Time of day a new world starts at: 0 sunrise, 0.25 noon, 0.5 sunset. */
  start: number;
  /** Seconds in a day. */
  dayLength: number;
  tickRate: number;
}

interface Sky {
  /** sRGB 0..1. */
  fogColor: Rgb;
  /** Metres. */
  fogVisibility: number;
  inscattering: Rgb;
  extinction: Rgb;
  atmospheric: boolean;
  sunIlluminance: number;
  shadows: boolean;
}

type Slot = { brush: { material: MaterialRef; shape?: Shape; size?: number; strength?: number } } | { item: string; count?: number } | null;

interface Voxel {
  tex: number;
  material: string | null;
  shape: Shape;
  /** Signed distance, -1 (empty) .. 1 (solid). */
  iso: number;
}

interface BlockEvent {
  /** The voxel the player's ray hit. */
  pos: [number, number, number];
  tex: number | null;
  material: string | null;
  brush: { material: number; shape: number; size: number; strength: number };
}

interface Player {
  position: [number, number, number];
  velocity: [number, number, number];
  yaw: number;
  pitch: number;
  flying: boolean;
  grounded: boolean;
  /** Selected hotbar slot, 0..8. */
  hotbar: number;
}

interface Ether {
  /** API version. */
  readonly version: string;
  /** This mod's name. */
  readonly mod: string;

  material(def: MaterialDef): void;
  foliage(def: FoliageDef): void;
  item(def: ItemDef): void;
  /** Each table merges: a later mod changes the keys it names. */
  worldgen(patch: Partial<Worldgen>): void;
  decor(patch: Partial<Decor>): void;
  controller(patch: Partial<Controller>): void;
  clock(patch: Partial<Clock>): void;
  sky(patch: Partial<Sky>): void;
  /** Up to nine slots; the last mod to call this sets the whole bar. */
  hotbar(slots: Slot[]): void;

  /** Before the player's brush breaks or places at `e.pos`. Returning false cancels it. */
  on(event: "break" | "place", handler: (e: BlockEvent) => boolean | void): void;
  /** Ten times a second (the engine's setting), with the seconds since the last run. */
  on(event: "tick", handler: (seconds: number) => void): void;
  /** `/name args…` from the chat line; a returned string is the answer. */
  command(name: string, handler: (args: string[], line: string) => string | void): void;
  commands(): string[];
  materials(): { id: number; name: string }[];

  log(...args: unknown[]): void;

  readonly world: {
    /** null where no chunk is loaded, and while the mods load. */
    get(x: number, y: number, z: number): Voxel | null;
    /** Queued; the engine applies it after the handler returns. Absent fields stay. */
    set(x: number, y: number, z: number, v: { tex?: MaterialRef | 0; shape?: Shape; iso?: number }): void;
  };
  /** Queued; the engine acts after the edits. */
  readonly game: {
    time(t: number): void;
    teleport(x: number, y: number, z: number): void;
    fly(on: boolean): void;
  };
  /** The player when the handler was called. */
  readonly player: Readonly<Player>;
}

declare const ether: Ether;
