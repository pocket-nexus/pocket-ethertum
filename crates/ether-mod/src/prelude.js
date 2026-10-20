// The realm's own script, evaluated before any mod. It builds the registry
// that mods write into, the `ether` object each mod receives, and the entry
// points the engine calls (`dispatch`, `command`, `registry`, `drain`). Its
// value is that internal object: the engine keeps it and no mod can reach it.
//
// A call that breaks a rule throws a TypeError at the call site, so the error
// names the mod's file and line.
(function () {
  "use strict";
  const native = globalThis.__native;
  delete globalThis.__native;

  const SHAPES = ["isosurface", "cube", "leaves", "grass", "slab_y_min", "slab_y_max", "slab_x_min", "slab_x_max", "slab_z_min", "slab_z_max", "fence"];
  const EVENTS = ["break", "place", "tick"];
  const NAME = /^[a-z][a-z0-9_]*$/;

  const reg = {
    mods: [],
    materials: new Map(), // id -> record
    foliage: new Map(),
    items: new Map(), // name -> record
    worldgen: {},
    decor: {},
    controller: {},
    clock: {},
    sky: {},
    hotbar: null,
    commands: new Map(), // name -> { mod, fn }
    hooks: { break: [], place: [], tick: [] },
  };
  // Who set each patched key, for errors found when the registry is resolved.
  const setters = {};

  let edits = [];
  let actions = [];
  let logs = [];
  let errors = [];
  let player = Object.freeze({ position: [0, 0, 0], flying: false });
  // The handler running now: what the engine blames when the budget stops a call.
  let current = null;

  function fail(what, message) {
    throw new TypeError(`${what}: ${message}`);
  }
  const check = {
    int(lo, hi) {
      return (v, what) => (Number.isInteger(v) && v >= lo && v <= hi ? v : fail(what, `an integer in ${lo}..${hi}, not ${JSON.stringify(v)}`));
    },
    num(lo = -Infinity, hi = Infinity) {
      return (v, what) => (typeof v === "number" && Number.isFinite(v) && v >= lo && v <= hi ? v : fail(what, `a number in ${lo}..${hi}, not ${JSON.stringify(v)}`));
    },
    bool() {
      return (v, what) => (typeof v === "boolean" ? v : fail(what, `true or false, not ${JSON.stringify(v)}`));
    },
    name() {
      return (v, what) => (typeof v === "string" && NAME.test(v) ? v : fail(what, `a lower-case name (a-z, 0-9, _), not ${JSON.stringify(v)}`));
    },
    // A material by name or id; resolved when the registry is read.
    material() {
      return (v, what) => (typeof v === "string" || (Number.isInteger(v) && v >= 1 && v <= 255) ? v : fail(what, `a material name or id 1..255, not ${JSON.stringify(v)}`));
    },
    rgb(scale) {
      return (v, what) => {
        if (!Array.isArray(v) || v.length !== 3) fail(what, `[r, g, b], not ${JSON.stringify(v)}`);
        v.forEach((c, i) => check.num(0, scale)(c, `${what}[${i}]`));
        return v.slice();
      };
    },
  };

  // The keys each patchable table takes, and the check for each.
  const TABLES = {
    worldgen: {
      seed: check.int(0, 4294967295),
      octaves: check.int(1, 32),
      terrainScale: check.num(1e-6),
      caveScale: check.num(1e-6),
      heightDiv: check.num(1e-6),
      caveAmp: check.num(),
      seaLevel: check.int(-2147483648, 2147483647),
      waterIso: check.num(-1, 1),
      stone: check.material(),
      water: check.material(),
    },
    decor: {
      seed: check.int(0, 4294967295),
      vinesPer256: check.num(0, 256),
      vineMax: check.num(0, 64),
      treesPer256: check.num(0, 256),
      bush: check.num(-1, 1),
      fern: check.num(-1, 1),
      rose: check.num(-1, 1),
      sandBelow: check.int(-2147483648, 2147483647),
      sandNoise: check.num(-1, 1),
    },
    controller: {
      jumpImpulse: check.num(0),
      acceleration: check.num(0),
      gravity: check.num(0),
      gravityScale: check.num(0),
      sprint: check.num(0),
      sneak: check.num(0),
      airControl: check.num(0),
      maxSlope: check.num(0, 90),
      groundDamping: check.num(0, 1),
      airDamping: check.num(0, 1),
      flyDamping: check.num(0, 1),
      lookSensitivity: check.num(0),
      stickLook: check.num(0),
      fov: check.num(1, 170),
      sprintFov: check.num(0, 90),
      doubleTap: check.num(0),
      jumpCooldown: check.num(0),
      unflyOnGround: check.bool(),
      radius: check.num(0.01),
      halfHeight: check.num(0),
      eyeHeight: check.num(),
    },
    clock: {
      start: check.num(0, 1),
      dayLength: check.num(0),
      tickRate: check.num(1, 240),
    },
    sky: {
      fogColor: check.rgb(1),
      fogVisibility: check.num(1),
      inscattering: check.rgb(1),
      extinction: check.rgb(1),
      atmospheric: check.bool(),
      sunIlluminance: check.num(0),
      shadows: check.bool(),
    },
  };

  function patcher(table, mod) {
    return (patch) => {
      if (patch === null || typeof patch !== "object" || Array.isArray(patch)) fail(table, "takes an object of settings");
      const keys = TABLES[table];
      for (const k of Object.keys(patch)) {
        if (!Object.prototype.hasOwnProperty.call(keys, k)) fail(`${table}.${k}`, `not a setting; the settings are ${Object.keys(keys).join(", ")}`);
        reg[table][k] = keys[k](patch[k], `${table}.${k}`);
        setters[`${table}.${k}`] = mod;
      }
    };
  }

  function text(args) {
    return args.map((a) => (typeof a === "string" ? a : JSON.stringify(a))).join(" ");
  }

  function materialId(ref, what) {
    if (typeof ref === "number") {
      if (!reg.materials.has(ref) && !reg.foliage.has(ref)) fail(what, `no material with id ${ref}`);
      return ref;
    }
    for (const m of reg.materials.values()) if (m.name === ref) return m.id;
    for (const m of reg.foliage.values()) if (m.name === ref) return m.id;
    fail(what, `no material named ${JSON.stringify(ref)}`);
  }

  function shapeIndex(ref, what) {
    if (Number.isInteger(ref) && ref >= 0 && ref < SHAPES.length) return ref;
    const i = SHAPES.indexOf(ref);
    return i >= 0 ? i : fail(what, `one of ${SHAPES.join(", ")}, not ${JSON.stringify(ref)}`);
  }

  function slot(s, i) {
    const what = `hotbar[${i}]`;
    if (s === null || s === undefined) return null;
    if (typeof s !== "object") fail(what, "a slot object or null");
    if (s.brush !== undefined) {
      const b = s.brush;
      return {
        brush: {
          material: check.material()(b.material, `${what}.brush.material`),
          shape: b.shape === undefined ? 0 : shapeIndex(b.shape, `${what}.brush.shape`),
          size: b.size === undefined ? 4 : check.num(0, 25)(b.size, `${what}.brush.size`),
          strength: b.strength === undefined ? 0.8 : check.num(0, 1)(b.strength, `${what}.brush.strength`),
        },
      };
    }
    if (s.item !== undefined) {
      return { item: check.name()(s.item, `${what}.item`), count: s.count === undefined ? 1 : check.int(1, 255)(s.count, `${what}.count`) };
    }
    fail(what, "a slot is { brush: { material, shape, size, strength } } or { item, count }");
  }

  function facade(mod) {
    const world = Object.freeze({
      get(x, y, z) {
        const v = native.get(x | 0, y | 0, z | 0);
        if (v < 0) return null;
        const tex = v & 255;
        const m = reg.materials.get(tex) || reg.foliage.get(tex);
        return { tex, material: m ? m.name : null, shape: SHAPES[(v >> 8) & 255] || "isosurface", iso: (((v >> 16) & 255) / 255) * 2 - 1 };
      },
      set(x, y, z, v) {
        if (v === null || typeof v !== "object") fail("world.set", "takes x, y, z and { tex, shape, iso }");
        const e = { pos: [x | 0, y | 0, z | 0] };
        if (v.tex !== undefined) e.tex = v.tex === 0 ? 0 : materialId(check.material()(v.tex, "world.set tex"), "world.set tex");
        if (v.shape !== undefined) e.shape = shapeIndex(v.shape, "world.set shape");
        if (v.iso !== undefined) e.iso = check.num(-1, 1)(v.iso, "world.set iso");
        edits.push(e);
      },
    });
    const game = Object.freeze({
      time(t) {
        actions.push({ time: check.num(0, 1)(t, "game.time") });
      },
      teleport(x, y, z) {
        actions.push({ teleport: [check.num()(x, "game.teleport x"), check.num()(y, "game.teleport y"), check.num()(z, "game.teleport z")] });
      },
      fly(on) {
        actions.push({ fly: check.bool()(on, "game.fly") });
      },
    });
    const api = {
      version: "0.1.0",
      mod,
      material(def) {
        const what = `material ${def && def.name !== undefined ? JSON.stringify(def.name) : ""}`.trim();
        if (def === null || typeof def !== "object") fail("material", "takes { id, name, tile, color, kind, light, hotbar }");
        const id = check.int(1, 255)(def.id, `${what}.id`);
        const name = check.name()(def.name, `${what}.name`);
        for (const m of reg.materials.values()) if (m.name === name && m.id !== id) fail(`${what}.name`, `already the name of material ${m.id}`);
        const kind = def.kind === undefined ? "terrain" : def.kind;
        if (kind !== "terrain" && kind !== "liquid") fail(`${what}.kind`, `"terrain" or "liquid", not ${JSON.stringify(kind)}`);
        reg.materials.set(id, {
          id,
          name,
          tile: def.tile === undefined ? id - 1 : check.int(0, 255)(def.tile, `${what}.tile`),
          color: def.color === undefined ? [128, 128, 128] : check.rgb(255)(def.color, `${what}.color`).map(Math.round),
          kind,
          light: def.light === undefined ? 0 : check.int(0, 15)(def.light, `${what}.light`),
          hotbar: def.hotbar === undefined ? kind === "terrain" : check.bool()(def.hotbar, `${what}.hotbar`),
          mod,
        });
      },
      foliage(def) {
        if (def === null || typeof def !== "object") fail("foliage", "takes { id, name, tile, color }");
        const what = `foliage ${JSON.stringify(def.name)}`;
        const id = check.int(1, 255)(def.id, `${what}.id`);
        reg.foliage.set(id, {
          id,
          name: check.name()(def.name, `${what}.name`),
          tile: def.tile === undefined ? id - 1 : check.int(0, 255)(def.tile, `${what}.tile`),
          color: def.color === undefined ? [128, 128, 128] : check.rgb(255)(def.color, `${what}.color`).map(Math.round),
          mod,
        });
      },
      item(def) {
        if (def === null || typeof def !== "object") fail("item", "takes { name, icon, stack }");
        const name = check.name()(def.name, "item.name");
        reg.items.set(name, {
          name,
          icon: check.int(0, 255)(def.icon, `item ${name}.icon`),
          stack: def.stack === undefined ? 64 : check.int(1, 255)(def.stack, `item ${name}.stack`),
          mod,
        });
      },
      worldgen: patcher("worldgen", mod),
      decor: patcher("decor", mod),
      controller: patcher("controller", mod),
      clock: patcher("clock", mod),
      sky: patcher("sky", mod),
      hotbar(slots) {
        if (!Array.isArray(slots) || slots.length > 9) fail("hotbar", "takes an array of at most 9 slots");
        reg.hotbar = { slots: slots.map(slot), mod };
      },
      on(event, fn) {
        if (!EVENTS.includes(event)) fail("on", `an event is one of ${EVENTS.join(", ")}, not ${JSON.stringify(event)}`);
        if (typeof fn !== "function") fail(`on(${JSON.stringify(event)})`, "takes a function");
        reg.hooks[event].push({ mod, fn });
      },
      command(name, fn) {
        check.name()(name, "command name");
        if (typeof fn !== "function") fail(`command ${name}`, "takes a function");
        reg.commands.set(name, { mod, fn });
      },
      commands() {
        return [...reg.commands.keys()].sort();
      },
      materials() {
        return [...reg.materials.values()].map((m) => ({ id: m.id, name: m.name }));
      },
      log(...args) {
        logs.push({ mod, text: text(args) });
      },
      world,
      game,
      get player() {
        return player;
      },
    };
    return Object.freeze(api);
  }

  function resolved(table, key, what) {
    const v = reg[table][key];
    try {
      return materialId(v, what);
    } catch (e) {
      throw new TypeError(`${e.message} (set by mod ${JSON.stringify(setters[`${table}.${key}`] || "?")})`);
    }
  }

  function run(mod, event, fn, args, name) {
    current = { mod, event, name: name || null };
    try {
      const r = fn(...args);
      current = null;
      return r;
    } catch (e) {
      current = null;
      errors.push({ mod, during: event, message: String(e), stack: (e && e.stack) || "" });
      return undefined;
    }
  }

  function words(line) {
    const out = [];
    const re = /"([^"]*)"|'([^']*)'|(\S+)/g;
    let m;
    while ((m = re.exec(line))) out.push(m[1] !== undefined ? m[1] : m[2] !== undefined ? m[2] : m[3]);
    return out;
  }

  return {
    facade(mod) {
      reg.mods.push(mod);
      return facade(mod);
    },
    // The registry as JSON, names resolved to ids.
    registry() {
      const out = {
        mods: reg.mods,
        materials: [...reg.materials.values()].sort((a, b) => a.id - b.id),
        foliage: [...reg.foliage.values()].sort((a, b) => a.id - b.id),
        // Ids as the reference numbers items: names sorted, from 1.
        items: [...reg.items.values()].sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0)).map((it, i) => Object.assign({ id: i + 1 }, it)),
        worldgen: Object.assign({}, reg.worldgen),
        decor: reg.decor,
        controller: reg.controller,
        clock: reg.clock,
        sky: reg.sky,
        hotbar: [],
        commands: [...reg.commands.keys()].sort(),
        hooks: { break: reg.hooks.break.length, place: reg.hooks.place.length, tick: reg.hooks.tick.length },
      };
      if (reg.worldgen.stone !== undefined) out.worldgen.stone = resolved("worldgen", "stone", "worldgen.stone");
      if (reg.worldgen.water !== undefined) out.worldgen.water = resolved("worldgen", "water", "worldgen.water");
      if (reg.hotbar) {
        out.hotbar = reg.hotbar.slots.map((s, i) => {
          if (s === null) return null;
          try {
            if (s.brush) return { brush: Object.assign({}, s.brush, { material: materialId(s.brush.material, `hotbar[${i}].brush.material`) }) };
            if (!reg.items.has(s.item)) fail(`hotbar[${i}].item`, `no item named ${JSON.stringify(s.item)}`);
            const id = out.items.find((it) => it.name === s.item).id;
            return { item: id, count: s.count };
          } catch (e) {
            throw new TypeError(`${e.message} (set by mod ${JSON.stringify(reg.hotbar.mod)})`);
          }
        });
      }
      return JSON.stringify(out);
    },
    // An event: every handler in load order. A break or place handler that returns false cancels the action.
    dispatch(event, payload, state) {
      player = Object.freeze(JSON.parse(state));
      const arg = JSON.parse(payload);
      let cancel = false;
      for (const h of reg.hooks[event]) {
        const e = event === "tick" ? arg : Object.freeze(Object.assign({}, arg));
        if (run(h.mod, event, h.fn, [e]) === false) cancel = true;
      }
      return JSON.stringify({ cancel });
    },
    command(line, state) {
      player = Object.freeze(JSON.parse(state));
      const w = words(line.replace(/^\//, ""));
      if (w.length === 0) return JSON.stringify({ found: false, reply: "" });
      const c = reg.commands.get(w[0]);
      if (!c) return JSON.stringify({ found: false, reply: `no command /${w[0]}; /help lists them` });
      const r = run(c.mod, "command", c.fn, [w.slice(1), line], w[0]);
      return JSON.stringify({ found: true, reply: r === undefined || r === null ? null : typeof r === "string" ? r : JSON.stringify(r) });
    },
    // Edits, log lines and caught errors since the last drain.
    drain() {
      const out = JSON.stringify({ edits, actions, logs, errors });
      edits = [];
      actions = [];
      logs = [];
      errors = [];
      return out;
    },
    current() {
      return JSON.stringify(current);
    },
    // Removes the handler the budget stopped, so it does not stop every later frame.
    dropCurrent() {
      if (!current) return;
      const { mod, event } = current;
      if (event === "command") {
        reg.commands.delete(current.name);
      } else {
        reg.hooks[event] = reg.hooks[event].filter((h) => h.mod !== mod);
      }
      current = null;
    },
  };
})()
