//! The foliage's share, held where the GPU keeps up with the display.
//!
//! Leaf cards are cut out, so the GPU shades every layer of cards in front of the terrain, and a view from just
//! above a canopy has more layers than one refresh allows. A frame that misses its refresh while the frame thread
//! waited on the GPU is a strike; two strikes within two seconds lower the share a step, for the cards drawn and
//! the cards casting the sun's shadow. Four seconds without a strike raise it a step, and a lowering within two
//! seconds of a raise bars raising for fifteen seconds, so a view at the edge does not swing between two steps.
//! The cards are stored in a hashed order (`mesh::Builder`): any share thins every crown evenly.

/// The shares, from all the cards down.
const LEVELS: [f32; 6] = [1.0, 0.85, 0.7, 0.55, 0.42, 0.3];

pub struct Governor {
    level: usize,
    /// The frame of an unanswered strike.
    strike: Option<u64>,
    /// The frame since which no strike came.
    calm: u64,
    raised: Option<u64>,
    barred_until: u64,
    /// A share set by hand (`ctl {"density": d}`), which stops the governor.
    pub fixed: Option<f32>,
}

impl Governor {
    pub fn new() -> Governor {
        Governor { level: 0, strike: None, calm: 0, raised: None, barred_until: 0, fixed: None }
    }

    pub fn density(&self) -> f32 {
        self.fixed.unwrap_or(LEVELS[self.level])
    }

    pub fn level(&self) -> usize {
        self.level
    }

    /// One shown frame: whether it missed its refresh, and whether the frame thread waited on the GPU for it.
    pub fn frame(&mut self, frame: u64, late: bool, waited_on_gpu: bool) {
        if late && waited_on_gpu {
            self.calm = frame;
            if self.strike.is_some_and(|f| frame - f <= 120) {
                self.strike = None;
                if self.level + 1 < LEVELS.len() {
                    self.level += 1;
                    if self.raised.is_some_and(|f| frame - f <= 120) {
                        self.barred_until = frame + 900;
                    }
                }
            } else {
                self.strike = Some(frame);
            }
        } else if self.level > 0 && frame - self.calm >= 240 && frame >= self.barred_until {
            self.level -= 1;
            self.raised = Some(frame);
            self.calm = frame;
        }
    }
}
