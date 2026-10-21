//! Pocket Ethertum on PS Vita.
//!
//! A frame at 960 × 544, sixty times a second: the sky, then the chunks
//! around the eye, streamed by `ether_world::World` (generation and meshing on
//! two worker threads, population on this one) and drawn by [`scene::Scene`];
//! then the interface, the PocketJS guest of `ui/` ([`interface`]).
//!
//! The player is `ether_sim` (Ethertum's character controller against the
//! terrain's triangles), its ray and brush edit the world, and the flow
//! (loading, title, play, menu) is `ether_interface::Session`, as on every
//! device. Mods (`ether_mod`, a QuickJS realm of their own) set the world's
//! parameters, the materials and the hotbar, hear breaks, places and ticks,
//! and are read again whenever their files change ([`mods`]).
//!
//! Development loop over PocketJS's wired debug transport: `host0:ether/boot.json`
//! holds the start switches, `host0:ether/control.json` steers the run, and
//! status receipts carry frame timings and the world's counts under `engine`.

mod gpu;
mod heap;
mod hostfs;
mod interface;
mod mods;
mod paths;
mod post;
mod scene;
mod shadow;
mod textures;

use std::sync::mpsc;
use std::time::{Duration, Instant};

use ether_interface::{channel, Command, Entry, Holds, Mode, Numbers, Session, Slot as UiSlot, HOTBAR};
use ether_mod::{Action, BlockEvent, Mods, Outcome, PlayerState, Registry, WorldView};
use ether_sim::brush::{self, Stroke, VoxelBrush};
use ether_sim::camera::Camera;
use ether_sim::clock::WorldClock;
use ether_sim::player::{self, Player};
use ether_sim::ray::{self, HitResult};
use ether_sim::Terrain;
use ether_voxel::glam::{IVec3, Vec2, Vec3};
use ether_voxel::vox::{Shape, Vox};
use ether_world::{save, Done, Job, Radius, World, Worker};
use pocket_vita_gxm::target::Fence;
use pocketjs_vita::{dev, dev_protocol::Op, devmenu::Action as MenuAction, graphics, input};
use serde_json::{json, Value};
use vita2d_sys as g;

#[no_mangle]
#[used]
pub static sceUserMainThreadStackSize: u32 = 1024 * 1024;

#[no_mangle]
#[used]
pub static _newlib_heap_size_user: u32 = 112 * 1024 * 1024;

extern "C" {
    fn scePowerSetArmClockFrequency(freq: i32) -> i32;
    fn scePowerSetBusClockFrequency(freq: i32) -> i32;
    fn scePowerSetGpuClockFrequency(freq: i32) -> i32;
    fn scePowerSetGpuXbarClockFrequency(freq: i32) -> i32;
    fn scePowerGetArmClockFrequency() -> i32;
    fn sceDisplayGetVcount() -> i32;
    fn sceDisplayWaitVblankStart() -> i32;
    fn sceKernelPowerTick(kind: i32) -> i32;
}

/// Bytes of the parameter buffer, where GXM keeps a scene's transformed vertices until its tiles are drawn. When
/// a scene fills it, GXM draws what it has and starts again at several times the cost; the default of 16 MB
/// fills near 150 000 triangles.
const PARAMETER_BUFFER: u32 = 32 * 1024 * 1024;

extern "C" {
    fn __real_sceGxmInitialize(params: *const g::SceGxmInitializeParams) -> i32;
}

/// vita2d's start of GXM, with a larger parameter buffer (see `build.rs`).
///
/// # Safety
/// Called by vita2d in place of `sceGxmInitialize`, with its parameters.
#[no_mangle]
pub unsafe extern "C" fn __wrap_sceGxmInitialize(params: *const g::SceGxmInitializeParams) -> i32 {
    let mut p = *params;
    p.parameterBufferSize = PARAMETER_BUFFER;
    __real_sceGxmInitialize(&p)
}

/// vita2d's pool of temporary vertices: the interface and the Devkit menu draw from it. A frame takes half of
/// it, in turn, so no frame writes vertices the GPU may still read.
const POOL_BYTES: u32 = 2 * 1024 * 1024;
/// ARM, bus, GPU and GPU crossbar clocks (MHz).
const CLOCKS: [i32; 4] = [444, 222, 222, 166];
/// Bytes for chunk meshes on the GPU.
const HEAP_BYTES: usize = 64 * 1024 * 1024;

unsafe fn set_clocks() {
    scePowerSetArmClockFrequency(CLOCKS[0]);
    scePowerSetBusClockFrequency(CLOCKS[1]);
    scePowerSetGpuClockFrequency(CLOCKS[2]);
    scePowerSetGpuXbarClockFrequency(CLOCKS[3]);
}

/// Display refreshes from the count `then` to `now`. The counter has 16 bits and wraps after 18 minutes.
fn refreshes(now: i32, then: i32) -> i32 {
    now.wrapping_sub(then) & 0xffff
}

/// Rolling frame statistics over the last `N` frames.
struct Timing {
    ms: [f32; Timing::N],
    at: usize,
    late: u32,
    frames: u32,
}

impl Timing {
    const N: usize = 240;
    fn push(&mut self, ms: f32, late: bool) {
        self.ms[self.at] = ms;
        self.at = (self.at + 1) % Self::N;
        self.frames += 1;
        self.late += late as u32;
    }
    fn avg(&self) -> f32 {
        self.ms.iter().sum::<f32>() / Self::N as f32
    }
    fn worst(&self) -> f32 {
        self.ms.iter().fold(0.0, |a, &b| a.max(b))
    }
}

/// Generation and meshing threads: jobs in, results out.
struct Workers {
    jobs: Vec<mpsc::Sender<Job>>,
    done: mpsc::Receiver<(Done, f32)>,
    next: usize,
}

impl Workers {
    fn new(n: usize) -> Workers {
        let (done_tx, done) = mpsc::channel();
        let mut jobs = Vec::new();
        for i in 0..n {
            let (tx, rx) = mpsc::channel::<Job>();
            let done_tx = done_tx.clone();
            std::thread::Builder::new()
                .name(format!("ether-worker-{i}"))
                .stack_size(256 * 1024)
                .spawn(move || {
                    let mut w = Worker::new();
                    while let Ok(job) = rx.recv() {
                        let t = Instant::now();
                        let d = w.run(job);
                        if done_tx.send((d, t.elapsed().as_secs_f32() * 1000.0)).is_err() {
                            return;
                        }
                    }
                })
                .expect("worker thread");
            jobs.push(tx);
        }
        Workers { jobs, done, next: 0 }
    }

    fn send(&mut self, job: Job) {
        let _ = self.jobs[self.next % self.jobs.len()].send(job);
        self.next += 1;
    }
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

/// The world as mods read it.
struct Seen<'a>(&'a World);

impl WorldView for Seen<'_> {
    fn voxel(&self, p: IVec3) -> Option<Vox> {
        self.0.voxel(p)
    }
}

/// Remote control: `host0:ether/control.json`, polled off the render thread.
fn control_watcher() -> mpsc::Receiver<Value> {
    let (tx, rx) = mpsc::channel();
    let _ = std::thread::Builder::new().name("ether-control".into()).stack_size(128 * 1024).spawn(move || {
        let path = format!("{}/control.json", paths::HOST);
        let mut last = hostfs::read(&path, 64 * 1024).unwrap_or_default();
        loop {
            std::thread::sleep(Duration::from_millis(300));
            if let Some(bytes) = hostfs::read(&path, 64 * 1024) {
                if bytes != last {
                    last = bytes.clone();
                    if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
                        if tx.send(v).is_err() {
                            return;
                        }
                    }
                }
            }
        }
    });
    rx
}

/// `{"fetch": name}`: copies `host0:ether/outbox/<name>` to the data folder on the card on a thread of its own (a
/// standalone package for VitaShell to install, for one) and answers in `outbox/<name>.done`.
fn fetch(name: &str) {
    use std::io::{Read, Write};
    let (from, to) = (format!("{}/outbox/{name}", paths::HOST), format!("{}/{name}", paths::DATA));
    let _ = std::thread::Builder::new().name("ether-fetch".into()).stack_size(64 * 1024).spawn(move || {
        let copied = (|| -> std::io::Result<u64> {
            std::fs::create_dir_all(paths::DATA)?;
            let mut src = std::fs::File::open(&from)?;
            let mut dst = std::fs::File::create(&to)?;
            let mut buf = vec![0u8; 256 * 1024];
            let mut bytes = 0u64;
            loop {
                let n = src.read(&mut buf)?;
                if n == 0 {
                    return Ok(bytes);
                }
                dst.write_all(&buf[..n])?;
                bytes += n as u64;
            }
        })();
        let said = match copied {
            Ok(bytes) => format!("{bytes} bytes at {to}"),
            Err(e) => format!("failed: {e}"),
        };
        let _ = hostfs::write(&format!("{from}.done"), said.as_bytes());
    });
}

/// The sky and the frame's light from the time of day, as the reference lights its world: the sun's light turns
/// about the world's z axis with the day and its strength is sin² of its height (none at night), while the sky
/// picture and the light it casts (the reference's environment light, at a fixed intensity) stay as they are all
/// day and all night.
struct Sky {
    sun_dir: Vec3,
    sun: f32,
    zenith: [f32; 3],
    horizon: [f32; 3],
    up: [f32; 3],
    down: [f32; 3],
    tint: [f32; 3],
    night: [f32; 3],
}

fn sky(clock: &WorldClock, reg: &Registry, picture_horizon: [f32; 3]) -> Sky {
    let _ = reg;
    Sky {
        sun_dir: clock.sun_dir().normalize(),
        sun: clock.sun_strength(),
        zenith: [0.30, 0.52, 0.82],
        horizon: picture_horizon,
        up: [0.58, 0.64, 0.74],
        down: [0.30, 0.29, 0.27],
        tint: [1.0; 3],
        night: [0.0; 3],
    }
}

/// What the interface shows of the registry: materials, items and the hotbar.
fn publish_registry(reg: &Registry) {
    let st = unsafe { &mut channel().state };
    st.materials = reg.materials.iter().map(|m| Entry { id: m.id as u32, name: m.name.clone(), picture: m.tile as u32 }).collect();
    st.items = reg.items.iter().map(|i| Entry { id: i.id as u32, name: i.name.clone(), picture: i.icon as u32 }).collect();
    let mut hotbar = [UiSlot::default(); HOTBAR];
    for (i, s) in reg.hotbar.iter().take(HOTBAR).enumerate() {
        hotbar[i] = match s {
            Some(ether_mod::registry::Slot::Brush { brush }) => UiSlot { holds: Holds::Material(brush.material as u32), count: 0 },
            Some(ether_mod::registry::Slot::Item { item, count }) => UiSlot { holds: Holds::Item(*item as u32), count: *count as u32 },
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

fn player_state(p: &Player, slot: usize) -> PlayerState {
    PlayerState { position: p.position.to_array(), velocity: p.velocity.to_array(), yaw: p.yaw, pitch: p.pitch, flying: p.flying, grounded: p.grounded, hotbar: slot }
}

/// Applies what a mod's handler asked for; the edits remesh what they change.
fn obey_mods(out: Outcome, world: &mut World, player: &mut Player, clock: &mut WorldClock, notices: &mut Vec<String>) {
    for e in out.edits {
        world.edit(e.pos, |v| e.apply(v));
    }
    for a in out.actions {
        match a {
            Action::Time(t) => clock.daytime = t.rem_euclid(1.0),
            Action::Teleport(p) => {
                player.position = Vec3::from_array(p);
                player.velocity = Vec3::ZERO;
            }
            Action::Fly(on) => player.flying = on,
        }
    }
    for e in out.errors {
        notices.push(e.to_string());
    }
    if let Some(r) = out.reply {
        notices.push(r);
    }
}

/// Writes the edited chunks and the player to the card.
fn save_world(world: &World, player: &Player, clock: &WorldClock, slot: usize) -> usize {
    let _ = std::fs::create_dir_all(paths::SAVE_DIR);
    let mut index = String::new();
    let kept = world.kept();
    for (at, v) in &kept {
        let name = format!("c_{}_{}_{}.bin", at.x, at.y, at.z);
        paths::write_atomic(&format!("{}/{name}", paths::SAVE_DIR), &save::encode(v));
        index.push_str(&name);
        index.push('\n');
    }
    paths::write_atomic(&format!("{}/index.txt", paths::SAVE_DIR), index.as_bytes());
    let p = json!({"pos": player.position.to_array(), "yaw": player.yaw, "pitch": player.pitch, "flying": player.flying, "daytime": clock.daytime, "slot": slot});
    paths::write_atomic(&format!("{}/player.json", paths::SAVE_DIR), p.to_string().as_bytes());
    kept.len()
}

/// Reads the edited chunks back; returns the player's saved state.
fn load_world(world: &mut World) -> (usize, Value) {
    let index = std::fs::read_to_string(format!("{}/index.txt", paths::SAVE_DIR)).unwrap_or_default();
    let mut n = 0;
    for name in index.lines() {
        let parts: Vec<i32> = name.trim_start_matches("c_").trim_end_matches(".bin").split('_').filter_map(|s| s.parse().ok()).collect();
        if parts.len() != 3 {
            continue;
        }
        if let Some(v) = std::fs::read(format!("{}/{name}", paths::SAVE_DIR)).ok().and_then(|b| save::decode(&b)) {
            world.restore(IVec3::new(parts[0], parts[1], parts[2]), v);
            n += 1;
        }
    }
    let player = std::fs::read(format!("{}/player.json", paths::SAVE_DIR)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
    (n, player)
}

fn main() {
    unsafe {
        let live = cfg!(feature = "usb-debug");
        // Development builds take start switches from the USB share: {"title": false, "msaa": 0 | 2 | 4,
        // "play": true (skip the title), "fresh": true (ignore the save), "shadows": true (start with the sun's
        // map; off by default, `ctl` turns it on and off)}.
        let boot: Value = if live { hostfs::read(&format!("{}/boot.json", paths::HOST), 4096).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null) } else { Value::Null };
        let samples = boot["msaa"].as_u64().unwrap_or(4);
        if !(live && boot["title"] == Value::Bool(false)) {
            pocket3d_title::vita::play();
        }
        let msaa = match samples {
            4 => g::SceGxmMultisampleMode_SCE_GXM_MULTISAMPLE_4X,
            2 => g::SceGxmMultisampleMode_SCE_GXM_MULTISAMPLE_2X,
            _ => g::SceGxmMultisampleMode_SCE_GXM_MULTISAMPLE_NONE,
        };
        if g::vita2d_init_advanced_with_msaa(POOL_BYTES, msaa) < 0 {
            pocketjs_vita::vita_log(format_args!("ether: vita2d did not start"));
            return;
        }
        if let Err(e) = graphics::init_with_pool(POOL_BYTES) {
            pocketjs_vita::vita_log(format_args!("ether: graphics {e}"));
            return;
        }
        set_clocks();
        input::init();
        let mut dev = dev::Host::new();
        let _ = std::fs::create_dir_all(paths::DATA);

        // The interface comes up first: it shows the loading and a failure.
        let mut ui = interface::Ui::boot();
        let mut session = Session::new();
        channel().state.prefs = std::fs::read_to_string(paths::PREFS_FILE).unwrap_or_default();
        let mut frame_no: u64 = 0;
        let loading = |ui: &mut interface::Ui, dev: &mut dev::Host, message: &str, progress: u32, frame_no: &mut u64| {
            let st = &mut channel().state;
            st.mode = Mode::Loading;
            st.message.clear();
            st.message.push_str(message);
            st.progress = progress;
            ui.turn(interface::TURN, &interface::NEUTRAL, true, None);
            graphics::begin_frame(0xff0c_0a09);
            ui.draw();
            dev.overlay();
            graphics::present();
            dev.engine = json!({"stage": "loading", "message": message, "progress": progress, "interface": {"up": ui.live(), "error": ui.error}});
            dev.publish(*frame_no as u32, "ether");
            serve(dev, *frame_no as u32, MenuAction::None);
            *frame_no += 1;
        };

        // ------------------------------------------------------------------------------------------ mods
        loading(&mut ui, &mut dev, "Reading mods", 0, &mut frame_no);
        let mut mods = Mods::new(ether_mod::Config::default());
        let sources = mods::read();
        let (_, errors) = mods.load_lenient(&sources);
        let mut notices: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
        let mod_reloads = if live { mods::watch(&sources) } else { mpsc::channel().1 };
        publish_registry(mods.registry());

        // ------------------------------------------------------------------------------------------ pictures and programs
        loading(&mut ui, &mut dev, "Reading the pictures", 2, &mut frame_no);
        let pack = [paths::PACK_HOST, paths::PACK_APP].iter().filter(|p| live || !p.starts_with("host0")).find_map(|p| hostfs::read(p, 64 << 20));
        let Some(pack) = pack else { fail(&mut ui, &mut dev, "ether.pack is missing: run `bun tools/vita.ts sync`".into()) };
        let textures = match textures::Textures::load(&pack) {
            Ok(t) => t,
            Err(e) => fail(&mut ui, &mut dev, e),
        };
        drop(pack);
        loading(&mut ui, &mut dev, "Building the programs", 4, &mut frame_no);
        let mut gpu = match gpu::Gpu::new(live) {
            Ok(g) => g,
            Err(e) => fail(&mut ui, &mut dev, e),
        };
        let t_programs = Instant::now();
        let mut scene = match scene::Scene::new(&mut gpu, msaa, HEAP_BYTES, textures) {
            Ok(s) => s,
            Err(e) => fail(&mut ui, &mut dev, e),
        };
        let target_msaa = match samples {
            4 => pocket_vita_gxm::target::Msaa::X4,
            2 => pocket_vita_gxm::target::Msaa::X2,
            _ => pocket_vita_gxm::target::Msaa::None,
        };
        // {"profile": true}: the scene goes to a target of its own, timed by its own notice.
        let profile = live && boot["profile"] == Value::Bool(true);
        let mut post = match post::Post::new(&mut gpu, target_msaa, profile) {
            Ok(p) => p,
            Err(e) => fail(&mut ui, &mut dev, e),
        };
        let mut shadow = match shadow::Shadow::new(&mut gpu) {
            Ok(s) => s,
            Err(e) => fail(&mut ui, &mut dev, e),
        };
        shadow.timed = profile;
        gpu.finish();
        let programs_ms = t_programs.elapsed().as_millis() as u64;

        // ------------------------------------------------------------------------------------------ the world
        let reg = mods.registry();
        let radius = |s: &Session| {
            let h = s.view_distance() as i32;
            Radius { h, v: h.min(3) }
        };
        let mut world = World::new(reg.worldgen.params(), reg.decor.params(), radius(&session));
        let (restored, saved) = if boot["fresh"] == Value::Bool(true) { (0, Value::Null) } else { load_world(&mut world) };
        let mut workers = Workers::new(2);
        let control = if live { control_watcher() } else { mpsc::channel().1 };
        let mut player = Player::new(Vec3::new(0.5, 48.0, 0.5));
        let mut placed = false;
        let mut camera = Camera::default();
        let mut clock = WorldClock { daytime: reg.clock.start, day_length: reg.clock.day_length, time_inhabited: 0.0 };
        if let Some(t) = saved["daytime"].as_f64() {
            clock.daytime = t as f32;
        }
        tune(&mut player, reg, session.settings.unfly);
        let saved_pos = saved["pos"].as_array().map(|a| Vec3::new(a[0].as_f64().unwrap_or(0.0) as f32, a[1].as_f64().unwrap_or(48.0) as f32, a[2].as_f64().unwrap_or(0.0) as f32));
        let spawn_center = World::chunk_index(saved_pos.unwrap_or(player.position));

        // Load around the spawn: the interface shows how far it is.
        let t_load = Instant::now();
        loop {
            while let Ok((done, _)) = workers.done.try_recv() {
                if let Some((at, mesh)) = world.finish(done) {
                    scene.upload(at, &mesh, frame_no);
                }
            }
            for job in world.plan(spawn_center, 6, 4) {
                workers.send(job);
            }
            let st = world.stats();
            let want = ((2 * world.radius.h + 1).pow(2) * (2 * world.radius.v + 1)) as usize;
            let progress = (st.meshed * 100 / want.max(1)).min(100) as u32;
            loading(&mut ui, &mut dev, &format!("Generating chunks: {} of {}", st.meshed, want), progress, &mut frame_no);
            if world.settled() || t_load.elapsed() > Duration::from_secs(60) {
                break;
            }
        }
        let load_ms = t_load.elapsed().as_millis() as u64;
        // The spawn: the saved place, else above the ground at the origin (the reference spawns flying).
        if let Some(p) = saved_pos {
            player.position = p;
            player.yaw = saved["yaw"].as_f64().unwrap_or(0.0) as f32;
            player.pitch = saved["pitch"].as_f64().unwrap_or(0.0) as f32;
            player.flying = saved["flying"].as_bool().unwrap_or(true);
            session.slot = saved["slot"].as_u64().unwrap_or(0) as usize % HOTBAR;
        } else {
            let mut scratch = Vec::new();
            let hit = ray::cast(&Ground(&mut world), Vec3::new(0.5, 47.0, 0.5), Vec3::NEG_Y, 95.0, &mut scratch);
            if hit.is_hit {
                player.position = hit.position + Vec3::new(0.0, 3.0, 0.0);
            }
        }
        session.loaded();
        if live && boot["play"] == Value::Bool(true) {
            session.mode = Mode::Play;
        }
        ui.collect();

        // ------------------------------------------------------------------------------------------ the frame loop
        let ctx = g::vita2d_get_context();
        let mut fence = Fence::new(0, 2);
        let mut scene_fence = Fence::new(2, 2);
        let mut water_look = [0.03f32, 0.05, 0.09, 0.35, 0.06];
        let mut shadows_on = boot["shadows"].as_bool().unwrap_or(false);
        let mut shadow_ms = 0f32;
        let (mut gpu_ms, mut reflection_ms, mut reflect_tris) = (0f32, 0f32, 0u32);
        let mut timing = Timing { ms: [16.7; Timing::N], at: 0, late: 0, frames: 0 };
        let mut last = Instant::now();
        let mut last_vcount = sceDisplayGetVcount();
        let (mut draw_ms, mut world_ms, mut sim_ms, mut ui_ms, mut gen_ms, mut mesh_ms, mut wait_ms) = (0f32, 0f32, 0f32, 0f32, 0f32, 0f32, 0f32);
        let mut t_frame = Instant::now();
        let mut work_ms = 0f32;
        let mut stats = scene::Stats::default();
        let mut uploads_failed = 0u32;
        let mut hit;
        let mut ray_scratch = Vec::new();
        let mut walk: Option<(Vec2, f32)> = None;
        let mut queued_stroke: Option<Stroke> = None;
        let mut last_slot = usize::MAX;
        let mut mode_was = session.mode;
        let mut saved_at = Instant::now();
        let mut title_yaw = 0.0f32;
        let mut edits = 0u32;
        let mut reloads = 0u32;
        let mut show = [true; 3];
        // Leaves in the sea's reflection: off, the title's view over the sea misses 60 with them (`ctl`
        // {"reflect": {"foliage": true}} draws them).
        let mut mirror_foliage = false;
        let mut held_frames = 0u32;
        let mut rescues = 0u32;
        let mut safe = player.position;

        loop {
            let raw = input::read();
            let (buttons, action) = dev.menu.input(raw.buttons);
            let menu = dev.menu.visible;
            let pad = if menu { interface::NEUTRAL } else { input::Pad { buttons, ..raw } };

            // Remote control.
            while let Ok(v) = control.try_recv() {
                if let Some(name) = v["fetch"].as_str().filter(|n| !n.contains('/') && !n.contains("..")) {
                    fetch(name);
                }
                let f = |a: &Value, i: usize, d: f32| a.get(i).and_then(Value::as_f64).map(|x| x as f32).unwrap_or(d);
                if v["pos"].is_array() {
                    player.position = Vec3::new(f(&v["pos"], 0, 0.0), f(&v["pos"], 1, 40.0), f(&v["pos"], 2, 0.0));
                    player.velocity = Vec3::ZERO;
                }
                player.yaw = v["yaw"].as_f64().map(|x| x as f32).unwrap_or(player.yaw);
                player.pitch = v["pitch"].as_f64().map(|x| x as f32).unwrap_or(player.pitch);
                clock.daytime = v["daytime"].as_f64().map(|x| x as f32).unwrap_or(clock.daytime);
                if let Some(fly) = v["fly"].as_bool() {
                    player.flying = fly;
                }
                if let Some(m) = v["mode"].as_str().and_then(Mode::parse) {
                    session.mode = m;
                }
                if v["walk"].is_array() {
                    walk = Some((Vec2::new(f(&v["walk"], 0, 0.0), f(&v["walk"], 1, 0.0)), f(&v["walk"], 2, 3.0)));
                }
                match v["stroke"].as_str() {
                    Some("break") => queued_stroke = Some(Stroke::Break),
                    Some("place") => queued_stroke = Some(Stroke::Place),
                    _ => {}
                }
                for (i, k) in ["terrain", "water", "foliage"].iter().enumerate() {
                    // (each pass on its own; turning one off leaves the others)
                    if let Some(on) = v["show"][k].as_bool() {
                        show[i] = on;
                    }
                }
                if let Some(on) = v["reflect"]["foliage"].as_bool() {
                    mirror_foliage = on;
                }
                if let Some(on) = v["shadows"].as_bool() {
                    shadows_on = on;
                }
                for (i, k) in ["terrain", "foliage"].iter().enumerate() {
                    if let Some(on) = v["casters"][k].as_bool() {
                        shadow.casters[i] = on;
                    }
                }
                if let Some(c) = v["casters"]["cull"].as_u64() {
                    shadow.cull = c as u8;
                }
                if let Some(a) = v["water"].as_array() {
                    for (i, x) in a.iter().enumerate().take(5) {
                        water_look[i] = x.as_f64().unwrap_or(water_look[i] as f64) as f32;
                    }
                }
                if v["save"] == Value::Bool(true) {
                    save_world(&world, &player, &clock, session.slot);
                }
            }

            // Mods read again after their files changed.
            while let Ok(sources) = mod_reloads.try_recv() {
                match mods.load(&sources) {
                    Ok(changes) => {
                        reloads += 1;
                        let reg = mods.registry();
                        if changes.regenerate() {
                            world.reset(Some(reg.worldgen.params()), Some(reg.decor.params()));
                        } else if changes.remesh() {
                            world.remesh_all();
                        }
                        if changes.player {
                            tune(&mut player, reg, session.settings.unfly);
                        }
                        if changes.clock {
                            clock.day_length = reg.clock.day_length;
                        }
                        publish_registry(reg);
                        notices.push(format!("mods reloaded: {}", reg.mods.join(", ")));
                    }
                    Err(errors) => notices.extend(errors.iter().map(|e| e.to_string())),
                }
            }

            // What the interface asked for on its last turn.
            let slot_now = session.slot;
            session.obey(|c| match c {
                Command::Start => {
                    placed = true;
                }
                Command::Title => {
                    save_world(&world, &player, &clock, slot_now);
                }
                Command::Prefs(text) => {
                    paths::write_atomic(paths::PREFS_FILE, text.as_bytes());
                    channel().state.prefs = text;
                }
                _ => {}
            });
            let _ = placed;

            let vcount = sceDisplayGetVcount();
            let dt = refreshes(vcount, last_vcount).clamp(1, 4) as f32 / 60.0;
            last_vcount = vcount;

            // ------------------------------------------------------------------ the player
            let ts = Instant::now();
            let axis = |v: u8, flip: bool| {
                let x = (v as f32 - 127.5) / 127.5;
                if flip {
                    -x
                } else {
                    x
                }
            };
            let spad = ether_interface::Pad { buttons: pad.buttons, lx: axis(pad.lx, false), ly: axis(pad.ly, true), rx: axis(pad.rx, false), ry: axis(pad.ry, true) };
            let mut input = session.run(&spad, dt);
            if let Some((w, left)) = walk.as_mut() {
                input.move_x = w.x;
                input.move_y = w.y;
                *left -= dt;
                if *left <= 0.0 {
                    walk = None;
                }
            }
            // A hotbar slot that holds a brush preset sets the brush.
            if session.slot != last_slot {
                last_slot = session.slot;
                if let Some(Some(ether_mod::registry::Slot::Brush { brush })) = mods.registry().hotbar.get(session.slot) {
                    session.brush.material = brush.material as u32;
                    session.brush.size = brush.size.round() as u32;
                    session.brush.strength = brush.strength;
                    session.brush.shape = if brush.shape() == Shape::Isosurface { ether_interface::Shape::Isosurface } else { ether_interface::Shape::Cube };
                }
            }
            player.ctl.unfly_on_ground = mods.registry().controller.unfly_on_ground && session.settings.unfly;
            if let Some(len) = session.day_length() {
                clock.day_length = len;
            } else {
                clock.day_length = 0.0;
            }
            let playing = session.mode == Mode::Play;
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
                // The player collides with the meshes' triangles: until the chunk it is in and the one under it
                // are meshed there is nothing to stand on, and it waits. A fall far below the last place it stood
                // (into rock that has no surface, under a chunk that was not meshed in time) brings it back there.
                let here = World::chunk_index(player.position);
                let ready = world.is_meshed(here) && world.is_meshed(here - IVec3::Y);
                if ready || player.flying {
                    player.step(&Ground(&mut world), &ppad, dt);
                } else {
                    held_frames += 1;
                }
                if player.grounded {
                    safe = player.position;
                }
                if player.position.y < safe.y - 256.0 {
                    player.position = safe + Vec3::new(0.0, 2.0, 0.0);
                    player.velocity = Vec3::ZERO;
                    player.flying = true;
                    rescues += 1;
                }
                camera.distance.target = session.camera_distance();
                camera.base_fov = session.fov();
                camera.follow(&player, dt);
                hit = ray::cast(&Ground(&mut world), camera.eye, camera.forward(), ray::REACH, &mut ray_scratch);
                let stroke = if input.attack {
                    Some(Stroke::Break)
                } else if input.use_item {
                    Some(Stroke::Place)
                } else {
                    queued_stroke.take()
                };
                if let (Some(stroke), true) = (stroke, hit.is_hit) {
                    let b = session.brush;
                    let shape = if b.shape == ether_interface::Shape::Cube { Shape::Cube } else { Shape::Isosurface };
                    let vb = VoxelBrush { shape, tex: b.material as u8, size: b.size as f32, strength: b.strength };
                    let ev = BlockEvent { pos: hit.voxel_pos, voxel: world.voxel(hit.voxel_pos), brush: ether_mod::registry::Brush { material: b.material as u8, shape: shape as u8, size: b.size as f32, strength: b.strength } };
                    let ps = player_state(&player, session.slot);
                    let out = match stroke {
                        Stroke::Break => mods.on_break(&ev, &Seen(&world), &ps),
                        Stroke::Place => mods.on_place(&ev, &Seen(&world), &ps),
                    };
                    if !out.cancel {
                        edits += brush::apply(&mut Ground(&mut world), &hit, &vb, stroke).len() as u32;
                    }
                    obey_mods(out, &mut world, &mut player, &mut clock, &mut notices);
                }
            } else {
                // Behind the title the eye turns slowly over the spawn; behind the menu it stays.
                if session.mode == Mode::Title {
                    title_yaw += dt * 0.05;
                    let p = player.position + Vec3::new(0.0, 6.0, 0.0);
                    camera.rotation = ether_voxel::glam::Quat::from_euler(ether_voxel::glam::EulerRot::YXZ, title_yaw, -0.25, 0.0);
                    camera.eye = p;
                }
                hit = HitResult::default();
            }
            clock.tick(if playing || session.mode == Mode::Title { dt } else { 0.0 });
            let ps = player_state(&player, session.slot);
            let out = mods.tick(dt, &Seen(&world), &ps);
            obey_mods(out, &mut world, &mut player, &mut clock, &mut notices);
            sim_ms = sim_ms * 0.9 + ts.elapsed().as_secs_f32() * 100.0;

            if session.mode != mode_was {
                if matches!(session.mode, Mode::Title) {
                    ui.collect();
                }
                mode_was = session.mode;
            }
            if playing && saved_at.elapsed() > Duration::from_secs(60) {
                save_world(&world, &player, &clock, session.slot);
                saved_at = Instant::now();
            }

            // ------------------------------------------------------------------ the world
            let tw = Instant::now();
            world.radius = radius(&session);
            while let Ok((done, ms)) = workers.done.try_recv() {
                match &done {
                    Done::Generated { .. } => gen_ms = gen_ms * 0.95 + ms * 0.05,
                    Done::Meshed { .. } => mesh_ms = mesh_ms * 0.95 + ms * 0.05,
                }
                if let Some((at, mesh)) = world.finish(done) {
                    if !scene.upload(at, &mesh, frame_no) {
                        uploads_failed += 1;
                    }
                }
            }
            for at in world.take_unloaded() {
                scene.remove(at, frame_no);
            }
            for job in world.plan(World::chunk_index(camera.eye), 6, 4) {
                workers.send(job);
            }
            world_ms = world_ms * 0.9 + tw.elapsed().as_secs_f32() * 100.0;

            // ------------------------------------------------------------------ the interface
            let tu = Instant::now();
            {
                let st = &mut channel().state;
                session.publish(st);
                st.health = 20;
                st.health_max = 20;
                st.flying = player.flying;
                st.sprinting = player.sprinting;
                if session.numbers_due() {
                    let p = player.position;
                    st.t = Numbers { minutes: (clock.daytime * 1440.0) as i32, x: (p.x * 10.0) as i32, y: (p.y * 10.0) as i32, z: (p.z * 10.0) as i32 };
                }
                if !session.settings.stats {
                    st.stats.clear();
                } else if frame_no % 30 == 0 {
                    st.stats = format!("{:.1} fps · {:.1} ms · late {} · {} chunks · {}k tris · {} draws", 1000.0 / timing.avg().max(0.1), timing.avg(), timing.late, stats.chunks, stats.tris / 1000, stats.draws);
                }
            }
            ui.turn(dt, &pad, menu, Some(&session));
            ui_ms = ui_ms * 0.9 + tu.elapsed().as_secs_f32() * 100.0;

            // ------------------------------------------------------------------ the frame
            let sk = sky(&clock, mods.registry(), scene.textures.horizon);
            let reach = (world.radius.h as f32 + 0.5) * 16.0;
            #[rustfmt::skip]
            let frame: [f32; 16] = [
                sk.sun_dir.x, sk.sun_dir.y, sk.sun_dir.z, sk.sun,
                sk.up[0], sk.up[1], sk.up[2], 1.0 / reach,
                sk.down[0], sk.down[1], sk.down[2], clock.time_inhabited,
                1.05, 0.98, 0.88, sk.tint[0],
            ];
            let proj = camera.projection_gl(960.0 / 544.0, 0.1, 1000.0);
            let v = scene::View { eye: camera.eye, vp: proj * camera.view_rotation(), frame, fog: sk.horizon, sky_zenith: sk.zenith, sky_horizon: sk.horizon, sky_tint: sk.tint, sky_night: sk.night, sun_dir: sk.sun_dir, sun: sk.sun, reach, terrain: show[0], water: show[1], foliage: show[2], mirror_foliage, plain: false, water_look, shadow: None, shadow_map: &shadow.texture, sun_colour: [1.05, 0.98, 0.88] };

            let t2 = Instant::now();
            let slot = (frame_no % 2) as usize;
            fence.wait(slot);
            let waited = t2.elapsed().as_secs_f32() * 1000.0;
            wait_ms = wait_ms * 0.9 + waited * 0.1;
            // Frame `frame_no - 2` used this slot: the GPU is done with it and with every mesh it drew.
            scene.heap.collect(frame_no.saturating_sub(2));
            // The sea's reflection, then the scene; with the profile switch the scene goes to its own target and
            // is timed by its notice, then copied to the display.
            let ts = Instant::now();
            let mut v = v;
            v.shadow = if shadows_on { shadow.update(ctx, &scene, v.eye, v.sun_dir, v.sun) } else { None };
            shadow_ms = shadow_ms * 0.9 + ts.elapsed().as_secs_f32() * 100.0;
            let tr = Instant::now();
            if v.water {
                if post.reflection.begin(ctx, 1.0).is_ok() {
                    reflect_tris = scene.draw_reflection(ctx, &v).tris;
                    post.reflection.end(ctx, None);
                }
            }
            reflection_ms = reflection_ms * 0.9 + tr.elapsed().as_secs_f32() * 100.0;
            let reflection = post.reflection.texture;
            if let Some(target) = post.profile.as_mut() {
                let tg = Instant::now();
                if target.begin(ctx, 1.0).is_ok() {
                    stats = scene.draw(ctx, &v, frame_no, &reflection);
                    target.end(ctx, Some(scene_fence.signal(slot)));
                    scene_fence.wait(slot);
                    gpu_ms = gpu_ms * 0.9 + tg.elapsed().as_secs_f32() * 100.0;
                }
            }
            g::vita2d_pool_reset();
            if slot == 1 {
                g::vita2d_pool_malloc(POOL_BYTES / 2);
            }
            g::vita2d_start_drawing_advanced(core::ptr::null_mut(), 0);
            if let Some(target) = post.profile.as_ref() {
                post.copy(ctx, &target.texture);
            } else {
                stats = scene.draw(ctx, &v, frame_no, &reflection);
            }
            g::sceGxmSetViewport(ctx, 480.0, 480.0, 272.0, -272.0, 0.5, 0.5);
            gpu::state_overlay(ctx);
            ui.draw();
            dev.overlay();
            g::sceGxmEndScene(ctx, core::ptr::null(), fence.signal(slot));
            draw_ms = draw_ms * 0.9 + t2.elapsed().as_secs_f32() * 100.0;
            // This thread's own work: the frame up to the swap, less the wait for the GPU.
            work_ms = work_ms * 0.9 + (t_frame.elapsed().as_secs_f32() * 1000.0 - waited) * 0.1;
            g::vita2d_swap_buffers();
            while refreshes(sceDisplayGetVcount(), last_vcount) < 1 {
                sceDisplayWaitVblankStart();
            }
            let now = Instant::now();
            t_frame = now;
            let shown = refreshes(sceDisplayGetVcount(), last_vcount);
            timing.push((now - last).as_secs_f32() * 1000.0, shown > 1);
            last = now;

            if frame_no % 120 == 0 {
                sceKernelPowerTick(0);
                if scePowerGetArmClockFrequency() < CLOCKS[0] - 20 {
                    set_clocks();
                }
            }
            if frame_no % 10 == 0 {
                let ws = world.stats();
                let reg = mods.registry();
                if notices.len() > 8 {
                    notices.drain(..notices.len() - 8);
                }
                dev.engine = json!({
                    "stage": "running",
                    "mode": session.mode.name(),
                    "frameMs": timing.avg(), "worstMs": timing.worst(), "late": timing.late, "frames": timing.frames,
                    "gpuMs": if profile { json!({"sceneWithReflection": gpu_ms}) } else { Value::Null }, "reflection": {"cpuMs": reflection_ms, "tris": reflect_tris}, "shadow": {"on": shadows_on, "cpuMs": shadow_ms, "gpuMs": shadow.gpu_ms, "rects": shadow.rects, "draws": shadow.draws, "tris": shadow.tris, "fullRedraws": shadow.full}, "waterLook": water_look,
                    "cpuMs": {"work": work_ms, "gpuWait": wait_ms, "draw": draw_ms, "world": world_ms, "sim": sim_ms, "interface": ui_ms},
                    "workerMs": {"generate": gen_ms, "mesh": mesh_ms},
                    "scene": {"byPass": {"terrain": stats.by_pass[0], "layers": stats.by_pass[1], "water": stats.by_pass[2], "foliage": stats.by_pass[3]}, "chunks": stats.chunks, "culled": stats.culled, "draws": stats.draws, "tris": stats.tris, "resident": scene.len(), "heapBytes": scene.bytes(), "heapPeak": scene.heap.peak, "heapLargest": scene.heap.largest(), "uploadsFailed": uploads_failed, "textureBytes": scene.textures.bytes},
                    "world": {"chunks": ws.chunks, "generated": ws.generated, "populated": ws.populated, "meshed": ws.meshed, "generating": ws.generating, "meshing": ws.meshing, "kept": ws.kept, "radius": [world.radius.h, world.radius.v], "restored": restored, "edits": edits},
                    "player": {"held": held_frames, "rescues": rescues, "safe": safe.to_array(), "pos": player.position.to_array(), "vel": player.velocity.to_array(), "yaw": player.yaw, "pitch": player.pitch, "flying": player.flying, "grounded": player.grounded, "sprinting": player.sprinting},
                    "hit": {"is": hit.is_hit, "voxel": hit.voxel_pos.to_array(), "distance": hit.distance},
                    "clock": {"daytime": clock.daytime, "dayLength": clock.day_length},
                    "mods": {"loaded": reg.mods, "commands": reg.commands, "reloads": reloads, "notices": notices, "memory": mods.memory().map(|m| m.malloc_size)},
                    "interface": {"up": ui.live(), "open": channel().is_open(), "error": ui.error, "turns": ui.turns, "turnMs": ui.turn_ms, "worstTurnMs": ui.worst_ms, "collections": ui.collections, "collectMs": ui.collect_ms},
                    "loadMs": load_ms,
                    "programs": {"compiled": gpu.compiled, "cached": gpu.cached, "ms": programs_ms},
                    "msaa": samples,
                });
            }
            dev.publish(frame_no as u32, "ether");
            serve(&mut dev, frame_no as u32, action);
            frame_no += 1;
        }
    }
}

/// The start failed: the reason stays on the screen and in the status receipt.
unsafe fn fail(ui: &mut interface::Ui, dev: &mut dev::Host, error: String) -> ! {
    pocketjs_vita::vita_log(format_args!("ether: {error}"));
    let mut frame = 0;
    loop {
        let st = &mut channel().state;
        st.mode = Mode::Error;
        st.message.clone_from(&error);
        ui.turn(interface::TURN, &interface::NEUTRAL, true, None);
        graphics::begin_frame(0xff0c_0a09);
        ui.draw();
        dev.overlay();
        graphics::present();
        dev.engine = json!({"stage": "error", "message": error});
        dev.publish(frame, "ether");
        serve(dev, frame, MenuAction::None);
        frame += 1;
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Answers wired-debug requests at a frame boundary.
unsafe fn serve(dev: &mut dev::Host, frame: u32, action: MenuAction) {
    let mut request = dev.poll();
    let op = request.as_ref().map(|r| r.command.op).or(match action {
        MenuAction::Capture => Some(Op::Capture),
        _ => None,
    });
    match op {
        Some(Op::Status) => request.take().unwrap().finish(Ok(dev.status(frame, "ether"))),
        Some(Op::Menu) => {
            dev.menu.visible = !dev.menu.visible;
            request.take().unwrap().finish(Ok(json!({"menu": dev.menu.visible})));
        }
        Some(Op::Capture) => {
            if let Some(request) = request.take() {
                let _ = request.reply.try_send(dev.capture(frame));
            } else {
                dev.capture_from_menu(frame);
            }
        }
        Some(Op::Native) => {
            let request = request.take().unwrap();
            g::vita2d_wait_rendering_done();
            let result = dev::exec_native(request.native_path.as_ref().unwrap());
            request.finish(result.map(|_| json!({})));
        }
        _ => {
            if let Some(request) = request.take() {
                request.finish(Err("Pocket Ethertum is native; use native to start a new build".into()));
            }
        }
    }
}
