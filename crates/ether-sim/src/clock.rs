//! Time of day (`client_world::tick_world`, `WorldInfo`).
//!
//! A day is `day_length` seconds (1440 by default) and starts at 0.15. The sun
//! turns about the z axis by `daytime · 2π`, after a fixed turn of π/2.3 about
//! y; it shines along its local −z. Its strength is `max(sin(angle), 0)²`,
//! zero through the night.

use core::f32::consts::PI;

use glam::{Quat, Vec3};

#[derive(Clone, Debug, PartialEq)]
pub struct WorldClock {
    /// Fraction of the day, in `[0, 1)`.
    pub daytime: f32,
    /// Seconds a day lasts; 0 stops the clock.
    pub day_length: f32,
    /// Seconds spent in the world.
    pub time_inhabited: f32,
}

impl Default for WorldClock {
    fn default() -> WorldClock {
        WorldClock { daytime: 0.15, day_length: 60.0 * 24.0, time_inhabited: 0.0 }
    }
}

impl WorldClock {
    pub fn tick(&mut self, dt: f32) {
        self.time_inhabited += dt;
        if self.day_length != 0.0 {
            self.daytime += dt / self.day_length;
            self.daytime -= self.daytime.trunc();
        }
    }

    pub fn sun_angle(&self) -> f32 {
        self.daytime * PI * 2.0
    }

    /// The sun light's rotation, as the reference sets it on the light's transform.
    pub fn sun_rotation(&self) -> Quat {
        Quat::from_rotation_z(self.sun_angle()) * Quat::from_rotation_y(PI / 2.3)
    }

    /// The direction the sunlight travels (unit).
    pub fn sun_light_dir(&self) -> Vec3 {
        self.sun_rotation() * Vec3::NEG_Z
    }

    /// The direction toward the sun.
    pub fn sun_dir(&self) -> Vec3 {
        -self.sun_light_dir()
    }

    /// The sun's strength in `[0, 1]`: the reference's illuminance over its scale (`skylight_illuminance × 1000`).
    pub fn sun_strength(&self) -> f32 {
        self.sun_angle().sin().max(0.0).powf(2.0)
    }
}
