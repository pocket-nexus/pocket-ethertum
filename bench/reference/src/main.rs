//! The reference, measured on this computer.
//!
//! `cargo run --release -- core`: world generation, population and meshing of the same chunks by the
//! reference's own functions and by `ether-voxel`, both optimized, one thread.
//!
//! `cargo run --release -- run [--out DIR]`: the reference game itself (its client and integrated server, as its
//! `world_smoke` example drives them), at its default load distance (4, 3), rendering at 960 × 544 into an
//! image (no window, no vsync). The player is put at `--x --y --z` (default (8, 32, 8)) flying with `--pitch`
//! (−0.4) and `--yaw` (0), the world is given 30 s to load, then 20 s of frame times are recorded and a picture is
//! saved. `--daytime` holds the time of day there (else it runs from the reference's 0.15). Prints JSON.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use avian3d::prelude::{LinearVelocity, Position};
use bevy::{
    camera::RenderTarget,
    prelude::*,
    render::{render_resource::TextureFormat, view::screenshot::{Screenshot, ScreenshotCaptured}},
    winit::WinitSettings,
};
use ethertia::{client::prelude::*, net::{CPacket, RenetClientHelper}, server::prelude::*};

fn main() -> AppExit {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("core") => {
            core();
            AppExit::Success
        }
        _ => {
            let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
            let out = arg("--out").unwrap_or_else(|| ".".into());
            let n = |k: &str, d: f32| arg(k).and_then(|v| v.parse().ok()).unwrap_or(d);
            let daytime = arg("--daytime").and_then(|v| v.parse().ok());
            run(out, Vec3::new(n("--x", 8.0), n("--y", 32.0), n("--z", 8.0)), n("--pitch", -0.4), n("--yaw", 0.0), daytime)
        }
    }
}

// --------------------------------------------------------------------------------------------- the engine core

fn core() {
    use ether_voxel::{chunk::{hood_dir, Hood, Voxels}, gen::{Params, Terrain}, glam::ivec3, mesh, populate::{Decor, DecorParams}, vox::Vox};
    use ethertia::util::AsMutRef;
    use ethertia::voxel::{meshgen, worldgen, Chunk, ChunkPtr};

    let (lo, hi) = (-3, 3);
    let range = || (lo..=hi).flat_map(|x| (lo..=hi).flat_map(move |y| (lo..=hi).map(move |z| (x, y, z))));
    let count = |r: i32| ((2 * r + 1) as u32).pow(3);

    // The reference.
    let t = Instant::now();
    let mut theirs: HashMap<bevy::math::IVec3, ChunkPtr> = HashMap::new();
    for (x, y, z) in range() {
        let cp = bevy::math::IVec3::new(x, y, z) * 16;
        let mut c = Chunk::new(cp);
        worldgen::generate_chunk(&mut c);
        theirs.insert(cp, Arc::new(c));
    }
    let gen_ref = t.elapsed().as_secs_f64() * 1000.0 / count(3) as f64;
    for (cp, c) in &theirs {
        let m = c.as_mut();
        m.chunkptr_weak = Arc::downgrade(c);
        for (i, dir) in Chunk::NEIGHBOR_DIR.iter().enumerate() {
            m.neighbor_chunks[i] = theirs.get(&(*cp + *dir * 16)).map(Arc::downgrade);
        }
    }
    let t = Instant::now();
    for (x, y, z) in range().filter(|p| [p.0, p.1, p.2].iter().all(|c| c.abs() <= 2)) {
        worldgen::populate_chunk(theirs[&(bevy::math::IVec3::new(x, y, z) * 16)].as_mut());
    }
    let pop_ref = t.elapsed().as_secs_f64() * 1000.0 / count(2) as f64;
    let t = Instant::now();
    let mut verts_ref = 0;
    for (x, y, z) in range().filter(|p| [p.0, p.1, p.2].iter().all(|c| c.abs() <= 1)) {
        let c = &theirs[&(bevy::math::IVec3::new(x, y, z) * 16)];
        for f in [meshgen::generate_chunk_mesh, meshgen::generate_chunk_mesh_foliage, meshgen::generate_chunk_mesh_liquid] {
            let mut v = ethertia::util::vtx::VertexBuffer::default();
            f(&mut v, c);
            verts_ref += v.vertices.len();
        }
    }
    let mesh_ref = t.elapsed().as_secs_f64() * 1000.0 / count(1) as f64;

    // ether-voxel.
    let terrain = Terrain::new(Params::default());
    let decor = Decor::new(DecorParams::default());
    let mut scratch = Box::new([0f64; 4096]);
    let t = Instant::now();
    let mut mine: HashMap<ether_voxel::glam::IVec3, Box<Voxels>> = HashMap::new();
    for (x, y, z) in range() {
        let cp = ivec3(x, y, z) * 16;
        let mut v = Box::new([Vox::AIR; 4096]);
        terrain.generate(cp, &mut v, &mut scratch);
        mine.insert(cp, v);
    }
    let gen_ours = t.elapsed().as_secs_f64() * 1000.0 / count(3) as f64;
    let hood = |w: &mut HashMap<ether_voxel::glam::IVec3, Box<Voxels>>, cp| {
        let mut h = Hood::new();
        for s in 0..27 {
            if let Some(v) = w.get_mut(&(cp + hood_dir(s) * 16)) {
                h.chunks[s] = Some(unsafe { &mut *(v.as_mut() as *mut Voxels) });
            }
        }
        h
    };
    let t = Instant::now();
    for (x, y, z) in range().filter(|p| [p.0, p.1, p.2].iter().all(|c| c.abs() <= 2)) {
        let cp = ivec3(x, y, z) * 16;
        let mut h = hood(&mut mine, cp);
        decor.populate(&mut h, cp);
    }
    let pop_ours = t.elapsed().as_secs_f64() * 1000.0 / count(2) as f64;
    let mut pad = Box::new(mesh::Pad::new());
    let mut cache = mesh::CellCache::new();
    let mut builder = mesh::Builder::new();
    let (mut stream_ms, mut device_ms, mut verts_ours, mut device_tris) = (0.0, 0.0, 0, 0);
    for (x, y, z) in range().filter(|p| [p.0, p.1, p.2].iter().all(|c| c.abs() <= 1)) {
        let cp = ivec3(x, y, z) * 16;
        let h = hood(&mut mine, cp);
        pad.fill(&h);
        let t = Instant::now();
        mesh::terrain(&pad, &mut cache, |_, _| verts_ours += 1);
        mesh::foliage(&pad, |_, _| verts_ours += 1);
        mesh::liquid(&pad, |_, _| verts_ours += 1);
        stream_ms += t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let m = builder.build(&pad);
        device_ms += t.elapsed().as_secs_f64() * 1000.0;
        device_tris += m.triangles();
    }
    let n = count(1) as f64;
    println!(
        "{}",
        serde_json::json!({
            "msPerChunk": {
                "generate": {"reference": gen_ref, "ours": gen_ours},
                "populate": {"reference": pop_ref, "ours": pop_ours},
                "mesh": {"reference": mesh_ref, "oursSameStream": stream_ms / n, "oursDeviceMesh": device_ms / n},
            },
            "verticesPerChunk": {"reference": verts_ref as f64 / n, "oursSameStream": verts_ours as f64 / n},
            "deviceTrianglesPerChunk": device_tris as f64 / n,
            "note": "one thread; the reference at opt-level 3 may drop its populate's leaf writes (it writes through &T cast to &mut T)",
        })
    );
}

// --------------------------------------------------------------------------------------------- the game

#[derive(Resource, Default)]
struct Bench {
    phase: u8,
    next_at: f64,
    image: Handle<Image>,
    measure_from: f64,
    frames: Vec<f32>,
    shot: bool,
    out: String,
    pos: Vec3,
    pitch: f32,
    yaw: f32,
    daytime: Option<f32>,
}

fn run(out: String, pos: Vec3, pitch: f32, yaw: f32, daytime: Option<f32>) -> AppExit {
    let runtime_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/run").join(std::process::id().to_string());
    std::fs::create_dir_all(&runtime_dir).expect("runtime directory");
    std::env::set_current_dir(&runtime_dir).expect("runtime directory");
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(AssetPlugin { file_path: concat!(env!("CARGO_MANIFEST_DIR"), "/../../ref/ethertum/assets").into(), ..default() })
            .set(WindowPlugin { primary_window: Some(Window { title: "Ethertum (reference) measured".into(), resolution: (960, 544).into(), visible: false, ..default() }), ..default() }),
    )
    .add_plugins(ClientGamePlugin)
    .insert_resource(bevy::audio::GlobalVolume::new(bevy::audio::Volume::Linear(0.0)))
    .insert_resource(WinitSettings::game())
    .insert_resource(Bench { out, pos, pitch, yaw, daytime, ..default() })
    .add_systems(PostStartup, setup)
    .add_systems(Last, step);
    app.run()
}

fn setup(mut commands: Commands, mut images: ResMut<Assets<Image>>, cameras: Query<Entity, With<CharacterControllerCamera>>, mut cfg: ResMut<ClientSettings>, mut cli: ResMut<ClientInfo>, mut bench: ResMut<Bench>) {
    let camera = cameras.single().expect("one camera");
    let image = images.add(Image::new_target_texture(960, 544, TextureFormat::Rgba8UnormSrgb, None));
    commands.entity(camera).insert(RenderTarget::Image(image.clone().into()));
    bench.image = image;
    // The reference's defaults.
    cfg.chunks_load_distance = IVec2::new(4, 3);
    cfg.fov = 85.0;
    cfg.username = "bench".into();
    cli.enable_cursor_look = false;
}

#[allow(clippy::too_many_arguments)]
fn step(
    mut commands: Commands,
    time: Res<Time<Real>>,
    mut bench: ResMut<Bench>,
    mut client: EthertiaClient,
    mut net: Option<ResMut<bevy_renet::RenetClient>>,
    server_cfg: Res<ServerSettings>,
    mut players: Query<(&mut Transform, &mut Position, &mut LinearVelocity, &mut CharacterController)>,
    meshes: Res<Assets<Mesh>>,
    mut exit: MessageWriter<AppExit>,
    world: Option<ResMut<WorldInfo>>,
) {
    let now = time.elapsed_secs_f64();
    if let (Some(d), Some(mut world)) = (bench.daytime, world) {
        world.daytime = d;
    }
    if bench.phase == 3 {
        bench.frames.push(time.delta_secs() * 1000.0);
    }
    if now > 150.0 {
        error!("timed out in phase {}", bench.phase);
        exit.write(AppExit::error());
        return;
    }
    if now < bench.next_at {
        if bench.phase >= 2 {
            if let Ok((mut tr, mut pos, mut vel, _)) = players.single_mut() {
                pos.0 = bench.pos;
                tr.translation = pos.0;
                vel.0 = Vec3::ZERO;
            }
        }
        return;
    }
    match bench.phase {
        0 => {
            client.connect_server(format!("127.0.0.1:{}", server_cfg.port));
            bench.phase = 1;
            bench.next_at = now + 1.0;
        }
        1 => {
            if let Ok((mut tr, mut pos, mut vel, mut ctl)) = players.single_mut() {
                net.as_mut().expect("connected").send_packet(&CPacket::LoadDistance { load_distance: IVec2::new(4, 3) });
                pos.0 = bench.pos;
                tr.translation = pos.0;
                ctl.is_flying = true;
                ctl.pitch = bench.pitch;
                ctl.yaw = bench.yaw;
                vel.0 = Vec3::ZERO;
                bench.phase = 2;
                bench.next_at = now + 30.0;
            }
        }
        2 => {
            bench.phase = 3;
            bench.measure_from = now;
            bench.next_at = now + 20.0;
        }
        3 => {
            bench.phase = 4;
            let path = std::path::Path::new(&bench.out).join(format!("reference-{}-{}-{}.png", bench.pos.x, bench.pos.y, bench.pos.z));
            commands.spawn(Screenshot::image(bench.image.clone())).observe(move |event: On<ScreenshotCaptured>, mut bench: ResMut<Bench>| {
                event.image.clone().try_into_dynamic().expect("readable").to_rgb8().save(&path).expect("save picture");
                bench.shot = true;
            });
        }
        4 => {
            if !bench.shot {
                return;
            }
            let mut f = bench.frames.clone();
            f.sort_by(|a, b| a.total_cmp(b));
            let mean = f.iter().sum::<f32>() / f.len().max(1) as f32;
            let pct = |p: f32| f.get(((f.len() as f32 - 1.0) * p) as usize).copied().unwrap_or(0.0);
            let tris: usize = meshes.iter().map(|(_, m)| m.indices().map(|i| i.len()).unwrap_or(m.count_vertices()) / 3).sum();
            println!(
                "{}",
                serde_json::json!({
                    "seconds": 20, "frames": f.len(), "meanMs": mean, "p50Ms": pct(0.5), "p99Ms": pct(0.99), "worstMs": pct(1.0),
                    "fps": 1000.0 / mean, "residentTriangles": tris, "loadDistance": [4, 3], "raster": [960, 544], "pose": [bench.pos.x, bench.pos.y, bench.pos.z, bench.pitch, bench.yaw],
                })
            );
            client.exit_world();
            exit.write(AppExit::Success);
            bench.phase = 5;
        }
        _ => {}
    }
}
