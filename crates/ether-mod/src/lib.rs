//! Mods for Pocket Ethertum, in QuickJS.
//!
//! A mod is a `main.js` that receives one object, `ether`, and registers
//! data and handlers through it. The base game is itself a mod
//! (`mods/base/main.js`, compiled in as [`BASE`]): Ethertum's materials, items,
//! world generation, population, controller, clock, sky and hotbar.
//!
//! | Part | What it holds |
//! | --- | --- |
//! | [`qjs`] | A QuickJS runtime and context of the mods' own: memory limit, time budget per call, located exceptions |
//! | [`registry`] | What the mods define, as the engine reads it; [`Registry::diff`] and [`Changes`] |
//! | [`Mods`] | Loading and reloading, `break` / `place` / `tick` events, commands, the edits they queue |
//!
//! The engine keeps the per-voxel work native: generation, population and
//! meshing read the registry's parameters, never a script. Scripts run at
//! events and a few times a second. `docs/MODDING.md` describes the API and
//! what each kind of change makes the engine redo.

mod mods;
pub mod qjs;
pub mod registry;

pub use mods::{base, discover, Action, BlockEvent, Config, During, Edit, LogLine, ModError, Mods, NoWorld, Outcome, PlayerState, Source, WorldView, BASE};
pub use registry::{Brush, Changes, Clock, Controller, Decor, Foliage, Item, Material, MaterialKind, Registry, Sky, Slot, Worldgen};
