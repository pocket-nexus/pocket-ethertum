//! The player's capsule: Ethertum's controller rules, then a move through the
//! terrain's triangles.
//!
//! A frame has the reference's two halves:
//!
//! 1. **The controller** (`character_controller::input_move`), on the state the
//!    last move left: gravity is off while flying; the right stick turns the
//!    view; the left stick is the movement in the yaw frame; two jump presses
//!    within 0.3 s toggle flying, touching the ground ends it; jump and sneak
//!    climb and descend in flight; sprint latches while the stick is forward; a
//!    held jump on the ground jumps at 7 m/s unless the last held frame on the
//!    ground was under 0.3 s ago. Acceleration is 50 m/s², doubled sprinting,
//!    ×0.3 sneaking and ×0.2 in the air. Damping: `0.01^dt` on every axis in
//!    flight, else `0.0001^dt` (ground) or `0.07^dt` (air) on x and z.
//! 2. **The physics** (avian3d's rigid body): gravity `9.81 × 2`, then the
//!    capsule (radius 0.3, half-length 0.9) moved in steps of at most 0.15 m,
//!    each followed by pushing it out of every triangle it overlaps and taking
//!    the velocity into that triangle away (no friction, no bounce). A step
//!    shorter than the radius cannot carry the capsule's axis through a surface,
//!    so a flight at any speed stops at the first wall. Then the ground test:
//!    the capsule scaled by 0.99 cast 0.2 m down; the player is grounded when
//!    the first surface it meets faces within 45° of up.
//!
//! The view turns by the stick a frame at 60 Hz as the reference's gamepad path
//! does (`0.003 × 10` rad at full deflection); here that amount is scaled by
//! `dt × 60`, so a slower frame turns as far per second.

use core::f32::consts::{FRAC_PI_2, PI};

use glam::{Mat3, Quat, Vec2, Vec3};

use crate::geom::{face_normal, overlaps, segment_triangle, tri_bounds};
use crate::Terrain;

/// One frame of input. Sticks are in `[-1, 1]`, y up/forward; `touch_drag` is a finger's movement in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pad {
    pub move_: Vec2,
    pub look: Vec2,
    pub touch_drag: Vec2,
    /// Held this frame.
    pub jump: bool,
    pub sneak: bool,
    pub sprint: bool,
    /// Pressed this frame.
    pub attack: bool,
    pub use_item: bool,
    /// Hotbar slots to move by (−1, 0, 1).
    pub hotbar: i32,
}

/// What the controller is tuned by. The defaults are the reference's.
#[derive(Clone, Debug, PartialEq)]
pub struct Controller {
    pub jump_impulse: f32,
    pub acceleration: f32,
    pub max_slope_angle: f32,
    pub unfly_on_ground: bool,
    pub gravity: f32,
    pub gravity_scale: f32,
    /// Radians per pixel of mouse or touch; the stick turns ten times this per frame at 60 Hz.
    pub look_sensitivity: f32,
    pub stick_look_scale: f32,
    pub radius: f32,
    pub half_length: f32,
    /// Reads the pad at all (the reference turns this off in menus).
    pub enable_input: bool,
}

impl Default for Controller {
    fn default() -> Controller {
        Controller {
            jump_impulse: 7.0,
            acceleration: 50.0,
            max_slope_angle: PI * 0.25,
            unfly_on_ground: true,
            gravity: 9.81,
            gravity_scale: 2.0,
            look_sensitivity: 0.003,
            stick_look_scale: 10.0,
            radius: 0.3,
            half_length: 0.9,
            enable_input: true,
        }
    }
}

/// Longest move between two collision passes.
pub const MAX_STEP: f32 = 0.15;
/// How far the ground test casts down.
pub const GROUND_CAST: f32 = 0.2;
/// Scale of the capsule that casts for the ground.
pub const GROUND_CAST_SCALE: f32 = 0.99;
const SKIN: f32 = 0.0005;
const RESOLVE_ITERATIONS: usize = 6;

#[derive(Clone, Debug)]
pub struct Player {
    /// Centre of the capsule.
    pub position: Vec3,
    pub velocity: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub flying: bool,
    pub grounded: bool,
    pub sprinting: bool,
    pub sneaking: bool,
    /// The direction the ground test's surface faces, when it met one.
    pub ground_normal: Option<Vec3>,
    pub ctl: Controller,
    /// Seconds since the player was made (the reference's `Time::elapsed_secs`).
    pub time: f32,
    last_fly_jump: f32,
    last_jump: f32,
    jump_held: bool,
    /// Triangles the last step tested, for diagnostics.
    pub tested: usize,
    tris: Vec<[Vec3; 3]>,
    bounds: Vec<(Vec3, Vec3)>,
}

impl Player {
    /// A player at `position`, flying, as the reference spawns its own.
    pub fn new(position: Vec3) -> Player {
        Player {
            position,
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            flying: true,
            grounded: false,
            sprinting: false,
            sneaking: false,
            ground_normal: None,
            ctl: Controller::default(),
            time: 0.0,
            last_fly_jump: f32::NEG_INFINITY,
            last_jump: f32::NEG_INFINITY,
            jump_held: false,
            tested: 0,
            tris: Vec::new(),
            bounds: Vec::new(),
        }
    }

    /// The body's yaw as a rotation (the reference sets the transform from it each frame).
    pub fn rotation(&self) -> Quat {
        Quat::from_rotation_y(self.yaw)
    }

    /// Bottom and top of the capsule's axis.
    pub fn segment(&self) -> (Vec3, Vec3) {
        let h = Vec3::Y * self.ctl.half_length;
        (self.position - h, self.position + h)
    }

    /// One frame: the controller on the pad, then the move through `terrain`.
    pub fn step(&mut self, terrain: &dyn Terrain, pad: &Pad, dt: f32) {
        self.time += dt;
        let gravity_scale = if self.flying { 0.0 } else { self.ctl.gravity_scale };
        self.control(pad, dt);
        self.velocity.y -= self.ctl.gravity * gravity_scale * dt;
        self.collide(terrain, dt);
    }

    /// `input_move`: everything the reference does to the velocity and flags before physics runs.
    fn control(&mut self, pad: &Pad, dt: f32) {
        let mut movement = Vec3::ZERO;
        let jump_just = pad.jump && !self.jump_held;
        self.jump_held = pad.jump;

        if self.ctl.enable_input {
            // Touch look, then the stick (scaled to the reference's per-frame amount at 60 Hz).
            self.pitch -= self.ctl.look_sensitivity * pad.touch_drag.y;
            self.yaw -= self.ctl.look_sensitivity * pad.touch_drag.x;
            let s = self.ctl.look_sensitivity * self.ctl.stick_look_scale * dt * 60.0;
            self.pitch += s * pad.look.y;
            self.yaw -= s * pad.look.x;

            let is_move_forward = pad.move_.y > 0.0;
            movement.x += pad.move_.x;
            movement.z -= pad.move_.y;

            self.pitch = self.pitch.clamp(-FRAC_PI_2, FRAC_PI_2);
            if self.yaw.abs() > PI {
                self.yaw = self.yaw.rem_euclid(2.0 * PI);
            }

            self.sneaking = pad.sneak;

            if self.flying {
                if self.sneaking {
                    movement.y -= 1.0;
                }
                if pad.jump {
                    movement.y += 1.0;
                }
            }
            if jump_just {
                if self.time - self.last_fly_jump < 0.3 {
                    self.flying = !self.flying;
                }
                self.last_fly_jump = self.time;
            }
            if self.ctl.unfly_on_ground && self.grounded && self.flying {
                self.flying = false;
            }

            if is_move_forward {
                if pad.sprint {
                    self.sprinting = true;
                }
            } else {
                self.sprinting = false;
            }

            if pad.jump && self.grounded && !self.flying {
                if self.time - self.last_jump > 0.3 {
                    self.velocity.y = self.ctl.jump_impulse;
                }
                self.last_jump = self.time;
            }

            movement = Mat3::from_rotation_y(self.yaw) * movement;
        }

        let mut acceleration = self.ctl.acceleration;
        if self.sprinting {
            acceleration *= 2.0;
        }
        if self.flying {
            self.velocity += movement * acceleration * dt;
        } else {
            if self.sneaking {
                acceleration *= 0.3;
            }
            if !self.grounded {
                acceleration *= 0.2;
            }
            self.velocity.x += movement.x * acceleration * dt;
            self.velocity.z += movement.z * acceleration * dt;
        }

        if self.flying {
            self.velocity *= 0.01f32.powf(dt);
        } else {
            let damping = if self.grounded { 0.0001f32.powf(dt) } else { 0.07f32.powf(dt) };
            self.velocity.x *= damping;
            self.velocity.z *= damping;
        }
    }

    /// The physics half: move in short steps, push out of the triangles, then cast for the ground.
    fn collide(&mut self, terrain: &dyn Terrain, dt: f32) {
        let r = self.ctl.radius;
        let h = self.ctl.half_length;
        let travel = self.velocity * dt;
        let reach = Vec3::new(r, h + r, r) + Vec3::splat(0.05);
        let start = self.position;
        let end = start + travel;
        let min = start.min(end) - reach - Vec3::Y * GROUND_CAST;
        let max = start.max(end) + reach;
        self.tris.clear();
        terrain.triangles(min, max, &mut self.tris);
        self.bounds.clear();
        self.bounds.extend(self.tris.iter().map(tri_bounds));
        self.tested = 0;

        let steps = ((travel.length() / MAX_STEP).ceil() as usize).clamp(1, 256);
        let sub = dt / steps as f32;
        for _ in 0..steps {
            let before = self.position;
            self.position += self.velocity * sub;
            self.resolve(before);
        }
        self.ground_test();
    }

    /// Pushes the capsule out of the triangle it overlaps most, removing the velocity into it, until it
    /// overlaps none (or the iterations run out).
    fn resolve(&mut self, before: Vec3) {
        let r = self.ctl.radius;
        let h = Vec3::Y * self.ctl.half_length;
        for _ in 0..RESOLVE_ITERATIONS {
            let (a, b) = (self.position - h, self.position + h);
            let (cmin, cmax) = (a.min(b) - Vec3::splat(r), a.max(b) + Vec3::splat(r));
            let mut deepest: Option<(f32, Vec3)> = None;
            for (tri, (tmin, tmax)) in self.tris.iter().zip(&self.bounds) {
                if !overlaps(cmin, cmax, *tmin, *tmax) {
                    continue;
                }
                self.tested += 1;
                let (s, t) = segment_triangle(a, b, tri);
                let d = s.distance(t);
                if d >= r {
                    continue;
                }
                let (push, n) = if d > 1e-5 {
                    (r - d, (s - t) / d)
                } else {
                    // The axis crosses the triangle: back to the side the capsule came from, clear of the plane.
                    let mut n = face_normal(tri);
                    if n == Vec3::ZERO {
                        continue;
                    }
                    if n.dot(before - tri[0]) < 0.0 {
                        n = -n;
                    }
                    let behind = (-n.dot(a - tri[0])).max(-n.dot(b - tri[0])).max(0.0);
                    (behind + r, n)
                };
                if deepest.is_none_or(|(p, _)| push > p) {
                    deepest = Some((push, n));
                }
            }
            let Some((push, n)) = deepest else { break };
            self.position += n * (push + SKIN);
            let into = self.velocity.dot(n);
            if into < 0.0 {
                self.velocity -= n * into;
            }
        }
    }

    /// avian's `ShapeCaster`: the capsule scaled by 0.99 swept 0.2 m down; the first surface met decides.
    fn ground_test(&mut self) {
        let r = self.ctl.radius * GROUND_CAST_SCALE;
        let h = Vec3::Y * self.ctl.half_length * GROUND_CAST_SCALE;
        let (top, bottom) = (self.position + h, self.position - h);
        let swept_bottom = bottom - Vec3::Y * GROUND_CAST;
        let (smin, smax) = (swept_bottom - Vec3::splat(r), top + Vec3::splat(r));
        let overlap_at = |tri: &[Vec3; 3], t: f32| -> (bool, Vec3, Vec3) {
            let o = Vec3::Y * t;
            let (s, p) = segment_triangle(bottom - o, top - o, tri);
            (s.distance(p) < r, s, p)
        };
        let mut first: Option<(f32, Vec3)> = None;
        for (tri, (tmin, tmax)) in self.tris.iter().zip(&self.bounds) {
            if !overlaps(smin, smax, *tmin, *tmax) {
                continue;
            }
            // The whole sweep is one longer capsule: skip what it does not touch.
            let (s, p) = segment_triangle(swept_bottom, top, tri);
            if s.distance(p) >= r {
                continue;
            }
            let (mut lo, mut hi) = (0.0f32, GROUND_CAST);
            if !overlap_at(tri, 0.0).0 {
                for _ in 0..12 {
                    let mid = 0.5 * (lo + hi);
                    if overlap_at(tri, mid).0 {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
            } else {
                hi = 0.0;
            }
            if first.is_some_and(|(t, _)| t <= hi) {
                continue;
            }
            let (_, s, p) = overlap_at(tri, lo);
            let mut n = (s - p).normalize_or_zero();
            if n == Vec3::ZERO {
                n = face_normal(tri);
                if n.y < 0.0 {
                    n = -n;
                }
            }
            first = Some((hi, n));
        }
        self.ground_normal = first.map(|(_, n)| n);
        self.grounded = first.is_some_and(|(_, n)| n.angle_between(Vec3::Y).abs() <= self.ctl.max_slope_angle);
    }

    /// Deepest overlap of the full capsule with `terrain` (0 when clear), for tests and diagnostics.
    pub fn penetration(&self, terrain: &dyn Terrain) -> f32 {
        let r = self.ctl.radius;
        let (a, b) = self.segment();
        let mut tris = Vec::new();
        terrain.triangles(a.min(b) - Vec3::splat(r), a.max(b) + Vec3::splat(r), &mut tris);
        tris.iter().map(|t| {
            let (s, p) = segment_triangle(a, b, t);
            (r - s.distance(p)).max(0.0)
        }).fold(0.0, f32::max)
    }
}
