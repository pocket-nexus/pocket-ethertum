//! The renderer's side of `ui/app/protocol.ts`.
//!
//! The interface is one PocketJS guest (`ui/`) drawn over the world. It owns
//! every 2D pixel that is not anchored to a point of the world (the title,
//! the crosshair, the hotbar, the health bar, the menus) and what a button
//! or a finger means while a list is up. The renderer keeps a [`State`] and
//! the guest is sent the members that changed, as one JSON line a turn; what
//! the guest asks for comes back as [`Command`]s. The lines travel over the
//! guest's service channel, PocketJS's `pocket.overlay`, answered in the
//! process (`guest` for the QuickJS API) instead of on a wire.
//!
//! [`session::Session`] is the flow (loading, the title, play, the menu over
//! play) and what the pad means in each mode, written once for every device.
//! The pad mapping is in [`session`]'s documentation.

use std::collections::VecDeque;
use std::fmt::Write;

#[cfg(feature = "guest")]
pub mod guest;
pub mod session;

pub use session::{pad, Input, Pace, Pad, Session};

/// What the screen is for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// The world is being prepared; `State::message` names the step and `State::progress` how far it is.
    #[default]
    Loading,
    /// `State::message` says what failed.
    Error,
    /// The title over the world.
    Title,
    /// The world has the screen and the pad is the player's.
    Play,
    /// A list is up over the world; the pad is the interface's.
    Menu,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Loading => "loading",
            Mode::Error => "error",
            Mode::Title => "title",
            Mode::Play => "play",
            Mode::Menu => "menu",
        }
    }

    pub fn parse(name: &str) -> Option<Mode> {
        [Mode::Loading, Mode::Error, Mode::Title, Mode::Play, Mode::Menu].into_iter().find(|m| m.name() == name)
    }
}

/// Something the person can set: a switch (0 or 1), or with `choices` one of
/// several named values.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Setting {
    pub key: &'static str,
    pub value: u32,
    pub choices: Vec<&'static str>,
}

impl Setting {
    pub fn switch(key: &'static str, on: bool) -> Self {
        Self { key, value: on as u32, choices: Vec::new() }
    }

    pub fn choice(key: &'static str, value: usize, choices: &[&'static str]) -> Self {
        Self { key, value: value as u32, choices: choices.to_vec() }
    }
}

/// A material or an item as the interface shows it: its id, its name, and
/// the picture it is drawn with (a tile of the diffuse atlas for a material,
/// an icon of `items.png` for an item; `tools/ui.ts` compiles both).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub id: u32,
    pub name: String,
    pub picture: u32,
}

/// What a hotbar slot holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Holds {
    #[default]
    Nothing,
    /// A material the brush places, by its id.
    Material(u32),
    /// An item, by its id.
    Item(u32),
}

/// A hotbar slot: what it holds and how many (0 draws no count).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Slot {
    pub holds: Holds,
    pub count: u32,
}

/// Slots in the hotbar (`ClientPlayerInfo::HOTBAR_SLOTS`).
pub const HOTBAR: usize = 9;

/// The voxel brush (`VoxelBrush`): its radius in voxels, its strength, its shape and the material it places.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Brush {
    pub size: u32,
    /// 0…1.
    pub strength: f32,
    pub shape: Shape,
    pub material: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shape {
    /// The brush adds to and takes from the isosurface.
    #[default]
    Isosurface,
    /// The brush places cubes.
    Cube,
}

impl Brush {
    /// The reference's default brush: size 4, strength 0.8, the isosurface, material 10.
    pub const DEFAULT: Brush = Brush { size: 4, strength: 0.8, shape: Shape::Isosurface, material: 10 };
    pub const MAX_SIZE: u32 = 20;
}

impl Default for Brush {
    fn default() -> Self {
        Brush::DEFAULT
    }
}

/// The numbers that change while the player moves, sent at most once a turn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Numbers {
    /// The world's clock, minutes since midnight.
    pub minutes: i32,
    /// The player's position, tenths of a voxel.
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// What the interface is shown.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub mode: Mode,
    /// The loading step, or why `mode` is `Error`.
    pub message: String,
    /// How far loading is, 0…100.
    pub progress: u32,
    /// The materials and items there are, for the hotbar's and the brush's pictures and names.
    pub materials: Vec<Entry>,
    pub items: Vec<Entry>,
    pub hotbar: [Slot; HOTBAR],
    /// The selected slot.
    pub slot: u32,
    pub health: u32,
    pub health_max: u32,
    pub flying: bool,
    pub sprinting: bool,
    pub brush: Brush,
    /// The camera's distance behind the eye: 0 is first person.
    pub camera: u32,
    /// Mods the world was set up with.
    pub mods: u32,
    /// What the device lets the person set, in menu order.
    pub options: Vec<Setting>,
    /// One line of renderer statistics while the `stats` setting is on.
    pub stats: String,
    /// What the interface last stored with `prefs`.
    pub prefs: String,
    /// The `wake` the interface asked for that last came due; 0 before any has.
    pub woke: u32,
    pub t: Numbers,
}

fn escape(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c < ' ' => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn entries(out: &mut String, list: &[Entry]) {
    out.push('[');
    for (i, e) in list.iter().enumerate() {
        let _ = write!(out, "{}{{\"id\":{},\"name\":", if i > 0 { "," } else { "" }, e.id);
        escape(out, &e.name);
        let _ = write!(out, ",\"picture\":{}}}", e.picture);
    }
    out.push(']');
}

impl State {
    /// The line the guest receives: every member, or with `since` the members that differ from it.
    /// `None` when nothing does.
    pub fn line(&self, since: Option<&State>) -> Option<String> {
        let mut s = String::with_capacity(256);
        s.push_str("{\"type\":\"state\",\"value\":{");
        let start = s.len();
        macro_rules! member {
            ($field:ident, $key:literal, $write:expr) => {
                if since.map_or(true, |old| old.$field != self.$field) {
                    if s.len() > start {
                        s.push(',');
                    }
                    s.push_str(concat!("\"", $key, "\":"));
                    #[allow(clippy::redundant_closure_call)]
                    ($write)(&mut s);
                }
            };
        }
        let flag = |s: &mut String, on: bool| s.push_str(if on { "true" } else { "false" });
        member!(mode, "mode", |s: &mut String| escape(s, self.mode.name()));
        member!(message, "message", |s: &mut String| escape(s, &self.message));
        member!(progress, "progress", |s: &mut String| { let _ = write!(s, "{}", self.progress); });
        member!(materials, "materials", |s: &mut String| entries(s, &self.materials));
        member!(items, "items", |s: &mut String| entries(s, &self.items));
        member!(hotbar, "hotbar", |s: &mut String| {
            s.push('[');
            for (i, slot) in self.hotbar.iter().enumerate() {
                let (kind, id) = match slot.holds {
                    Holds::Nothing => (0, 0),
                    Holds::Material(id) => (1, id),
                    Holds::Item(id) => (2, id),
                };
                let _ = write!(s, "{}{{\"kind\":{kind},\"id\":{id},\"count\":{}}}", if i > 0 { "," } else { "" }, slot.count);
            }
            s.push(']');
        });
        member!(slot, "slot", |s: &mut String| { let _ = write!(s, "{}", self.slot); });
        member!(health, "health", |s: &mut String| { let _ = write!(s, "{}", self.health); });
        member!(health_max, "healthMax", |s: &mut String| { let _ = write!(s, "{}", self.health_max); });
        member!(flying, "flying", |s: &mut String| flag(s, self.flying));
        member!(sprinting, "sprinting", |s: &mut String| flag(s, self.sprinting));
        member!(brush, "brush", |s: &mut String| {
            let b = &self.brush;
            let shape = match b.shape {
                Shape::Isosurface => 0,
                Shape::Cube => 1,
            };
            let _ = write!(s, "{{\"size\":{},\"strength\":{},\"shape\":{shape},\"material\":{}}}", b.size, (b.strength * 100.0 + 0.5) as u32, b.material);
        });
        member!(camera, "camera", |s: &mut String| { let _ = write!(s, "{}", self.camera); });
        member!(mods, "mods", |s: &mut String| { let _ = write!(s, "{}", self.mods); });
        member!(options, "options", |s: &mut String| {
            s.push('[');
            for (i, o) in self.options.iter().enumerate() {
                let _ = write!(s, "{}{{\"key\":\"{}\",\"value\":{}", if i > 0 { "," } else { "" }, o.key, o.value);
                if !o.choices.is_empty() {
                    s.push_str(",\"choices\":[");
                    for (j, c) in o.choices.iter().enumerate() {
                        if j > 0 {
                            s.push(',');
                        }
                        escape(s, c);
                    }
                    s.push(']');
                }
                s.push('}');
            }
            s.push(']');
        });
        member!(stats, "stats", |s: &mut String| escape(s, &self.stats));
        member!(prefs, "prefs", |s: &mut String| escape(s, &self.prefs));
        member!(woke, "woke", |s: &mut String| { let _ = write!(s, "{}", self.woke); });
        member!(t, "t", |s: &mut String| {
            let t = &self.t;
            let _ = write!(s, "[{},{},{},{}]", t.minutes, t.x, t.y, t.z);
        });
        if s.len() == start {
            return None;
        }
        s.push_str("}}\n");
        Some(s)
    }
}

/// What the interface asks of the renderer.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Leave the title for the world.
    Start,
    /// A list comes up over the world, or leaves it.
    Menu(bool),
    /// Back to the title.
    Title,
    Option { key: String, value: u32 },
    /// The brush as the menu set it.
    Brush(Brush),
    /// A hotbar slot was chosen by a finger.
    Slot(u32),
    /// To store, and to hand back in [`State::prefs`].
    Prefs(String),
    /// The interface has nothing scheduled (nothing fading): a turn is worth its cost only when
    /// the renderer has news for it or a button it listens to changes.
    Idle(bool),
    /// Say `id` back in [`State::woke`] in this many seconds. The interface's own timers count its
    /// turns, and an idle guest takes none: what must happen later (a hint leaving) is timed here.
    Wake { id: u32, seconds: f32 },
}

/// The value after `"key":` in a flat JSON object. Quoted text is skipped
/// when looking for the key, so a value cannot pose as one.
fn field<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let bytes = json.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'"' {
            i += 1;
            continue;
        }
        let begin = i + 1;
        let mut end = begin;
        while end < bytes.len() && bytes[end] != b'"' {
            end += if bytes[end] == b'\\' { 2 } else { 1 };
        }
        if end >= bytes.len() {
            return None;
        }
        let rest = json[end + 1..].trim_start();
        if rest.starts_with(':') && &json[begin..end] == key {
            return Some(rest[1..].trim_start());
        }
        i = end + 1;
    }
    None
}

fn number(json: &str, key: &str) -> f32 {
    let Some(value) = field(json, key) else { return 0.0 };
    let end = value.find(|c: char| !(c.is_ascii_digit() || matches!(c, '-' | '.' | 'e' | 'E' | '+'))).unwrap_or(value.len());
    value[..end].parse().unwrap_or(0.0)
}

fn text(json: &str, key: &str) -> Option<String> {
    let value = field(json, key)?.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = value.chars();
    loop {
        match chars.next()? {
            '"' => return Some(out),
            '\\' => out.push(chars.next()?),
            c => out.push(c),
        }
    }
}

fn flag(json: &str, key: &str) -> bool {
    field(json, key).is_some_and(|v| v.starts_with("true"))
}

impl Command {
    pub fn parse(line: &str) -> Option<Command> {
        let n = |key| number(line, key);
        let whole = |key| n(key).max(0.0) as u32;
        Some(match text(line, "type")?.as_str() {
            "start" => Command::Start,
            "menu" => Command::Menu(flag(line, "on")),
            "title" => Command::Title,
            "option" => Command::Option { key: text(line, "key")?, value: whole("value") },
            "brush" => Command::Brush(Brush {
                size: whole("size").min(Brush::MAX_SIZE),
                strength: (n("strength") / 100.0).clamp(0.0, 1.0),
                shape: if whole("shape") == 1 { Shape::Cube } else { Shape::Isosurface },
                material: whole("material"),
            }),
            "slot" => Command::Slot(whole("index")),
            "prefs" => Command::Prefs(text(line, "value")?),
            "idle" => Command::Idle(flag(line, "on")),
            "wake" => Command::Wake { id: whole("id"), seconds: n("seconds") },
            _ => return None,
        })
    }
}

/// The channel's two ends: the state last sent, and what the guest said.
#[derive(Default)]
pub struct Interface {
    pub state: State,
    sent: Option<State>,
    open: bool,
    inbox: VecDeque<String>,
}

impl Interface {
    /// The guest opened its service: only the overlay is here.
    pub fn open(&mut self, service: &str) -> bool {
        self.open = service == "pocket.overlay";
        // A fresh guest gets the whole state.
        self.sent = None;
        self.open
    }

    /// A guest holds the channel: the interface is on the screen.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The guest is gone (it failed, or is being replaced).
    pub fn close(&mut self) {
        self.open = false;
        self.inbox.clear();
    }

    /// The state line, when the guest has not seen this state.
    pub fn poll(&mut self) -> Option<String> {
        if !self.open {
            return None;
        }
        let line = self.state.line(self.sent.as_ref())?;
        self.sent = Some(self.state.clone());
        Some(line)
    }

    /// The guest has not seen the state as it now is.
    pub fn news(&self) -> bool {
        self.open && self.sent.as_ref() != Some(&self.state)
    }

    /// What the guest has not seen is at most readouts (the numbers in play, the statistics line):
    /// text swapped in place, with nothing set moving.
    pub fn only_readouts(&self) -> bool {
        let Some(sent) = &self.sent else { return false };
        let mut now = self.state.clone();
        now.stats.clone_from(&sent.stats);
        now.t = sent.t;
        now == *sent
    }

    /// A line from the guest.
    pub fn receive(&mut self, line: &str) {
        if self.inbox.len() < 32 {
            self.inbox.push_back(String::from(line));
        }
    }

    /// The next command the guest sent, oldest first.
    pub fn next(&mut self) -> Option<Command> {
        while let Some(line) = self.inbox.pop_front() {
            if let Some(command) = Command::parse(&line) {
                return Some(command);
            }
        }
        None
    }
}

static mut INTERFACE: Option<Interface> = None;

/// The one channel, on the thread that runs the guest and the world.
///
/// # Safety
/// Call only on that thread; do not keep the reference across a guest turn.
pub unsafe fn channel() -> &'static mut Interface {
    (*core::ptr::addr_of_mut!(INTERFACE)).get_or_insert_with(Interface::default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_from_the_interface() {
        // Lines as ui/test/harness.ts logs them.
        let parse = |line| Command::parse(line).unwrap();
        assert_eq!(parse(r#"{"type":"start"}"#), Command::Start);
        assert_eq!(parse(r#"{"type":"menu","on":true}"#), Command::Menu(true));
        assert_eq!(parse(r#"{"type":"title"}"#), Command::Title);
        assert_eq!(parse(r#"{"type":"option","key":"view","value":3}"#), Command::Option { key: "view".into(), value: 3 });
        assert_eq!(parse(r#"{"type":"brush","size":6,"strength":50,"shape":1,"material":22}"#), Command::Brush(Brush { size: 6, strength: 0.5, shape: Shape::Cube, material: 22 }));
        assert_eq!(parse(r#"{"type":"brush","size":99,"strength":140,"shape":0,"material":1}"#), Command::Brush(Brush { size: 20, strength: 1.0, shape: Shape::Isosurface, material: 1 }));
        assert_eq!(parse(r#"{"type":"slot","index":4}"#), Command::Slot(4));
        assert_eq!(parse(r#"{"type":"prefs","value":"{\"view\":3}"}"#), Command::Prefs(r#"{"view":3}"#.into()));
        assert_eq!(parse(r#"{"type":"idle","on":true}"#), Command::Idle(true));
        assert_eq!(parse(r#"{"type":"wake","id":4,"seconds":7}"#), Command::Wake { id: 4, seconds: 7.0 });
        assert_eq!(Command::parse(r#"{"type":"pocket.overlay.control","name":"x","node":3}"#), None);
        // A value that looks like a key is not one.
        assert_eq!(parse(r#"{"type":"prefs","value":"\"type\":\"title\""}"#), Command::Prefs(r#""type":"title""#.into()));
    }

    #[test]
    fn the_guest_is_sent_what_changed() {
        let mut interface = Interface::default();
        assert_eq!(interface.poll(), None);
        assert!(!interface.open("youtube"));
        assert!(interface.open("pocket.overlay"));
        interface.state.mode = Mode::Title;
        interface.state.materials = vec![Entry { id: 22, name: "Stone".into(), picture: 21 }];
        interface.state.hotbar[0] = Slot { holds: Holds::Material(22), count: 0 };
        interface.state.hotbar[1] = Slot { holds: Holds::Item(7), count: 3 };
        interface.state.options = vec![Setting::switch("stats", false), Setting::choice("view", 2, &["2", "3", "4"])];
        let first = interface.poll().unwrap();
        assert!(first.starts_with("{\"type\":\"state\",\"value\":{\"mode\":\"title\",\"message\":\"\",\"progress\":0,\"materials\":[{\"id\":22,\"name\":\"Stone\",\"picture\":21}],\"items\":[],"));
        assert!(first.contains("\"hotbar\":[{\"kind\":1,\"id\":22,\"count\":0},{\"kind\":2,\"id\":7,\"count\":3},{\"kind\":0,\"id\":0,\"count\":0},"));
        assert!(first.contains("\"brush\":{\"size\":4,\"strength\":80,\"shape\":0,\"material\":10}"));
        assert!(first.contains("\"options\":[{\"key\":\"stats\",\"value\":0},{\"key\":\"view\",\"value\":2,\"choices\":[\"2\",\"3\",\"4\"]}]"));
        assert!(first.ends_with("\"t\":[0,0,0,0]}}\n"));
        assert_eq!(interface.poll(), None);
        // A turn in play: only the numbers go out.
        interface.state.t = Numbers { minutes: 216, x: 125, y: -40, z: 3 };
        assert!(interface.news() && interface.only_readouts());
        assert_eq!(interface.poll().unwrap(), "{\"type\":\"state\",\"value\":{\"t\":[216,125,-40,3]}}\n");
        interface.state.mode = Mode::Play;
        interface.state.slot = 2;
        assert!(!interface.only_readouts());
        assert_eq!(interface.poll().unwrap(), "{\"type\":\"state\",\"value\":{\"mode\":\"play\",\"slot\":2}}\n");
        interface.receive(r#"{"type":"menu","on":true}"#);
        interface.receive("not a command");
        interface.receive(r#"{"type":"title"}"#);
        assert_eq!(interface.next(), Some(Command::Menu(true)));
        assert_eq!(interface.next(), Some(Command::Title));
        assert_eq!(interface.next(), None);
        assert!(!interface.news());
        // A guest that starts again is sent everything.
        assert!(interface.open("pocket.overlay"));
        assert!(interface.poll().unwrap().contains("\"slot\":2"));
    }
}
