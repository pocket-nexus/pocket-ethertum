use ether_voxel::gen::Params;
use ether_voxel::glam::{ivec3, IVec3, Vec3};
use ether_voxel::populate::DecorParams;
use ether_voxel::vox::{tex, Vox};
use ether_world::{save, Done, Job, Radius, World, Worker};

/// Runs `plan` and every job until the world is settled; returns the number of jobs run.
fn settle(w: &mut World, worker: &mut Worker, center: IVec3) -> (usize, usize) {
    let (mut gens, mut meshes) = (0, 0);
    for _ in 0..10_000 {
        let jobs = w.plan(center, 8, 8);
        if jobs.is_empty() && w.settled() {
            return (gens, meshes);
        }
        for j in jobs {
            match &j {
                Job::Generate { .. } => gens += 1,
                Job::Mesh { .. } => meshes += 1,
            }
            let d = worker.run(j);
            w.finish(d);
        }
    }
    panic!("the world did not settle");
}

fn small() -> World {
    World::new(Params::default(), DecorParams::default(), Radius { h: 1, v: 1 })
}

#[test]
fn streams_to_a_meshed_radius() {
    let mut w = small();
    let mut worker = Worker::new();
    let (gens, meshes) = settle(&mut w, &mut worker, IVec3::ZERO);
    // Radius 1 meshed, generated to 3: 7³ chunks.
    assert_eq!(gens, 343);
    assert_eq!(meshes, 27, "each chunk is meshed once: none of its neighbours changes after it is meshed");
    assert_eq!(w.ids_meshed_within().len(), 27);
    let st = w.stats();
    assert_eq!(st.populated, 125, "every chunk whose neighbourhood is generated is populated");
}

#[test]
fn moving_drops_far_chunks_and_meshes_new_ones() {
    let mut w = small();
    let mut worker = Worker::new();
    settle(&mut w, &mut worker, IVec3::ZERO);
    let (_, meshes) = settle(&mut w, &mut worker, ivec3(5, 0, 0));
    assert_eq!(meshes, 27);
    // Meshed chunks more than the generated rings and one of slack from the new centre are dropped.
    let dropped = w.take_unloaded();
    assert!(!dropped.is_empty());
    assert!(dropped.iter().all(|at| (*at - ivec3(5, 0, 0)).abs().max_element() > 4));
    assert_eq!(w.ids_meshed_within().len(), 27);
    assert!(w.stats().chunks <= 9 * 9 * 9);
}

#[test]
fn an_edit_remeshes_the_chunks_that_read_it() {
    let mut w = small();
    let mut worker = Worker::new();
    settle(&mut w, &mut worker, IVec3::ZERO);
    // A voxel at a chunk corner is read by 8 meshes.
    assert!(w.edit(ivec3(0, 0, 0), |v| *v = Vox::new(tex::DIRT, ether_voxel::vox::Shape::Cube, 0.0)));
    let (_, meshes) = settle(&mut w, &mut worker, IVec3::ZERO);
    assert_eq!(meshes, 8);
    // One in the middle of a chunk: 1.
    assert!(w.edit(ivec3(8, 8, 8), |v| v.tex = tex::SAND));
    let (_, meshes) = settle(&mut w, &mut worker, IVec3::ZERO);
    assert_eq!(meshes, 1);
}

#[test]
fn edited_chunks_come_back() {
    let mut w = small();
    let mut worker = Worker::new();
    settle(&mut w, &mut worker, IVec3::ZERO);
    let p = ivec3(3, 4, 5);
    w.edit(p, |v| *v = Vox::new(tex::LOG, ether_voxel::vox::Shape::Cube, 0.0));
    settle(&mut w, &mut worker, ivec3(20, 0, 0));
    assert!(w.voxel(p).is_none(), "the chunk left");
    assert_eq!(w.kept().len(), 1);
    settle(&mut w, &mut worker, IVec3::ZERO);
    assert_eq!(w.voxel(p).unwrap().tex, tex::LOG);
}

#[test]
fn collision_triangles_cover_the_ground() {
    let mut w = small();
    let mut worker = Worker::new();
    settle(&mut w, &mut worker, IVec3::ZERO);
    let mut tris = Vec::new();
    w.triangles(Vec3::splat(-8.0), Vec3::splat(8.0), &mut tris);
    assert!(!tris.is_empty());
}

#[test]
fn save_round_trip() {
    let mut w = small();
    let mut worker = Worker::new();
    settle(&mut w, &mut worker, IVec3::ZERO);
    w.edit(ivec3(1, 1, 1), |v| v.tex = tex::SAND);
    for (_, v) in w.kept() {
        let bytes = save::encode(v);
        let back = save::decode(&bytes).unwrap();
        assert!(back.iter().zip(v.iter()).all(|(a, b)| a == b));
        assert!(bytes.len() < 4096 * 4 / 2, "{} bytes", bytes.len());
    }
}

#[allow(dead_code)]
fn _done_is_send(d: Done) -> impl Send {
    d
}
