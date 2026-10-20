//! The player in Ethertum's world: what moves, what it sees, what it edits.
//!
//! Ethertum runs its player through Bevy and avian3d: a dynamic capsule with
//! its rotation locked and no friction or bounce, steered each frame by
//! `client/character_controller.rs`, colliding with each chunk's terrain mesh.
//! This crate does the same without either:
//!
//! | Module | Does | Reference |
//! | --- | --- | --- |
//! | [`player`] | The controller's input rules, gravity and damping; the capsule moved through the terrain's triangles | `character_controller.rs`, avian3d's rigid body and shape caster |
//! | [`camera`] | Eye, rotation, field of view and the matrices a renderer takes | `sync_camera` |
//! | [`ray`] | The crosshair's ray against the terrain | `voxel_client::raycast` (avian's spatial query) |
//! | [`brush`] | Breaking and placing voxels around a hit | `voxel_client::raycast`, `VoxelBrush` |
//! | [`clock`] | Time of day and the sun | `client_world::tick_world`, `WorldInfo` |
//! | [`inventory`] | Item stacks, the inventory, the hotbar, health | `item/mod.rs`, `ClientPlayerInfo` |
//!
//! The world is reached through [`Terrain`], which the loaded world
//! implements: the triangles the player collides with, and voxels to read and
//! write.

pub mod brush;
pub mod camera;
pub mod clock;
pub mod geom;
pub mod inventory;
pub mod player;
pub mod ray;

pub use ether_voxel::vox::Vox;
pub use glam;
use glam::{IVec3, Vec3};

/// What the player needs from the loaded world.
pub trait Terrain {
    /// Appends to `out` every collision triangle, in world coordinates, whose bounds overlap `min..max`. A
    /// triangle may be appended that does not overlap (the player tests bounds again), but none that overlaps
    /// may be left out. The triangles are the terrain mesh's (`ChunkMesh::collision` moved by its chunk's
    /// corner): the surface net and cubes, not foliage or water, as the reference gives its colliders.
    fn triangles(&self, min: Vec3, max: Vec3, out: &mut Vec<[Vec3; 3]>);

    /// The voxel at `p`, if its chunk is loaded.
    fn voxel(&self, p: IVec3) -> Option<Vox>;

    /// Runs `f` on the voxel at `p`; false when its chunk is not loaded.
    fn edit(&mut self, p: IVec3, f: &mut dyn FnMut(&mut Vox)) -> bool;
}
