//! The mod realm with real QuickJS: the base mod against the reference's values, errors, the budget, events,
//! commands and reload.

use std::time::{Duration, Instant};

use ether_mod::*;
use ether_voxel::gen::Params;
use ether_voxel::glam::IVec3;
use ether_voxel::populate::DecorParams;
use ether_voxel::vox::{tex, Shape, Vox};

fn loaded(extra: &[(&str, &str)]) -> Mods {
    let mut m = Mods::new(Config::default());
    let mut s = vec![base()];
    s.extend(extra.iter().map(|(n, t)| Source::new(n, t)));
    m.load(&s).unwrap_or_else(|e| panic!("{}", e.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n")));
    m
}

struct Stone;

impl WorldView for Stone {
    fn voxel(&self, p: IVec3) -> Option<Vox> {
        (p.y < 100).then(|| Vox::new(if p.y < 0 { tex::STONE } else { tex::NIL }, Shape::Isosurface, if p.y < 0 { 1.0 } else { -1.0 }))
    }
}

#[test]
fn base_mod_is_the_reference() {
    let m = loaded(&[]);
    let r = m.registry();
    assert_eq!(r.worldgen.params(), Params::default());
    assert_eq!(r.decor.params(), DecorParams::default());
    assert_eq!(r.controller, Controller::default());
    assert_eq!(r.clock, Clock::default());
    assert_eq!(r.sky, Sky::default());
    // The registry the engine starts with when no mod is loaded is the same world.
    let d = Registry::default();
    assert_eq!((&r.worldgen, &r.decor, &r.controller, &r.clock, &r.sky), (&d.worldgen, &d.decor, &d.controller, &d.clock, &d.sky));

    assert_eq!(r.materials.len(), 24);
    for (id, name) in [(1, "dirt"), (12, "grass"), (13, "log"), (19, "sand"), (22, "stone"), (23, "leaves"), (24, "water")] {
        let mat = r.material(id).unwrap();
        assert_eq!((mat.name.as_str(), mat.tile), (name, id - 1));
    }
    assert_eq!(r.material(24).unwrap().kind, MaterialKind::Liquid);
    assert_eq!(r.foliage.iter().map(|f| (f.id, f.name.as_str())).collect::<Vec<_>>(), [(13, "short_grass"), (14, "bush"), (15, "rose"), (16, "fern"), (23, "leaves")]);
    // Item ids as the reference numbers them: sorted names, from 1; icons in the same order.
    let items: Vec<_> = r.items.iter().map(|i| (i.id, i.name.as_str(), i.icon)).collect();
    assert_eq!(items[0], (1, "apple", 0));
    assert_eq!(items[9], (10, "stick", 9));
    assert_eq!(r.hotbar.len(), 9);
    assert_eq!(r.hotbar[0], Some(Slot::Brush { brush: Brush { material: 10, shape: 0, size: 4.0, strength: 0.8 } }));
    assert_eq!(r.hotbar[7], Some(Slot::Item { item: r.items.iter().find(|i| i.name == "pickaxe").unwrap().id, count: 1 }));
    assert_eq!(r.mods, ["base"]);
    let mem = m.memory().unwrap();
    println!("realm after the base mod: {} bytes in use, {} allocations, {} objects", mem.memory_used_size, mem.malloc_count, mem.obj_count);
}

#[test]
fn without_the_base_mod_the_settings_are_missing() {
    let mut m = Mods::new(Config::default());
    let e = m.load(&[Source::new("lonely", "ether.material({ id: 30, name: \"glass\" });")]).unwrap_err();
    assert!(e[0].message.contains("worldgen") && e[0].message.contains("load it first"), "{}", e[0]);
}

#[test]
fn a_broken_mod_is_located_and_the_previous_registry_stays() {
    let mut m = loaded(&[]);
    let before = m.registry().clone();

    // A rule broken at a call site.
    let bad = "// a comment\n\nether.material({ id: 300, name: \"too_far\" });\n";
    let e = m.load(&[base(), Source::new("bad", bad)]).unwrap_err();
    assert_eq!(e.len(), 1);
    assert_eq!((e[0].module.as_str(), e[0].file.as_deref(), e[0].line), ("bad", Some("bad/main.js"), Some(3)), "{}", e[0]);
    assert!(e[0].message.contains("material \"too_far\".id: an integer in 1..255"), "{}", e[0]);
    assert_eq!(m.registry(), &before);

    // A syntax error.
    let e = m.load(&[base(), Source::new("typo", "ether.log(1);\nether.log(;\n")]).unwrap_err();
    assert_eq!((e[0].module.as_str(), e[0].line), ("typo", Some(2)), "{}", e[0]);
    assert!(e[0].message.contains("SyntaxError"), "{}", e[0]);

    // A name that resolves to nothing, blamed on the mod that set it.
    let e = m.load(&[base(), Source::new("granite", "ether.worldgen({ stone: \"granite\" });")]).unwrap_err();
    assert_eq!(e[0].module, "granite", "{}", e[0]);
    assert!(e[0].message.contains("no material named \"granite\""), "{}", e[0]);
    assert_eq!(m.registry(), &before);

    // The lenient load leaves the broken mods out and keeps the good ones.
    let (changes, errors) = m.load_lenient(&[base(), Source::new("bad", bad), Source::new("good", "ether.sky({ fogVisibility: 600 });")]);
    assert_eq!(errors.len(), 1);
    assert!(changes.sky && !changes.worldgen);
    assert_eq!(m.registry().mods, ["base", "good"]);
}

#[test]
fn the_budget_stops_a_runaway_mod() {
    let mut m = Mods::new(Config { load_budget: Duration::from_millis(50), ..Default::default() });
    let t = Instant::now();
    let e = m.load(&[base(), Source::new("spin", "let i = 0;\nwhile (true) { i++; }\n")]).unwrap_err();
    assert!(e[0].interrupted && e[0].module == "spin", "{}", e[0]);
    assert!(t.elapsed() < Duration::from_millis(500));

    // A tick handler that never returns is stopped, named and removed; the others keep running.
    let mut m = loaded(&[("spin", "ether.on(\"tick\", () => { for (;;) {} });"), ("count", "let n = 0;\nether.on(\"tick\", () => { n++; ether.log(\"tick\", n); });")]);
    assert_eq!(m.registry().hooks.tick, 2);
    let p = PlayerState::default();
    let t = Instant::now();
    let out = m.tick(0.2, &Stone, &p);
    assert!(t.elapsed() < Duration::from_millis(100), "{:?}", t.elapsed());
    assert_eq!(out.errors.len(), 1);
    assert!(out.errors[0].interrupted && out.errors[0].module == "spin" && out.errors[0].during == During::Tick, "{}", out.errors[0]);
    assert_eq!(m.registry().hooks.tick, 1);
    let out = m.tick(0.2, &Stone, &p);
    assert!(out.errors.is_empty());
    let logs = m.take_logs();
    assert_eq!(logs.last().unwrap(), &LogLine { module: "count".into(), text: "tick 1".into() });
}

#[test]
fn ticks_are_throttled() {
    let mut m = loaded(&[("count", "let n = 0;\nether.on(\"tick\", (dt) => { n++; ether.log(n, dt.toFixed(3)); });")]);
    let p = PlayerState::default();
    // Sixty frames of one sixtieth: ten handler runs at 10 Hz.
    for _ in 0..60 {
        m.tick(1.0 / 60.0, &Stone, &p);
    }
    let logs = m.take_logs();
    assert!((9..=10).contains(&logs.len()), "{} runs", logs.len());
    assert!(logs[0].text.ends_with("0.100"), "{}", logs[0].text);
}

#[test]
fn events_read_the_world_and_queue_edits() {
    let src = r#"
ether.on("break", (e) => {
  const below = ether.world.get(e.pos[0], e.pos[1] - 1, e.pos[2]);
  ether.log("break", e.material, "on", below && below.material);
  ether.world.set(e.pos[0], e.pos[1] + 1, e.pos[2], { tex: "sand", shape: "cube" });
  if (e.material === "stone") return false;
});
ether.on("place", (e) => { ether.world.set(0, 0, 0, { iso: 0.5 }); });
"#;
    let mut m = loaded(&[("guard", src)]);
    let p = PlayerState { position: [1.0, 2.0, 3.0], ..Default::default() };
    let brush = Brush { material: 22, shape: 0, size: 4.0, strength: 0.8 };
    let e = BlockEvent { pos: IVec3::new(5, -3, 7), voxel: Stone.voxel(IVec3::new(5, -3, 7)), brush };
    let out = m.on_break(&e, &Stone, &p);
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    assert!(out.cancel);
    assert_eq!(out.edits, [Edit { pos: IVec3::new(5, -2, 7), tex: Some(19), shape: Some(Shape::Cube), iso: None }]);
    assert_eq!(m.take_logs()[0].text, "break stone on stone");

    let e = BlockEvent { pos: IVec3::new(5, 3, 7), voxel: Stone.voxel(IVec3::new(5, 3, 7)), brush };
    let out = m.on_break(&e, &Stone, &p);
    assert!(!out.cancel);
    let out = m.on_place(&e, &Stone, &p);
    let mut v = Vox::AIR;
    out.edits[0].apply(&mut v);
    assert_eq!(v.iso, ether_voxel::vox::iso_u8(0.5));

    // A handler that throws is reported with its line; the others still run.
    let mut m = loaded(&[("oops", "ether.on(\"break\", () => {\n  null.x;\n});"), ("guard", src)]);
    let out = m.on_break(&BlockEvent { pos: IVec3::new(0, -5, 0), voxel: None, brush }, &Stone, &p);
    assert_eq!(out.errors.len(), 1);
    assert_eq!((out.errors[0].module.as_str(), out.errors[0].line, out.errors[0].during), ("oops", Some(2), During::Break), "{}", out.errors[0]);
    assert_eq!(out.edits.len(), 1);
}

#[test]
fn commands_answer() {
    let mut m = loaded(&[]);
    let p = PlayerState { position: [1.25, 64.0, -3.0], flying: true, ..Default::default() };
    let out = m.command("/where", &NoWorld, &p);
    assert_eq!(out.reply.as_deref(), Some("x 1.3 y 64 z -3 (flying)"));
    let out = m.command("help", &NoWorld, &p);
    assert_eq!(out.reply.as_deref(), Some("commands: /help /materials /where"));
    let out = m.command("/nope 1 2", &NoWorld, &p);
    assert_eq!(out.reply.as_deref(), Some("no command /nope; /help lists them"));
    let mut m = loaded(&[("echo", "ether.command(\"echo\", (args) => args.join(\"|\"));")]);
    assert_eq!(m.command("/echo a \"b c\" d", &NoWorld, &p).reply.as_deref(), Some("a|b c|d"));
}

#[test]
fn reload_says_what_changed() {
    let mut m = loaded(&[]);
    // The same mods again: nothing.
    assert!(!m.load(&[base()]).unwrap().any());
    // A new seed regenerates.
    let c = m.load(&[base(), Source::new("seed", "ether.worldgen({ seed: 7 });")]).unwrap();
    assert!(c.worldgen && c.regenerate() && c.remesh());
    assert!(!(c.decor || c.materials || c.player || c.sky || c.hotbar || c.clock));
    assert_eq!(m.registry().worldgen.params().seed, 7);
    // Back to the base mod: the seed changes back; a new material changes materials and needs no remesh by its colour.
    let c = m.load(&[base(), Source::new("glass", "ether.material({ id: 30, name: \"glass\", tile: 3, color: [200, 220, 230] });")]).unwrap();
    assert!(c.worldgen && c.materials && c.remesh_materials);
    let c = m.load(&[base(), Source::new("glass", "ether.material({ id: 30, name: \"glass\", tile: 3, color: [10, 20, 30] });")]).unwrap();
    assert!(c.materials && !c.remesh_materials && !c.regenerate());
    // Faster player: the controller only.
    let c = m.load(&[base(), Source::new("glass", "ether.material({ id: 30, name: \"glass\", tile: 3, color: [10, 20, 30] });"), Source::new("fast", "ether.controller({ acceleration: 80 });")]).unwrap();
    assert_eq!(c, Changes { player: true, ..Default::default() });
    // A handler added: hooks only.
    let c = m.load(&[base(), Source::new("glass", "ether.material({ id: 30, name: \"glass\", tile: 3, color: [10, 20, 30] });"), Source::new("fast", "ether.controller({ acceleration: 80 }); ether.on(\"tick\", () => {});")]).unwrap();
    assert_eq!(c, Changes { hooks: true, ..Default::default() });
}

#[test]
fn the_example_mod_loads() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../mods/examples");
    let sources = discover(&dir);
    assert_eq!(sources[0].name, "base");
    assert!(sources.len() >= 2, "{:?}", sources.iter().map(|s| &s.name).collect::<Vec<_>>());
    let mut m = Mods::new(Config::default());
    let c = m.load(&sources).unwrap_or_else(|e| panic!("{}", e.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n")));
    assert!(c.any());
    let glow = m.registry().material_named("glowstone").expect("the example registers glowstone");
    assert_eq!(glow.light, 15);
    let p = PlayerState::default();
    let out = m.command("/time 0.5", &NoWorld, &p);
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    assert!(out.reply.unwrap().contains("0.5"));
    assert_eq!(out.actions, [Action::Time(0.5)]);
}
