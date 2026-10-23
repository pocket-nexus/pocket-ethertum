# Pocket Ethertum in a browser tab

The game of the PS Vita build, drawn with [wgpu](https://wgpu.rs) 25: over WebGPU in a tab (wasm32), and over
Metal on the machine that builds it, where frames go to files. It reads **the PS Vita's pack** (`ether.pack`,
`bun tools/cook.ts`) as it is: the same pictures, in BC blocks where the adapter reads them. The world is
`crates/ether-world`, the player `crates/ether-sim` and the flow `crates/ether-interface`, as the PS Vita links
them; the sky and the light are `crates/ether-look`, which the PS Vita takes too. The game's own interface (`ui/`,
the bundle the PS Vita loads) runs over the world, and the page shows the game in a PS Vita's shell with its keys
as the controls.

| Part | What it is |
| --- | --- |
| `src/app.rs` | The shell: a frame as `step` (the interface's commands, the player, the world's jobs), the guest's turn, `draw`; the control words. |
| `src/render.rs`, `src/shaders/` | The renderer: the PS Vita's passes (`vita/src/scene.rs`, `shadow.rs`, `post.rs`) as WGSL programs. |
| `src/mesh.rs` | A chunk's meshes on the GPU, laid out again for WebGPU's four-byte attributes, and its row of the chunks' table. |
| `src/pack.rs` | The pack's pictures on the GPU: blocks as they are, or taken apart into rows of RGBA where the GPU reads no blocks. |
| `src/web.rs`, `page/` | The tab: what the page calls, and the page itself. |
| `src/bin/shot.rs` | A frame on this machine's GPU, as a PNG, and the run's status. |
| `vendor/pocketjs/devices/web/pocket-web-wgpu` | PocketJS's browser kernel: the device and the screens, the overlay pass, the pack's reads, the page's title card and frame loop, the interface's realm, and **the Pocket3D player** (the bar, the PS Vita's shell, the dock with Download and Remix). |
| `tools/web.ts` | build, serve, dist, shot, check. |

```
bun tools/cook.ts                     # the PS Vita's pack → .pocket-build/pack/ether.pack
bun tools/web.ts serve                # build when there is no site, then http://127.0.0.1:8803/
bun tools/web.ts shot --words "mode=play pos=13.8,30,-23.3 yaw=0.8 pitch=-0.35 daytime=0.33 fly=1" --out a.png
bun tools/web.ts check                # Chrome: from the title into play, a walk, a look, the menu, what a frame costs
bun tools/web.ts dist                 # the directory a static host serves
bun tools/web.ts check --dist         # the same check of that directory
bun test tools/web.test.ts            # the page's devices, the player's and the release's are one list
cargo test --release --manifest-path wgpu/Cargo.toml
```

The build needs the `wasm32-unknown-unknown` target and `wasm-bindgen` 0.2.126 on the path (the version in
`Cargo.lock`; `build` refuses another), and `bun install` in `vendor/pocketjs` for the interface.

## The mods

**A tab runs no QuickJS of the game's.** The PS Vita runs the mods in a QuickJS realm of their own
(`crates/ether-mod`); the browser takes the registry the base mod makes, as JSON, which `build` writes by running
the same mod on this machine (`cargo run -p ether-mod --example registry`, `ether-mod` without its `script`
feature in the tab). So the world, the materials, the items, the hotbar and the controller are the base mod's,
and no handler hears a break, a place or a tick. A mod of a player's own runs on the PS Vita alone.

## The world's jobs

**The world owns no threads** (`ether_world`): `World::plan` hands out jobs and the host runs them where it likes.
The PS Vita runs them on two worker threads; a tab has one, so `App::step` runs them itself, as many as a frame's
budget holds: 12 ms while the world is first read around the spawn, 4 ms in play. On an M3 Max in Chrome the world
around the spawn (567 chunks meshed, about 2 000 generated) stands 0.7 s after the first frame.

## The frame

1. `step`: what the interface asked for, the player (`ether_sim`), the crosshair's ray and the brush, the clock,
   the world's jobs.
2. The guest's turn, offered 30 times a second as the PS Vita's host offers it, and taken when `Pace` says it is
   worth one; its picture when that has changed.
3. **The sun's map**: the terrain's caster meshes (the faces turned away from the sun) and the leaf cards (cut out
   from a fixed level of the atlas), into a depth texture of 2048² over 128 m around the eye, moved in whole
   texels.
4. **The sea's reflection**, when water is in view: the sky and the terrain above the sea, mirrored in its plane,
   at half the screen's size.
5. **The scene**, at four samples a pixel: the sky (a cube read along each ray, z negated as Bevy reads cube
   maps), the terrain's batches and blended layers, the water, the leaf cards. Then the interface over it.

A chunk has a row of a table the programs read (`mesh::ChunkInfo`, a storage buffer); a draw names its row by its
instance, the terrain's with the material's index too (row × 32 + material: the materials are the layers of one
texture array). One bind group serves every draw of a pipeline.

## What differs from the PS Vita's picture

- **The sun's map is drawn whole each frame**, 2048² over 128 m (the PS Vita: 1024² over 96 m, a sixteenth a
  frame), stores depth (the PS Vita packs a distance into 24 bits of colour), and the terrain reads it through a
  comparison sampler, which mixes four texels.
- **The leaf cards are drawn at every distance**, as the reference draws them, and no governor thins them (the
  PS Vita draws a quarter of them at the edge of the loaded world and fewer where the GPU misses refreshes).
- **Four samples a pixel** (the PS Vita: two), and the terrain's pictures are filtered with anisotropy 8.
- **The world is not saved.** The interface's settings are, in the browser's `localStorage` under
  `pocket-ethertum.interface`.
- **The mods** are the base mod's registry (above).

## Measured

Chrome 155 (headless, WebGPU on the Apple GPU through Metal) on an M3 Max, the pack `6642aae0…`, served from the
same machine, 2026-10-23. `bun tools/web.ts check` writes these to `.pocket-build/validation/web/`.

| | |
| --- | --- |
| Frames a second over 5 s in play, with the interface over it | 59.98 (300 frames, 0 late) |
| A frame's cost without the display, over 300 frames | 0.38 ms |
| What a frame draws by the spawn | 370 to 380 draws, 64 000 to 67 000 triangles, 123 000 of casters |
| From the page's start: the pack read, the interface up, the first frame, the world standing | 0.06 s, 0.08 s, 2.45 s (after the title card), 3.15 s |
| The deployable directory | 40 files, 7.8 MB (the pack 3.6 MB in one piece) |
