//! The shell: what `vita/src/main.rs` does a frame, for a tab and for the machine that builds the game.
//!
//! A frame is [`App::step`] (the commands the interface sent, the player, the world's streaming), the guest's
//! turn (which the page runs: [`App::guest_due`], [`App::heard`], [`App::say`]), then [`App::draw`]. The world
//! owns no threads (`ether_world`): a tab has one, so the jobs `World::plan` hands out are run here, as many as
//! a frame's budget holds ([`BUDGET_LOADING`] while the world is first read, [`BUDGET_PLAY`] after).
//!
//! What differs from the PS Vita's frame: the mods are the base mod's registry (a tab runs no QuickJS of the
//! game's), so no handler hears a break, a place or a tick; the world is not saved (the interface's settings are,
//! by the page); the leaf cards are drawn at every distance, as the reference draws them, and no governor thins
//! them.

use std::collections::HashMap;

use ether_interface::{channel, Command, Entry, Holds, Mode, Numbers, Pace, Pad as UiPad, Session, Slot as UiSlot, HOTBAR};
use ether_mod::Registry;
use ether_sim::brush::{self, Stroke, VoxelBrush};
use ether_sim::camera::Camera;
use ether_sim::clock::WorldClock;
use ether_sim::player::{self, Player};
use ether_sim::ray::{self, HitResult};
use ether_sim::Terrain;
use ether_voxel::glam::{IVec3, Vec2, Vec3};
use ether_voxel::vox::{Shape, Vox};
use ether_world::{Radius, World, Worker};
use pocket_web_wgpu::gpu::{Gpu, Screen};
use pocket_web_wgpu::overlay::Overlay;
use pocket_web_wgpu::task;
use serde_json::json;

use crate::mesh::ChunkGpu;
use crate::pack::Pictures;
use crate::render::{Drawn, Renderer, View, SLOTS};

/// Milliseconds of a frame the world's jobs may take while it is first read, and in play.
pub const BUDGET_LOADING: f64 = 12.0;
pub const BUDGET_PLAY: f64 = 4.0;
/// The screen: the PS Vita's.
pub const WIDTH: u32 = 960;
pub const HEIGHT: u32 = 544;
/// Samples a pixel of the scene.
pub const SAMPLES: u32 = 4;
/// Turns a second the interface's guest is offered, as the PS Vita's host offers them (`vita/src/interface.rs`).
pub const TURNS: u32 = 30;
/// The ground behind the interface while there is no world.
const GROUND: [f32; 3] = [0.047, 0.039, 0.035];
/// Frames over which the frame times are averaged.
const WINDOW: usize = 120;

/// The pad as the page reads it: PocketJS's bits and two sticks in −1…1, right and up positive.
#[derive(Clone, Copy, Default)]
pub struct Held {
    pub buttons: u32,
    pub left: [f32; 2],
    pub right: [f32; 2],
}

/// The world as the player's controller, ray and brush see it.
struct Ground<'a>(&'a mut World);

impl Terrain for Ground<'_> {
    fn triangles(&self, min: Vec3, max: Vec3, out: &mut Vec<[Vec3; 3]>) {
        self.0.triangles(min, max, out);
    }
    fn voxel(&self, p: IVec3) -> Option<Vox> {
        self.0.voxel(p)
    }
    fn edit(&mut self, p: IVec3, f: &mut dyn FnMut(&mut Vox)) -> bool {
        self.0.edit(p, |v| f(v))
    }
}

/// The world once the pack has been read: the chunks, and what draws them.
struct Live {
    world: World,
    worker: Worker,
    renderer: Renderer,
    chunks: HashMap<IVec3, ChunkGpu>,
    free: Vec<u32>,
    next_slot: u32,
    /// The world around the spawn has been meshed: the title stands.
    settled: bool,
    mesh_bytes: u64,
}

pub struct App {
    pub gpu: Gpu,
    pub screen: Screen,
    overlay: Overlay,
    session: Session,
    registry: Registry,
    live: Option<Live>,
    player: Player,
    camera: Camera,
    clock: WorldClock,
    hit: HitResult,
    ray_scratch: Vec<[Vec3; 3]>,
    safe: Vec3,
    rescues: u32,
    edits: u32,
    title_yaw: f32,
    last_slot: usize,
    /// Settings by control words: the sun's map, the leaf cards' share.
    shadows: bool,
    density: f32,
    show: [bool; 3],
    water_look: [f32; 4],
    /// Stick input the control words hold for a while (`walk=x,y,seconds`), and a stroke they queued.
    walk: Option<(Vec2, f32)>,
    stroke: Option<Stroke>,
    /// The interface's guest: turns owed (sixtieths of a second) and the buttons held since the last.
    guest: (u32, u32),
    pace: Pace,
    skipped: u32,
    touched: bool,
    prefs: Option<String>,
    pub trouble: String,
    // the frame's clock and its counts
    last: Option<f64>,
    began: f64,
    frames: u64,
    intervals: [f32; WINDOW],
    late: u32,
    cpu_ms: f32,
    world_ms: f32,
    drawn: Drawn,
    loaded_ms: f64,
    opened_at: f64,
}

fn publish_registry(reg: &Registry) {
    let st = unsafe { &mut channel().state };
    st.materials = reg.materials.iter().map(|m| Entry { id: m.id as u32, name: m.name.clone(), picture: m.tile as u32 }).collect();
    st.items = reg.items.iter().map(|i| Entry { id: i.id as u32, name: i.name.clone(), picture: i.icon as u32 }).collect();
    let mut hotbar = [UiSlot::default(); HOTBAR];
    for (i, s) in reg.hotbar.iter().take(HOTBAR).enumerate() {
        hotbar[i] = match s {
            Some(ether_mod::Slot::Brush { brush }) => UiSlot { holds: Holds::Material(brush.material as u32), count: 0 },
            Some(ether_mod::Slot::Item { item, count }) => UiSlot { holds: Holds::Item(*item as u32), count: *count as u32 },
            None => UiSlot::default(),
        };
    }
    st.hotbar = hotbar;
    st.mods = reg.mods.len() as u32;
}

/// The sim's controller from the registry's.
fn tune(player: &mut Player, reg: &Registry, unfly: bool) {
    let c = &reg.controller;
    let p = &mut player.ctl;
    p.jump_impulse = c.jump_impulse;
    p.acceleration = c.acceleration;
    p.max_slope_angle = c.max_slope.to_radians();
    p.gravity = c.gravity;
    p.gravity_scale = c.gravity_scale;
    p.look_sensitivity = c.look_sensitivity;
    p.stick_look_scale = c.stick_look;
    p.radius = c.radius;
    p.half_length = c.half_height;
    p.unfly_on_ground = c.unfly_on_ground && unfly;
}

fn radius(session: &Session) -> Radius {
    let h = session.view_distance() as i32;
    Radius { h, v: h.min(3) }
}

impl App {
    /// The shell with no world: its frames show the interface alone. `registry`: the base mod's, as JSON.
    pub fn open(gpu: Gpu, screen: Screen, registry: &str) -> Result<App, String> {
        let registry: Registry = serde_json::from_str(registry).map_err(|e| format!("the registry: {e}"))?;
        let overlay = Overlay::new(&gpu, screen.format);
        let mut session = Session::new();
        session.numbers_every = 2;
        publish_registry(&registry);
        let mut player = Player::new(Vec3::new(0.5, 48.0, 0.5));
        tune(&mut player, &registry, session.settings.unfly);
        let clock = WorldClock { daytime: registry.clock.start, day_length: registry.clock.day_length, time_inhabited: 0.0 };
        Ok(App {
            gpu,
            screen,
            overlay,
            session,
            registry,
            live: None,
            player,
            camera: Camera::default(),
            clock,
            hit: HitResult::default(),
            ray_scratch: Vec::new(),
            safe: Vec3::ZERO,
            rescues: 0,
            edits: 0,
            title_yaw: 0.0,
            last_slot: usize::MAX,
            shadows: true,
            density: 1.0,
            show: [true; 3],
            water_look: [0.04, 1.0, 0.24, 1.0],
            walk: None,
            stroke: None,
            guest: (0, 0),
            pace: Pace::new(),
            skipped: 0,
            touched: false,
            prefs: None,
            trouble: String::new(),
            last: None,
            began: 0.0,
            frames: 0,
            intervals: [1000.0 / 60.0; WINDOW],
            late: 0,
            cpu_ms: 0.0,
            world_ms: 0.0,
            drawn: Drawn::default(),
            loaded_ms: 0.0,
            opened_at: task::now(),
        })
    }

    /// Whether this GPU reads the pack's pictures as blocks.
    pub fn reads_blocks(&self) -> bool {
        self.gpu.features.contains(pocket_web_wgpu::wgpu::Features::TEXTURE_COMPRESSION_BC)
    }

    /// The pack has been read: the world starts to load around the spawn.
    pub fn enter(&mut self, pictures: Pictures) {
        let renderer = Renderer::new(&self.gpu, self.screen.format, self.screen.samples, pictures);
        let world = World::new(self.registry.worldgen.params(), self.registry.decor.params(), radius(&self.session));
        self.live = Some(Live { world, worker: Worker::new(), renderer, chunks: HashMap::new(), free: Vec::new(), next_slot: 0, settled: false, mesh_bytes: 0 });
    }

    pub fn entered(&self) -> bool {
        self.live.as_ref().is_some_and(|l| l.settled)
    }

    /// Every chunk around the last place the world was planned for is meshed.
    pub fn world_settled(&self) -> bool {
        self.live.as_ref().is_some_and(|l| l.settled && l.world.settled())
    }

    /// The start failed: the interface says why.
    pub fn fail(&mut self, why: &str) {
        self.trouble = why.into();
        self.session.mode = Mode::Error;
        let st = unsafe { &mut channel().state };
        st.message = why.into();
    }

    /// Words from a development tool or a page's address: `mode=play pos=x,y,z yaw=r pitch=r daytime=f fly=1
    /// walk=x,y,seconds stroke=break|place shadows=0|1 density=f water=f,f,f,f show=terrain,water,foliage`.
    pub fn control(&mut self, words: &str) {
        for word in words.split_ascii_whitespace() {
            let Some((k, v)) = word.split_once('=') else { continue };
            let list: Vec<f32> = v.split(',').filter_map(|x| x.parse().ok()).collect();
            let x = list.first().copied().unwrap_or(0.0);
            match k {
                "mode" => {
                    if let Some(m) = Mode::parse(v) {
                        self.session.mode = m;
                    }
                }
                "pos" if list.len() == 3 => {
                    self.player.position = Vec3::new(list[0], list[1], list[2]);
                    self.player.velocity = Vec3::ZERO;
                    self.safe = self.player.position;
                }
                "yaw" => self.player.yaw = x,
                "pitch" => self.player.pitch = x,
                "daytime" => self.clock.daytime = x.rem_euclid(1.0),
                "fly" => self.player.flying = x != 0.0,
                "walk" if list.len() >= 2 => self.walk = Some((Vec2::new(list[0], list[1]), list.get(2).copied().unwrap_or(3.0))),
                "stroke" => self.stroke = if v == "place" { Some(Stroke::Place) } else { Some(Stroke::Break) },
                "shadows" => self.shadows = x != 0.0,
                "density" => self.density = x.clamp(0.0, 1.0),
                "show" => self.show = [v.contains("terrain"), v.contains("water"), v.contains("foliage")],
                "water" if list.len() == 4 => self.water_look = [list[0], list[1], list[2], list[3]],
                _ => {}
            }
        }
    }

    // ---- the interface: a PocketJS guest the page runs, heard and answered through the channel

    /// A guest has opened the interface's channel: it is told the whole state on its next turn.
    pub fn interface_opened(&mut self) {
        unsafe { channel() }.open("pocket.overlay");
        self.guest = (60 / TURNS, 0);
        self.skipped = 0;
        self.pace = Pace::new();
        self.session.idle = false;
    }

    /// The guest went with its realm; until another opens the channel the pad keeps the flow.
    pub fn interface_closed(&mut self) {
        unsafe { channel() }.close();
    }

    /// The guest's turn in this frame: the sixtieths of a second it is for and the buttons it is handed, or `None`.
    /// A turn is offered `TURNS` times a second and taken when it is worth its cost (`Pace`); the turns passed over
    /// are time the interface's clock is owed, no more than three of them.
    pub fn guest_due(&mut self, touching: bool) -> Option<(u32, u32)> {
        let turn = 60 / TURNS;
        let (owed, buttons) = &mut self.guest;
        *owed = (*owed).min(2 * turn);
        if *owed < turn {
            return None;
        }
        *owed -= turn;
        let buttons = core::mem::take(buttons);
        let contact = touching || self.touched;
        self.touched = touching;
        if self.entered() && !self.pace.due(&self.session, buttons, contact) {
            self.skipped += 1;
            return None;
        }
        Some((turn * (1 + core::mem::take(&mut self.skipped).min(3)), buttons))
    }

    /// The line of state the guest has not seen.
    pub fn heard(&mut self) -> Option<String> {
        unsafe { channel() }.poll()
    }

    /// A line the guest sent: a command for the next step.
    pub fn say(&mut self, line: &str) {
        unsafe { channel() }.receive(line);
    }

    /// The settings the page kept from the last visit.
    pub fn prefs_stored(&mut self, text: &str) {
        unsafe { channel() }.state.prefs = text.into();
    }

    /// What the interface asked to have kept since the last call.
    pub fn prefs_take(&mut self) -> Option<String> {
        self.prefs.take()
    }

    pub fn overlay(&mut self, pixels: &[u8], width: u32, height: u32) -> Result<(), String> {
        self.overlay.write(&self.gpu, pixels, width, height)
    }

    pub fn overlay_hide(&mut self) {
        self.overlay.hide();
    }

    // ---- a frame

    /// The first half of a frame at `now` (milliseconds on a clock that goes forward): what the interface asked
    /// for, the player, the world. Returns the sixtieths of a second that passed.
    pub fn step(&mut self, now: f64, held: &Held) -> u32 {
        self.began = task::now();
        let interval = self.last.map(|last| (now - last) as f32).unwrap_or(1000.0 / 60.0);
        self.last = Some(now);
        let interval = if interval > 0.0 { interval } else { 1000.0 / 60.0 };
        self.intervals[self.frames as usize % WINDOW] = interval;
        if self.entered() && interval > 1.5 * 1000.0 / 60.0 {
            self.late += 1;
        }
        let ticks = ((interval * 0.06).round() as u32).clamp(1, 6);
        self.guest.0 += ticks;
        self.guest.1 |= held.buttons;
        let dt = (interval / 1000.0).clamp(0.001, 0.1);

        // What the interface asked for; what it wants kept comes back.
        let mut prefs = None;
        self.session.obey(|c| {
            if let Command::Prefs(text) = c {
                unsafe { channel() }.state.prefs = text.clone();
                prefs = Some(text);
            }
        });
        if prefs.is_some() {
            self.prefs = prefs;
        }

        let Some(live) = &mut self.live else {
            if self.trouble.is_empty() {
                let st = unsafe { &mut channel().state };
                st.message = "Reading the pack".into();
                st.progress = 0;
            }
            self.session.publish(unsafe { &mut channel().state });
            return ticks;
        };

        // The world around the spawn, read before the title stands; the interface says how far it is.
        if !live.settled {
            let tw = task::now();
            let center = World::chunk_index(self.player.position);
            while task::now() - tw < BUDGET_LOADING {
                let jobs = live.world.plan(center, 6, 4);
                if jobs.is_empty() {
                    break;
                }
                for job in jobs {
                    let done = live.worker.run(job);
                    if let Some((at, mesh)) = live.world.finish(done) {
                        upload(live, &self.gpu, at, &mesh);
                    }
                }
            }
            let st = live.world.stats();
            let want = ((2 * live.world.radius.h + 1).pow(2) * (2 * live.world.radius.v + 1)) as usize;
            let state = unsafe { &mut channel().state };
            state.message = format!("Generating chunks: {} of {}", st.meshed, want);
            state.progress = (st.meshed * 100 / want.max(1)).min(100) as u32;
            if live.world.settled() {
                live.settled = true;
                self.loaded_ms = task::now() - self.opened_at;
                // The spawn: above the ground at the origin (the reference spawns flying).
                let hit = ray::cast(&Ground(&mut live.world), Vec3::new(0.5, 47.0, 0.5), Vec3::NEG_Y, 95.0, &mut self.ray_scratch);
                if hit.is_hit && self.player.position == Vec3::new(0.5, 48.0, 0.5) {
                    self.player.position = hit.position + Vec3::new(0.0, 3.0, 0.0);
                }
                self.safe = self.player.position;
                self.session.loaded();
            }
            self.session.publish(state);
            return ticks;
        }

        // The player, as the PS Vita's frame moves it.
        let spad = UiPad { buttons: held.buttons, lx: held.left[0], ly: held.left[1], rx: held.right[0], ry: held.right[1] };
        let mut input = self.session.run(&spad, dt);
        if let Some((w, left)) = self.walk.as_mut() {
            input.move_x = w.x;
            input.move_y = w.y;
            *left -= dt;
            if *left <= 0.0 {
                self.walk = None;
            }
        }
        // A hotbar slot that holds a brush preset sets the brush.
        if self.session.slot != self.last_slot {
            self.last_slot = self.session.slot;
            if let Some(Some(ether_mod::Slot::Brush { brush })) = self.registry.hotbar.get(self.session.slot) {
                self.session.brush.material = brush.material as u32;
                self.session.brush.size = brush.size.round() as u32;
                self.session.brush.strength = brush.strength;
                self.session.brush.shape = if brush.shape() == Shape::Isosurface { ether_interface::Shape::Isosurface } else { ether_interface::Shape::Cube };
            }
        }
        self.player.ctl.unfly_on_ground = self.registry.controller.unfly_on_ground && self.session.settings.unfly;
        self.clock.day_length = self.session.day_length().unwrap_or(0.0);
        let playing = self.session.mode == Mode::Play;
        if playing {
            let rate = 1.8;
            let ppad = player::Pad {
                move_: Vec2::new(input.move_x, input.move_y),
                look: Vec2::new(input.yaw / rate, input.pitch / rate),
                touch_drag: Vec2::ZERO,
                jump: input.jump,
                sneak: input.sneak,
                sprint: input.sprint_pressed,
                attack: input.attack,
                use_item: input.use_item,
                hotbar: 0,
            };
            // The player collides with the meshes' triangles: until the chunk it is in and the one under it are
            // meshed there is nothing to stand on, and it waits. A fall far below the last place it stood brings it
            // back there.
            let here = World::chunk_index(self.player.position);
            let ready = live.world.is_meshed(here) && live.world.is_meshed(here - IVec3::Y);
            if ready || self.player.flying {
                self.player.step(&Ground(&mut live.world), &ppad, dt);
            }
            if self.player.grounded {
                self.safe = self.player.position;
            }
            if self.player.position.y < self.safe.y - 256.0 {
                self.player.position = self.safe + Vec3::new(0.0, 2.0, 0.0);
                self.player.velocity = Vec3::ZERO;
                self.player.flying = true;
                self.rescues += 1;
            }
            self.camera.distance.target = self.session.camera_distance();
            self.camera.base_fov = self.session.fov();
            self.camera.follow(&self.player, dt);
            self.hit = ray::cast(&Ground(&mut live.world), self.camera.eye, self.camera.forward(), ray::REACH, &mut self.ray_scratch);
            let stroke = if input.attack {
                Some(Stroke::Break)
            } else if input.use_item {
                Some(Stroke::Place)
            } else {
                self.stroke.take()
            };
            if let (Some(stroke), true) = (stroke, self.hit.is_hit) {
                let b = self.session.brush;
                let shape = if b.shape == ether_interface::Shape::Cube { Shape::Cube } else { Shape::Isosurface };
                let vb = VoxelBrush { shape, tex: b.material as u8, size: b.size as f32, strength: b.strength };
                self.edits += brush::apply(&mut Ground(&mut live.world), &self.hit, &vb, stroke).len() as u32;
            }
        } else {
            // Behind the title the eye turns slowly over the spawn; behind the menu it stays.
            if self.session.mode == Mode::Title {
                self.title_yaw += dt * 0.05;
                self.camera.rotation = ether_voxel::glam::Quat::from_euler(ether_voxel::glam::EulerRot::YXZ, self.title_yaw, -0.25, 0.0);
                self.camera.eye = self.player.position + Vec3::new(0.0, 6.0, 0.0);
            }
            self.hit = HitResult::default();
        }
        self.clock.tick(if playing || self.session.mode == Mode::Title { dt } else { 0.0 });

        // The world around the eye: what the workers finished, what left the radius, what is next.
        let tw = task::now();
        live.world.radius = radius(&self.session);
        let center = World::chunk_index(self.camera.eye);
        loop {
            let jobs = live.world.plan(center, 6, 4);
            if jobs.is_empty() {
                break;
            }
            for job in jobs {
                let done = live.worker.run(job);
                if let Some((at, mesh)) = live.world.finish(done) {
                    upload(live, &self.gpu, at, &mesh);
                }
            }
            if task::now() - tw > BUDGET_PLAY {
                break;
            }
        }
        for at in live.world.take_unloaded() {
            if let Some(c) = live.chunks.remove(&at) {
                live.mesh_bytes -= c.bytes;
                live.free.push(c.slot);
            }
        }
        self.world_ms += ((task::now() - tw) as f32 - self.world_ms) * 0.1;

        // What the interface shows.
        let st = unsafe { &mut channel().state };
        self.session.publish(st);
        st.health = 20;
        st.health_max = 20;
        st.flying = self.player.flying;
        st.sprinting = self.player.sprinting;
        if self.session.numbers_due() {
            let p = self.player.position;
            st.t = Numbers { minutes: (self.clock.daytime * 1440.0) as i32, x: (p.x * 10.0) as i32, y: (p.y * 10.0) as i32, z: (p.z * 10.0) as i32 };
        }
        if !self.session.settings.stats {
            st.stats.clear();
        } else if self.frames % 30 == 0 {
            let average = self.intervals.iter().sum::<f32>() / WINDOW as f32;
            st.stats = format!("{:.1} fps · {:.1} ms · late {} · {} chunks · {}k tris · {} draws", 1000.0 / average.max(0.1), self.cpu_ms, self.late, self.drawn.chunks, self.drawn.tris / 1000, self.drawn.draws);
        }
        ticks
    }

    /// The second half: the world, with the interface over it. Before the world stands, the interface alone.
    pub fn draw(&mut self) -> Result<(), String> {
        self.frames += 1;
        let Some(live) = self.live.as_mut().filter(|l| l.settled) else {
            let target = self.screen.frame(&self.gpu)?;
            let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
            drop(target.pass(&mut encoder, GROUND));
            self.overlay.draw(&mut encoder, &target);
            self.gpu.queue.submit([encoder.finish()]);
            target.present();
            return Ok(());
        };
        let aspect = self.screen.width as f32 / self.screen.height as f32;
        let projection = self.camera.projection(aspect, 0.1, 1000.0);
        let reach = (live.world.radius.h as f32 + 0.5) * 16.0;
        let view = View {
            eye: self.camera.eye,
            forward: self.camera.forward(),
            vp: projection * self.camera.view_rotation(),
            sun_dir: self.clock.sun_dir().normalize(),
            sun_light: self.clock.sun_strength() * ether_look::SUN_LIGHT,
            day: 1.0,
            seconds: self.clock.time_inhabited,
            reach,
            shadows: self.shadows,
            water_look: self.water_look,
            density: self.density,
            show: self.show,
        };
        self.drawn = live.renderer.frame(&self.gpu, &self.screen, &self.overlay, &live.chunks, &view)?;
        self.cpu_ms += ((task::now() - self.began) as f32 - self.cpu_ms) * 0.1;
        Ok(())
    }

    /// The run as a JSON object.
    pub fn status(&self) -> String {
        let average = self.intervals.iter().sum::<f32>() / WINDOW as f32;
        let worst = self.intervals.iter().fold(0.0f32, |a, &b| a.max(b));
        let p = &self.player;
        let mut status = json!({
            "target": "web",
            "stage": if self.entered() { "running" } else if self.trouble.is_empty() { "loading" } else { "error" },
            "settled": self.world_settled(),
            "mode": self.session.mode.name(),
            "frames": self.frames, "frameMs": average, "worstMs": worst, "late": self.late, "cpuMs": self.cpu_ms, "worldMs": self.world_ms,
            "loadMs": self.loaded_ms,
            "interface": {"open": unsafe { channel() }.is_open()},
            "player": {"pos": p.position.to_array(), "yaw": p.yaw, "pitch": p.pitch, "flying": p.flying, "grounded": p.grounded, "rescues": self.rescues},
            "eye": self.camera.eye.to_array(),
            "daytime": self.clock.daytime,
            "hit": {"is": self.hit.is_hit, "voxel": self.hit.voxel_pos.to_array(), "distance": self.hit.distance},
            "edits": self.edits,
            "shadows": self.shadows,
            "adapter": self.gpu.adapter,
            "trouble": self.trouble,
            "drawn": {"draws": self.drawn.draws, "tris": self.drawn.tris, "chunks": self.drawn.chunks, "reflected": self.drawn.reflected, "casters": self.drawn.casters},
        });
        if let Some(live) = &self.live {
            let st = live.world.stats();
            status["world"] = json!({"chunks": st.chunks, "generated": st.generated, "populated": st.populated, "meshed": st.meshed, "resident": live.chunks.len(), "radius": [live.world.radius.h, live.world.radius.v]});
            status["gpuBytes"] = json!({"meshes": live.mesh_bytes, "renderer": live.renderer.bytes});
            status["blocks"] = json!(live.renderer.pictures.blocks);
        }
        status.to_string()
    }
}

/// A meshed chunk onto the GPU, in a row of the table.
fn upload(live: &mut Live, gpu: &Gpu, at: IVec3, mesh: &ether_voxel::mesh::ChunkMesh) {
    if let Some(old) = live.chunks.remove(&at) {
        live.mesh_bytes -= old.bytes;
        live.free.push(old.slot);
    }
    let slot = match live.free.pop() {
        Some(s) => s,
        None if live.next_slot < SLOTS => {
            live.next_slot += 1;
            live.next_slot - 1
        }
        None => return,
    };
    let c = ChunkGpu::upload(gpu, at, slot, mesh);
    if c.is_empty() {
        live.free.push(slot);
        return;
    }
    live.renderer.place(gpu, slot, at);
    live.mesh_bytes += c.bytes;
    live.chunks.insert(at, c);
}
