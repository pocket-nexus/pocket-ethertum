//! The player in real Ethertum terrain (generated, populated and meshed by
//! `ether-voxel` around the origin) and in a box of planes.

use std::collections::HashMap;
use std::time::Instant;

use ether_sim::brush::{self, Stroke, VoxelBrush};
use ether_sim::clock::WorldClock;
use ether_sim::geom::{overlaps, tri_bounds};
use ether_sim::inventory::PlayerInfo;
use ether_sim::player::{Pad, Player};
use ether_sim::ray::{self, HitResult};
use ether_sim::{Terrain, Vox};
use ether_voxel::chunk::{hood_dir, idx, Hood, Voxels};
use ether_voxel::gen::{Params, Terrain as Gen};
use ether_voxel::mesh::{Builder, Pad as MeshPad};
use ether_voxel::populate::{Decor, DecorParams};
use ether_voxel::vox::{iso_u8, Shape};
use glam::{ivec3, vec2, vec3, IVec3, Vec3};

const DT: f32 = 1.0 / 60.0;

/// Chunks generated over `-3..=2` (chunk units) on each axis, populated over `-2..=1` and meshed there: the
/// playable box is `-32..32` voxels on each axis.
struct World {
    voxels: HashMap<IVec3, Box<Voxels>>,
    tris: HashMap<IVec3, Vec<[Vec3; 3]>>,
}

impl World {
    fn build() -> World {
        let gen = Gen::new(Params::default());
        let decor = Decor::new(DecorParams::default());
        let mut scratch = [0f64; 4096];
        let mut voxels: HashMap<IVec3, Box<Voxels>> = HashMap::new();
        for x in -3..=2 {
            for y in -3..=2 {
                for z in -3..=2 {
                    let cp = ivec3(x, y, z) * 16;
                    let mut v = Box::new([Vox::AIR; 4096]);
                    gen.generate(cp, &mut v, &mut scratch);
                    voxels.insert(cp, v);
                }
            }
        }
        let mut w = World { voxels, tris: HashMap::new() };
        for y in (-2..=1).rev() {
            for x in -2..=1 {
                for z in -2..=1 {
                    let cp = ivec3(x, y, z) * 16;
                    let mut hood = w.hood(cp);
                    decor.populate(&mut hood, cp);
                }
            }
        }
        w.remesh_all();
        w
    }

    fn hood(&mut self, cp: IVec3) -> Hood<'_> {
        let mut hood = Hood::new();
        for slot in 0..27 {
            if let Some(v) = self.voxels.get_mut(&(cp + hood_dir(slot) * 16)) {
                hood.chunks[slot] = Some(unsafe { &mut *(v.as_mut() as *mut Voxels) });
            }
        }
        hood
    }

    fn remesh_all(&mut self) {
        let mut builder = Builder::new();
        let mut pad = Box::new(MeshPad::new());
        self.tris.clear();
        for x in -2..=1 {
            for y in -2..=1 {
                for z in -2..=1 {
                    let cp = ivec3(x, y, z) * 16;
                    pad.fill(&self.hood(cp));
                    let mesh = builder.build(&pad);
                    let o = cp.as_vec3();
                    self.tris.insert(cp, mesh.collision.iter().map(|t| [t[0] + o, t[1] + o, t[2] + o]).collect());
                }
            }
        }
    }
}

impl Terrain for World {
    fn triangles(&self, min: Vec3, max: Vec3, out: &mut Vec<[Vec3; 3]>) {
        let lo = (min / 16.0).floor().as_ivec3() * 16;
        let hi = (max / 16.0).floor().as_ivec3() * 16;
        for x in (lo.x..=hi.x).step_by(16) {
            for y in (lo.y..=hi.y).step_by(16) {
                for z in (lo.z..=hi.z).step_by(16) {
                    if let Some(ts) = self.tris.get(&ivec3(x, y, z)) {
                        out.extend(ts.iter().filter(|t| {
                            let (a, b) = tri_bounds(t);
                            overlaps(min, max, a, b)
                        }));
                    }
                }
            }
        }
    }

    fn voxel(&self, p: IVec3) -> Option<Vox> {
        let c = self.voxels.get(&IVec3::new(p.x & !15, p.y & !15, p.z & !15))?;
        Some(c[idx(p.x & 15, p.y & 15, p.z & 15)])
    }

    fn edit(&mut self, p: IVec3, f: &mut dyn FnMut(&mut Vox)) -> bool {
        let Some(c) = self.voxels.get_mut(&IVec3::new(p.x & !15, p.y & !15, p.z & !15)) else { return false };
        f(&mut c[idx(p.x & 15, p.y & 15, p.z & 15)]);
        true
    }
}

/// Planes: a floor at y = 0 and, optionally, a wall at x = 10 one voxel thick.
struct Planes {
    tris: Vec<[Vec3; 3]>,
}

impl Planes {
    fn new(wall: bool) -> Planes {
        let mut tris = Vec::new();
        let mut quad = |a: Vec3, b: Vec3, c: Vec3, d: Vec3| {
            tris.push([a, b, c]);
            tris.push([a, c, d]);
        };
        // A floor of 1 m tiles, like a mesh.
        for x in -40..40 {
            for z in -40..40 {
                let (x, z) = (x as f32, z as f32);
                quad(vec3(x, 0., z), vec3(x, 0., z + 1.), vec3(x + 1., 0., z + 1.), vec3(x + 1., 0., z));
            }
        }
        if wall {
            for x in [10.0f32, 11.0] {
                quad(vec3(x, -50., -50.), vec3(x, 50., -50.), vec3(x, 50., 50.), vec3(x, -50., 50.));
            }
        }
        Planes { tris }
    }
}

impl Terrain for Planes {
    fn triangles(&self, min: Vec3, max: Vec3, out: &mut Vec<[Vec3; 3]>) {
        out.extend(self.tris.iter().filter(|t| {
            let (a, b) = tri_bounds(t);
            overlaps(min, max, a, b)
        }));
    }
    fn voxel(&self, _: IVec3) -> Option<Vox> {
        None
    }
    fn edit(&mut self, _: IVec3, _: &mut dyn FnMut(&mut Vox)) -> bool {
        false
    }
}

/// A column whose surface is level over a 2 m square with 12 m of air over it, as rays from 31 m find it.
fn flat_ground(w: &World) -> (Vec3, HitResult) {
    let mut scratch = Vec::new();
    for x in (-16..16).step_by(2) {
        for z in (-16..16).step_by(2) {
            let top = vec3(x as f32 + 0.25, 31.0, z as f32 + 0.25);
            let hit = ray::cast(w, top, Vec3::NEG_Y, 60.0, &mut scratch);
            if !hit.is_hit || hit.normal.y < 0.995 || hit.position.y > 18.0 || hit.position.y < -28.0 {
                continue;
            }
            let level = [vec3(-1., 0., -1.), vec3(1., 0., -1.), vec3(-1., 0., 1.), vec3(1., 0., 1.)].iter().all(|d| {
                let h = ray::cast(w, top + *d, Vec3::NEG_Y, 60.0, &mut scratch);
                h.is_hit && h.normal.y > 0.98 && (h.position.y - hit.position.y).abs() < 0.2
            });
            if level {
                return (top, hit);
            }
        }
    }
    panic!("no level ground in the test block");
}

/// The capsule's bottom against the surface under it: negative when it is below the first surface a ray from
/// above meets in its column (within 3 m, so a cave ceiling far above does not count).
fn clearance(w: &World, p: &Player, scratch: &mut Vec<[Vec3; 3]>) -> Option<f32> {
    let bottom = p.position.y - 1.2;
    let from = vec3(p.position.x, p.position.y + 1.0, p.position.z);
    let down = ray::cast(w, from, Vec3::NEG_Y, 5.0, scratch);
    if down.is_hit {
        Some(bottom - down.position.y)
    } else {
        None
    }
}

#[test]
fn lands_rests_and_does_not_sink() {
    let w = World::build();
    let (_, ground) = flat_ground(&w);
    let mut p = Player::new(ground.position + Vec3::Y * (10.0 + 1.2));
    p.flying = false;
    let pad = Pad::default();
    let mut scratch = Vec::new();
    let mut grounded_frames = 0;
    let mut rest = Vec3::ZERO;
    let t = Instant::now();
    for frame in 0..600 {
        p.step(&w, &pad, DT);
        let pen = p.penetration(&w);
        assert!(pen < 0.02, "frame {frame}: penetrates {pen}");
        if let Some(c) = clearance(&w, &p, &mut scratch) {
            assert!(c > -0.35, "frame {frame}: {c} under the surface at {}", p.position);
        }
        if frame == 300 {
            rest = p.position;
        }
        if frame >= 300 {
            grounded_frames += p.grounded as u32;
        }
    }
    let per = t.elapsed().as_secs_f64() * 1e6 / 600.0;
    println!("landing: {per:.1} µs a step ({} triangle tests in the last), rest at {} over ground {}", p.tested, p.position, ground.position);
    assert_eq!(grounded_frames, 300, "grounded through the last 5 s");
    assert!(p.position.distance(rest) < 0.5, "rests on level ground: moved {}", p.position.distance(rest));
    let bottom = p.position.y - 1.2;
    assert!((bottom - ground.position.y).abs() < 0.3, "the capsule's bottom {bottom} rests on {}", ground.position.y);
}

#[test]
fn walks_over_terrain_without_sinking() {
    let w = World::build();
    let (_, ground) = flat_ground(&w);
    let mut p = Player::new(ground.position + Vec3::Y * 1.25);
    p.flying = false;
    for _ in 0..60 {
        p.step(&w, &Pad::default(), DT);
    }
    let mut scratch = Vec::new();
    let start = p.position;
    let mut cost = 0.0f64;
    for (i, yaw) in [0.0f32, 1.6, 3.1, 4.7].iter().enumerate() {
        p.yaw = *yaw;
        let pad = Pad { move_: vec2(0.0, 1.0), sprint: i % 2 == 1, ..Default::default() };
        for frame in 0..90 {
            let t = Instant::now();
            p.step(&w, &pad, DT);
            cost += t.elapsed().as_secs_f64();
            let pen = p.penetration(&w);
            assert!(pen < 0.03, "heading {yaw} frame {frame}: penetrates {pen} at {}", p.position);
            if let Some(c) = clearance(&w, &p, &mut scratch) {
                assert!(c > -0.35, "heading {yaw} frame {frame}: {c} under the surface at {}", p.position);
            }
            assert!(p.position.abs().max_element() < 30.0, "left the meshed block at {}", p.position);
        }
    }
    println!("walking: {:.1} µs a step ({} triangle tests in the last), ended {:.1} m from the start", cost * 1e6 / 360.0, p.tested, (p.position - start).length());
    assert!(p.position.distance(start) > 1.0, "moved");
}

#[test]
fn walk_speeds_match_the_damping() {
    // On level planes: walking settles at a·dt·k/(1−k) with k = 0.0001^dt, about 5 m/s at 60 Hz; sprinting twice that.
    let planes = Planes::new(false);
    for (sprint, expect) in [(false, 5.02f32), (true, 10.04)] {
        let mut p = Player::new(vec3(0.0, 1.21, 0.0));
        p.flying = false;
        let pad = Pad { move_: vec2(0.0, 1.0), sprint, ..Default::default() };
        for _ in 0..120 {
            p.step(&planes, &pad, DT);
        }
        let v = vec2(p.velocity.x, p.velocity.z).length();
        assert!((v - expect).abs() < 0.1, "sprint {sprint}: {v} m/s");
        assert!(p.velocity.z < 0.0, "yaw 0 walks toward −z");
    }
}

#[test]
fn walking_into_a_wall_slides_along_it() {
    let planes = Planes::new(true);
    let mut p = Player::new(vec3(8.0, 1.25, 0.0));
    p.flying = false;
    // Toward +x and +z, into the wall at an angle.
    p.yaw = -std::f32::consts::FRAC_PI_4 - std::f32::consts::FRAC_PI_2;
    let pad = Pad { move_: vec2(0.0, 1.0), ..Default::default() };
    for _ in 0..300 {
        p.step(&planes, &pad, DT);
        assert!(p.position.x <= 10.0 - 0.3 + 1e-3, "through the wall: {}", p.position);
    }
    assert!(p.position.z > 5.0, "slid along the wall: {}", p.position);
    assert!(p.grounded);
}

#[test]
fn fast_flight_stops_at_a_wall() {
    let planes = Planes::new(true);
    for speed in [60.0f32, 200.0] {
        let mut p = Player::new(vec3(0.0, 5.0, 0.0));
        assert!(p.flying);
        p.velocity = vec3(speed, 0.0, 0.0);
        for _ in 0..120 {
            p.step(&planes, &Pad::default(), DT);
            assert!(p.position.x <= 10.0 - 0.3 + 1e-3, "{speed} m/s: through the wall at {}", p.position);
        }
        assert!(p.position.x > 9.6, "{speed} m/s reached the wall: {}", p.position);
        assert!(p.velocity.x.abs() < 1e-3, "no velocity into the wall");
    }
}

#[test]
fn two_jumps_toggle_flying_and_the_ground_ends_it() {
    let planes = Planes::new(false);
    let mut p = Player::new(vec3(0.0, 1.21, 0.0));
    p.flying = false;
    for _ in 0..30 {
        p.step(&planes, &Pad::default(), DT);
    }
    assert!(p.grounded && !p.flying);
    let jump = Pad { jump: true, ..Default::default() };
    let none = Pad::default();
    p.step(&planes, &jump, DT);
    assert!(p.velocity.y > 6.0, "jumped: {}", p.velocity);
    for _ in 0..8 {
        p.step(&planes, &none, DT);
    }
    p.step(&planes, &jump, DT);
    assert!(p.flying, "second press within 0.3 s");
    for _ in 0..60 {
        p.step(&planes, &none, DT);
    }
    assert!(p.flying && p.position.y > 1.5, "hovers: {}", p.position);
    // Descend with sneak until the ground ends the flight.
    let down = Pad { sneak: true, ..Default::default() };
    for _ in 0..600 {
        p.step(&planes, &down, DT);
        if !p.flying {
            break;
        }
    }
    assert!(!p.flying && p.grounded, "landing ends the flight at {}", p.position);
}

#[test]
fn a_held_jump_jumps_once_per_landing() {
    let planes = Planes::new(false);
    let mut p = Player::new(vec3(0.0, 1.21, 0.0));
    p.flying = false;
    for _ in 0..30 {
        p.step(&planes, &Pad::default(), DT);
    }
    let jump = Pad { jump: true, ..Default::default() };
    let mut jumps = 0;
    let mut last_vy = 0.0;
    for _ in 0..240 {
        p.step(&planes, &jump, DT);
        if p.velocity.y > 6.0 && last_vy <= 6.0 {
            jumps += 1;
        }
        last_vy = p.velocity.y;
    }
    // A jump at 7 m/s under 19.62 m/s² lasts 0.71 s: four seconds of holding is five or six jumps.
    assert!((4..=7).contains(&jumps), "{jumps} jumps");
}

#[test]
fn the_ray_names_the_voxel_under_the_surface() {
    let w = World::build();
    let mut scratch = Vec::new();
    let mut checked = 0;
    for x in (-28..28).step_by(5) {
        for z in (-28..28).step_by(5) {
            let top = vec3(x as f32 + 0.5, 31.0, z as f32 + 0.5);
            let hit = ray::cast(&w, top, Vec3::NEG_Y, 60.0, &mut scratch);
            if !hit.is_hit || hit.normal.y < 0.9 {
                continue;
            }
            assert!(hit.normal.y > 0.0, "faces the eye");
            assert!((hit.distance - (top.y - hit.position.y)).abs() < 1e-3);
            // A voxel's sample sits at its corner + 0.5: the surface crosses between the highest solid sample
            // and the empty one above it.
            let col = hit.voxel_pos;
            let solid = (col.y - 1..=col.y).rev().find(|&y| w.voxel(ivec3(col.x, y, col.z)).is_some_and(|v| v.isovalue() > 0.0));
            assert!(solid.is_some(), "the hit voxel {col} or the one below is solid ({:?})", hit);
            checked += 1;
        }
    }
    assert!(checked > 20, "{checked} columns");
}

#[test]
fn break_and_place_follow_the_reference() {
    let mut w = World::build();
    let (top, _) = flat_ground(&w);
    let mut scratch = Vec::new();
    let hit = ray::cast(&w, top, Vec3::NEG_Y, 60.0, &mut scratch);
    let brush = VoxelBrush::default();
    let n = brush.size as i32;
    let before: HashMap<IVec3, Vox> = (-n..=n).flat_map(|y| (-n..=n).flat_map(move |z| (-n..=n).map(move |x| ivec3(x, y, z)))).map(|lp| (lp, w.voxel(hit.voxel_pos + lp).unwrap())).collect();
    let touched = brush::apply(&mut w, &hit, &brush, Stroke::Break);
    assert_eq!(touched.len(), (2 * n + 1).pow(3) as usize);
    let mut emptied = 0;
    for (lp, b) in &before {
        let a = w.voxel(hit.voxel_pos + *lp).unwrap();
        let f = (n as f32 - lp.as_vec3().length()).max(0.) * brush.strength;
        let mut expect = *b;
        expect.iso = iso_u8(b.isovalue() - f);
        if f > 0.0 && expect.is_isoval_empty() {
            expect.tex = 0;
        }
        assert_eq!(a, expect, "break at offset {lp}");
        if b.tex != 0 && a.tex == 0 {
            emptied += 1;
        }
    }
    assert!(emptied > 20, "the break emptied {emptied} voxels");

    // Place a cube of material 5 on a face square to an axis: one voxel out along the normal.
    let flat = HitResult { is_hit: true, position: hit.position, normal: Vec3::Y, distance: 1.0, voxel_pos: hit.voxel_pos };
    let cube = VoxelBrush { shape: Shape::Cube, tex: 5, size: 1.0, strength: 0.8 };
    let touched = brush::apply(&mut w, &flat, &cube, Stroke::Place);
    assert_eq!(touched[0], hit.voxel_pos + ivec3(-1, 0, -1), "first visited: one up, then -1 on each axis");
    let centre = w.voxel(hit.voxel_pos + IVec3::Y).unwrap();
    assert_eq!((centre.tex, centre.shape, centre.iso), (5, Shape::Cube, iso_u8(0.0)));
    // The corners of the 3³ block are beyond the radius: f = 0, so they keep their material.
    let corner = w.voxel(hit.voxel_pos + ivec3(1, 2, 1)).unwrap();
    assert_ne!(corner.tex, 5);

    // A slanted normal truncates to no offset.
    assert_eq!(brush::place_offset(vec3(0.6, 0.8, 0.0)), IVec3::ZERO);

    let mut chunks = Vec::new();
    brush::chunks_reading(ivec3(15, 0, 1), 2, &mut chunks);
    chunks.sort_by_key(|c| (c.x, c.y, c.z));
    assert_eq!(chunks, vec![ivec3(0, -16, -16), ivec3(0, -16, 0), ivec3(0, 0, -16), ivec3(0, 0, 0), ivec3(16, -16, -16), ivec3(16, -16, 0), ivec3(16, 0, -16), ivec3(16, 0, 0)]);
}

#[test]
fn the_clock_turns_the_sun() {
    let mut c = WorldClock::default();
    assert!(c.sun_dir().y > 0.7, "morning sun high: {}", c.sun_dir());
    assert!((c.sun_strength() - (0.15f32 * std::f32::consts::TAU).sin().powi(2)).abs() < 1e-6);
    for _ in 0..(1440 * 60 / 2) {
        c.tick(DT);
    }
    assert!((c.daytime - 0.65).abs() < 1e-3, "{}", c.daytime);
    assert_eq!(c.sun_strength(), 0.0);
    assert!(c.sun_dir().y < 0.0);
}

#[test]
fn hotbar_wraps() {
    let mut info = PlayerInfo::default();
    assert_eq!((info.inventory.items.len(), info.health, info.health_max), (36, 20, 20));
    info.scroll_hotbar(-1);
    assert_eq!(info.hotbar_index, 8);
    info.scroll_hotbar(2);
    assert_eq!(info.hotbar_index, 1);
    assert_eq!(info.hotbar().len(), 9);
}

