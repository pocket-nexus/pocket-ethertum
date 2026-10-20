//! What the mods define, as the engine reads it.
//!
//! The realm hands the registry over as JSON (names resolved to ids). The
//! world tables map onto `ether-voxel`'s own parameter types; the controller,
//! clock and sky tables carry the reference's values. [`Registry::diff`] says
//! which parts changed between two loads, and so what the engine redoes.

use ether_voxel::gen::Params;
use ether_voxel::populate::DecorParams;
use ether_voxel::vox::Shape;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MaterialKind {
    /// Part of the isosurface or a cube: drawn with its tile of the terrain atlas.
    Terrain,
    /// Drawn as water faces.
    Liquid,
}

/// A voxel material: what an isosurface or cube voxel with this id is drawn and named as.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Material {
    pub id: u8,
    pub name: String,
    /// Tile of the terrain atlas (`id - 1` unless the mod says otherwise).
    pub tile: u8,
    /// sRGB colour the interface shows for it.
    pub color: [u8; 3],
    pub kind: MaterialKind,
    /// Light it gives, 0..15.
    pub light: u8,
    /// Whether the hotbar's brushes may place it.
    pub hotbar: bool,
    /// The mod that registered it last.
    #[serde(rename = "mod")]
    pub module: String,
}

/// A foliage card: what a `leaves` or `grass` voxel with this id is drawn and named as.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Foliage {
    pub id: u8,
    pub name: String,
    /// Tile of the foliage atlas.
    pub tile: u8,
    pub color: [u8; 3],
    #[serde(rename = "mod")]
    pub module: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    /// The reference's numbering: names sorted, from 1.
    pub id: u16,
    pub name: String,
    /// Icon in the item atlas (64 px squares, left to right).
    pub icon: u8,
    pub stack: u8,
    #[serde(rename = "mod")]
    pub module: String,
}

/// Terrain generation (`ether_voxel::gen::Params`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Worldgen {
    pub seed: u32,
    pub octaves: u32,
    pub terrain_scale: f64,
    pub cave_scale: f64,
    pub height_div: f32,
    pub cave_amp: f32,
    pub sea_level: i32,
    pub water_iso: f32,
    pub stone: u8,
    pub water: u8,
}

impl Worldgen {
    pub fn params(&self) -> Params {
        Params {
            seed: self.seed,
            octaves: self.octaves,
            terrain_scale: self.terrain_scale,
            cave_scale: self.cave_scale,
            height_div: self.height_div,
            cave_amp: self.cave_amp,
            sea_level: self.sea_level,
            water_iso: self.water_iso,
            stone: self.stone,
            water: self.water,
        }
    }
}

impl From<&Params> for Worldgen {
    fn from(p: &Params) -> Worldgen {
        Worldgen { seed: p.seed, octaves: p.octaves, terrain_scale: p.terrain_scale, cave_scale: p.cave_scale, height_div: p.height_div, cave_amp: p.cave_amp, sea_level: p.sea_level, water_iso: p.water_iso, stone: p.stone, water: p.water }
    }
}

/// Population (`ether_voxel::populate::DecorParams`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Decor {
    pub seed: u32,
    pub vines_per256: f32,
    pub vine_max: f32,
    pub trees_per256: f32,
    pub bush: f64,
    pub fern: f64,
    pub rose: f64,
    pub sand_below: i32,
    pub sand_noise: f64,
}

impl Decor {
    pub fn params(&self) -> DecorParams {
        DecorParams {
            seed: self.seed,
            vines_per_256: self.vines_per256,
            vine_max: self.vine_max,
            trees_per_256: self.trees_per256,
            bush: self.bush,
            fern: self.fern,
            rose: self.rose,
            sand_below: self.sand_below,
            sand_noise: self.sand_noise,
        }
    }
}

impl From<&DecorParams> for Decor {
    fn from(p: &DecorParams) -> Decor {
        Decor { seed: p.seed, vines_per256: p.vines_per_256, vine_max: p.vine_max, trees_per256: p.trees_per_256, bush: p.bush, fern: p.fern, rose: p.rose, sand_below: p.sand_below, sand_noise: p.sand_noise }
    }
}

/// The character controller (`client/character_controller.rs`, the player's capsule in
/// `net/netproc_client.rs`, the field of view in `client/settings.rs`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Controller {
    /// Upward speed a jump sets, m/s.
    pub jump_impulse: f32,
    /// m/s² along the input.
    pub acceleration: f32,
    /// m/s², times `gravity_scale` while not flying (avian's default 9.81, scale 2).
    pub gravity: f32,
    pub gravity_scale: f32,
    /// Acceleration factors: sprinting, sneaking, in the air.
    pub sprint: f32,
    pub sneak: f32,
    pub air_control: f32,
    /// Steepest ground, degrees.
    pub max_slope: f32,
    /// Fraction of horizontal speed left after one second: on the ground, in the air, flying.
    pub ground_damping: f32,
    pub air_damping: f32,
    pub fly_damping: f32,
    /// Radians per mouse count; a stick turns `stick_look` times that per unit per frame.
    pub look_sensitivity: f32,
    pub stick_look: f32,
    /// Field of view, degrees, and what sprinting adds.
    pub fov: f32,
    pub sprint_fov: f32,
    /// Seconds within which a second jump press toggles flying.
    pub double_tap: f32,
    /// Seconds between jumps while the button is held.
    pub jump_cooldown: f32,
    pub unfly_on_ground: bool,
    /// The capsule: radius, half the length of its middle, and the eye above its centre.
    pub radius: f32,
    pub half_height: f32,
    pub eye_height: f32,
}

impl Default for Controller {
    fn default() -> Controller {
        Controller {
            jump_impulse: 7.0,
            acceleration: 50.0,
            gravity: 9.81,
            gravity_scale: 2.0,
            sprint: 2.0,
            sneak: 0.3,
            air_control: 0.2,
            max_slope: 45.0,
            ground_damping: 0.0001,
            air_damping: 0.07,
            fly_damping: 0.01,
            look_sensitivity: 0.003,
            stick_look: 10.0,
            fov: 85.0,
            sprint_fov: 20.0,
            double_tap: 0.3,
            jump_cooldown: 0.3,
            unfly_on_ground: true,
            radius: 0.3,
            half_height: 0.9,
            eye_height: 0.8,
        }
    }
}

/// The day (`client/client_world.rs` `WorldInfo`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Clock {
    /// Time of day a new world starts at, 0..1. The sun's angle is `daytime · 2π`: it rises at 0, stands
    /// highest at 0.25 and sets at 0.5.
    pub start: f32,
    /// Seconds in a day.
    pub day_length: f32,
    /// World ticks per second.
    pub tick_rate: f32,
}

impl Default for Clock {
    fn default() -> Clock {
        Clock { start: 0.15, day_length: 1440.0, tick_rate: 20.0 }
    }
}

/// Fog and sun (`client/game_client.rs` `ClientInfo` defaults).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Sky {
    /// sRGB, 0..1.
    pub fog_color: [f32; 3],
    /// Metres at which the fog hides the scene.
    pub fog_visibility: f32,
    pub inscattering: [f32; 3],
    pub extinction: [f32; 3],
    /// Atmospheric fog (inscattering and extinction) rather than squared-exponential.
    pub atmospheric: bool,
    /// The sun's illuminance factor (×1000 lux in the reference).
    pub sun_illuminance: f32,
    pub shadows: bool,
}

impl Default for Sky {
    fn default() -> Sky {
        Sky {
            fog_color: [0.0, 0.666, 1.0],
            fog_visibility: 1200.0,
            inscattering: [110.0 / 255.0, 230.0 / 255.0, 1.0],
            extinction: [0.35, 0.5, 0.66],
            atmospheric: true,
            sun_illuminance: 20.0,
            shadows: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Brush {
    pub material: u8,
    /// `ether_voxel::vox::Shape` as its number.
    pub shape: u8,
    pub size: f32,
    pub strength: f32,
}

impl Brush {
    pub fn shape(&self) -> Shape {
        Shape::from_u8(self.shape)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Slot {
    Brush { brush: Brush },
    Item { item: u16, count: u8 },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hooks {
    #[serde(rename = "break")]
    pub break_: usize,
    pub place: usize,
    pub tick: usize,
}

/// Everything the loaded mods define.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Registry {
    /// Mods in load order.
    pub mods: Vec<String>,
    pub materials: Vec<Material>,
    pub foliage: Vec<Foliage>,
    pub items: Vec<Item>,
    pub worldgen: Worldgen,
    pub decor: Decor,
    pub controller: Controller,
    pub clock: Clock,
    pub sky: Sky,
    /// Nine slots at most; `None` is an empty slot.
    pub hotbar: Vec<Option<Slot>>,
    /// Command names, sorted.
    pub commands: Vec<String>,
    /// How many handlers each event has.
    pub hooks: Hooks,
}

impl Default for Registry {
    /// No mods: the reference's world, controller, clock and sky, and nothing registered.
    fn default() -> Registry {
        Registry {
            mods: Vec::new(),
            materials: Vec::new(),
            foliage: Vec::new(),
            items: Vec::new(),
            worldgen: Worldgen::from(&Params::default()),
            decor: Decor::from(&DecorParams::default()),
            controller: Controller::default(),
            clock: Clock::default(),
            sky: Sky::default(),
            hotbar: Vec::new(),
            commands: Vec::new(),
            hooks: Hooks::default(),
        }
    }
}

impl Registry {
    pub fn material(&self, id: u8) -> Option<&Material> {
        self.materials.iter().find(|m| m.id == id)
    }

    pub fn material_named(&self, name: &str) -> Option<&Material> {
        self.materials.iter().find(|m| m.name == name)
    }

    pub fn foliage(&self, id: u8) -> Option<&Foliage> {
        self.foliage.iter().find(|m| m.id == id)
    }

    pub fn item(&self, id: u16) -> Option<&Item> {
        self.items.iter().find(|m| m.id == id)
    }

    /// What differs from `old`. Which mod registered an entry is not a difference.
    pub fn diff(&self, old: &Registry) -> Changes {
        fn strip<T: Clone>(v: &[T], f: impl Fn(&mut T)) -> Vec<T> {
            v.iter()
                .cloned()
                .map(|mut x| {
                    f(&mut x);
                    x
                })
                .collect()
        }
        let mats = |r: &Registry| strip(&r.materials, |m| m.module.clear());
        let kinds = |r: &Registry| r.materials.iter().map(|m| (m.id, m.kind, m.light)).collect::<Vec<_>>();
        Changes {
            materials: mats(self) != mats(old),
            remesh_materials: kinds(self) != kinds(old),
            foliage: strip(&self.foliage, |m| m.module.clear()) != strip(&old.foliage, |m| m.module.clear()),
            items: strip(&self.items, |m| m.module.clear()) != strip(&old.items, |m| m.module.clear()),
            worldgen: self.worldgen != old.worldgen,
            decor: self.decor != old.decor,
            player: self.controller != old.controller,
            clock: self.clock != old.clock,
            sky: self.sky != old.sky,
            hotbar: self.hotbar != old.hotbar,
            hooks: self.hooks != old.hooks || self.commands != old.commands,
        }
    }
}

/// Which parts of the registry a load changed. `docs/MODDING.md` lists what the engine redoes for each.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// Any material's tile, colour, name, kind, light or hotbar flag; or a material added or removed.
    pub materials: bool,
    /// A material's kind or light: meshes depend on these.
    pub remesh_materials: bool,
    pub foliage: bool,
    pub items: bool,
    pub worldgen: bool,
    pub decor: bool,
    /// The character controller.
    pub player: bool,
    pub clock: bool,
    pub sky: bool,
    pub hotbar: bool,
    /// Event handlers or commands.
    pub hooks: bool,
}

impl Changes {
    pub fn any(&self) -> bool {
        *self != Changes::default()
    }

    /// Chunks the player has not edited are generated and populated again.
    pub fn regenerate(&self) -> bool {
        self.worldgen || self.decor
    }

    /// Every loaded chunk is meshed again.
    pub fn remesh(&self) -> bool {
        self.regenerate() || self.remesh_materials
    }

    /// Every part, for a first load.
    pub fn all() -> Changes {
        Changes { materials: true, remesh_materials: true, foliage: true, items: true, worldgen: true, decor: true, player: true, clock: true, sky: true, hotbar: true, hooks: true }
    }
}
