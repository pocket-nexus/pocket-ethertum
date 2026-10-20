//! The flow, the same on every device: loading, the title over the world,
//! play, and the menu over play. A device owns the pad's wiring, the world
//! and the player; it hands this its [`Pad`] each frame and gives the player
//! the [`Input`] that comes back.
//!
//! ## The pad in play
//!
//! The reference's verbs (`client/input.rs`, `character_controller.rs`) on a
//! PS Vita's buttons, in PocketJS's bits ([`pad`]):
//!
//! | Button | Verb | Reference |
//! | --- | --- | --- |
//! | Left stick | Move | `Move` (WASD) |
//! | Right stick | Look | `Look` (gamepad right stick) |
//! | ✕ | Jump; twice within 0.3 s flies or lands; held, climbs while flying | `Jump` (Space) |
//! | ○ | Sneak; held, descends while flying | `Sneak` (Shift) |
//! | △ | Sprint, held until the stick stops pushing forward | `Sprint` |
//! | R | Break: the brush takes from the world | `Attack` |
//! | L | Place: the brush adds the brush's material | `UseItem` |
//! | D-pad left, right | The hotbar's slot before, after | `Hotbar1`…`Hotbar8` |
//! | D-pad up, down | The brush's size up, down | the settings panel's brush slider |
//! | □ | The camera: first person, behind, far behind | Alt + wheel |
//! | SELECT | The statistics | F3 |
//! | START | The menu | Esc |
//!
//! While a list is up (the title, the menu) the pad is the interface's:
//! the d-pad moves the focus, ○ chooses, ✕ goes back, START closes the menu.

use crate::{channel, Brush, Command, Mode, Setting, State, HOTBAR};

/// PocketJS's button bits (the PSP's, which every host uses).
pub mod pad {
    pub const SELECT: u32 = 0x0001;
    pub const START: u32 = 0x0008;
    pub const UP: u32 = 0x0010;
    pub const RIGHT: u32 = 0x0020;
    pub const DOWN: u32 = 0x0040;
    pub const LEFT: u32 = 0x0080;
    pub const L: u32 = 0x0100;
    pub const R: u32 = 0x0200;
    pub const TRIANGLE: u32 = 0x1000;
    pub const CIRCLE: u32 = 0x2000;
    pub const CROSS: u32 = 0x4000;
    pub const SQUARE: u32 = 0x8000;
}

/// Sticks in -1…1 (right and forward positive; `ry` up positive) and [`pad`] bits.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Pad {
    pub buttons: u32,
    pub lx: f32,
    pub ly: f32,
    pub rx: f32,
    pub ry: f32,
}

/// What the player reads this frame: all zero unless the mode is play.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Input {
    /// Movement in the player's frame, -1…1: right, forward.
    pub move_x: f32,
    pub move_y: f32,
    /// Radians a second to turn: yaw to the right, pitch up.
    pub yaw: f32,
    pub pitch: f32,
    /// Held this frame, and pressed since the last one.
    pub jump: bool,
    pub jump_pressed: bool,
    pub sneak: bool,
    pub sprint_pressed: bool,
    pub attack: bool,
    pub use_item: bool,
}

/// Distances the camera stands behind the eye, by the □ button's steps.
pub const CAMERA: [f32; 3] = [0.0, 4.0, 10.0];
/// Chunks loaded around the player: the setting's choices.
pub const VIEW: [&str; 5] = ["2", "3", "4", "5", "6"];
pub const FOV: [(&str, f32); 3] = [("70°", 70.0), ("85°", 85.0), ("100°", 100.0)];
/// Radians a second the right stick turns the view at full travel. The reference turns 0.03 rad a
/// frame at full travel (`character_controller.rs`: 0.003 × 10), 1.8 rad/s at 60 frames a second.
pub const LOOK: [(&str, f32); 3] = [("Slow", 1.2), ("Normal", 1.8), ("Fast", 2.6)];
/// Seconds in a day: the reference's 24 minutes, a quarter of it, or the clock stopped.
pub const DAY: [(&str, f32); 3] = [("Stopped", 0.0), ("24 min", 1440.0), ("6 min", 360.0)];

/// The settings this flow keeps, by the index of their choice (a switch is 0 or 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub view: usize,
    pub fov: usize,
    pub look: usize,
    pub invert: bool,
    pub day: usize,
    /// Flying ends when the player lands (`unfly_on_ground`).
    pub unfly: bool,
    pub stats: bool,
}

impl Default for Settings {
    /// The reference's: load distance 4, 85°, a day of 24 minutes, landing ends a flight.
    fn default() -> Self {
        Settings { view: 2, fov: 1, look: 1, invert: false, day: 1, unfly: true, stats: false }
    }
}

pub struct Session {
    pub mode: Mode,
    /// The selected hotbar slot.
    pub slot: usize,
    pub brush: Brush,
    /// The step of [`CAMERA`].
    pub camera: usize,
    pub settings: Settings,
    /// Settings the device lists after this flow's own, with their values; a command for one of
    /// them comes back from [`Session::obey`].
    pub extra: Vec<Setting>,
    /// The interface has nothing scheduled: see [`Pace`].
    pub idle: bool,
    /// Frames between two refreshes of the numbers in play.
    pub numbers_every: u32,
    prev_buttons: u32,
    frames: u32,
    /// Seconds this flow has run, and what the interface asked to be woken for: an id and the second
    /// it is due.
    seconds: f32,
    wakes: Vec<(u32, f32)>,
    options: Vec<Setting>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

/// The sticks rest off centre by up to a fifth of their travel: nothing inside that counts.
pub const DEAD: f32 = 0.2;

fn dead(x: f32) -> f32 {
    if x.abs() < DEAD {
        0.0
    } else {
        (x - DEAD * x.signum()) / (1.0 - DEAD)
    }
}

impl Session {
    pub fn new() -> Session {
        Session {
            mode: Mode::Loading,
            slot: 0,
            brush: Brush::DEFAULT,
            camera: 0,
            settings: Settings::default(),
            extra: Vec::new(),
            idle: false,
            numbers_every: 2,
            prev_buttons: u32::MAX,
            frames: 0,
            seconds: 0.0,
            wakes: Vec::new(),
            options: Vec::new(),
        }
    }

    /// The world is ready: the title comes up.
    pub fn loaded(&mut self) {
        if self.mode == Mode::Loading {
            self.mode = Mode::Title;
        }
    }

    /// Chunks to load around the player, horizontally.
    pub fn view_distance(&self) -> u32 {
        self.settings.view as u32 + 2
    }

    pub fn fov(&self) -> f32 {
        FOV[self.settings.fov.min(FOV.len() - 1)].1
    }

    /// Seconds in a day; `None` while the clock is stopped.
    pub fn day_length(&self) -> Option<f32> {
        let s = DAY[self.settings.day.min(DAY.len() - 1)].1;
        (s > 0.0).then_some(s)
    }

    /// Metres the camera stands behind the eye.
    pub fn camera_distance(&self) -> f32 {
        CAMERA[self.camera % CAMERA.len()]
    }

    /// Does what the interface asked. A command that is the device's own comes back: `Start` and
    /// `Title` (after the mode has changed), a setting this flow does not keep, and `Prefs`.
    pub fn command(&mut self, command: Command) -> Option<Command> {
        match command {
            Command::Start => {
                if self.mode != Mode::Title {
                    return None;
                }
                self.mode = Mode::Play;
                return Some(Command::Start);
            }
            Command::Menu(on) => match (self.mode, on) {
                (Mode::Play, true) => self.mode = Mode::Menu,
                (Mode::Menu, false) => self.mode = Mode::Play,
                _ => {}
            },
            Command::Title => {
                if matches!(self.mode, Mode::Play | Mode::Menu) {
                    self.mode = Mode::Title;
                    return Some(Command::Title);
                }
            }
            Command::Option { key, value } => {
                let s = &mut self.settings;
                let pick = |n: usize| (value as usize).min(n - 1);
                match key.as_str() {
                    "view" => s.view = pick(VIEW.len()),
                    "fov" => s.fov = pick(FOV.len()),
                    "look" => s.look = pick(LOOK.len()),
                    "invert" => s.invert = value != 0,
                    "day" => s.day = pick(DAY.len()),
                    "unfly" => s.unfly = value != 0,
                    "stats" => s.stats = value != 0,
                    _ => return Some(Command::Option { key, value }),
                }
            }
            Command::Brush(brush) => self.brush = brush,
            Command::Slot(index) => self.slot = (index as usize).min(HOTBAR - 1),
            Command::Idle(on) => self.idle = on,
            Command::Wake { id, seconds } => {
                if self.wakes.len() < 16 {
                    self.wakes.push((id, self.seconds + seconds.max(0.0)));
                }
            }
            other @ Command::Prefs(_) => return Some(other),
        }
        None
    }

    /// Does everything the interface asked since the last frame. `device` is handed what
    /// [`Session::command`] gives back.
    pub fn obey(&mut self, mut device: impl FnMut(Command)) {
        while let Some(command) = unsafe { channel() }.next() {
            if let Some(back) = self.command(command) {
                device(back);
            }
        }
    }

    /// The settings as the interface lists them: this flow's, then the device's.
    pub fn options(&self, out: &mut Vec<Setting>) {
        let s = &self.settings;
        let names = |list: &[(&'static str, f32)]| list.iter().map(|c| c.0).collect::<Vec<_>>();
        out.push(Setting::choice("view", s.view, &VIEW));
        out.push(Setting::choice("fov", s.fov, &names(&FOV)));
        out.push(Setting::choice("look", s.look, &names(&LOOK)));
        out.push(Setting::switch("invert", s.invert));
        out.push(Setting::choice("day", s.day, &names(&DAY)));
        out.push(Setting::switch("unfly", s.unfly));
        out.push(Setting::switch("stats", s.stats));
        out.extend(self.extra.iter().cloned());
    }

    /// One frame of `dt` seconds: the pad as the player reads it, or nothing while the title or a
    /// list has the pad.
    pub fn run(&mut self, pad: &Pad, dt: f32) -> Input {
        let pressed = pad.buttons & !self.prev_buttons;
        self.prev_buttons = pad.buttons;
        self.frames = self.frames.wrapping_add(1);
        self.seconds += dt;
        // No interface on the screen: the world has it, and the pad keeps the player.
        if !unsafe { channel() }.is_open() && self.frames > 30 && matches!(self.mode, Mode::Title | Mode::Menu) {
            self.mode = Mode::Play;
        }
        if self.mode != Mode::Play {
            return Input::default();
        }
        if pressed & pad::LEFT != 0 {
            self.slot = (self.slot + HOTBAR - 1) % HOTBAR;
        }
        if pressed & pad::RIGHT != 0 {
            self.slot = (self.slot + 1) % HOTBAR;
        }
        if pressed & pad::UP != 0 {
            self.brush.size = (self.brush.size + 1).min(Brush::MAX_SIZE);
        }
        if pressed & pad::DOWN != 0 {
            self.brush.size = self.brush.size.saturating_sub(1);
        }
        if pressed & pad::SQUARE != 0 {
            self.camera = (self.camera + 1) % CAMERA.len();
        }
        if pressed & pad::SELECT != 0 {
            self.settings.stats = !self.settings.stats;
        }
        let rate = LOOK[self.settings.look.min(LOOK.len() - 1)].1;
        let pitch = dead(pad.ry) * rate;
        Input {
            move_x: dead(pad.lx),
            move_y: dead(pad.ly),
            yaw: dead(pad.rx) * rate,
            pitch: if self.settings.invert { -pitch } else { pitch },
            jump: pad.buttons & pad::CROSS != 0,
            jump_pressed: pressed & pad::CROSS != 0,
            sneak: pad.buttons & pad::CIRCLE != 0,
            sprint_pressed: pressed & pad::TRIANGLE != 0,
            attack: pressed & pad::R != 0,
            use_item: pressed & pad::L != 0,
        }
    }

    /// Writes what this flow decides in the interface's state: the mode, the slot, the brush, the
    /// camera, the settings and the wake that came due. The device writes the rest (the hotbar's
    /// contents, health, flying, the numbers when [`Session::numbers_due`] says so).
    pub fn publish(&mut self, state: &mut State) {
        state.mode = self.mode;
        state.slot = self.slot as u32;
        state.brush = self.brush;
        state.camera = self.camera as u32;
        self.options.clear();
        let mut options = core::mem::take(&mut self.options);
        self.options(&mut options);
        if state.options != options {
            state.options.clone_from(&options);
        }
        self.options = options;
        // One wake a frame: each is a line of its own, so the interface hears every id.
        if let Some(at) = self.wakes.iter().position(|w| w.1 <= self.seconds) {
            state.woke = self.wakes.swap_remove(at).0;
        }
    }

    /// The numbers in play are refreshed this frame.
    pub fn numbers_due(&self) -> bool {
        self.frames.is_multiple_of(self.numbers_every.max(1))
    }
}

/// PocketJS's START: the one button the interface listens to in play (it opens the menu).
const GUEST_MENU: u32 = pad::START;
/// Turns the guest still takes after the last reason for one: what a press or a line started
/// (a list sliding in, a mark moving) plays out.
const LINGER: u8 = 12;

/// Which of the turns a device offers the guest are worth their cost. A turn runs the whole
/// framework's frame, a millisecond on the Vita however little changed.
#[derive(Default)]
pub struct Pace {
    buttons: u32,
    linger: u8,
}

impl Pace {
    pub const fn new() -> Pace {
        Pace { buttons: 0, linger: 0 }
    }

    /// `buttons`: the guest's pad (PocketJS's bits) as it would be handed over this turn;
    /// `touching`: a finger is on a surface the guest draws.
    pub fn due(&mut self, session: &Session, buttons: u32, touching: bool) -> bool {
        let heard = if session.mode == Mode::Play { buttons & GUEST_MENU } else { buttons };
        let changed = heard != self.buttons;
        self.buttons = heard;
        let interface = unsafe { channel() };
        let news = interface.news();
        if !session.idle || changed || touching || (news && !interface.only_readouts()) {
            self.linger = LINGER;
            return true;
        }
        if self.linger > 0 {
            self.linger -= 1;
            return true;
        }
        news
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Holds, Numbers, Shape, Slot};

    fn press(session: &mut Session, buttons: u32) -> Input {
        let input = session.run(&Pad { buttons, ..Pad::default() }, 1.0 / 60.0);
        session.run(&Pad::default(), 1.0 / 60.0);
        input
    }

    // One test: the channel is one for the process.
    #[test]
    fn the_flow_follows_the_interface() {
        unsafe { channel() }.open("pocket.overlay");
        let dt = 1.0 / 60.0;
        let mut session = Session::new();
        assert_eq!(session.mode, Mode::Loading);
        // Nothing reaches the player before play.
        assert_eq!(session.run(&Pad { ly: 1.0, buttons: pad::CROSS, ..Pad::default() }, dt), Input::default());
        session.loaded();
        assert_eq!(session.mode, Mode::Title);
        assert_eq!(session.command(Command::Menu(true)), None);
        assert_eq!(session.mode, Mode::Title, "the title has no menu over it");
        assert_eq!(session.command(Command::Start), Some(Command::Start));
        assert_eq!(session.mode, Mode::Play);

        // In play the sticks and the buttons are the reference's verbs.
        session.run(&Pad::default(), dt);
        let input = session.run(&Pad { lx: 0.1, ly: 1.0, rx: -1.0, ry: 0.5, buttons: pad::CROSS | pad::CIRCLE }, dt);
        assert_eq!((input.move_x, input.move_y), (0.0, 1.0), "the stick's rest is dead");
        assert!((input.yaw + 1.8).abs() < 1e-5 && (input.pitch - 1.8 * 0.375).abs() < 1e-5, "{input:?}");
        assert!(input.jump && input.jump_pressed && input.sneak && !input.attack);
        let held = session.run(&Pad { buttons: pad::CROSS, ..Pad::default() }, dt);
        assert!(held.jump && !held.jump_pressed);
        assert!(press(&mut session, pad::R).attack);
        assert!(press(&mut session, pad::L).use_item);
        assert!(press(&mut session, pad::TRIANGLE).sprint_pressed);
        // The hotbar wraps; the brush's size stops at its ends.
        press(&mut session, pad::LEFT);
        assert_eq!(session.slot, HOTBAR - 1);
        press(&mut session, pad::RIGHT);
        press(&mut session, pad::RIGHT);
        assert_eq!(session.slot, 1);
        for _ in 0..30 {
            press(&mut session, pad::UP);
        }
        assert_eq!(session.brush.size, Brush::MAX_SIZE);
        press(&mut session, pad::DOWN);
        assert_eq!(session.brush.size, Brush::MAX_SIZE - 1);
        press(&mut session, pad::SQUARE);
        assert_eq!(session.camera_distance(), 4.0);
        press(&mut session, pad::SELECT);
        assert!(session.settings.stats);
        // Inverted look turns the pitch over.
        session.command(Command::Option { key: "invert".into(), value: 1 });
        assert!(session.run(&Pad { ry: 1.0, ..Pad::default() }, dt).pitch < 0.0);

        // Under a list the player reads nothing.
        session.command(Command::Menu(true));
        assert_eq!(session.mode, Mode::Menu);
        assert_eq!(session.run(&Pad { ly: 1.0, buttons: pad::R, ..Pad::default() }, dt), Input::default());
        session.command(Command::Brush(Brush { size: 7, strength: 0.25, shape: Shape::Cube, material: 22 }));
        assert_eq!(session.brush.material, 22);
        session.command(Command::Slot(40));
        assert_eq!(session.slot, HOTBAR - 1);

        // Settings: this flow's, and the device's own coming back.
        assert_eq!(session.command(Command::Option { key: "view".into(), value: 9 }), None);
        assert_eq!(session.view_distance(), 6);
        assert_eq!(session.command(Command::Option { key: "day".into(), value: 0 }), None);
        assert_eq!(session.day_length(), None);
        session.extra.push(Setting::switch("msaa", true));
        let glow = Command::Option { key: "msaa".into(), value: 0 };
        assert_eq!(session.command(glow.clone()), Some(glow));
        let mut state = State::default();
        session.publish(&mut state);
        assert_eq!(state.mode, Mode::Menu);
        assert_eq!(state.slot, 8);
        assert_eq!(state.brush.size, 7);
        assert_eq!(state.camera, 1);
        let keys: Vec<_> = state.options.iter().map(|o| (o.key, o.value)).collect();
        assert_eq!(keys, [("view", 4), ("fov", 1), ("look", 1), ("invert", 1), ("day", 0), ("unfly", 1), ("stats", 1), ("msaa", 1)]);
        session.command(Command::Menu(false));
        assert_eq!(session.command(Command::Title), Some(Command::Title));
        assert_eq!(session.mode, Mode::Title);

        // What the interface asked to be woken for comes due on this side's clock, one id a frame.
        session.command(Command::Wake { id: 7, seconds: 0.5 });
        session.command(Command::Wake { id: 8, seconds: 0.5 });
        for _ in 0..28 {
            session.run(&Pad::default(), dt);
            session.publish(&mut state);
        }
        assert_eq!(state.woke, 0);
        let mut heard = vec![];
        for _ in 0..6 {
            session.run(&Pad::default(), dt);
            session.publish(&mut state);
            heard.push(state.woke);
        }
        heard.retain(|&id| id != 0);
        heard.dedup();
        assert_eq!(heard, [7, 8]);

        // An idle guest is turned when there is a reason.
        session.command(Command::Start);
        let mut pace = Pace::default();
        assert!(pace.due(&session, 0, false));
        session.command(Command::Idle(true));
        state.hotbar[0] = Slot { holds: Holds::Material(22), count: 0 };
        session.publish(&mut state);
        *unsafe { &mut channel().state } = state.clone();
        unsafe { channel() }.poll();
        for _ in 0..LINGER {
            assert!(pace.due(&session, 0, false));
        }
        assert!(!pace.due(&session, 0, false));
        // In play the interface hears the menu button alone.
        assert!(!pace.due(&session, pad::CROSS | pad::R, false));
        assert!(pace.due(&session, GUEST_MENU, false));
        for _ in 0..LINGER {
            pace.due(&session, GUEST_MENU, false);
        }
        assert!(!pace.due(&session, GUEST_MENU, false));
        // New numbers are worth one turn; a new slot is worth the lingering turns; a finger every turn.
        unsafe { channel() }.state.t = Numbers { minutes: 300, x: 1, y: 2, z: 3 };
        assert!(pace.due(&session, GUEST_MENU, false));
        unsafe { channel() }.poll();
        assert!(!pace.due(&session, GUEST_MENU, false));
        unsafe { channel() }.state.slot = 3;
        assert!(pace.due(&session, GUEST_MENU, false));
        unsafe { channel() }.poll();
        for _ in 0..LINGER {
            pace.due(&session, GUEST_MENU, false);
        }
        assert!(!pace.due(&session, GUEST_MENU, false));
        assert!(pace.due(&session, GUEST_MENU, true));

        // With no interface on the screen the pad keeps the player.
        unsafe { channel() }.close();
        let mut session = Session::new();
        session.loaded();
        for _ in 0..40 {
            session.run(&Pad::default(), dt);
        }
        assert_eq!(session.mode, Mode::Play);
        assert!(session.run(&Pad { buttons: pad::CROSS, ..Pad::default() }, dt).jump_pressed);
    }
}
