// Drives one device's bundle through the title, play and the menu against
// the mock renderer and checks what it asked for.
//   bun ui/test/flow.ts vita     (after `bun tools/ui.ts vita`)
import assert from "node:assert/strict";
import { BTN } from "../../vendor/pocketjs/contracts/spec/spec.ts";
import type { Command } from "../app/protocol.ts";
import { boot, type Device } from "./harness.ts";

const device = (process.argv[2] ?? "vita") as Device;
const rig = await boot(device);
const mock = rig.mock;
/** The commands since the last call, without the pacing ones. */
let read = 0;
const asked = (): Command[] => {
  const all = mock.log.slice(read).filter((command) => !["idle", "wake"].includes(command.type));
  read = mock.log.length;
  return all;
};
/** Chooses row `index` of the list that is up with the d-pad. */
const choose = (index: number) => {
  for (let i = 0; i < index; i++) rig.press(BTN.DOWN);
  rig.press(BTN.CIRCLE);
  rig.step(4);
};

// What was stored comes back: the settings are told again.
mock.state.prefs = JSON.stringify({ options: { invert: 1 } });
rig.step(10);
assert.deepEqual(asked(), [{ type: "option", key: "invert", value: 1 }]);
assert.equal(mock.state.options.find((o) => o.key === "invert")!.value, 1);

// Settings from the title: a press steps a choice on, the d-pad's right and left step it.
choose(1);
choose(0);
assert.equal(mock.state.options[0].value, 3);
rig.press(BTN.RIGHT);
rig.step(2);
rig.press(BTN.LEFT);
rig.press(BTN.LEFT);
rig.step(2);
const stored = (view: number) => ({ type: "prefs", value: JSON.stringify({ options: { invert: 1, view } }) });
assert.deepEqual(asked(), [
  { type: "option", key: "view", value: 3 }, stored(3),
  { type: "option", key: "view", value: 4 }, stored(4),
  { type: "option", key: "view", value: 3 }, stored(3),
  { type: "option", key: "view", value: 2 }, stored(2),
]);
rig.press(BTN.CROSS);
rig.step(6);

// Play.
choose(0);
assert.deepEqual(asked(), [{ type: "start" }]);
assert.equal(mock.state.mode, "play");
// The hint fades in, stands with nothing scheduled (the renderer counts its seconds), and fades out.
assert.equal(mock.idle, false);
rig.step(60);
assert.equal(mock.idle, true);
rig.step(360);
assert.notEqual(mock.state.woke, 0);
rig.step(60);
assert.equal(mock.idle, true);
// In play the pad is the player's: the d-pad and the face buttons ask the interface for nothing.
rig.press(BTN.DOWN);
rig.press(BTN.CIRCLE);
rig.press(BTN.CROSS);
rig.step(4);
assert.deepEqual(asked(), []);
// A finger on the hotbar's fifth slot chooses it; the slot's name is said for a moment.
rig.tap(79 + 4 * 36 + 17, 272 - 8 - 17);
rig.step(2);
assert.deepEqual(asked(), [{ type: "slot", index: 4 }]);
assert.equal(mock.state.slot, 4);
assert.equal(mock.idle, false);
rig.step(150);
assert.equal(mock.idle, true);

// The menu: the brush.
rig.press(BTN.START);
rig.step(8);
assert.deepEqual(asked(), [{ type: "menu", on: true }]);
assert.equal(mock.state.mode, "menu");
choose(1);
// Size: a press steps it up; the d-pad steps it either way.
rig.press(BTN.CIRCLE);
rig.step(2);
rig.press(BTN.LEFT);
rig.press(BTN.LEFT);
rig.step(2);
// Strength down a tenth, the shape to cubes, the material to the next one listed.
rig.press(BTN.DOWN);
rig.press(BTN.LEFT);
rig.press(BTN.DOWN);
rig.press(BTN.CIRCLE);
rig.press(BTN.DOWN);
rig.press(BTN.RIGHT);
rig.step(4);
const brush = (size: number, strength: number, shape: number, material: number) => ({ type: "brush", size, strength, shape, material });
assert.deepEqual(asked(), [brush(5, 80, 0, 10), brush(4, 80, 0, 10), brush(3, 80, 0, 10), brush(3, 70, 0, 10), brush(3, 70, 1, 10), brush(3, 70, 1, 22)]);
assert.deepEqual(mock.state.brush, { size: 3, strength: 70, shape: 1, material: 22 });
// Back to the menu, and START leaves it.
rig.press(BTN.CROSS);
rig.step(6);
rig.press(BTN.START);
rig.step(8);
assert.deepEqual(asked(), [{ type: "menu", on: false }]);
assert.equal(mock.state.mode, "play");
// The statistics are a setting: on, the renderer's line stands at the top left.
rig.press(BTN.START);
rig.step(8);
choose(2);
choose(6);
assert.ok(mock.state.stats.length > 0);
rig.press(BTN.CROSS);
rig.step(6);
// The title is one row away.
choose(4);
assert.deepEqual(asked(), [
  { type: "menu", on: true },
  { type: "option", key: "stats", value: 1 }, { type: "prefs", value: JSON.stringify({ options: { invert: 1, view: 2, stats: 1 } }) },
  { type: "title" },
]);
assert.equal(mock.state.mode, "title");
console.log(`${device}: the interface asked for what the flow expects`);
