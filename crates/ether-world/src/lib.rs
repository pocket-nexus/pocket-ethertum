//! The loaded world: chunks streamed around the player, population and
//! meshing scheduled, edits, collision triangles.
//!
//! A chunk goes through three states, each needing more of its neighbours:
//!
//! | State | Needs | Done by |
//! | --- | --- | --- |
//! | generated | its position | a [`Job::Generate`] on a worker |
//! | populated | itself and all 26 neighbours generated | [`World`] on the calling thread, when the last of them arrives |
//! | meshed | itself and all 26 neighbours populated | a [`Job::Mesh`] on a worker, from a [`Pad`] copied here |
//!
//! Population writes leaves into neighbours, so a chunk is meshed only when
//! no neighbour can still change it. The world generates two rings beyond the
//! meshed radius for that. The reference meshes a chunk as soon as it arrives
//! and populates a chunk only when one of its neighbours arrives after it, so a
//! chunk that completes its own neighbourhood is never populated there; here
//! every chunk is populated once its neighbourhood is complete.
//!
//! An edit changes a voxel and marks every chunk whose mesh reads it (the
//! chunk and the neighbours whose two-voxel border covers it) for meshing
//! again. An edited chunk is kept when it leaves the radius and comes back
//! as it was; [`World::kept`] is what a save writes.
//!
//! The world owns no threads. [`World::plan`] hands out jobs, a host runs them
//! where it likes ([`run`] does the work), and [`World::finish`] takes the
//! results back.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use ether_voxel::chunk::{chunk_of, hood_dir, idx_v, local_of, Hood, Voxels, LEN};
use ether_voxel::gen::{Params, Scratch, Terrain};
use ether_voxel::glam::{IVec3, Vec3};
use ether_voxel::mesh::{Builder, ChunkMesh, Pad};
use ether_voxel::populate::{Decor, DecorParams};
use ether_voxel::vox::Vox;

/// Radii in chunks: horizontal and vertical. The reference's default load distance is (4, 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Radius {
    pub h: i32,
    pub v: i32,
}

impl Radius {
    pub const REFERENCE: Radius = Radius { h: 4, v: 3 };

    pub fn contains(&self, d: IVec3) -> bool {
        d.x.abs() <= self.h && d.z.abs() <= self.h && d.y.abs() <= self.v
    }

    fn grow(self, n: i32) -> Radius {
        Radius { h: self.h + n, v: self.v + n }
    }
}

/// Work a host runs off the world's thread.
pub enum Job {
    /// Generate the chunk with this index.
    Generate { at: IVec3 },
    /// Mesh the chunk from its pad; `rev` is the chunk's revision the pad was copied at.
    Mesh { at: IVec3, rev: u32, pad: Box<Pad> },
}

/// What a job produced.
pub enum Done {
    Generated { at: IVec3, voxels: Box<Voxels> },
    Meshed { at: IVec3, rev: u32, mesh: ChunkMesh, pad: Box<Pad> },
}

/// A worker's scratch.
pub struct Worker {
    pub terrain: Arc<Terrain>,
    builder: Builder,
    scratch: Box<Scratch>,
}

impl Worker {
    pub fn new(terrain: Arc<Terrain>) -> Worker {
        Worker { terrain, builder: Builder::new(), scratch: Box::new([0.0; 4096]) }
    }

    /// Runs a job.
    pub fn run(&mut self, job: Job) -> Done {
        match job {
            Job::Generate { at } => {
                let mut voxels: Box<Voxels> = Box::new([Vox::AIR; 4096]);
                self.terrain.generate(at * LEN, &mut voxels, &mut self.scratch);
                Done::Generated { at, voxels }
            }
            Job::Mesh { at, rev, pad } => {
                let mesh = self.builder.build(&pad);
                Done::Meshed { at, rev, mesh, pad }
            }
        }
    }
}

/// Runs a job on a fresh worker (for tests and hosts with one thread).
pub fn run(terrain: &Arc<Terrain>, job: Job) -> Done {
    Worker::new(terrain.clone()).run(job)
}

struct Slot {
    voxels: Option<Box<Voxels>>,
    /// A generation job is out.
    generating: bool,
    populated: bool,
    /// Bumped by every change a mesh would show.
    rev: u32,
    /// The revision of the last mesh taken back, and whether a mesh job is out.
    meshed: Option<u32>,
    meshing: bool,
    /// Changed by an edit since generation.
    edited: bool,
    /// The terrain's collision triangles in chunk coordinates, from the last mesh.
    collision: Arc<Vec<[Vec3; 3]>>,
}

impl Slot {
    fn new() -> Slot {
        Slot { voxels: None, generating: false, populated: false, rev: 0, meshed: None, meshing: false, edited: false, collision: Arc::new(Vec::new()) }
    }

    fn generated(&self) -> bool {
        self.voxels.is_some()
    }
}

/// Counts for a status line.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub chunks: usize,
    pub generated: usize,
    pub populated: usize,
    pub meshed: usize,
    pub generating: usize,
    pub meshing: usize,
    pub kept: usize,
}

pub struct World {
    pub terrain: Arc<Terrain>,
    pub decor: Decor,
    slots: HashMap<IVec3, Slot>,
    /// Edited chunks that left the radius, as they were.
    kept: HashMap<IVec3, Box<Voxels>>,
    /// The chunk the player is in.
    pub center: IVec3,
    /// Chunks within this radius are meshed.
    pub radius: Radius,
    /// Chunks that left the radius since the last call of [`World::take_unloaded`].
    unloaded: Vec<IVec3>,
    pad_pool: Vec<Box<Pad>>,
    /// The order chunks are wanted in, around the centre (the reference's centre spread).
    spread: Vec<IVec3>,
    spread_for: Radius,
}

/// Rings of chunks generated beyond the meshed radius: one for population's neighbours, one for theirs.
const MARGIN: i32 = 2;

impl World {
    pub fn new(params: Params, decor: DecorParams, radius: Radius) -> World {
        World {
            terrain: Arc::new(Terrain::new(params)),
            decor: Decor::new(decor),
            slots: HashMap::new(),
            kept: HashMap::new(),
            center: IVec3::ZERO,
            radius,
            unloaded: Vec::new(),
            pad_pool: Vec::new(),
            spread: Vec::new(),
            spread_for: Radius { h: -1, v: -1 },
        }
    }

    /// The chunk index of a world position.
    pub fn chunk_index(p: Vec3) -> IVec3 {
        chunk_of(p.floor().as_ivec3()) / LEN
    }

    /// Positions within `r` from nearest to farthest: shells of growing Chebyshev distance, as the reference's
    /// `iter_center_spread`.
    fn spread(r: Radius) -> Vec<IVec3> {
        let mut v = Vec::new();
        let n_max = r.h.max(r.v);
        for n in 0..=n_max {
            for y in -n..=n {
                for z in -n..=n {
                    for x in -n..=n {
                        if x.abs() < n && y.abs() < n && z.abs() < n {
                            continue;
                        }
                        if x.abs() > r.h || y.abs() > r.v || z.abs() > r.h {
                            continue;
                        }
                        v.push(IVec3::new(x, y, z));
                    }
                }
            }
        }
        v
    }

    pub fn voxel(&self, p: IVec3) -> Option<Vox> {
        let s = self.slots.get(&(chunk_of(p) / LEN))?;
        Some(s.voxels.as_ref()?[idx_v(local_of(p))])
    }

    /// Changes a voxel and marks every mesh that reads it. False when its chunk is not loaded.
    pub fn edit(&mut self, p: IVec3, f: impl FnOnce(&mut Vox)) -> bool {
        let at = chunk_of(p) / LEN;
        let Some(slot) = self.slots.get_mut(&at) else { return false };
        let Some(v) = slot.voxels.as_mut() else { return false };
        let before = v[idx_v(local_of(p))];
        f(&mut v[idx_v(local_of(p))]);
        if v[idx_v(local_of(p))] == before {
            return true;
        }
        slot.edited = true;
        // Chunks whose pad covers p: a pad reaches two voxels into each neighbour.
        let l = local_of(p);
        let span = |c: i32| -> (i32, i32) { (if c < 2 { -1 } else { 0 }, if c >= LEN - 2 { 1 } else { 0 }) };
        let (sx, sy, sz) = (span(l.x), span(l.y), span(l.z));
        for dx in sx.0..=sx.1 {
            for dy in sy.0..=sy.1 {
                for dz in sz.0..=sz.1 {
                    if let Some(s) = self.slots.get_mut(&(at + IVec3::new(dx, dy, dz))) {
                        s.rev = s.rev.wrapping_add(1);
                    }
                }
            }
        }
        true
    }

    /// Moves the centre and the radius, drops chunks beyond the generated rings (keeping edited ones), and
    /// returns jobs: generation of the nearest missing chunks up to `generate` jobs out at once, meshing of the
    /// nearest chunks whose mesh is out of date up to `mesh` jobs out at once (edited chunks first).
    pub fn plan(&mut self, center: IVec3, generate: usize, mesh: usize) -> Vec<Job> {
        self.center = center;
        let outer = self.radius.grow(MARGIN);
        if self.spread_for != outer {
            self.spread = World::spread(outer);
            self.spread_for = outer;
        }
        // Leave a ring of slack before dropping, so standing on a border does not churn.
        let keep = outer.grow(1);
        let gone: Vec<IVec3> = self.slots.iter().filter(|(at, s)| !keep.contains(**at - center) && !s.generating && !s.meshing).map(|(at, _)| *at).collect();
        for at in gone {
            let s = self.slots.remove(&at).unwrap();
            if s.edited {
                if let Some(v) = s.voxels {
                    self.kept.insert(at, v);
                }
            }
            if s.meshed.is_some() {
                self.unloaded.push(at);
            }
        }

        let mut jobs = Vec::new();
        let mut generating = self.slots.values().filter(|s| s.generating).count();
        let mut meshing = self.slots.values().filter(|s| s.meshing).count();
        let mut restored = Vec::new();
        for i in 0..self.spread.len() {
            if generating >= generate {
                break;
            }
            let at = center + self.spread[i];
            if self.slots.contains_key(&at) {
                continue;
            }
            if let Some(v) = self.kept.remove(&at) {
                let mut s = Slot::new();
                s.voxels = Some(v);
                s.populated = true;
                s.edited = true;
                self.slots.insert(at, s);
                restored.push(at);
                continue;
            }
            let mut s = Slot::new();
            s.generating = true;
            self.slots.insert(at, s);
            jobs.push(Job::Generate { at });
            generating += 1;
        }
        for at in restored {
            self.after_arrival(at);
        }

        // Meshing: edited chunks first, then by distance.
        let mut want: Vec<(bool, i32, IVec3)> = Vec::new();
        for (at, s) in &self.slots {
            if s.meshing || !s.populated || s.meshed == Some(s.rev) || !self.radius.contains(*at - center) {
                continue;
            }
            want.push((!s.edited, (*at - center).abs().max_element(), *at));
        }
        want.sort_unstable_by_key(|w| (w.0, w.1, w.2.to_array()));
        for (_, _, at) in want {
            if meshing >= mesh {
                break;
            }
            if !self.neighbours(at, |s| s.populated) {
                continue;
            }
            let mut pad = self.pad_pool.pop().unwrap_or_else(|| Box::new(Pad::new()));
            self.fill(at, &mut pad);
            let s = self.slots.get_mut(&at).unwrap();
            s.meshing = true;
            jobs.push(Job::Mesh { at, rev: s.rev, pad });
            meshing += 1;
        }
        jobs
    }

    /// Takes a job's result back. A mesh is returned for the renderer when it is still the chunk's (the chunk
    /// may have left the radius meanwhile).
    pub fn finish(&mut self, done: Done) -> Option<(IVec3, ChunkMesh)> {
        match done {
            Done::Generated { at, voxels } => {
                let Some(s) = self.slots.get_mut(&at) else { return None };
                s.generating = false;
                s.voxels = Some(voxels);
                self.after_arrival(at);
                None
            }
            Done::Meshed { at, rev, mesh, pad } => {
                self.pad_pool.push(pad);
                let s = self.slots.get_mut(&at)?;
                s.meshing = false;
                s.meshed = Some(rev);
                s.collision = Arc::new(mesh.collision.clone());
                Some((at, mesh))
            }
        }
    }

    /// Chunks whose meshes the renderer should drop, since the last call.
    pub fn take_unloaded(&mut self) -> Vec<IVec3> {
        core::mem::take(&mut self.unloaded)
    }

    fn neighbours(&self, at: IVec3, ok: impl Fn(&Slot) -> bool) -> bool {
        (0..27).all(|slot| self.slots.get(&(at + hood_dir(slot))).is_some_and(&ok))
    }

    /// A chunk arrived: populate it and every neighbour whose neighbourhood it completed, nearest first.
    fn after_arrival(&mut self, at: IVec3) {
        let mut ready: Vec<IVec3> = (0..27).map(|slot| at + hood_dir(slot)).filter(|c| self.slots.get(c).is_some_and(|s| s.generated() && !s.populated) && self.neighbours(*c, |s| s.generated())).collect();
        let center = self.center;
        ready.sort_unstable_by_key(|c| ((*c - center).abs().max_element(), c.to_array()));
        for c in ready {
            self.populate(c);
        }
    }

    fn populate(&mut self, at: IVec3) {
        // Leaves a neighbour's tree would drop into an edited chunk land on a copy: what the player changed stays.
        let mut copies: Vec<Box<Voxels>> = Vec::new();
        let mut hood = Hood::new();
        for slot in 0..27 {
            let c = at + hood_dir(slot);
            let s = self.slots.get_mut(&c).unwrap();
            let v = s.voxels.as_mut().unwrap();
            if slot != 13 && s.edited {
                copies.push(v.clone());
                let p: *mut Voxels = copies.last_mut().unwrap().as_mut();
                // The copy lives in `copies` until the hood is dropped below.
                hood.chunks[slot] = Some(unsafe { &mut *p });
            } else {
                let p: *mut Voxels = v.as_mut();
                // Distinct chunks, distinct boxes: the 27 borrows do not alias, and none outlives this call.
                hood.chunks[slot] = Some(unsafe { &mut *p });
            }
        }
        self.decor.populate(&mut hood, at * LEN);
        drop(hood);
        drop(copies);
        for slot in 0..27 {
            let s = self.slots.get_mut(&(at + hood_dir(slot))).unwrap();
            s.rev = s.rev.wrapping_add(1);
        }
        self.slots.get_mut(&at).unwrap().populated = true;
    }

    fn fill(&self, at: IVec3, pad: &mut Pad) {
        pad.fill_from(|slot| self.slots.get(&(at + hood_dir(slot))).and_then(|s| s.voxels.as_deref()));
    }

    /// The terrain's triangles in world coordinates whose chunk boxes meet `[min, max]`. A chunk not meshed yet
    /// contributes none.
    pub fn triangles(&self, min: Vec3, max: Vec3, out: &mut Vec<[Vec3; 3]>) {
        let (lo, hi) = (World::chunk_index(min - 1.0), World::chunk_index(max + 1.0));
        for x in lo.x..=hi.x {
            for y in lo.y..=hi.y {
                for z in lo.z..=hi.z {
                    let at = IVec3::new(x, y, z);
                    let Some(s) = self.slots.get(&at) else { continue };
                    let o = (at * LEN).as_vec3();
                    for t in s.collision.iter() {
                        let (a, b, c) = (t[0] + o, t[1] + o, t[2] + o);
                        let tmin = a.min(b).min(c);
                        let tmax = a.max(b).max(c);
                        if tmax.cmpge(min).all() && tmin.cmple(max).all() {
                            out.push([a, b, c]);
                        }
                    }
                }
            }
        }
    }

    /// Whether the chunk at `at` has a mesh taken back.
    pub fn is_meshed(&self, at: IVec3) -> bool {
        self.slots.get(&at).is_some_and(|s| s.meshed.is_some())
    }

    /// Edited chunks: loaded ones and kept ones.
    pub fn kept(&self) -> Vec<(IVec3, &Voxels)> {
        let mut v: Vec<(IVec3, &Voxels)> = self.kept.iter().map(|(at, b)| (*at, b.as_ref())).collect();
        v.extend(self.slots.iter().filter(|(_, s)| s.edited).filter_map(|(at, s)| Some((*at, s.voxels.as_deref()?))));
        v
    }

    /// Restores an edited chunk read from a save; it takes the place of generation when it is wanted.
    pub fn restore(&mut self, at: IVec3, voxels: Box<Voxels>) {
        self.kept.insert(at, voxels);
    }

    /// Drops everything loaded (after a change of generation parameters), keeping edited chunks.
    pub fn reset(&mut self, params: Option<Params>, decor: Option<DecorParams>) {
        if let Some(p) = params {
            self.terrain = Arc::new(Terrain::new(p));
        }
        if let Some(d) = decor {
            self.decor = Decor::new(d);
        }
        let all: Vec<IVec3> = self.slots.keys().copied().collect();
        for at in all {
            let s = self.slots.remove(&at).unwrap();
            if s.edited {
                if let Some(v) = s.voxels {
                    self.kept.insert(at, v);
                }
            }
            if s.meshed.is_some() {
                self.unloaded.push(at);
            }
        }
    }

    /// Marks every loaded mesh out of date (after a change of how materials look).
    pub fn remesh_all(&mut self) {
        for s in self.slots.values_mut() {
            s.rev = s.rev.wrapping_add(1);
        }
    }

    pub fn stats(&self) -> Stats {
        let mut st = Stats { chunks: self.slots.len(), kept: self.kept.len(), ..Default::default() };
        for s in self.slots.values() {
            st.generated += s.generated() as usize;
            st.populated += s.populated as usize;
            st.meshed += s.meshed.is_some() as usize;
            st.generating += s.generating as usize;
            st.meshing += s.meshing as usize;
        }
        st
    }

    /// Whether every chunk within the meshed radius has a mesh at its current revision.
    pub fn settled(&self) -> bool {
        let r = self.radius;
        World::spread(r).iter().all(|d| self.slots.get(&(self.center + *d)).is_some_and(|s| s.meshed == Some(s.rev)))
    }

    pub fn ids_meshed_within(&self) -> HashSet<IVec3> {
        self.slots.iter().filter(|(at, s)| s.meshed.is_some() && self.radius.contains(**at - self.center)).map(|(at, _)| *at).collect()
    }
}

/// Edited chunks on disk: the four bytes of every voxel as four planes of 4096 (material, shape, iso value,
/// light), each stored as a flag byte and then either the plane itself (flag 0) or `(run, byte)` pairs (flag 1),
/// whichever is shorter. Materials and shapes run long; iso values vary voxel to voxel.
pub mod save {
    use super::*;

    fn plane(v: &Voxels, k: usize) -> [u8; 4096] {
        core::array::from_fn(|i| v[i].to_bits().to_le_bytes()[k])
    }

    pub fn encode(v: &Voxels) -> Vec<u8> {
        let mut out = Vec::new();
        for k in 0..4 {
            let p = plane(v, k);
            let mut runs = Vec::new();
            let mut i = 0;
            while i < p.len() {
                let mut n = 1;
                while i + n < p.len() && n < 255 && p[i + n] == p[i] {
                    n += 1;
                }
                runs.extend_from_slice(&[n as u8, p[i]]);
                i += n;
            }
            if runs.len() < p.len() {
                out.push(1);
                out.extend_from_slice(&(runs.len() as u16).to_le_bytes());
                out.extend_from_slice(&runs);
            } else {
                out.push(0);
                out.extend_from_slice(&p);
            }
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<Box<Voxels>> {
        let mut planes = [[0u8; 4096]; 4];
        let mut at = 0;
        for plane in planes.iter_mut() {
            match *bytes.get(at)? {
                0 => {
                    plane.copy_from_slice(bytes.get(at + 1..at + 1 + 4096)?);
                    at += 1 + 4096;
                }
                1 => {
                    let len = u16::from_le_bytes([*bytes.get(at + 1)?, *bytes.get(at + 2)?]) as usize;
                    let runs = bytes.get(at + 3..at + 3 + len)?;
                    let mut i = 0;
                    for r in runs.chunks_exact(2) {
                        let n = r[0] as usize;
                        plane.get_mut(i..i + n)?.fill(r[1]);
                        i += n;
                    }
                    if i != 4096 {
                        return None;
                    }
                    at += 3 + len;
                }
                _ => return None,
            }
        }
        let mut v: Box<Voxels> = Box::new([Vox::AIR; 4096]);
        for (i, x) in v.iter_mut().enumerate() {
            *x = Vox::from_bits(u32::from_le_bytes([planes[0][i], planes[1][i], planes[2][i], planes[3][i]]));
        }
        Some(v)
    }
}
