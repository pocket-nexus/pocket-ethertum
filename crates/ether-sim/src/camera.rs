//! The camera that follows the player (`character_controller::sync_camera`).
//!
//! The eye is 0.8 m above the capsule's centre, pulled back along the view by
//! `distance` for the third person view; the rotation is yaw then pitch
//! (`EulerRot::YXZ`). The vertical field of view is 85°, 105° while sprinting
//! and 24° while zooming, eased toward its target at 16 per second. The third
//! person distance eases at 18 per second toward 0 or 4 m when toggled.

use glam::camera::rh::{proj, view::look_to_mat4};
use glam::{EulerRot, Mat4, Quat, Vec3};

use crate::player::Player;

/// `util::SmoothValue`: `current += dt · (target − current)`, with `dt` already scaled by the rate.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SmoothValue {
    pub target: f32,
    pub current: f32,
}

impl SmoothValue {
    pub fn new(v: f32) -> SmoothValue {
        SmoothValue { target: v, current: v }
    }

    pub fn update(&mut self, dt: f32) {
        self.current += dt * (self.target - self.current);
    }
}

#[derive(Clone, Debug)]
pub struct Camera {
    pub eye: Vec3,
    pub rotation: Quat,
    /// Vertical field of view in degrees, as it eases.
    pub fov: SmoothValue,
    /// The field of view the settings ask for (the reference's `ClientSettings::fov`).
    pub base_fov: f32,
    pub zoom: bool,
    /// Third person distance in metres.
    pub distance: SmoothValue,
}

impl Default for Camera {
    fn default() -> Camera {
        // The reference eases its field of view up from 0 while its menu shows; it starts settled here.
        Camera { eye: Vec3::ZERO, rotation: Quat::IDENTITY, fov: SmoothValue::new(85.0), base_fov: 85.0, zoom: false, distance: SmoothValue::new(0.0) }
    }
}

impl Camera {
    /// Follows the player after its step.
    pub fn follow(&mut self, player: &Player, dt: f32) {
        self.distance.update(dt * 18.0);
        self.rotation = Quat::from_euler(EulerRot::YXZ, player.yaw, player.pitch, 0.0);
        self.eye = player.position + Vec3::new(0.0, 0.8, 0.0) + self.forward() * -self.distance.current;
        self.fov.target = if self.zoom {
            24.0
        } else if player.sprinting {
            self.base_fov + 20.0
        } else {
            self.base_fov
        };
        self.fov.update(dt * 16.0);
    }

    /// Between the first person view and 4 m behind the player.
    pub fn toggle_third_person(&mut self) {
        self.distance.target = if self.distance.target < 1.0 { 4.0 } else { 0.0 };
    }

    pub fn forward(&self) -> Vec3 {
        self.rotation * Vec3::NEG_Z
    }

    pub fn up(&self) -> Vec3 {
        self.rotation * Vec3::Y
    }

    pub fn fov_radians(&self) -> f32 {
        self.fov.current.to_radians()
    }

    /// World to view, right-handed (−Z forward).
    pub fn view(&self) -> Mat4 {
        look_to_mat4(self.eye, self.forward(), self.up())
    }

    /// The view without its translation, for drawing relative to the eye (chunks far from the origin keep
    /// their precision when the renderer subtracts the eye on the CPU).
    pub fn view_rotation(&self) -> Mat4 {
        look_to_mat4(Vec3::ZERO, self.forward(), self.up())
    }

    /// Perspective projection with depth in `[0, 1]`.
    pub fn projection(&self, aspect: f32, near: f32, far: f32) -> Mat4 {
        proj::directx::perspective(self.fov_radians(), aspect, near, far)
    }

    /// Perspective projection with depth in `[-1, 1]` (OpenGL convention).
    pub fn projection_gl(&self, aspect: f32, near: f32, far: f32) -> Mat4 {
        proj::opengl::perspective(self.fov_radians(), aspect, near, far)
    }
}
