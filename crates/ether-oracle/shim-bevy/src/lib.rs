//! Stand-ins for what `ref/ethertum/src/voxel/{vox,chunk,worldgen,meshgen}.rs`
//! take from Bevy: its math (Bevy's math is glam), `default()`, and the entity,
//! handle and mesh types a chunk carries but the oracle never uses.

pub mod math {
    pub use glam::*;
}

pub mod prelude {
    pub use ether_oracle_derive::Reflect;
    pub use glam::*;
    pub use std::path::Path;

    pub fn default<T: Default>() -> T {
        T::default()
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Entity(u32);

    impl Entity {
        pub const PLACEHOLDER: Entity = Entity(u32::MAX);
    }

    pub struct Mesh;

    pub struct Handle<T>(core::marker::PhantomData<fn() -> T>);

    impl<T> Default for Handle<T> {
        fn default() -> Self {
            Handle(core::marker::PhantomData)
        }
    }

    impl<T> Clone for Handle<T> {
        fn clone(&self) -> Self {
            Handle(core::marker::PhantomData)
        }
    }
}
