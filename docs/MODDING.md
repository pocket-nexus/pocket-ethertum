# Modding Pocket Ethertum

A mod is a directory with a `main.js`. The game runs every mod in a QuickJS
realm of its own, separate from the interface's realm: a mod that throws or
runs too long is reported and removed, and the interface keeps drawing.

The base game is a mod: `mods/base/main.js` registers Ethertum's materials,
items, world generation, population, controller, clock, sky, hotbar and three
commands. It is compiled into the program, so a console with no mod files
runs it. `mods/ether.d.ts` types the API; `mods/examples/glow` is an example.

## What a mod is given

`main.js` is the body of a function that receives `ether`. Its top level runs
once per load and registers things; names it declares stay its own.

| Call | Registers | Merges |
| --- | --- | --- |
| `ether.material({ id, name, tile, color, kind, light, hotbar })` | A terrain or liquid material, id 1..255 | An id registered again is replaced |
| `ether.foliage({ id, name, tile, color })` | The card a `grass` or `leaves` voxel with this id shows | Same |
| `ether.item({ name, icon, stack })` | An item; ids are the names sorted, from 1, as in the reference | Same name replaced |
| `ether.worldgen({...})`, `ether.decor({...})` | Generation and population constants | A later mod changes the keys it names |
| `ether.controller({...})`, `ether.clock({...})`, `ether.sky({...})` | Player physics, the day, fog and sun | Same |
| `ether.hotbar([...])` | Nine slots of brush presets and items | The last call sets the whole bar |
| `ether.on("break" \| "place", fn)` | A handler called before the player's brush applies; `return false` cancels it | Handlers of every mod run, in load order |
| `ether.on("tick", fn)` | A handler called with the seconds since its last run | Same |
| `ether.command(name, fn)` | `/name args…` from the chat line; a returned string is the answer | A later mod replaces the name |

Inside handlers:

| Call | Does |
| --- | --- |
| `ether.world.get(x, y, z)` | `{ tex, material, shape, iso }`, or `null` where no chunk is loaded and while the mods load |
| `ether.world.set(x, y, z, { tex, shape, iso })` | Queues an edit; the engine applies it after the handler returns. Absent fields stay |
| `ether.game.time(t)`, `ether.game.teleport(x, y, z)`, `ether.game.fly(on)` | Queues an action; the engine acts after the edits |
| `ether.player` | Position, velocity, yaw, pitch, flying, grounded, hotbar slot, as of the call |
| `ether.log(...)` | A line in the log the interface shows |

A call that breaks a rule (an id out of range, an unknown setting, a material
name that resolves to nothing) throws a `TypeError` at the call site, so the
error names the mod's file, line and column. A name that is resolved when the
registry is read (`worldgen.stone`, a hotbar material) names the mod that set
it.

## The base mod sets everything

The engine's own defaults equal the reference's values, and so does the base
mod: `cargo test -p ether-mod` loads `mods/base/main.js` and compares its
world, controller, clock and sky with them. A load without the base mod fails
with `worldgen: missing field ...; the base mod sets every setting: load it
first`. Seven of the 24 terrain tiles have names in the reference (dirt,
grass, log, sand, stone, leaves, water); the base mod names the others after
what the tiles show and marks those names as guesses.

## Where mods are read from

| Build | Directory | Listed by |
| --- | --- | --- |
| Packaged (`.vpk`) | `ux0:data/pocket-ethertum/mods/<name>/main.js` | The directory listing, names sorted |
| Development (Pocket Devkit over USB) | `host0:ether/mods/<name>/main.js` on the USB share | `host0:ether/mods/index.txt`, one name per line: the share cannot list a directory |

A `base` directory replaces the compiled-in base mod. The base mod loads
first, then the others in order. A mod name is lower-case letters, digits,
`_` and `-`.

At start the game loads leniently: a mod that fails is left out, its error is
shown, and the rest load. The base mod is never left out; if it fails, the
compiled-in one is used.

## Reloading

A reload evaluates every mod in a new realm. If any mod fails, the new realm
is dropped and the running one stays, with its registry and handlers; the
errors are shown. If all load, the new realm replaces the old one and the
engine compares the two registries (`Registry::diff`):

| Changed | The engine |
| --- | --- |
| `worldgen` or `decor` | Generates and populates again every loaded chunk the player has not edited, and meshes them |
| A material's `kind` or `light` (`remesh_materials`) | Meshes every loaded chunk again |
| Other material or foliage fields | Binds the new tiles; the interface takes the new colours |
| `items`, `hotbar` | The interface redraws the bar |
| `controller` | The player's next step uses the new values |
| `clock` | The day's length changes; `start` applies to new worlds |
| `sky` | The renderer's fog and sun take the new values |
| Handlers or commands | Nothing: events read the new realm |

Which mod registered a value is not a change: moving a setting from one mod to
another changes nothing.

## Limits

| Limit | Value | What happens |
| --- | --- | --- |
| Memory of the realm | 16 MiB | An allocation past it throws `out of memory` in the mod |
| A mod's top level at load | 1 s | The load fails with an `interrupted` error at that mod |
| All `break` or `place` handlers of one action | 4 ms | The running handler is stopped; its mod's handlers for that event are removed |
| All `tick` handlers of one run | 2 ms | Same |
| One command | 50 ms | The command is stopped and removed |
| `tick` runs | 10 a second | Frames between runs add their seconds to the next run, up to four runs' worth |

QuickJS checks the time every 10 000 bytecode operations, so a handler runs
past its budget by at most that many operations before it is stopped. The
stop cannot be caught by a `try` in the mod. The base mod's realm holds about
160 KB.

## The engine's side

`crates/ether-mod` holds the realm (`qjs`), the registry (`registry`) and
`Mods`, which the game owns on its frame thread:

- `Mods::load(&sources)` / `load_lenient` at start and at a reload, then
  `registry()` for the tables and `Changes` for what to redo.
- `Mods::on_break` / `on_place` when the player's brush is about to apply;
  if `cancel` is set the brush does not apply. `edits` and `actions` apply in
  order either way.
- `Mods::tick(dt, …)` every frame; it crosses into the realm only when a tick
  handler is due.
- `Mods::command(line, …)` for a chat line; `reply` is the answer.
- `take_logs()` and each `Outcome`'s `errors` feed the interface's log.

The world a handler reads comes from a `WorldView` the engine passes to each
call; the realm keeps no pointer to it after the call returns.
