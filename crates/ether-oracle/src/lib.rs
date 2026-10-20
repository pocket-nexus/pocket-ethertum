//! The reference's voxel module, compiled outside Bevy.
//!
//! `voxel` is Ethertum's own `vox.rs`, `chunk.rs`, `worldgen.rs` and
//! `meshgen.rs` from the `ref/ethertum` submodule (by `#[path]`, unmodified).
//! What they import resolves to the shims: `bevy` (glam and three empty types),
//! `bevy_egui::egui::emath` (the real emath), `noise` (the real crate at the
//! reference's version), and `crate::util` (its `hash` and `iter`, cut out of the
//! reference's `util/mod.rs` by `build.rs`, plus a vertex buffer that records
//! what is pushed).
//!
//! `tests/parity.rs` runs these functions and `ether-voxel` on the same inputs
//! and compares bytes.

pub mod prelude {
    pub use crate::util::AsMutRef;
    pub use crate::voxel::*;
    pub use bevy::math::{ivec3, vec3};
    pub use bevy::prelude::*;
    pub use ether_oracle_derive::{Deserialize, Serialize};
}

pub mod util {
    include!(concat!(env!("OUT_DIR"), "/util_extract.rs"));

    pub mod vtx {
        use bevy::prelude::*;

        #[derive(Clone, Copy, Debug)]
        pub struct Vertex {
            pub pos: Vec3,
            pub uv: Vec2,
            pub norm: Vec3,
        }

        /// Records vertices in the order they are pushed.
        #[derive(Default)]
        pub struct VertexBuffer {
            pub vertices: Vec<Vertex>,
        }

        impl VertexBuffer {
            pub fn push_vertex(&mut self, pos: Vec3, uv: Vec2, norm: Vec3) {
                self.vertices.push(Vertex { pos, uv, norm });
            }
        }
    }

    /// The reference's unchecked `&T` to `&mut T` (`util::as_mut`), which its chunk code relies on.
    #[allow(invalid_reference_casting, clippy::mut_from_ref)]
    pub fn as_mut<T>(v: &T) -> &mut T {
        unsafe { &mut *((v as *const T) as *mut T) }
    }

    pub trait AsMutRef<T> {
        #[allow(clippy::mut_from_ref)]
        fn as_mut(&self) -> &mut T;
    }

    impl AsMutRef<crate::voxel::Chunk> for std::sync::Arc<crate::voxel::Chunk> {
        fn as_mut(&self) -> &mut crate::voxel::Chunk {
            as_mut(self.as_ref())
        }
    }

    impl AsMutRef<crate::voxel::Vox> for crate::voxel::Vox {
        fn as_mut(&self) -> &mut crate::voxel::Vox {
            as_mut(self)
        }
    }
}

pub mod voxel;
