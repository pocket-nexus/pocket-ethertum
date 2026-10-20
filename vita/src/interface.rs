//! The interface: one PocketJS guest (`ui/`) drawn over the world through
//! PocketJS's Vita host library. It owns every 2D pixel and what the buttons
//! mean while a list is up; this side owns the world and the player, shows the
//! guest their state and does what it asks (`crates/ether-interface`).

use std::time::Instant;

use ether_interface::{channel, guest, Pace, Session};
use libquickjs_sys::{JSContext, JSRuntime, JS_RunGC, JS_SetGCThreshold};
use pocketjs_vita::{input, Runtime};

// libquickjs-sys omits this one; the linked QuickJS provides it.
extern "C" {
    fn JS_GetRuntime(ctx: *mut JSContext) -> *mut JSRuntime;
}

/// The guest turns this often, and is told so before it mounts.
pub const TURN: f32 = 1.0 / 30.0;
const RATE: &[u8] = b"globalThis.__simHz=30;";
const SCRIPT: &str = "ether.js";
const PAK: &str = "ether.pak";

/// A pad nobody touches.
pub const NEUTRAL: input::Pad = input::Pad { buttons: 0, lx: 128, ly: 128, rx: 128, ry: 128 };

pub struct Ui {
    runtime: Option<Runtime>,
    /// Seconds the guest is owed.
    owed: f32,
    /// Buttons seen since the last turn: a press shorter than a turn still reaches the guest.
    latched: u32,
    pace: Pace,
    /// Why there is no interface, for the status receipt.
    pub error: String,
    pub turns: u32,
    pub turn_ms: f32,
    pub worst_ms: f32,
    pub collect_ms: f32,
    pub collections: u32,
}

impl Ui {
    /// Boots the guest from `ether.js` and `ether.pak` (the USB share in a development build, else the package,
    /// else the data folder).
    ///
    /// # Safety
    /// Render thread, after vita2d is up and outside any scene.
    pub unsafe fn boot() -> Self {
        let read = |name: &str| crate::paths::candidates(name).iter().find_map(|p| crate::hostfs::read(p, 16 << 20)).ok_or(format!("{name} is missing"));
        let boot = || -> Result<Runtime, String> {
            // The guest borrows the pak for as long as it lives.
            let pak: &'static [u8] = Box::leak(read(PAK)?.into_boxed_slice());
            let mut script = RATE.to_vec();
            script.extend(read(SCRIPT)?);
            script.push(0);
            let script = String::from_utf8(script).map_err(|e| e.to_string())?;
            let mut runtime = Runtime::new(pak)?;
            // The channel to this renderer takes the place of the host's wire.
            guest::mount(runtime.context(), runtime.global());
            runtime.eval(&script)?;
            // A collection walks the whole heap (about 35 ms on this console); one that starts inside a turn is two
            // late frames. Reference counts free what a turn leaves; cycles wait for `collect`.
            JS_SetGCThreshold(JS_GetRuntime(runtime.context()), usize::MAX as _);
            Ok(runtime)
        };
        let mut ui = Self { runtime: None, owed: TURN, latched: 0, pace: Pace::default(), error: String::new(), turns: 0, turn_ms: 0.0, worst_ms: 0.0, collect_ms: 0.0, collections: 0 };
        match boot() {
            Ok(runtime) => ui.runtime = Some(runtime),
            Err(error) => ui.fail(error),
        }
        ui
    }

    pub fn live(&self) -> bool {
        self.runtime.is_some()
    }

    /// Without a guest the channel is shut, and the session keeps the flow on the pad.
    unsafe fn fail(&mut self, error: String) {
        pocketjs_vita::vita_log(format_args!("ether: interface: {error}"));
        self.error = error;
        self.runtime = None;
        channel().close();
    }

    /// Collects the guest's garbage now. Called where the screen changes whole.
    ///
    /// # Safety
    /// Render thread, outside a turn.
    pub unsafe fn collect(&mut self) {
        let Some(runtime) = &self.runtime else { return };
        let start = Instant::now();
        JS_RunGC(JS_GetRuntime(runtime.context()));
        self.collect_ms = start.elapsed().as_secs_f32() * 1000.0;
        self.collections += 1;
    }

    /// The guest's turn when `dt` more seconds make one due. With a `session` the turn is taken only when it is
    /// worth its cost (`Pace`); without one (loading) always.
    ///
    /// # Safety
    /// Render thread, outside any scene.
    pub unsafe fn turn(&mut self, dt: f32, pad: &input::Pad, quiet: bool, session: Option<&Session>) {
        self.latched |= pad.buttons;
        self.owed = (self.owed + dt).min(2.0 * TURN);
        if self.owed < TURN {
            return;
        }
        self.owed -= TURN;
        let touches = if quiet { input::TouchSnapshot::EMPTY } else { input::read_touches() };
        if let Some(session) = session {
            if !self.pace.due(session, self.latched, !touches.packed().is_empty()) {
                self.latched = 0;
                return;
            }
        }
        let Some(runtime) = &mut self.runtime else { return };
        let start = Instant::now();
        let buttons = core::mem::take(&mut self.latched);
        if let Err(error) = runtime.frame_with_input(buttons as i32, pad.left_analog(), &touches) {
            self.fail(error);
            return;
        }
        // Two core ticks to a turn: the core counts sixtieths.
        runtime.tick();
        runtime.tick();
        let ms = start.elapsed().as_secs_f32() * 1000.0;
        self.turns += 1;
        if self.turns > 8 {
            self.turn_ms += (ms - self.turn_ms) * 0.1;
            self.worst_ms = self.worst_ms.max(ms);
        }
    }

    /// Draws the interface into the open vita2d scene, over what is there.
    ///
    /// # Safety
    /// Render thread, inside the display scene.
    pub unsafe fn draw(&mut self) {
        if let Some(runtime) = &mut self.runtime {
            runtime.render_over();
        }
    }
}
