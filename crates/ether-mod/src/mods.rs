//! The mod realm: loading, events, commands, and the edits they ask for.

use core::ffi::c_int;
use std::path::Path;
use std::time::Duration;

use ether_voxel::glam::IVec3;
use ether_voxel::vox::{iso_u8, Shape, Vox};
use libquickjs_sys as q;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::qjs::{Exception, JSContext, JSValue, MemoryUsage, Realm};
use crate::registry::{Brush, Changes, Registry};

const PRELUDE: &str = include_str!("prelude.js");

/// The base game, compiled into the binary so a console with no mod files still has it.
pub const BASE: &str = include_str!("../../../mods/base/main.js");

/// One mod: its name (a directory under `mods/`) and its `main.js`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub name: String,
    pub text: String,
}

impl Source {
    pub fn new(name: &str, text: &str) -> Source {
        Source { name: name.into(), text: text.into() }
    }
}

/// The base mod.
pub fn base() -> Source {
    Source::new("base", BASE)
}

/// The mods in a directory: `<dir>/<name>/main.js` for each line of `<dir>/index.txt` when that file is there
/// (a USB share cannot list a directory), otherwise for each subdirectory, sorted by name. A `base` directory
/// replaces the compiled-in base mod; the base mod always loads first.
pub fn discover(dir: &Path) -> Vec<Source> {
    let mut names: Vec<String> = match std::fs::read_to_string(dir.join("index.txt")) {
        Ok(index) => index.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(String::from).collect(),
        Err(_) => {
            let mut v: Vec<String> = std::fs::read_dir(dir).map(|it| it.filter_map(|e| e.ok()).filter(|e| e.path().join("main.js").is_file()).filter_map(|e| e.file_name().into_string().ok()).collect()).unwrap_or_default();
            v.sort();
            v
        }
    };
    names.retain(|n| valid_name(n));
    names.dedup();
    let mut out: Vec<Source> = names.iter().filter_map(|n| std::fs::read_to_string(dir.join(n).join("main.js")).ok().map(|t| Source::new(n, &t))).collect();
    if let Some(i) = out.iter().position(|s| s.name == "base") {
        let b = out.remove(i);
        out.insert(0, b);
    } else {
        out.insert(0, base());
    }
    out
}

fn valid_name(n: &str) -> bool {
    let b = n.as_bytes();
    !b.is_empty() && b[0].is_ascii_lowercase() && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-')
}

/// Limits of the realm.
#[derive(Clone, Debug)]
pub struct Config {
    /// Bytes QuickJS may allocate.
    pub memory_limit: usize,
    /// Native stack a script may use.
    pub stack: usize,
    /// Time each mod's top level may run while loading.
    pub load_budget: Duration,
    /// Time all handlers of one `break` or `place` may run together.
    pub event_budget: Duration,
    /// Time all `tick` handlers may run together.
    pub tick_budget: Duration,
    pub command_budget: Duration,
    /// `tick` handlers run at most this many times a second.
    pub tick_hz: f32,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            memory_limit: 16 << 20,
            stack: 256 << 10,
            load_budget: Duration::from_millis(1000),
            event_budget: Duration::from_millis(4),
            tick_budget: Duration::from_millis(2),
            command_budget: Duration::from_millis(50),
            tick_hz: 10.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum During {
    Load,
    Break,
    Place,
    Tick,
    Command,
}

impl During {
    fn parse(s: &str) -> During {
        match s {
            "break" => During::Break,
            "place" => During::Place,
            "tick" => During::Tick,
            "command" => During::Command,
            _ => During::Load,
        }
    }
}

/// A mod's error: where it is, what it says, when it happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModError {
    pub module: String,
    /// `<mod>/main.js` with line and column, when the stack names them.
    pub file: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub message: String,
    pub during: During,
    /// The budget stopped it; its handler (or command) was removed.
    pub interrupted: bool,
}

impl core::fmt::Display for ModError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match (&self.file, self.line, self.column) {
            (Some(file), Some(l), Some(c)) => write!(f, "{file}:{l}:{c}: {}", self.message),
            (Some(file), Some(l), None) => write!(f, "{file}:{l}: {}", self.message),
            _ => write!(f, "mod {}: {}", self.module, self.message),
        }
    }
}

impl ModError {
    fn plain(module: &str, during: During, message: String) -> ModError {
        ModError { module: module.into(), file: None, line: None, column: None, message, during, interrupted: false }
    }

    /// An error located from a stack that names `<module>/main.js:line:column`.
    fn located(module: &str, during: During, message: String, stack: &str, interrupted: bool) -> ModError {
        let file = format!("{module}/main.js");
        let mut e = ModError { module: module.into(), file: None, line: None, column: None, message, during, interrupted };
        if let Some(at) = stack.find(&format!("{file}:")) {
            let rest = &stack[at + file.len() + 1..];
            let num = |s: &str| -> (Option<u32>, usize) {
                let n = s.bytes().take_while(u8::is_ascii_digit).count();
                (s[..n].parse().ok(), n)
            };
            let (line, n) = num(rest);
            e.line = line;
            if rest[n..].starts_with(':') {
                e.column = num(&rest[n + 1..]).0;
            }
            e.file = Some(file);
        }
        e
    }
}

/// What a handler can read of the world: a voxel by its position, if its chunk is loaded.
pub trait WorldView {
    fn voxel(&self, p: IVec3) -> Option<Vox>;
}

/// No world (loading, the title screen).
pub struct NoWorld;

impl WorldView for NoWorld {
    fn voxel(&self, _: IVec3) -> Option<Vox> {
        None
    }
}

/// The player as handlers see it (`ether.player`).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PlayerState {
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub flying: bool,
    pub grounded: bool,
    /// The selected hotbar slot, 0..9.
    pub hotbar: usize,
}

/// A break or a place: the voxel the ray hit and the brush in hand.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockEvent {
    pub pos: IVec3,
    /// The voxel at `pos`, before the brush.
    pub voxel: Option<Vox>,
    pub brush: Brush,
}

/// A voxel change a handler asked for; absent fields stay as they are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edit {
    pub pos: IVec3,
    pub tex: Option<u8>,
    pub shape: Option<Shape>,
    pub iso: Option<f32>,
}

impl Edit {
    pub fn apply(&self, v: &mut Vox) {
        if let Some(t) = self.tex {
            v.tex = t;
        }
        if let Some(s) = self.shape {
            v.shape = s;
        }
        if let Some(i) = self.iso {
            v.iso = iso_u8(i);
        }
    }
}

/// Something a handler asked the game to do (`ether.game`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    /// Set the time of day, 0..1.
    Time(f32),
    Teleport([f32; 3]),
    Fly(bool),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLine {
    pub module: String,
    pub text: String,
}

/// What a call into the mods produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    /// A `break` or `place` handler returned `false`: the engine does not apply the brush.
    pub cancel: bool,
    /// Edits to apply now, in order.
    pub edits: Vec<Edit>,
    /// Game actions, in order, after the edits.
    pub actions: Vec<Action>,
    /// A command's answer, for the chat log.
    pub reply: Option<String>,
    pub errors: Vec<ModError>,
}

struct Live {
    realm: Realm,
    internal: JSValue,
}

impl Drop for Live {
    fn drop(&mut self) {
        self.realm.free(self.internal);
    }
}

/// The world a call may read, for `ether.world.get`. Set for the length of one call.
struct CallCtx {
    world: Option<*const (dyn WorldView + 'static)>,
}

unsafe extern "C" fn native_get(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *mut JSValue) -> JSValue {
    let call = Realm::opaque(ctx) as *const CallCtx;
    let mut p = [0i32; 3];
    if argc >= 3 {
        for (i, slot) in p.iter_mut().enumerate() {
            q::JS_ToInt32(ctx, slot, *argv.add(i));
        }
    }
    let v = if call.is_null() { None } else { (*call).world.and_then(|w| (*w).voxel(IVec3::new(p[0], p[1], p[2]))) };
    q::JS_NewInt32(ctx, v.map_or(-1, |v| v.tex as i32 | (v.shape as i32) << 8 | (v.iso as i32) << 16))
}

pub struct Mods {
    pub config: Config,
    live: Option<Live>,
    registry: Registry,
    logs: Vec<LogLine>,
    tick_acc: f32,
}

impl Mods {
    /// No mods loaded: the default registry (the reference's world, no materials).
    pub fn new(config: Config) -> Mods {
        Mods { config, live: None, registry: Registry::default(), logs: Vec::new(), tick_acc: 0.0 }
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn loaded(&self) -> bool {
        self.live.is_some()
    }

    /// Evaluates `sources` in a new realm, in order. On success the new realm and registry replace the old
    /// ones and the result says what changed (everything, on the first load). On failure nothing changes:
    /// the previous realm keeps running and every error is returned.
    pub fn load(&mut self, sources: &[Source]) -> Result<Changes, Vec<ModError>> {
        let (live, registry, logs) = self.build(sources)?;
        let changes = if self.live.is_some() { registry.diff(&self.registry) } else { Changes::all() };
        self.live = Some(live);
        self.registry = registry;
        self.logs.extend(logs);
        self.tick_acc = 0.0;
        Ok(changes)
    }

    /// [`Mods::load`], leaving out each mod that fails until the rest load. The base mod is never left out: if
    /// it fails, nothing changes. Returns what changed and the errors of the mods left out.
    pub fn load_lenient(&mut self, sources: &[Source]) -> (Changes, Vec<ModError>) {
        let mut set: Vec<Source> = sources.to_vec();
        let mut all = Vec::new();
        loop {
            match self.load(&set) {
                Ok(c) => return (c, all),
                Err(errs) => {
                    let bad: Vec<String> = errs.iter().map(|e| e.module.clone()).filter(|m| m != "base" && set.iter().any(|s| &s.name == m)).collect();
                    all.extend(errs);
                    if bad.is_empty() {
                        return (Changes::default(), all);
                    }
                    set.retain(|s| !bad.contains(&s.name));
                }
            }
        }
    }

    fn build(&self, sources: &[Source]) -> Result<(Live, Registry, Vec<LogLine>), Vec<ModError>> {
        let cfg = &self.config;
        let mut realm = Realm::new(cfg.memory_limit, cfg.stack);
        let g = realm.global();
        let native = unsafe { q::JS_NewObject(realm.ctx) };
        realm.set(native, "get", realm.function("get", native_get, 3));
        realm.set(g, "__native", native);
        realm.free(g);
        let internal = realm.eval("<ether>", PRELUDE, cfg.load_budget).map_err(|e| vec![ModError::plain("<ether>", During::Load, format!("{}\n{}", e.message, e.stack))])?;
        let mut live = Live { realm, internal };
        let mut errors = Vec::new();
        for s in sources {
            if !valid_name(&s.name) {
                errors.push(ModError::plain(&s.name, During::Load, "a mod's name is lower-case letters, digits, `_` and `-`".into()));
                continue;
            }
            if let Err(e) = Self::run_mod(&mut live, s, cfg.load_budget) {
                errors.push(e);
            }
        }
        // The prelude's lists keep what each mod registered; the registry is read once all ran.
        if !errors.is_empty() {
            return Err(errors);
        }
        let json = match Self::entry(&mut live, "registry", &[], cfg.load_budget) {
            Ok(Some(j)) => j,
            Ok(None) => return Err(vec![ModError::plain("<ether>", During::Load, "the registry did not come back".into())]),
            Err(e) => return Err(vec![Self::blame_text(&e.message, sources)]),
        };
        let registry = parse_registry(&json).map_err(|e| vec![e])?;
        let (_, _, logs, errors) = Self::drain(&mut live);
        if !errors.is_empty() {
            return Err(errors);
        }
        Ok((live, registry, logs))
    }

    /// A registry error says which mod set the value (`(set by mod "x")`): blame that mod.
    fn blame_text(message: &str, sources: &[Source]) -> ModError {
        let module = message.split("(set by mod \"").nth(1).and_then(|r| r.split('"').next()).filter(|m| sources.iter().any(|s| &s.name == m)).unwrap_or("<ether>");
        ModError::plain(module, During::Load, message.into())
    }

    fn run_mod(live: &mut Live, s: &Source, budget: Duration) -> Result<(), ModError> {
        // A mod is the body of a function given its `ether`: its names stay its own. The body starts on the
        // first line, so lines in errors are the file's.
        let wrapped = format!("(function (ether) {{{}\n}})", s.text);
        let file = format!("{}/main.js", s.name);
        let realm = &mut live.realm;
        let f = realm.eval(&file, &wrapped, budget).map_err(|e| ModError::located(&s.name, During::Load, e.message.clone(), &format!("{}\n{}", e.message, e.stack), e.interrupted))?;
        let facade_fn = realm.get(live.internal, "facade");
        let name = realm.new_string(&s.name);
        let facade = realm.call(facade_fn, &mut [name], budget);
        realm.free(name);
        realm.free(facade_fn);
        let facade = facade.map_err(|e| ModError::plain(&s.name, During::Load, e.message))?;
        let r = realm.call(f, &mut [facade], budget);
        realm.free(facade);
        realm.free(f);
        match r {
            Ok(v) => {
                realm.free(v);
                Ok(())
            }
            Err(e) => Err(ModError::located(&s.name, During::Load, if e.interrupted { format!("{} (its top level ran past the {} ms budget)", e.message, budget.as_millis()) } else { e.message.clone() }, &e.stack, e.interrupted)),
        }
    }

    /// Calls an entry point of the prelude with string arguments; returns its string result.
    fn entry(live: &mut Live, name: &str, args: &[&str], budget: Duration) -> Result<Option<String>, Exception> {
        let realm = &mut live.realm;
        let f = realm.get(live.internal, name);
        let mut argv: Vec<JSValue> = args.iter().map(|a| realm.new_string(a)).collect();
        let r = realm.call(f, &mut argv, budget);
        for a in argv {
            realm.free(a);
        }
        realm.free(f);
        let v = r?;
        let out = unsafe { q::JS_IsString(v) }.then(|| realm.string(v));
        realm.free(v);
        Ok(out)
    }

    /// Edits, log lines and the errors handlers threw since the last drain.
    fn drain(live: &mut Live) -> (Vec<Edit>, Vec<Action>, Vec<LogLine>, Vec<ModError>) {
        let Ok(Some(json)) = Self::entry(live, "drain", &[], Duration::from_millis(100)) else { return Default::default() };
        let v: Value = serde_json::from_str(&json).unwrap_or(Value::Null);
        let edits = v["edits"].as_array().into_iter().flatten().filter_map(parse_edit).collect();
        let actions = v["actions"].as_array().into_iter().flatten().filter_map(parse_action).collect();
        let logs = v["logs"].as_array().into_iter().flatten().map(|l| LogLine { module: l["mod"].as_str().unwrap_or("").into(), text: l["text"].as_str().unwrap_or("").into() }).collect();
        let errors = v["errors"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|e| {
                let module = e["mod"].as_str().unwrap_or("");
                ModError::located(module, During::parse(e["during"].as_str().unwrap_or("")), e["message"].as_str().unwrap_or("").into(), e["stack"].as_str().unwrap_or(""), false)
            })
            .collect();
        (edits, actions, logs, errors)
    }

    /// One call into the realm with the world readable; collects edits, logs and errors.
    fn call(&mut self, during: During, entry: &str, args: &[&str], world: &dyn WorldView, budget: Duration) -> (Option<Value>, Outcome) {
        let mut out = Outcome::default();
        let Some(live) = self.live.as_mut() else { return (None, out) };
        // Safety: the pointer lives in `ctx` for this call only and is cleared before `world` goes out of scope.
        let world: &'static dyn WorldView = unsafe { core::mem::transmute::<&dyn WorldView, &'static dyn WorldView>(world) };
        let mut ctx = CallCtx { world: Some(world as *const _) };
        unsafe { live.realm.set_opaque((&mut ctx as *mut CallCtx).cast()) };
        let r = Self::entry(live, entry, args, budget);
        unsafe { live.realm.set_opaque(core::ptr::null_mut()) };
        let mut result = None;
        match r {
            Ok(Some(json)) => result = serde_json::from_str::<Value>(&json).ok(),
            Ok(None) => {}
            Err(e) => {
                // A handler ran past the budget (or the realm ran out of memory): name it and remove it.
                let cur = Self::entry(live, "current", &[], Duration::from_millis(100)).ok().flatten().and_then(|j| serde_json::from_str::<Value>(&j).ok()).unwrap_or(Value::Null);
                let module = cur["mod"].as_str().unwrap_or("<ether>").to_string();
                let what = cur["name"].as_str().map(|n| format!("/{n}")).unwrap_or_else(|| cur["event"].as_str().unwrap_or("?").to_string());
                let message = if e.interrupted { format!("{what} ran past its {} ms budget and was removed", budget.as_millis()) } else { e.message.clone() };
                out.errors.push(ModError::located(&module, during, message, &e.stack, e.interrupted));
                let _ = Self::entry(live, "dropCurrent", &[], Duration::from_millis(100));
                if let Ok(Some(json)) = Self::entry(live, "registry", &[], Duration::from_millis(100)) {
                    if let Ok(r) = parse_registry(&json) {
                        self.registry.hooks = r.hooks;
                        self.registry.commands = r.commands;
                    }
                }
            }
        }
        let live = self.live.as_mut().unwrap();
        let (edits, actions, logs, errors) = Self::drain(live);
        out.edits = edits;
        out.actions = actions;
        out.errors.extend(errors);
        self.logs.extend(logs);
        if let Some(v) = &result {
            out.cancel = v["cancel"].as_bool().unwrap_or(false);
        }
        (result, out)
    }

    fn player_json(p: &PlayerState) -> String {
        serde_json::to_string(p).unwrap()
    }

    fn block(&mut self, event: &str, during: During, e: &BlockEvent, world: &dyn WorldView, player: &PlayerState) -> Outcome {
        let count = if during == During::Break { self.registry.hooks.break_ } else { self.registry.hooks.place };
        if count == 0 {
            return Outcome::default();
        }
        let v = e.voxel;
        let payload = serde_json::json!({
            "pos": [e.pos.x, e.pos.y, e.pos.z],
            "tex": v.map(|v| v.tex),
            "material": v.and_then(|v| self.registry.material(v.tex).map(|m| m.name.clone())),
            "brush": { "material": e.brush.material, "shape": e.brush.shape, "size": e.brush.size, "strength": e.brush.strength },
        })
        .to_string();
        let budget = self.config.event_budget;
        self.call(during, "dispatch", &[event, &payload, &Self::player_json(player)], world, budget).1
    }

    /// The player is breaking at `e.pos`. Fire before the brush applies: `cancel` keeps the world as it is.
    pub fn on_break(&mut self, e: &BlockEvent, world: &dyn WorldView, player: &PlayerState) -> Outcome {
        self.block("break", During::Break, e, world, player)
    }

    /// The player is placing at `e.pos`.
    pub fn on_place(&mut self, e: &BlockEvent, world: &dyn WorldView, player: &PlayerState) -> Outcome {
        self.block("place", During::Place, e, world, player)
    }

    /// Call every frame with its length; `tick` handlers run at most `config.tick_hz` times a second, with the
    /// seconds since their last run (at most four periods).
    pub fn tick(&mut self, dt: f32, world: &dyn WorldView, player: &PlayerState) -> Outcome {
        if self.registry.hooks.tick == 0 || self.live.is_none() {
            self.tick_acc = 0.0;
            return Outcome::default();
        }
        let period = 1.0 / self.config.tick_hz;
        self.tick_acc += dt;
        if self.tick_acc < period {
            return Outcome::default();
        }
        let seconds = self.tick_acc.min(period * 4.0);
        self.tick_acc = 0.0;
        let budget = self.config.tick_budget;
        self.call(During::Tick, "dispatch", &["tick", &seconds.to_string(), &Self::player_json(player)], world, budget).1
    }

    /// A chat line (`/name args…`, the slash optional). `reply` is the command's answer or why none ran.
    pub fn command(&mut self, line: &str, world: &dyn WorldView, player: &PlayerState) -> Outcome {
        let budget = self.config.command_budget;
        let (r, mut out) = self.call(During::Command, "command", &[line, &Self::player_json(player)], world, budget);
        if let Some(r) = r {
            out.reply = r["reply"].as_str().map(String::from);
        } else if out.errors.is_empty() && self.live.is_none() {
            out.reply = Some("no mods are loaded".into());
        }
        out
    }

    /// Log lines since the last call (`ether.log`).
    pub fn take_logs(&mut self) -> Vec<LogLine> {
        core::mem::take(&mut self.logs)
    }

    /// What the realm holds now.
    pub fn memory(&self) -> Option<MemoryUsage> {
        self.live.as_ref().map(|l| l.realm.memory())
    }

    /// Runs QuickJS's cycle collector. Call it where a pause of a few milliseconds is not seen.
    pub fn collect(&self) {
        if let Some(l) = &self.live {
            l.realm.collect();
        }
    }
}

fn parse_edit(e: &Value) -> Option<Edit> {
    let p = e["pos"].as_array()?;
    let c = |i: usize| p.get(i).and_then(Value::as_i64).map(|v| v as i32);
    Some(Edit {
        pos: IVec3::new(c(0)?, c(1)?, c(2)?),
        tex: e["tex"].as_u64().map(|t| t as u8),
        shape: e["shape"].as_u64().map(|s| Shape::from_u8(s as u8)),
        iso: e["iso"].as_f64().map(|i| i as f32),
    })
}

fn parse_action(a: &Value) -> Option<Action> {
    if let Some(t) = a["time"].as_f64() {
        return Some(Action::Time(t as f32));
    }
    if let Some(p) = a["teleport"].as_array() {
        let c = |i: usize| p.get(i).and_then(Value::as_f64).map(|v| v as f32);
        return Some(Action::Teleport([c(0)?, c(1)?, c(2)?]));
    }
    a["fly"].as_bool().map(Action::Fly)
}

/// The registry from the realm's JSON, table by table so an error names its table.
fn parse_registry(json: &str) -> Result<Registry, ModError> {
    let v: Value = serde_json::from_str(json).map_err(|e| ModError::plain("<ether>", During::Load, format!("registry: {e}")))?;
    let table = |name: &str| -> Result<Value, ModError> { Ok(v.get(name).cloned().unwrap_or(Value::Null)) };
    fn read<T: for<'de> Deserialize<'de>>(name: &str, v: Value) -> Result<T, ModError> {
        serde_json::from_value(v).map_err(|e| {
            let hint = if e.to_string().starts_with("missing field") { "; the base mod sets every setting: load it first" } else { "" };
            ModError::plain("<ether>", During::Load, format!("{name}: {e}{hint}"))
        })
    }
    Ok(Registry {
        mods: read("mods", table("mods")?)?,
        materials: read("materials", table("materials")?)?,
        foliage: read("foliage", table("foliage")?)?,
        items: read("items", table("items")?)?,
        worldgen: read("worldgen", table("worldgen")?)?,
        decor: read("decor", table("decor")?)?,
        controller: read("controller", table("controller")?)?,
        clock: read("clock", table("clock")?)?,
        sky: read("sky", table("sky")?)?,
        hotbar: read("hotbar", table("hotbar")?)?,
        commands: read("commands", table("commands")?)?,
        hooks: read("hooks", table("hooks")?)?,
    })
}
