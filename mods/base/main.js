// The base game: Ethertum's materials, items, world and player, as a mod.
//
// Every value here is the reference's (ref/ethertum at d6d4159). The engine's
// own defaults equal these too, and `cargo test -p ether-mod` checks that this
// file sets every world, controller, clock and sky setting to them. A mod
// loaded after this one changes what it names and leaves the rest.

// ------------------------------------------------------------------ materials
// Terrain materials, numbered as Ethertum's `VoxTex`; material `id` is tile
// `id - 1` of the diffuse atlas (`assets/baked/atlas_diff.png`, 24 tiles of
// 256 px). `color` is each tile's mean colour, for the interface.
//
// The reference names seven of the 24 (dirt, grass, log, sand, stone,
// leaves, water). The other names describe what the tile shows; they are
// guesses, marked `// looks like`. Tiles 10 and 11 are the ones the
// reference's terrain shader makes metallic.
const terrain = [
  [1, "dirt", [53, 45, 36]],
  [2, "roof_tiles", [116, 92, 68]], // looks like: red clay roof tiles
  [3, "slate", [64, 63, 57]], // looks like: dark streaked rock
  [4, "concrete", [117, 116, 111]], // looks like: pale grey plaster
  [5, "basalt", [54, 53, 48]], // looks like: dark fine rock
  [6, "clay", [100, 69, 50]], // looks like: plain brown earth
  [7, "planks", [82, 58, 37]], // looks like: wood grain
  [8, "moss", [52, 62, 38]], // looks like: dark green growth
  [9, "cobblestone", [106, 105, 105]], // looks like: grey cobbles
  [10, "steel", [102, 108, 109]], // looks like: brushed metal; metallic in the reference's shader
  [11, "iron", [86, 85, 82]], // looks like: rough metal; metallic in the reference's shader
  [12, "grass", [48, 59, 16]],
  [13, "log", [58, 61, 17]],
  [14, "dry_grass", [83, 71, 48]], // looks like: olive-brown turf
  [15, "mud", [95, 73, 54]], // looks like: red-brown soil
  [16, "gravel", [74, 70, 68]], // looks like: dark grit
  [17, "mossy_rock", [63, 63, 28]], // looks like: rock under moss
  [18, "smooth_stone", [77, 77, 74]], // looks like: even grey stone
  [19, "sand", [148, 124, 95]],
  [20, "andesite", [81, 77, 72]], // looks like: grey speckled rock
  [21, "dark_soil", [59, 52, 45]], // looks like: dark brown earth
  [22, "stone", [90, 86, 84]],
  [23, "leaves", [72, 69, 65]], // its terrain tile is a rock; leaves are drawn as foliage cards
];
for (const [id, name, color] of terrain) {
  ether.material({ id, name, color, hotbar: name !== "leaves" });
}
ether.material({ id: 24, name: "water", color: [9, 142, 126], kind: "liquid", hotbar: false });

// Foliage cards: tiles of `atlas_diff_foli.png`, drawn for `grass` and
// `leaves` voxels. Ids are shared with terrain materials (13 is log as terrain,
// short grass as a card); a voxel's shape decides which one it is.
ether.foliage({ id: 13, name: "short_grass", color: [162, 172, 127] });
ether.foliage({ id: 14, name: "bush", color: [91, 98, 53] });
ether.foliage({ id: 15, name: "rose", color: [59, 37, 5] });
ether.foliage({ id: 16, name: "fern", color: [62, 99, 18] });
ether.foliage({ id: 23, name: "leaves", color: [128, 83, 42] });

// ------------------------------------------------------------------ items
// The reference's item registry (`item/mod.rs`). Its icons sit in
// `assets/baked/items.png` in sorted name order, 64 px each.
const items = ["apple", "avocado", "coal", "frame", "grapple", "iron_ingot", "lantern", "pickaxe", "shears", "stick"];
items.forEach((name, icon) => ether.item({ name, icon }));

// ------------------------------------------------------------------ world
// `voxel/worldgen.rs` generate_chunk: fbm(x/130, z/130) - y/18 + 4.5 fbm(p/90).
ether.worldgen({
  seed: 100,
  octaves: 5,
  terrainScale: 130,
  caveScale: 90,
  heightDiv: 18,
  caveAmp: 4.5,
  seaLevel: 0,
  waterIso: -0.1,
  stone: "stone",
  water: "water",
});

// `voxel/worldgen.rs` populate_chunk.
ether.decor({
  seed: 123,
  vinesPer256: 18,
  vineMax: 12,
  treesPer256: 3,
  bush: 0.24,
  fern: 0.8,
  rose: 0.94,
  sandBelow: 2,
  sandNoise: 0.1,
});

// ------------------------------------------------------------------ player
// `client/character_controller.rs`, the capsule of `net/netproc_client.rs`,
// the field of view of `client/settings.rs`.
ether.controller({
  jumpImpulse: 7,
  acceleration: 50,
  gravity: 9.81,
  gravityScale: 2,
  sprint: 2,
  sneak: 0.3,
  airControl: 0.2,
  maxSlope: 45,
  groundDamping: 0.0001,
  airDamping: 0.07,
  flyDamping: 0.01,
  lookSensitivity: 0.003,
  stickLook: 10,
  fov: 85,
  sprintFov: 20,
  doubleTap: 0.3,
  jumpCooldown: 0.3,
  unflyOnGround: true,
  radius: 0.3,
  halfHeight: 0.9,
  eyeHeight: 0.8,
});

// `client/client_world.rs` WorldInfo: a day of 24 minutes, starting in the morning.
ether.clock({ start: 0.15, dayLength: 1440, tickRate: 20 });

// `client/game_client.rs` ClientInfo.
ether.sky({
  fogColor: [0, 0.666, 1],
  fogVisibility: 1200,
  inscattering: [110 / 255, 230 / 255, 1],
  extinction: [0.35, 0.5, 0.66],
  atmospheric: true,
  sunIlluminance: 20,
  shadows: true,
});

// ------------------------------------------------------------------ hotbar
// The reference starts with an empty inventory and sets its brush in a
// settings panel (size 4, strength 0.8, isosurface, tile 10). Here the brush
// presets are the hotbar: the reference's brush first, then the ground
// materials, a one-voxel cube brush, and two tools.
ether.hotbar([
  { brush: { material: "steel", shape: "isosurface", size: 4, strength: 0.8 } },
  { brush: { material: "stone", shape: "isosurface", size: 4, strength: 0.8 } },
  { brush: { material: "dirt", shape: "isosurface", size: 4, strength: 0.8 } },
  { brush: { material: "grass", shape: "isosurface", size: 4, strength: 0.8 } },
  { brush: { material: "sand", shape: "isosurface", size: 4, strength: 0.8 } },
  { brush: { material: "log", shape: "isosurface", size: 2, strength: 0.8 } },
  { brush: { material: "cobblestone", shape: "cube", size: 1, strength: 1 } },
  { item: "pickaxe" },
  { item: "lantern" },
]);

// ------------------------------------------------------------------ commands
ether.command("help", () => "commands: " + ether.commands().map((c) => "/" + c).join(" "));
ether.command("materials", () => ether.materials().map((m) => `${m.id} ${m.name}`).join(", "));
ether.command("where", () => {
  const p = ether.player.position.map((v) => Math.round(v * 10) / 10);
  return `x ${p[0]} y ${p[1]} z ${p[2]}${ether.player.flying ? " (flying)" : ""}`;
});
