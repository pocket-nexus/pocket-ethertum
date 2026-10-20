//! Pocket Ethertum on PS Vita.
//!
//! A frame at 960 × 544: the sky, then the chunks around the eye, streamed by
//! `ether_world::World` (generation and meshing on two worker threads,
//! population on this one) and drawn by [`scene::Scene`].
//!
//! Development loop over PocketJS's wired debug transport: `host0:ether/boot.json`
//! holds the start switches, `host0:ether/control.json` steers the run, and
//! status receipts carry frame timings and the world's counts under `engine`.

mod gpu;
mod heap;
mod hostfs;
mod paths;
mod scene;

use std::sync::mpsc;
use std::time::{Duration, Instant};

use ether_voxel::gen::Params;
use ether_voxel::glam::{EulerRot, IVec3, Mat4, Quat, Vec3};
use ether_voxel::populate::DecorParams;
use ether_world::{Done, Job, Radius, World, Worker};
use pocket_vita_gxm::target::Fence;
use pocketjs_vita::{dev, dev_protocol::Op, devmenu::Action, graphics, input};
use serde_json::{json, Value};
use vita2d_sys as g;

#[no_mangle]
#[used]
pub static sceUserMainThreadStackSize: u32 = 1024 * 1024;

#[no_mangle]
#[used]
pub static _newlib_heap_size_user: u32 = 96 * 1024 * 1024;

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

/// vita2d's pool of temporary vertices (the Devkit menu draws from it).
const POOL_BYTES: u32 = 1024 * 1024;
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

// Vita controller bits.
const P_SELECT: u32 = 0x1;
const P_UP: u32 = 0x10;
const P_DOWN: u32 = 0x40;
const P_L: u32 = 0x100 | 0x400;
const P_R: u32 = 0x200 | 0x800;
const P_CROSS: u32 = 0x4000;

/// Display refreshes from the count `then` to `now`. The counter has 16 bits and wraps after 18 minutes.
fn refreshes(now: i32, then: i32) -> i32 {
    now.wrapping_sub(then) & 0xffff
}

fn axis(v: u8) -> f32 {
    let x = (v as f32 - 127.5) / 127.5;
    const DEAD: f32 = 0.24;
    if x.abs() < DEAD {
        0.0
    } else {
        (x - DEAD * x.signum()) / (1.0 - DEAD)
    }
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
    fn new(world: &World, n: usize) -> Workers {
        let (done_tx, done) = mpsc::channel();
        let mut jobs = Vec::new();
        for i in 0..n {
            let (tx, rx) = mpsc::channel::<Job>();
            let done_tx = done_tx.clone();
            let terrain = world.terrain.clone();
            std::thread::Builder::new()
                .name(format!("ether-worker-{i}"))
                .stack_size(256 * 1024)
                .spawn(move || {
                    let mut w = Worker::new(terrain);
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

/// The eye before the player arrives: a free camera on the sticks.
struct Eye {
    pos: Vec3,
    yaw: f32,
    pitch: f32,
}

impl Eye {
    fn rotation(&self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.yaw, self.pitch, 0.0)
    }

    fn step(&mut self, pad: &input::Pad, dt: f32) {
        let (lx, ly, rx, ry) = (axis(pad.lx), axis(pad.ly), axis(pad.rx), axis(pad.ry));
        self.yaw -= rx * 2.2 * dt;
        self.pitch = (self.pitch - ry * 1.8 * dt).clamp(-1.55, 1.55);
        let rot = self.rotation();
        let fwd = rot * Vec3::NEG_Z;
        let right = rot * Vec3::X;
        let speed = if pad.buttons & P_CROSS != 0 { 40.0 } else { 12.0 };
        let mut m = fwd * -ly + right * lx;
        if pad.buttons & P_R != 0 {
            m.y += 1.0;
        }
        if pad.buttons & P_L != 0 {
            m.y -= 1.0;
        }
        self.pos += m * speed * dt;
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

/// The sky's colours and the frame's light from the time of day (the reference's sun: angle = daytime · 2π, its
/// strength sin² above the horizon).
fn sky(daytime: f32) -> (Vec3, f32, [f32; 3], [f32; 3], [f32; 3], [f32; 3], [f32; 3]) {
    let a = daytime * core::f32::consts::TAU;
    // The reference turns the light by Rz(a)·Ry(π/2.3); this is the direction it comes from.
    let dir = (Quat::from_rotation_z(a) * Quat::from_rotation_y(core::f32::consts::PI / 2.3)) * Vec3::Z;
    let dir = if dir.y < 0.0 && a.sin() > 0.0 { -dir } else { dir };
    let sun = a.sin().max(0.0).powi(2);
    let day = (a.sin() * 3.0 + 0.4).clamp(0.0, 1.0);
    let mix = |n: [f32; 3], d: [f32; 3]| [n[0] + (d[0] - n[0]) * day, n[1] + (d[1] - n[1]) * day, n[2] + (d[2] - n[2]) * day];
    // The reference's fog colours: inscattering (110, 230, 255)/255 by day, its sky (0, 0.666, 1).
    let zenith = mix([0.02, 0.03, 0.08], [0.16, 0.45, 0.85]);
    let horizon = mix([0.05, 0.06, 0.12], [0.62, 0.84, 0.98]);
    let ambient_up = mix([0.10, 0.12, 0.20], [0.55, 0.65, 0.80]);
    let ambient_down = mix([0.05, 0.05, 0.08], [0.30, 0.28, 0.25]);
    let sun_col = [1.05, 0.98, 0.88];
    (dir.normalize(), sun, zenith, horizon, ambient_up, ambient_down, sun_col)
}

fn main() {
    unsafe {
        let live = cfg!(feature = "usb-debug");
        // Development builds take start switches from the USB share: {"title": false, "msaa": 0 | 2 | 4,
        // "radius": [4, 3], "pos": [x, y, z]}.
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

        let mut gpu = match gpu::Gpu::new(live) {
            Ok(g) => g,
            Err(e) => fail(&mut dev, e),
        };
        let t_programs = Instant::now();
        let mut scene = match scene::Scene::new(&mut gpu, msaa, HEAP_BYTES) {
            Ok(s) => s,
            Err(e) => fail(&mut dev, e),
        };
        gpu.finish();
        let programs_ms = t_programs.elapsed().as_millis() as u64;

        let radius = match (boot["radius"][0].as_i64(), boot["radius"][1].as_i64()) {
            (Some(h), Some(v)) => Radius { h: h as i32, v: v as i32 },
            _ => Radius::REFERENCE,
        };
        let mut world = World::new(Params::default(), DecorParams::default(), radius);
        let mut workers = Workers::new(&world, 2);
        let control = if live { control_watcher() } else { mpsc::channel().1 };

        let f = |a: &Value, i: usize, d: f32| a.get(i).and_then(Value::as_f64).map(|x| x as f32).unwrap_or(d);
        let mut eye = Eye { pos: Vec3::new(f(&boot["pos"], 0, 0.0), f(&boot["pos"], 1, 24.0), f(&boot["pos"], 2, 0.0)), yaw: 0.0, pitch: -0.3 };
        let mut daytime = 0.15f32;
        let day_length = 1440.0f32;
        let mut day_rate = 1.0f32;

        let ctx = g::vita2d_get_context();
        let mut fence = Fence::new(0, 2);
        let mut timing = Timing { ms: [16.7; Timing::N], at: 0, late: 0, frames: 0 };
        let mut last = Instant::now();
        let mut last_vcount = sceDisplayGetVcount();
        let mut frame_no: u64 = 0;
        let (mut draw_ms, mut world_ms, mut upload_ms, mut gen_ms, mut mesh_ms) = (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
        let mut stats = scene::Stats::default();
        let mut uploads_failed = 0u32;
        let mut fov = 85.0f32;
        let mut pace = 1;
        let t_start = Instant::now();
        let mut settled_at: Option<f32> = None;
        let mut prev_buttons = 0u32;
        let mut held = false;

        loop {
            let t_frame = Instant::now();
            let raw = input::read();
            let (buttons, action) = dev.menu.input(raw.buttons);
            let menu = dev.menu.visible;
            let pad = if menu { input::Pad { buttons: 0, lx: 128, ly: 128, rx: 128, ry: 128 } } else { input::Pad { buttons, ..raw } };
            while let Ok(v) = control.try_recv() {
                let n = |k: &str, cur: f32| v[k].as_f64().map(|x| x as f32).unwrap_or(cur);
                if v["pos"].is_array() {
                    eye.pos = Vec3::new(f(&v["pos"], 0, eye.pos.x), f(&v["pos"], 1, eye.pos.y), f(&v["pos"], 2, eye.pos.z));
                }
                eye.yaw = n("yaw", eye.yaw);
                eye.pitch = n("pitch", eye.pitch);
                daytime = n("daytime", daytime);
                day_rate = n("rate", day_rate);
                fov = n("fov", fov);
                held = v["hold"].as_bool().unwrap_or(held);
                if let Some(p) = v["pace"].as_i64() {
                    pace = (p as i32).clamp(1, 4);
                }
                if let (Some(h), Some(vr)) = (v["radius"][0].as_i64(), v["radius"][1].as_i64()) {
                    world.radius = Radius { h: h as i32, v: vr as i32 };
                }
            }
            let vcount = sceDisplayGetVcount();
            let dt = refreshes(vcount, last_vcount).clamp(1, 4) as f32 / 60.0;
            last_vcount = vcount;
            if !held {
                eye.step(&pad, dt);
            }
            if pad.buttons & P_UP != 0 && prev_buttons & P_UP == 0 {
                world.radius = Radius { h: world.radius.h + 1, v: world.radius.v };
            }
            if pad.buttons & P_DOWN != 0 && prev_buttons & P_DOWN == 0 {
                world.radius = Radius { h: (world.radius.h - 1).max(1), v: world.radius.v };
            }
            if pad.buttons & P_SELECT != 0 && prev_buttons & P_SELECT == 0 {
                daytime = (daytime + 0.125) % 1.0;
            }
            prev_buttons = pad.buttons;
            daytime = (daytime + dt * day_rate / day_length) % 1.0;

            // ------------------------------------------------------------------ the world
            let tw = Instant::now();
            while let Ok((done, ms)) = workers.done.try_recv() {
                match &done {
                    Done::Generated { .. } => gen_ms = gen_ms * 0.95 + ms * 0.05,
                    Done::Meshed { .. } => mesh_ms = mesh_ms * 0.95 + ms * 0.05,
                }
                let tu = Instant::now();
                if let Some((at, mesh)) = world.finish(done) {
                    if !scene.upload(at, &mesh, frame_no) {
                        uploads_failed += 1;
                    }
                }
                upload_ms = upload_ms * 0.95 + tu.elapsed().as_secs_f32() * 1000.0 * 0.05;
            }
            for at in world.take_unloaded() {
                scene.remove(at, frame_no);
            }
            let center = World::chunk_index(eye.pos);
            for job in world.plan(center, 6, 4) {
                workers.send(job);
            }
            world_ms = world_ms * 0.9 + tw.elapsed().as_secs_f32() * 100.0;
            if settled_at.is_none() && world.settled() {
                settled_at = Some(t_start.elapsed().as_secs_f32());
            }

            // ------------------------------------------------------------------ the frame
            let (sun_dir, sun, zenith, horizon, up, down, sun_col) = sky(daytime);
            let rot = eye.rotation();
            let view = Mat4::from_quat(rot.inverse());
            let proj = Mat4::perspective_rh_gl(fov.to_radians() * 0.75, 960.0 / 544.0, 0.1, 1000.0);
            let reach = (world.radius.h as f32 + 0.5) * 16.0;
            #[rustfmt::skip]
            let frame: [f32; 16] = [
                sun_dir.x, sun_dir.y, sun_dir.z, sun,
                up[0], up[1], up[2], 1.6 / reach,
                down[0], down[1], down[2], t_start.elapsed().as_secs_f32(),
                sun_col[0], sun_col[1], sun_col[2], 0.0,
            ];
            let v = scene::View { eye: eye.pos, vp: proj * view, frame, fog: horizon, sky_zenith: zenith, sky_horizon: horizon, sun_dir, sun, reach };

            let t2 = Instant::now();
            let slot = (frame_no % 2) as usize;
            fence.wait(slot);
            // Frame `frame_no - 2` used this slot: the GPU is done with it, and with every mesh it drew.
            scene.heap.collect(frame_no.saturating_sub(2));
            g::vita2d_pool_reset();
            g::vita2d_start_drawing_advanced(core::ptr::null_mut(), 0);
            stats = scene.draw(ctx, &v, frame_no);
            g::sceGxmSetViewport(ctx, 480.0, 480.0, 272.0, -272.0, 0.5, 0.5);
            gpu::state_overlay(ctx);
            dev.overlay();
            g::sceGxmEndScene(ctx, core::ptr::null(), fence.signal(slot));
            draw_ms = draw_ms * 0.9 + t2.elapsed().as_secs_f32() * 100.0;
            g::vita2d_swap_buffers();
            while refreshes(sceDisplayGetVcount(), last_vcount) < pace {
                sceDisplayWaitVblankStart();
            }
            let now = Instant::now();
            let shown = refreshes(sceDisplayGetVcount(), last_vcount);
            timing.push((now - last).as_secs_f32() * 1000.0, shown > pace);
            last = now;
            let _ = t_frame;

            if frame_no % 120 == 0 {
                sceKernelPowerTick(0);
                if scePowerGetArmClockFrequency() < CLOCKS[0] - 20 {
                    set_clocks();
                }
            }
            if frame_no % 10 == 0 {
                let ws = world.stats();
                dev.engine = json!({
                    "stage": "running",
                    "frameMs": timing.avg(), "worstMs": timing.worst(), "late": timing.late, "frames": timing.frames, "pace": pace,
                    "cpuMs": {"draw": draw_ms, "world": world_ms, "upload": upload_ms},
                    "workerMs": {"generate": gen_ms, "mesh": mesh_ms},
                    "scene": {"chunks": stats.chunks, "culled": stats.culled, "draws": stats.draws, "tris": stats.tris, "resident": scene.len(), "heapBytes": scene.bytes(), "heapPeak": scene.heap.peak, "heapLargest": scene.heap.largest(), "uploadsFailed": uploads_failed},
                    "world": {"chunks": ws.chunks, "generated": ws.generated, "populated": ws.populated, "meshed": ws.meshed, "generating": ws.generating, "meshing": ws.meshing, "kept": ws.kept, "radius": [world.radius.h, world.radius.v], "settledAt": settled_at},
                    "eye": {"pos": [eye.pos.x, eye.pos.y, eye.pos.z], "yaw": eye.yaw, "pitch": eye.pitch, "chunk": [center.x, center.y, center.z]},
                    "clock": {"daytime": daytime, "rate": day_rate},
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

/// The start failed: the reason stays in the status receipt.
unsafe fn fail(dev: &mut dev::Host, error: String) -> ! {
    pocketjs_vita::vita_log(format_args!("ether: {error}"));
    let mut frame = 0;
    loop {
        graphics::begin_frame(0xff20_1010);
        dev.overlay();
        graphics::present();
        dev.engine = json!({"stage": "error", "message": error});
        dev.publish(frame, "ether");
        serve(dev, frame, Action::None);
        frame += 1;
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Answers wired-debug requests at a frame boundary.
unsafe fn serve(dev: &mut dev::Host, frame: u32, action: Action) {
    let mut request = dev.poll();
    let op = request.as_ref().map(|r| r.command.op).or(match action {
        Action::Capture => Some(Op::Capture),
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
        Some(Op::Push | Op::Reload | Op::Reset) => {
            if let Some(request) = request.take() {
                request.finish(Err("Pocket Ethertum is native; use native to start a new build".into()));
            }
        }
        _ => {
            if let Some(request) = request.take() {
                request.finish(Err("unsupported command".into()));
            }
        }
    }
}

#[allow(dead_code)]
fn _ivec(_: IVec3) {}
