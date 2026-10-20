// Pocket Ethertum on PS Vita: build the native runtime, replace the running development container's binary
// over PocketJS's wired debug transport (vendor/pocketjs), steer and measure.
//
//   bun tools/vita.ts build   [--title P3B1D7273] [--debug]
//   bun tools/vita.ts native                      # build + USB SELF replacement
//   bun tools/vita.ts serve                       # USB host (keep running, detached)
//   bun tools/vita.ts status | capture [--out f.png]
//   bun tools/vita.ts ctl '{"pos":[0,40,0]}'      # host0:ether/control.json
//   bun tools/vita.ts boot '{"title":false}'      # host0:ether/boot.json, read at start
//   bun tools/vita.ts bench [--seconds 60]        # frame timings → .pocket-build/validation/bench/<time>.json
//
// `--share DIR` uses an already-running USB host's root directory instead of this repository's
// `.pocket-build/vita-usb/share`. The default title is Pocket Devkit (PocketJS apps/devkit), the development
// container installed on the console: its native slots accept replacement SELFs. The Devkit on the development
// console answers as P3B1D7273 (installed before the pocket-nexus rename).

import { $ } from "bun";
import { createHash, randomBytes } from "node:crypto";
import { cpSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { packageVitaVpk } from "../vendor/pocketjs/tools/vita-package.ts";
import { prepareVitaUsb } from "../vendor/pocketjs/tools/vita-usb.ts";

const ROOT = resolve(import.meta.dir, "..");
const POCKETJS = resolve(ROOT, "vendor/pocketjs");
const APP_DIR = resolve(ROOT, "vita");
const OUT_DIR = resolve(ROOT, "dist/vita");
const home = process.env.HOME ?? "";
const vitasdk = process.env.VITASDK || `${home}/vitasdk`;
const rustup = Bun.which("rustup") ?? `${home}/.cargo/bin/rustup`;
const BIN = "pocket-ethertum-vita";
const DEVKIT = "P3B1D7273";

function value(argv: string[], flag: string, fallback: string): string {
  const i = argv.indexOf(flag);
  return i >= 0 && argv[i + 1] ? argv[i + 1]! : fallback;
}

export function context(argv: string[]) {
  const standalone = argv.includes("--standalone");
  const title = value(argv, "--title", standalone ? "PKET00001" : DEVKIT);
  const share = resolve(value(argv, "--share", process.env.ETHER_SHARE ?? resolve(ROOT, ".pocket-build/vita-usb/share")));
  return { title, share, appShare: resolve(share, "ether"), output: `pocket-ethertum-${title}`, release: !argv.includes("--debug"), standalone };
}

export async function build(argv: string[], assets?: string): Promise<string> {
  const c = context(argv);
  if (!existsSync(`${vitasdk}/bin/vita-pack-vpk`)) throw new Error(`VitaSDK not found at ${vitasdk}`);
  const usb = c.standalone ? undefined : await prepareVitaUsb();
  const nativeBuild = randomBytes(16).toString("hex");
  const env = {
    ...process.env,
    PATH: `${vitasdk}/bin:${home}/.cargo/bin:${process.env.PATH ?? ""}`,
    VITASDK: vitasdk,
    VITA_DEFAULT_TITLE_ID: c.title,
    POCKETJS_VITA_TITLE_ID: c.title,
    POCKETJS_NATIVE_BUILD: nativeBuild,
    POCKETJS_EMBED_APP: "0",
    TARGET_AR: "arm-vita-eabi-ar",
    AR_armv7_sony_vita_newlibeabihf: "arm-vita-eabi-ar",
    TARGET_CC: "arm-vita-eabi-gcc",
    CC_armv7_sony_vita_newlibeabihf: "arm-vita-eabi-gcc",
    TARGET_CXX: "arm-vita-eabi-g++",
    CXX_armv7_sony_vita_newlibeabihf: "arm-vita-eabi-g++",
  };
  const cargoArgs = [...(c.release ? ["--release"] : []), ...(c.standalone ? ["--no-default-features"] : [])];
  console.log(`ether: cargo vita build vpk (title ${c.title}, ${c.release ? "release" : "debug"})`);
  await $`${rustup} run nightly-2026-05-28 cargo vita build vpk -- ${cargoArgs}`.cwd(APP_DIR).env(env);

  const target = `${APP_DIR}/target/armv7-sony-vita-newlibeabihf/${c.release ? "release" : "debug"}`;
  const eboot = `${target}/${BIN}.self`;
  const sfo = `${target}/${BIN}.sfo`;
  const vpk = `${target}/${BIN}.vpk`;
  // Unsafe-homebrew SELF: loading the USB driver and writing the inactive native slot need the standard permissions.
  await $`${vitasdk}/bin/vita-make-fself ${target}/${BIN}.velf ${eboot}`;
  await $`${vitasdk}/bin/vita-mksfoex -d ATTRIBUTE2=12 -s TITLE_ID=${c.title} ${"Pocket Ethertum"} ${sfo}`;
  // The bubble's icon is Pocket3D's (skills/pocket3d-brand); the tree holds no icon of its own.
  const iconModule: string = `${POCKETJS}/tools/pocket3d-icon.ts`;
  const { POCKET3D_ICON } = (await import(iconModule)) as { POCKET3D_ICON: { vita: string } };
  await packageVitaVpk({ tool: `${vitasdk}/bin/vita-pack-vpk`, sfo, eboot, output: vpk, usbDriver: usb?.driver, applicationAssets: assets ?? (existsSync(`${APP_DIR}/assets`) ? `${APP_DIR}/assets` : undefined), icon: POCKET3D_ICON.vita });

  mkdirSync(OUT_DIR, { recursive: true });
  cpSync(vpk, `${OUT_DIR}/${c.output}.vpk`);
  cpSync(eboot, `${OUT_DIR}/${c.output}.self`);
  const selfSha256 = createHash("sha256").update(readFileSync(eboot)).digest("hex");
  const runtime = `${OUT_DIR}/${c.output}.runtime.json`;
  await Bun.write(runtime, JSON.stringify({ version: 1, titleId: c.title, applicationId: "dev.pocket-nexus.ethertum", output: c.output, nativeBuild, plan: null, self: `${c.output}.self`, usbDebug: !c.standalone, usbDriver: usb?.fingerprint ?? null, selfSha256 }, null, 2) + "\n");
  console.log(`ether: ${OUT_DIR}/${c.output}.self (native build ${nativeBuild})`);
  return runtime;
}

/** PocketJS's wired debug tool, pointed at the chosen USB share. */
export async function dev(argv: string[], ...args: string[]): Promise<void> {
  const c = context(argv);
  mkdirSync(c.share, { recursive: true });
  const runtime = `${OUT_DIR}/${c.output}.runtime.json`;
  const r = existsSync(runtime) ? ["--runtime", runtime] : [];
  await $`bun ${POCKETJS}/tools/vita-dev.ts ${args} ${r} --title ${c.title} --dir ${c.share}`.cwd(POCKETJS);
}

/** The share's folder for the app: every directory the device writes into exists up front (it never creates one on host0:). */
function prepareShare(argv: string[]) {
  const c = context(argv);
  for (const dir of ["", "gxp"]) mkdirSync(`${c.appShare}/${dir}`, { recursive: true });
}

export function ctl(argv: string[], json: string): void {
  const c = context(argv);
  mkdirSync(c.appShare, { recursive: true });
  JSON.parse(json);
  writeFileSync(`${c.appShare}/control.json`, json);
}

export function status(argv: string[]): any {
  const c = context(argv);
  // The device replaces the file every few frames; a read can land between the old one and the new one.
  for (let i = 0; ; i++) {
    try {
      return JSON.parse(readFileSync(`${c.share}/pocket-vita/${c.title}/status.json`, "utf8"));
    } catch (e) {
      if (i >= 20) throw e;
      Bun.sleepSync(25);
    }
  }
}

async function bench(argv: string[]) {
  const seconds = Number(value(argv, "--seconds", "60"));
  const first = status(argv).engine;
  await Bun.sleep(seconds * 1000);
  const last = status(argv).engine;
  const out = { seconds, frames: last.frames - first.frames, late: last.late - first.late, frameMs: last.frameMs, worstMs: last.worstMs, first, last };
  const dir = resolve(ROOT, ".pocket-build/validation/bench");
  mkdirSync(dir, { recursive: true });
  const file = `${dir}/${new Date().toISOString().replace(/[:.]/g, "-")}.json`;
  writeFileSync(file, JSON.stringify(out, null, 2));
  console.log(JSON.stringify({ seconds, frames: out.frames, late: out.late, frameMs: last.frameMs, worstMs: last.worstMs, scene: last.scene, world: last.world }, null, 2));
  console.log(file);
}

if (import.meta.main) {
  const [cmd, ...argv] = Bun.argv.slice(2);
  switch (cmd) {
    case "build":
      await build(argv);
      break;
    case "native":
      prepareShare(argv);
      await build(argv);
      await dev(argv, "native");
      break;
    case "push":
      await dev(argv, "native");
      break;
    case "serve":
      prepareShare(argv);
      await dev(argv, "serve");
      break;
    case "status":
      console.log(JSON.stringify(status(argv), null, 2));
      break;
    case "capture":
      await dev(argv, "capture", ...(argv.includes("--out") ? ["--out", value(argv, "--out", "")] : []));
      break;
    case "ctl":
      ctl(argv, argv.find((a) => a.startsWith("{")) ?? "{}");
      break;
    case "boot": {
      const c = context(argv);
      prepareShare(argv);
      const json = argv.find((a) => a.startsWith("{")) ?? "{}";
      JSON.parse(json);
      writeFileSync(`${c.appShare}/boot.json`, json);
      break;
    }
    case "bench":
      await bench(argv);
      break;
    default:
      console.log("usage: bun tools/vita.ts build|native|push|serve|status|capture|ctl|boot|bench");
  }
}
