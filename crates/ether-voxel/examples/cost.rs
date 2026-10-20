//! Host timings of generation, population and meshing, per chunk.
use ether_voxel::chunk::{hood_dir, Hood, Voxels};
use ether_voxel::gen::{Params, Terrain};
use ether_voxel::glam::ivec3;
use ether_voxel::mesh::{Builder, Pad};
use ether_voxel::populate::{Decor, DecorParams};
use ether_voxel::vox::Vox;
use std::collections::HashMap;
use std::time::Instant;

fn main() {
    let t = Terrain::new(Params::default());
    let d = Decor::new(DecorParams::default());
    let mut scratch = [0f64; 4096];
    let mut w: HashMap<_, Box<Voxels>> = HashMap::new();
    let r = 3;
    let t0 = Instant::now();
    for x in -r..=r { for y in -r..=r { for z in -r..=r {
        let cp = ivec3(x, y, z) * 16;
        let mut v = Box::new([Vox::AIR; 4096]);
        t.generate(cp, &mut v, &mut scratch);
        w.insert(cp, v);
    }}}
    let n = (2 * r + 1).pow(3);
    println!("generate: {:.3} ms/chunk", t0.elapsed().as_secs_f64() * 1000.0 / n as f64);
    let hood = |w: &mut HashMap<_, Box<Voxels>>, cp| {
        let mut h = Hood::new();
        for s in 0..27 { if let Some(v) = w.get_mut(&(cp + hood_dir(s) * 16)) { h.chunks[s] = Some(unsafe { &mut *(v.as_mut() as *mut Voxels) }); } }
        h
    };
    let t0 = Instant::now();
    let mut m = 0;
    for x in -r + 1..r { for y in -r + 1..r { for z in -r + 1..r {
        let cp = ivec3(x, y, z) * 16;
        let mut h = hood(&mut w, cp);
        d.populate(&mut h, cp);
        m += 1;
    }}}
    println!("populate: {:.3} ms/chunk", t0.elapsed().as_secs_f64() * 1000.0 / m as f64);
    let mut b = Builder::new();
    let mut pad = Box::new(Pad::new());
    let (mut tris, mut verts, mut bytes, mut draws) = (0, 0, 0, 0);
    let t0 = Instant::now();
    for x in -r + 2..r - 1 { for y in -r + 2..r - 1 { for z in -r + 2..r - 1 {
        let cp = ivec3(x, y, z) * 16;
        let h = hood(&mut w, cp);
        pad.fill(&h);
        let mesh = b.build(&pad);
        tris += mesh.triangles();
        verts += mesh.terrain.vertices.len() + mesh.foliage.vertices.len() + mesh.liquid.vertices.len();
        bytes += (mesh.terrain.vertices.len() + mesh.foliage.vertices.len() + mesh.liquid.vertices.len()) * 12 + (mesh.terrain.indices.len() + mesh.foliage.indices.len() + mesh.liquid.indices.len()) * 2;
        draws += mesh.terrain.batches.iter().map(|b| (b.opaque.1 > 0) as usize + (b.layer.1 > 0) as usize).sum::<usize>() + (!mesh.foliage.indices.is_empty()) as usize + (!mesh.liquid.indices.is_empty()) as usize;
    }}}
    let k = (2 * (r - 2) + 1).pow(3);
    println!("mesh: {:.3} ms/chunk, {} tris, {} verts, {} KB, {:.1} draws per chunk", t0.elapsed().as_secs_f64() * 1000.0 / k as f64, tris / k as usize, verts / k as usize, bytes / k as usize / 1024, draws as f64 / k as f64);
}
