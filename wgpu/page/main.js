// Pocket Ethertum in a browser tab: the wgpu renderer (../src, built to pkg/) and the game's own interface
// (../../ui, the bundle the PS Vita loads, compiled by tools/ui.ts), which a realm of the page runs on PocketJS's
// UI core and the renderer lays over the world. The page itself is the Pocket3D player of PocketJS's browser
// kernel (vendor/pocketjs/devices/web/pocket-web-wgpu, staged beside this file): the bar, the PS Vita's shell with
// its keys, the dock that leads to the game's files and to a remix of it on Pocket Studio. This file says what
// the game is and draws into the player's canvas.
//
// The Pocket3D title card plays first; the interface, the base mod's registry and the pack are read while it
// plays, and the world loads around the spawn while the interface counts its chunks.
//
//   ?pack=URL                   the pack: its file, or a manifest of its pieces and its name after `#`.
//                               Without it, the page's own (<meta name="pocket-pack">)
//   ?words=mode=play+fly=1      what a development host would send the game once the world stands (App::control)
//   ?interface=off              the world alone, played by the pad: no guest is started
//
// `window.pocketEthertum` is the running game, for a console and for tools/web.ts.
import { playTitle } from "./pocket3d-title.js";
import { frames, hasWebGPU, titleCard } from "./pocket3d-shell.js";
import { openInterface, screens } from "./pocket3d-interface.js";
import { createPlayer } from "./pocket3d-player.js";
import init, { Ethertum, shape } from "./pkg/ether_wgpu.js";

// The handheld the game runs on: its screen is the renderer's and its interface the bundle under ui/vita/, as
// the PS Vita's own build compiles it. (tools/web.test.ts holds this list, `runsOn` below, the page's
// `pocket-devices` and the release's targets to one another: a device with a package is a device here.)
const DEVICES = [{ id: "vita", label: "PS Vita", sticks: 2 }];
// Where the interface's settings are kept between visits (a PS Vita keeps them in a file on its card).
const KEPT = "pocket-ethertum.interface";

const query = new URLSearchParams(location.search);
const beside = (name) => new URL(name, import.meta.url).href;
const message = (error) => String(error?.message ?? error);
const started = performance.now();

const player = createPlayer({
  title: "Pocket Ethertum",
  tagline: { en: "Dig, build and fly over floating islands.", ja: "浮かぶ島々で、掘って、建てて、飛ぶ。" },
  devices: DEVICES,
  device: "vita",
  // (the targets a package is built for: the PS Vita's .vpk, `bun tools/vita.ts vpk`)
  runsOn: ["vita"],
  pick: () => {},
});
const { canvas, stage, controls } = player;
const say = (text) => player.say(text);

function kept() {
  try {
    return localStorage.getItem(KEPT) ?? "";
  } catch {
    return "";
  }
}

async function start() {
  // The card is the first picture of every launch, and covers the page while the world is read.
  const title = titleCard(playTitle);
  if (!hasWebGPU()) {
    await title;
    player.sayNoWebGPU();
    return;
  }
  await init();
  const screen = JSON.parse(shape());
  stage.show({ device: "vita", width: screen.width, height: screen.height });
  const registry = await (await fetch(beside("registry.json"))).text();
  const ether = await Ethertum.open(canvas, registry, kept());
  // (the flow waits at the title for a guest: one is on its way)
  ether.interface_opened();

  // What a frame's parts cost, in milliseconds summed since the start: the guest's turns, and the redraws of its
  // picture (the UI core's drawing, `drawMs` of it, then the upload).
  const timing = { turns: 0, turnMs: 0, redraws: 0, redrawMs: 0, drawMs: 0 };
  // (milliseconds from the page's start: the pack read, the interface up, the first frame, the first of the world)
  const report = { ether, ready: 0, interfaceReady: 0, firstFrame: 0, firstWorld: 0, frames: 0, failure: "", timing };
  window.pocketEthertum = report;

  // The pack, read beside everything else.
  const pack = query.get("pack") ?? document.querySelector('meta[name="pocket-pack"]').content;
  ether.reader().read(new URL(pack, location.href).href).then((read) => {
    ether.enter(read);
    report.ready = performance.now() - started;
  }).catch((error) => {
    report.failure = message(error);
    ether.fail(report.failure);
  });

  // The device on the page: its screen, its controls, and its presentation of the interface in a realm.
  let ui = null;
  const plan = await (await fetch(beside("ui/vita/plan.json"))).json();
  const of = screens(plan);
  player.show("vita", { width: screen.width, height: screen.height, lower: of.auxiliary, sticks: 2, glyphs: of.glyphs, touch: of.touch === "none" ? null : of.touch, viewport: of.viewport });
  if (query.get("interface") === "off") {
    ether.interface_closed();
  } else {
    try {
      // (`simHz`: the turns a second the PS Vita's host gives its interface)
      ui = await openInterface({ realm: beside("app-instance.html"), wasm: beside("pocketjs.wasm"), bundle: beside("ui/vita/ether.js"), pak: beside("ui/vita/ether.pak"), plan, simHz: screen.turns });
      ether.interface_opened();
      report.interfaceReady = performance.now() - started;
    } catch (error) {
      // Without its interface the game is played by the pad alone.
      report.failure = message(error);
      ether.interface_closed();
    }
  }

  // One frame: the player and the world, the guest's turn when it is worth one, its picture when that has
  // changed, the world drawn.
  const frame = (now) => {
    const held = controls.read();
    ether.step(now, held.buttons, held.left[0], held.left[1], held.right[0], held.right[1]);
    const ticks = ui !== null ? ether.guest_due(held.touching) : 0;
    const turned = ticks > 0;
    if (turned) {
      const from = performance.now();
      const line = ether.heard();
      if (line) ui.send(line);
      ui.turn(ether.guest_buttons(), held.contacts, ticks);
      for (const said of ui.drain()) ether.say(said);
      const turnedAt = performance.now();
      if (ui.changed()) {
        const picture = ui.picture();
        const drew = performance.now();
        ether.overlay(picture.pixels, picture.width, picture.height);
        timing.redraws++;
        timing.drawMs += drew - turnedAt;
        timing.redrawMs += performance.now() - turnedAt;
      }
      timing.turns++;
      timing.turnMs += turnedAt - from;
    }
    controls.next(turned);
    const settings = ether.prefs();
    if (settings) {
      try {
        localStorage.setItem(KEPT, settings);
      } catch {
        // A browser that keeps nothing starts from the defaults next time.
      }
    }
    ether.draw();
    report.frames++;
    report.firstFrame ||= performance.now() - started;
    if (!report.firstWorld && ether.entered()) {
      report.firstWorld = performance.now() - started;
      if (query.get("words")) ether.control(query.get("words"));
      // (the world is on the screen: the player may read what it kept back)
      player.ready();
    }
  };

  await title;
  canvas.hidden = false;
  stage.fit();
  const loop = frames(() => 60, (now) => {
    try {
      frame(now);
    } catch (error) {
      // A frame the canvas had no texture for is skipped; anything else stops the game and says why.
      report.failure = message(error);
      if (!/Outdated|Lost|Timeout/.test(report.failure)) {
        loop.stop();
        say(report.failure);
      }
    }
  });

  report.interface = () => ui;
  report.status = () => JSON.parse(ether.status());
  // The frame as the canvas holds it, one pixel of the world to a pixel: a PNG as a data URL.
  report.capture = () => {
    frame(performance.now());
    return canvas.toDataURL("image/png");
  };
  // A held game for a picture or a clip (tools/listing.ts): the display's loop stops, `words` go to the game,
  // `count` frames of a sixtieth of a second are made one after another, and the screen comes back as the last of
  // them left it.
  let clock = performance.now();
  report.hold = () => {
    loop.stop();
    clock = performance.now();
  };
  report.take = (words, count) => {
    loop.stop();
    if (words) ether.control(words);
    for (let i = 0; i < count; i++) frame((clock += 1000 / 60));
    return canvas.toDataURL("image/png");
  };
  // Milliseconds a frame costs the processor and the GPU together, over `count` frames made without waiting for
  // the display.
  report.burst = async (count = 300) => {
    const gpu = canvas.getContext("webgpu").getConfiguration().device;
    const from = performance.now();
    for (let i = 0; i < count; i++) frame(from + ((i + 1) * 1000) / 60);
    await gpu.queue.onSubmittedWorkDone();
    return (performance.now() - from) / count;
  };
}

start().catch((error) => {
  window.pocketEthertum = { failure: message(error) };
  say(window.pocketEthertum.failure);
});
