//! Pocket Ethertum drawn with wgpu: in a browser tab over WebGPU, and on the machine that builds it (Metal),
//! where frames go to files (`src/bin/shot.rs`). See `wgpu/README.md`.

pub mod app;
pub mod mesh;
pub mod pack;
pub mod render;
#[cfg(target_arch = "wasm32")]
pub mod web;
