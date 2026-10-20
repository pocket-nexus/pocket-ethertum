//! Ethertum's voxel world, as an engine of its own.
//!
//! Ethertum (Dreamtowards, GPL-3.0) is built on Bevy. This crate does what
//! its `voxel` module does, without Bevy and without `std`, so the same world
//! comes out on a PS Vita:
//!
//! | Module | Does | Reference |
//! | --- | --- | --- |
//! | [`noise`] | Perlin noise and its fractal sum, value for value | `noise` 0.9 `Perlin`, `Fbm<Perlin>` |
//! | [`vox`], [`chunk`] | The voxel in four bytes, chunks of 16³, a chunk with its 26 neighbours | `voxel::vox`, `voxel::chunk` |
//! | [`gen`] | Terrain of one chunk from its position | `worldgen::generate_chunk` |
//! | [`populate`] | Sand, grass, dirt, tall grass, vines, trees | `worldgen::populate_chunk` |
//! | [`mesh`] | Surface nets, cubes, foliage cards, water faces; device meshes | `meshgen` |
//!
//! `ether-oracle` compiles the reference's own `worldgen` and `meshgen` source
//! and checks this crate against it byte for byte.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod chunk;
pub mod gen;
pub mod mesh;
pub mod noise;
pub mod populate;
pub mod vox;

pub use glam;
