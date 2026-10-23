//! Frames on this machine's GPU (Metal), with no browser and no interface: the world loads around the spawn, the
//! words are applied, the world loads around the eye again, and the frame after `--frames` more is written as a
//! PNG; the run's status is printed as JSON.
//!
//!   ether-shot --pack .pocket-build/pack/ether.pack --registry .pocket-build/web/registry.json --out a.png
//!              [--words "mode=play pos=13.8,30,-23.3 yaw=0.8 pitch=-0.35 daytime=0.33 fly=1"] [--frames 60]

use ether_wgpu::app::{App, Held, HEIGHT, SAMPLES, WIDTH};
use ether_wgpu::pack::Pictures;
use pocket_web_wgpu::gpu::{Gpu, Screen};
use pocket_web_wgpu::task;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let pack = arg("--pack").unwrap_or_else(|| ".pocket-build/pack/ether.pack".into());
    let registry = arg("--registry").unwrap_or_else(|| ".pocket-build/web/registry.json".into());
    let out = arg("--out").unwrap_or_else(|| "shot.png".into());
    let words = arg("--words").unwrap_or_default();
    let frames: u32 = arg("--frames").and_then(|v| v.parse().ok()).unwrap_or(30);

    let gpu = task::wait(Gpu::headless_wanting(pocket_web_wgpu::wgpu::Features::TEXTURE_COMPRESSION_BC)).expect("a GPU");
    let screen = Screen::texture(&gpu, WIDTH, HEIGHT, SAMPLES);
    let registry = std::fs::read_to_string(&registry).expect("the registry: bun tools/web.ts build writes it");
    let mut app = App::open(gpu.clone(), screen, &registry).expect("the shell");
    let bytes = std::fs::read(&pack).expect("the pack: bun tools/cook.ts writes it");
    let blocks = app.reads_blocks();
    app.enter(Pictures::load(&gpu, &bytes, blocks).expect("the pack's pictures"));
    let mut now = 0.0f64;
    let mut frame = |app: &mut App| {
        now += 1000.0 / 60.0;
        app.step(now, &Held::default());
        app.draw().expect("a frame");
    };
    while !app.entered() {
        frame(&mut app);
    }
    app.control(&words);
    // The world around the eye the words moved, then the frames asked for.
    for _ in 0..4000 {
        frame(&mut app);
        if app.world_settled() {
            break;
        }
    }
    for _ in 0..frames {
        frame(&mut app);
    }
    let pixels = task::wait(app.screen.read(&gpu)).expect("the frame read back");
    let file = std::fs::File::create(&out).expect("the picture's file");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), WIDTH, HEIGHT);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().and_then(|mut w| w.write_image_data(&pixels)).expect("the PNG");
    println!("{}", app.status());
}
