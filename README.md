# Pocket Ethertum

[Ethertum](https://github.com/Dreamtowards/Ethertum) (Dreamtowards, GPL-3.0),
a voxel sandbox of smooth isosurface terrain built on Bevy, on the PS Vita,
on an engine of its own built on Pocket3D's mechanisms. The world is the
reference's world: the same noise, the same terrain, the same trees and grass,
the same surface nets, **byte for byte**, checked against the reference's own
source.

## Layout

| Path | What it is |
| --- | --- |
| `ref/ethertum` | The reference, pinned (`d6d4159`). Read and compiled by the oracle, never edited. Its art is cooked from here. |
| `crates/ether-voxel` | The world without Bevy and without `std`: noise, voxels, chunks, generation, population, meshing. |
| `crates/ether-oracle` | The reference's own `vox.rs`, `chunk.rs`, `worldgen.rs`, `meshgen.rs`, compiled against small shims, and the parity tests. |
| `crates/ether-world` | The loaded world: streaming, population and meshing in dependency order, edits, collision triangles, saves. |
| `crates/ether-sim` | The player: the reference's character controller against the terrain's triangles, camera, ray, brush, clock, inventory. |
| `crates/ether-pack`, `crates/ether-cook` | The cooked pack (terrain materials BC1, foliage atlas BC3, sky faces, water normals, item icons) and its cooker. |
| `crates/ether-mod`, `mods/` | Mods in a QuickJS realm of their own; the base game is written as one (`mods/base/main.js`). `docs/MODDING.md`. |
| `crates/ether-interface`, `ui/` | The PocketJS interface over the scene (title, HUD, hotbar, menus, settings) and `Session`, the flow. |
| `vita/` | The PS Vita runtime: GXM renderer, worker threads, the interface and the mod realm embedded. |
| `tools/` | `cook.ts`, `ui.ts`, `vita.ts` (Bun). |
| `vendor/pocketjs` | PocketJS, pinned: device toolchains, the Vita dev host and wire, VPK packaging, the GXM kernel, the title card, the icon. |

## The engine

| Stage | Where it runs | Needs |
| --- | --- | --- |
| Generate a chunk (16³) | a worker thread | its position |
| Populate it (sand, grass, dirt, tall grass, vines, trees) | the frame thread | it and its 26 neighbours generated |
| Mesh it | a worker thread, from a copy of it and a two-voxel border | it and its 26 neighbours populated |
| Upload its mesh | the frame thread, into a GPU heap with deferred frees | |

A chunk is meshed only when no neighbour can still change it (trees drop
leaves into neighbours), so each chunk is meshed once unless it is edited.
The world generates two rings beyond the drawn radius for that. An edit remeshes
every chunk whose border reads the voxel. Edited chunks are kept across unloads
and saved to the card as byte planes.

**Noise and generation** reproduce `noise` 0.9's `Perlin` and `Fbm<Perlin>`
value for value (its XorShift-seeded permutation and `rand` 0.8's shuffle,
`floor(0.0) = -1`, and Ethertum's five octaves scaled as six). The 2D term is
computed once per column and the 3D term per axis per octave: the same numbers
as the reference's per-voxel loop for about a tenth of the work.

**Meshing** emits the reference's vertex stream (surface nets, cubes, foliage
cards, water faces) in the reference's order with the reference's bits, a
cell's feature point, normal and material computed once instead of per quad.
`mesh::Builder` lowers that stream for the device: 16-bit positions, indexed
vertices, ambient occlusion, terrain batched by material with a blended layer
where materials meet.

**The renderer** (GXM, 960 × 544, 2× multisampling) draws per chunk: terrain
one draw per material with that material's texture, mapped from three sides as
the reference's triplanar shader does (the three projections' coordinates and
weights are computed per vertex, so the fragment program's three reads need
nothing computed first); the layers; the water, opaque, showing the terrain
mirrored in the sea's plane (drawn each frame at half the display's size, only
in the part of the screen the water in view covers) moved by three octaves of
ripples, and the sun's glint; the foliage cards with the reference's cut-out at
half alpha. Back faces are clockwise on GXM after the viewport's flip of y. Far
terrain fades towards the sky picture folded into one 128 × 128 octahedral map,
read per vertex with no angle computed; a chunk wholly inside the fade's start
skips the read.

The light is the reference's: every face takes the sky box's light in its
normal's direction (zenith, horizon and nadir, measured from the picture at
load) and the sun's 20 klux × sin² of its angle, through Bevy's default
exposure, then one curve that fits the reference's tone map (TonyMcMapface) and
sRGB encoding, `sqrt(x / (x + 0.8))`, within 0.012 RMS of Bevy's lookup table.
The curve is folded into each vertex's light for its material's mean texel, so
the fragment programs keep one multiply; the water, a black mirror at 4%
reflectance looking down, mixes its reflection and the reference's distance fog
(which only its water takes) in linear light and applies the curve per
fragment. The sky picture is drawn through the same curve, with z negated as
Bevy reads cube maps.

The sun's shadow (`vita/src/shadow.rs`): a 1024² distance map over 96 m around
the player, a sixteenth redrawn each frame from two-voxel caster meshes and the
leaf cards. Terrain tests it per fragment, half a voxel out along its normal;
a leaf card tests it per corner, at a point three quarters of the way to the
centre of its voxel, inside the crown. `ctl '{"shadows":false}'` turns it off.

Over a canopy the leaf cards' layers cost more than a refresh. A governor
(`vita/src/governor.rs`) lowers the share of cards drawn and casting, in steps
from 1 to 0.3, when two frames within two seconds miss their refresh while the
frame thread waited on the GPU, and raises it after four seconds without one.

**Mods** set the world's parameters, the materials, the items, the hotbar and
the player's tuning, hear breaks, places and ticks, answer commands, and queue
edits. Their realm is separate from the interface's. A development build reads
them from the USB share once a second and reloads them in place; what changed
decides what is redone (regenerate, remesh, retune).

## Checks

```sh
cargo test --workspace --release        # parity with the reference, world, player, pack, mods, interface
bun tools/ui.ts test                     # the interface's flow against a mock renderer
```

`ether-oracle` compares, by the bits of every value: noise at 30 000 points
against the `noise` crate; 288 generated chunks; a 125-chunk block populated
in one order on both sides; the terrain, foliage and water vertex streams of 27
chunks. The reference's chunk writes go through `&T` cast to `&mut T`, which an
optimizing build may drop (a tree then grows no leaves; seen with the oracle at
opt-level 3), so the oracle compiles the reference without optimization.

## On the console

The development loop runs in Pocket Devkit (`vendor/pocketjs/apps/devkit`) over
the wired debug transport:

```sh
bun tools/vita.ts serve          # USB host (detached; once)
bun tools/vita.ts native         # sync the pack, interface and mods, build, replace Devkit's binary
bun tools/vita.ts status | capture --out f.png
bun tools/vita.ts ctl '{"mode":"play","pos":[14,24,-23],"fly":true}'
bun tools/vita.ts bench --seconds 60
bun tools/vita.ts vpk            # standalone package, title PKET00001
bun tools/vita.ts push-vpk       # the running build copies it to ux0:data/pocket-ethertum/ for VitaShell
```

`ctl` keys: `mode` (`title`, `play`, `menu`), `pos`, `yaw`, `pitch`, `fly`,
`daytime`, `walk: [x, y, seconds]` (stick input), `stroke` (`break`, `place`),
`show: {terrain, water, foliage}`, `reflect: {foliage}` (leaves in the sea's
reflection), `shadows`, `casters: {terrain, foliage, leafShare, leafBias,
leafCut}`, `density` (fixes the cards' share), `governor: true` (releases it),
`farShare` (the share at the edge of the loaded radius), `probe` (measurement
switches, `scene.rs`), `water: [reflectance, ripple, shift, glint]`, `bloom`,
`save`. `boot` switches, read at start: `title`, `msaa`, `play`, `fresh`,
`shadows`, `bloom`, `profile` (the scene drawn to a target of its own and
timed: `gpuMs` in the status). `ETHER_MODS=glow bun tools/vita.ts
sync` adds an example mod to the share.

Measured on the development console (clocks 444/222/222/166 MHz,
2026-10-20): 60 frames a second at the reference's load distance (4 chunks
around, 3 up and down) — a 60 s flight with chunks streaming ahead, 3 600
frames, 1 late, worst 16.8 ms; 75–120 k triangles and 400–500 draws in a
typical view, 4.5–7.5 ms of frame-thread work. A chunk takes about 11 ms to
generate and 8 ms to mesh on a worker; loading the first 567 meshed chunks
takes 13.6 s. Looking straight down at the sea from 30 m was the heaviest view
measured: 17 ms.

With the sea's reflection (2026-10-21): the title's view over the sea, 60 s,
3 590 frames, 0 late; the same 60 s flight, 3 590 frames, 1 late, about 100 k
triangles and 460 draws. Leaves drawn in the reflection as well cost the title's
view 59 late frames in 30 s; they are off.

With the reference's light and the sun's shadow (2026-10-23), five fixed views
at daytime 0.33, 10 s each with the governor: over the sea from the spawn
island, 2 and 0 late frames at shares 0.85; the trees west of spawn, 2 late at
the full share; a valley at (150, 45, −150), 0 late at 0.7; hovering just above
a forest at (194, 34, −199), 50 frames a second at the lowest share. The
title's view, 60 s: 3 590 frames, 6 late; the 60 s flight: 3 590 frames, 1
late; a 60 s flight low over the sea north-east of (395, 8, 416): 3 520 frames,
36 late. 4× multisampling costs the forest view 3 ms (36 against 40 frames a
second) and the trees 13 late frames in 6 s; the bloom's scene target alone
costs the sea and trees views 13–17 late frames in 6 s, and 34–42 with its
passes, so it is off (`boot {"bloom": true}`).

## Where this differs from the reference

| | The reference | Here | Why |
| --- | --- | --- | --- |
| Population | when a neighbour arrives after the chunk; a chunk that completes its own neighbourhood is never populated | when the chunk's neighbourhood is complete, whichever arrives last | the reference leaves bare stone chunks |
| Remeshing after an edit | the edited voxel's chunk | every chunk whose border reads it | the reference leaves seams |
| Meshing | as soon as a chunk arrives, again as neighbours arrive | once its 26 neighbours are populated | each chunk meshed once |
| Water | every face not against water | faces against solid terrain left out | under the opaque surface or inside the terrain |
| Leaves | every card | cards of a leaf voxel enclosed by leaves or ground left out | inside a crown |
| Foliage | every card at every distance | all within 24 m, thinning to a quarter at the edge; a governor thins every crown where the GPU misses refreshes | fill rate |
| The world's edge | the loaded world ends against the sky box | the last quarter of the radius fades towards the sky picture behind it | |
| Lighting | PBR, deferred: sky box environment light, sun, cascaded shadows per pixel, normal maps, tone map, bloom, FXAA | the same light per vertex with the tone map folded in; shadows per fragment on terrain, per corner on leaf cards; no normal maps, no bloom; 2× multisampling | the GPU |
| Water's reflection | screen-space: what is on the screen | the terrain mirrored in the sea's plane and the sky picture; leaves left out | the GPU (see the measurements) |
| The sky at night | the same picture, sun off | the same | |
| Hotbar | empty at start, the brush set in a panel | brush presets and two items from the base mod | the Vita has no mouse for the panel |
| Controls | keyboard, mouse, gamepad, touch stick | the Vita's pad (see Controls in the interface) | |
| Multiplayer | renet client and server | not ported | |
| Music | five tracks | not used | no license statement for the tracks |

## Licence

GPL-3.0, as the reference (`LICENSE`). The reference's art is cooked from the
pinned submodule at build time and never committed. The sky picture is Poly
Haven's (CC0). PocketJS and Pocket3D are under their own licences
(`vendor/pocketjs`).
