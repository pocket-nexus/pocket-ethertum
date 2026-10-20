# Repository instructions

- Pocket Ethertum is Ethertum (Dreamtowards, GPL-3.0, `ref/ethertum`, pinned) on the PS Vita, on an engine of its own built on Pocket3D's mechanisms. The whole repository is GPL-3.0, like the reference; keep `LICENSE` and the reference's notices.
- `ref/ethertum` is the reference: read it, compile it in `crates/ether-oracle`, never edit it. Its art (`assets/baked`, the sky) is read from the submodule at cook time; cooked packs stay under `.pocket-build/` and are never committed. Its music is not used (no license statement for the tracks).
- `vendor/pocketjs` (pinned) owns the device toolchains, the Vita dev host and wired debug transport, VPK packaging, the GXM device kernel, the title card and the icon. Do not edit the submodule to fix application behaviour.
- `crates/ether-voxel` (`no_std` + `alloc`) is the world: noise, generation, population, meshing. **Its streams are bit-exact with the reference**: `cargo test -p ether-oracle` compares noise, generation, population and the terrain, foliage and water vertex streams with the reference's own `worldgen.rs` and `meshgen.rs`. A change that moves a byte of those must come with the reason, in the commit and in the oracle's tests. Device-only lowering (16-bit positions, batches by material, occlusion) is `mesh::Builder` and is not compared.
- The oracle compiles the reference without optimization: its chunk writes go through `&T` cast to `&mut T`, which an optimizing build may drop (trees lose their leaves). Keep `[profile.*.package.ether-oracle] opt-level = 0`.
- Deviations from the reference are deliberate and listed in `README.md` ("Where this differs from the reference"); add a row when you add one.
- Use Conventional Commits (`type(scope): summary`). Keep captures, logs and receipts in `.pocket-build/`; put commands and results in PR descriptions.
- PS Vita work happens in Pocket Devkit (`vendor/pocketjs/apps/devkit`); the Devkit on the development console answers as title `P3B1D7273`.
