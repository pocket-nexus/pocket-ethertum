// An example mod: a material that gives light, and two commands.
//
// Load it beside the base mod (see docs/MODDING.md). `glowstone` appears in
// the hotbar's material list; the engine meshes it with light 15 around it.

ether.material({
  id: 30,
  name: "glowstone",
  tile: 3, // the pale grey tile, tinted by the light it gives
  color: [250, 214, 120],
  light: 15,
});

// The last hotbar slot places glowstone as one-voxel cubes.
ether.hotbar([
  { brush: { material: "steel", shape: "isosurface", size: 4, strength: 0.8 } },
  { brush: { material: "stone", shape: "isosurface", size: 4, strength: 0.8 } },
  { brush: { material: "dirt", shape: "isosurface", size: 4, strength: 0.8 } },
  { brush: { material: "grass", shape: "isosurface", size: 4, strength: 0.8 } },
  { brush: { material: "sand", shape: "isosurface", size: 4, strength: 0.8 } },
  { brush: { material: "log", shape: "isosurface", size: 2, strength: 0.8 } },
  { brush: { material: "cobblestone", shape: "cube", size: 1, strength: 1 } },
  { item: "pickaxe" },
  { brush: { material: "glowstone", shape: "cube", size: 1, strength: 1 } },
]);

// /time 0.25 sets noon; /time alone answers with the usage.
ether.command("time", (args) => {
  const t = Number(args[0]);
  if (args.length !== 1 || !(t >= 0 && t <= 1)) return "/time <0..1>: 0 sunrise, 0.25 noon, 0.5 sunset";
  ether.game.time(t);
  return `time of day ${t}`;
});

// /lamp puts a glowstone voxel two metres above the player.
ether.command("lamp", () => {
  const [x, y, z] = ether.player.position.map(Math.floor);
  ether.world.set(x, y + 2, z, { tex: "glowstone", shape: "cube", iso: 0 });
  return "lamp placed";
});

// Breaking glowstone leaves a log line.
ether.on("break", (e) => {
  if (e.material === "glowstone") ether.log("glowstone broken at", e.pos);
});
