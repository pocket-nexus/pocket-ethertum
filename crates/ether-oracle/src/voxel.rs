//! The reference's `voxel` module: its files, and what its `mod.rs` puts in
//! scope for them.

#![allow(dead_code, unused_imports, unused_variables, unused_mut, static_mut_refs, clippy::all)]

#[path = "../../../ref/ethertum/src/voxel/vox.rs"]
mod vox;
#[path = "../../../ref/ethertum/src/voxel/chunk.rs"]
mod chunk;
#[path = "../../../ref/ethertum/src/voxel/meshgen.rs"]
pub mod meshgen;
#[path = "../../../ref/ethertum/src/voxel/worldgen.rs"]
pub mod worldgen;

pub use chunk::Chunk;
pub use vox::{Vox, VoxLight, VoxShape, VoxTex};

pub type ChunkPtr = Arc<Chunk>;

use crate::util::AsMutRef;
use bevy::prelude::*;
use std::sync::Arc;
