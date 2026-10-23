#!/usr/bin/env bun
// Pocket Ethertum drawn with wgpu (wgpu/): in a browser tab over WebGPU, and on this machine, where frames go to
// files.
//
//   bun tools/web.ts build                 wasm32 + wasm-bindgen + the page + the interface as the PS Vita loads it
//                                          (tools/ui.ts) + the base mod's registry + the pack → .pocket-build/web/site
//   bun tools/web.ts serve [--port 8803]   the site, the pack answered by byte ranges
//   bun tools/web.ts dist [--piece 2]      the directory a static host serves → .pocket-build/web/dist: the page,
//                                          the module under its build's name, the pack in pieces with its manifest
//   bun tools/web.ts serve --dist          that directory as such a host serves it: whole files only
//   bun tools/web.ts shot [--out f.png] [--words "mode=play pos=…"] [--frames N]
//                                          one frame on this machine's GPU (Metal) → a PNG and the status
//   bun tools/web.ts check [--headed] [--seconds 5] [--dist]
//                                          the page in Chrome, driven by keys: from the title into play, a walk,
//                                          the menu, what a frame costs → .pocket-build/validation/web/
//
// The pack is the PS Vita's (`bun tools/cook.ts` writes .pocket-build/pack/ether.pack). The site and the captures
// stay under the ignored .pocket-build/.

import { $ } from "bun";
import { cpSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { gzipSync } from "node:zlib";
import { join, resolve } from "node:path";
import { POCKET3D_ICON } from "../vendor/pocketjs/tools/pocket3d-icon.ts";
import { CUT_SIZES, writePacks } from "../vendor/pocketjs/tools/pocket3d-packs.ts";
import { stagePocket3dWeb } from "../vendor/pocketjs/tools/pocket3d-web.ts";
import { compileInterface } from "./ui.ts";

export const ROOT = resolve(import.meta.dir, "..");
const CRATE = join(ROOT, "wgpu");
const BUILD = join(ROOT, ".pocket-build/web");
const SITE = join(BUILD, "site");
const DIST = join(BUILD, "dist");
export const PACK = join(ROOT, ".pocket-build/pack/ether.pack");
const REGISTRY = join(BUILD, "registry.json");
/** What Pocket Studio's site deployments take: a file of at most 32 MiB, 4 000 files, 1 GiB, and two top-level
 * names that are the host's own. */
const HOST = { file: 32 << 20, files: 4000, bytes: 1 << 30, reserved: ["play", "runtime"] };

const [command, ...rest] = import.meta.main ? process.argv.slice(2) : [];
const option = (flag: string, fallback = "") => {
  const at = rest.indexOf(flag);
  return at >= 0 && rest[at + 1] ? rest[at + 1]! : fallback;
};

function needPack() {
  if (!existsSync(PACK)) throw new Error(`${PACK} is missing: bun tools/cook.ts`);
}

/** The base mod's registry as JSON (crates/ether-mod's `registry` example): what the tab reads in place of running
 * `mods/base/main.js`, since it runs no QuickJS of the game's. */
async function registry(): Promise<string> {
  const json = (await $`cargo run -q -p ether-mod --example registry`.cwd(ROOT).text()).trim();
  JSON.parse(json);
  mkdirSync(BUILD, { recursive: true });
  writeFileSync(REGISTRY, json);
  return json;
}

export async function build() {
  needPack();
  // The crate and the command line tool write two halves of one interface: their versions must be the same.
  const lock = readFileSync(join(CRATE, "Cargo.lock"), "utf8").match(/name = "wasm-bindgen"\nversion = "([^"]+)"/)?.[1];
  const tool = (await $`wasm-bindgen --version`.text()).trim().split(" ")[1];
  if (lock !== tool) throw new Error(`wasm-bindgen ${tool} is installed and wgpu/Cargo.lock has ${lock}: cargo install wasm-bindgen-cli --version ${lock}`);
  await $`cargo build --release --lib --target wasm32-unknown-unknown`.cwd(CRATE);
  rmSync(SITE, { recursive: true, force: true });
  mkdirSync(join(SITE, "pkg"), { recursive: true });
  await $`wasm-bindgen --target web --no-typescript --out-dir ${join(SITE, "pkg")} ${join(CRATE, "target/wasm32-unknown-unknown/release/ether_wgpu.wasm")}`;
  cpSync(join(CRATE, "page/main.js"), join(SITE, "main.js"));
  cpSync(join(CRATE, "page/index.html"), join(SITE, "index.html"));
  // What the page loads from PocketJS's browser kernel, as PocketJS stages it: the page's modules, the Pocket3D
  // title card, the realm of the interface's guest with the UI core, the host helpers. Then the tab's icon.
  await stagePocket3dWeb(SITE);
  cpSync(POCKET3D_ICON.ios2x, join(SITE, "icon.png"));
  // The interface as the PS Vita's own build compiles it, with the plan PocketJS resolved.
  const built = await compileInterface("vita");
  mkdirSync(join(SITE, "ui", "vita"), { recursive: true });
  for (const file of ["ether.js", "ether.pak", "plan.json"]) cpSync(join(built.directory, file), join(SITE, "ui", "vita", file));
  writeFileSync(join(SITE, "registry.json"), await registry());
  cpSync(PACK, join(SITE, "ether.pack"));

  const sizes: Record<string, { bytes: number; gzip: number }> = {};
  for (const file of files(SITE).sort()) {
    const bytes = readFileSync(join(SITE, file));
    sizes[file] = { bytes: bytes.length, gzip: gzipSync(bytes, { level: 9 }).length };
  }
  writeFileSync(join(BUILD, "site.json"), JSON.stringify({ wasmBindgen: tool, sizes }, null, 1));
  return sizes;
}

const sha256 = (bytes: Uint8Array | string) => new Bun.CryptoHasher("sha256").update(bytes).digest("hex");

/** Every file under a directory, as paths from it. */
function files(directory: string, under = ""): string[] {
  return readdirSync(join(directory, under), { withFileTypes: true }).flatMap((entry) => (entry.isDirectory() ? files(directory, join(under, entry.name)) : [join(under, entry.name)]));
}

/**
 * The directory a static host serves, for a host that limits a file's size and keeps a file ten minutes in a
 * browser's cache. Only the page is asked for again at every visit, so everything it names has a name of its own
 * contents: the module and its scripts under `app/<build>/`, the pack's manifest by the pack's hash, a piece by its
 * own.
 */
export async function dist(pieceBytes: number) {
  await build();
  rmSync(DIST, { recursive: true, force: true });
  const app = files(SITE).filter((file) => !["index.html", "icon.png", "ether.pack"].includes(file)).sort().map((file) => [file, readFileSync(join(SITE, file))] as const);
  const id = sha256(Buffer.concat(app.flatMap(([file, bytes]) => [Buffer.from(file), bytes]))).slice(0, 12);
  for (const [file, bytes] of app) {
    mkdirSync(join(DIST, "app", id, file, ".."), { recursive: true });
    writeFileSync(join(DIST, "app", id, file), bytes);
  }
  // The pack in pieces named by their bytes, and the manifest that lists them (vendor/pocketjs/tools/pocket3d-packs.ts).
  const cut = writePacks(join(DIST, "pack"), [{ name: "ether", file: PACK }], { min: Math.max(64 << 10, pieceBytes >> 3), max: pieceBytes, bits: Math.max(12, Math.round(Math.log2(pieceBytes)) - 2) });
  const manifest = `pack/${cut.manifest}#ether`;
  let page = readFileSync(join(SITE, "index.html"), "utf8");
  for (const [from, to] of [[`<meta name="pocket-pack" content="ether.pack">`, `<meta name="pocket-pack" content="${manifest}">`], [`src="main.js"`, `src="app/${id}/main.js"`], [`href="pocket3d-stage.css"`, `href="app/${id}/pocket3d-stage.css"`], [`href="pocket3d-player.css"`, `href="app/${id}/pocket3d-player.css"`]] as const) {
    if (!page.includes(from)) throw new Error(`wgpu/page/index.html has no ${from}`);
    page = page.replace(from, to);
  }
  // The game in Pocket Studio, for a host that answers no /app.json: the project this checkout is registered as
  // (`pocket-studio register` wrote .pocket-studio.json). The game's host on Pocket Studio answers one, and the
  // player's dock takes its links from that.
  const link = join(ROOT, ".pocket-studio.json");
  const project = existsSync(link) ? (JSON.parse(readFileSync(link, "utf8")) as { kind?: string; server?: string; app?: string }) : null;
  const registered = project?.kind === "site" && /^[A-Za-z0-9_-]+$/.test(project.app ?? "") && /^https:\/\/[A-Za-z0-9.-]+$/.test(project.server ?? "") ? project : null;
  if (registered) page = page.replace(`<meta name="pocket-pack"`, `<meta name="pocket-app" content="${registered.app}">\n<meta name="pocket-studio" content="${registered.server}">\n<meta name="pocket-pack"`);
  writeFileSync(join(DIST, "index.html"), page);
  cpSync(join(SITE, "icon.png"), join(DIST, "icon.png"));

  // What was written is what the host takes.
  const all = files(DIST).map((file) => ({ file, bytes: statSync(join(DIST, file)).size }));
  const total = all.reduce((sum, f) => sum + f.bytes, 0);
  const refused = [
    ...all.filter((f) => f.bytes > HOST.file).map((f) => `${f.file} is ${f.bytes} bytes (a file is at most ${HOST.file})`),
    ...(all.length > HOST.files ? [`${all.length} files (at most ${HOST.files})`] : []),
    ...(total > HOST.bytes ? [`${total} bytes (at most ${HOST.bytes})`] : []),
    ...readdirSync(DIST).filter((name) => HOST.reserved.includes(name)).map((name) => `${name}/ is the host's own`),
  ];
  if (refused.length) throw new Error(`the host would refuse the directory: ${refused.join("; ")}`);
  const report = { directory: DIST, files: all.length, bytes: total, build: id, studio: registered ? { app: registered.app, server: registered.server } : null, pack: { manifest, pieces: cut.packs[0]!.pieces.length, sha256: cut.packs[0]!.sha256, bytes: cut.packs[0]!.bytes } };
  writeFileSync(join(BUILD, "dist.json"), JSON.stringify(report, null, 1));
  return report;
}

/** What Chrome is started with for a page that draws with WebGPU: the real GPU, through Metal. */
export const CHROME = ["--use-angle=metal", "--enable-gpu", "--ignore-gpu-blocklist", "--enable-unsafe-webgpu", "--no-proxy-server", "--autoplay-policy=no-user-gesture-required"];
const TYPES: Record<string, string> = { html: "text/html; charset=utf-8", js: "text/javascript; charset=utf-8", css: "text/css; charset=utf-8", wasm: "application/wasm", json: "application/json", png: "image/png", webp: "image/webp", woff2: "font/woff2", txt: "text/plain; charset=utf-8", pak: "application/octet-stream" };
/** What a game's host of Pocket Studio answers at `/app.json`, for the server here. */
const APP = {
  kind: "site", id: "local", slug: "ethertum", url: null, title: "Pocket Ethertum", author: "local", tagline: "Dig, build and fly over floating islands.", verified: true, status: "published",
  packages: [{ target: "vita", filename: "pocket-ethertum-0.1.0-vita.vpk", size: 3_600_000, version: "0.1.0" }],
  remixable: true, repository: "https://github.com/pocket-nexus/pocket-ethertum",
};

/** The deployable directory as its host serves it: whole files, the page asked for again at every visit. */
export function serveDist(port: number) {
  return Bun.serve({
    port,
    hostname: "127.0.0.1",
    fetch(request) {
      const path = decodeURIComponent(new URL(request.url).pathname);
      const file = join(DIST, path === "/" ? "index.html" : path);
      if (!file.startsWith(DIST) || !existsSync(file) || !statSync(file).isFile()) return new Response("not found", { status: 404 });
      const type = file.split(".").pop()!;
      return new Response(Bun.file(file), { headers: { "Content-Type": TYPES[type] ?? "application/octet-stream", "Cache-Control": type === "html" ? "no-cache" : "public, max-age=600" } });
    },
  });
}

/** The site, the pack answered a range at a time. */
export function serve(port: number) {
  return Bun.serve({
    port,
    hostname: "127.0.0.1",
    fetch(request) {
      const path = decodeURIComponent(new URL(request.url).pathname);
      if (path === "/app.json") return Response.json(APP, { headers: { "Cache-Control": "no-store" } });
      const file = join(SITE, path === "/" ? "index.html" : path);
      if (!file.startsWith(SITE) || !existsSync(file) || !statSync(file).isFile()) return new Response("not found", { status: 404 });
      const type = file.split(".").pop()!;
      const head = { "Content-Type": TYPES[type] ?? "application/octet-stream", "Cache-Control": "no-store", "Accept-Ranges": "bytes" };
      const range = request.headers.get("range")?.match(/^bytes=(\d+)-(\d*)$/);
      if (!range) return new Response(Bun.file(file), { headers: head });
      const size = statSync(file).size;
      const from = Number(range[1]);
      const to = Math.min(range[2] ? Number(range[2]) : size - 1, size - 1);
      if (from > to) return new Response(null, { status: 416, headers: { "Content-Range": `bytes */${size}` } });
      return new Response(Bun.file(file).slice(from, to + 1), { status: 206, headers: { ...head, "Content-Range": `bytes ${from}-${to}/${size}`, "Content-Length": String(to - from + 1) } });
    },
  });
}

/** The program that draws frames on this machine's GPU (wgpu/src/bin/shot.rs), built. */
export async function shotProgram(): Promise<string> {
  await $`cargo build --release --bin ether-shot`.cwd(CRATE).quiet();
  return join(CRATE, "target/release/ether-shot");
}

const stamp = () => new Date().toISOString().replace(/[:.]/g, "-");
const validation = (run: string) => {
  const directory = join(ROOT, ".pocket-build/validation/web", run);
  mkdirSync(directory, { recursive: true });
  return directory;
};

if (command === "build") {
  console.log(JSON.stringify(await build(), null, 1));
} else if (command === "dist") {
  console.log(JSON.stringify(await dist(Math.round(Number(option("--piece", String(CUT_SIZES.max >> 20))) * (1 << 20))), null, 1));
} else if (command === "serve" && rest.includes("--dist")) {
  if (!existsSync(join(DIST, "index.html"))) await dist(2 << 20);
  const server = serveDist(Number(option("--port", "8803")));
  console.log(`http://127.0.0.1:${server.port}/   (${DIST})`);
} else if (command === "serve") {
  if (!existsSync(join(SITE, "pkg/ether_wgpu_bg.wasm")) || rest.includes("--build")) await build();
  const server = serve(Number(option("--port", "8803")));
  console.log(`http://127.0.0.1:${server.port}/`);
} else if (command === "shot") {
  needPack();
  if (!existsSync(REGISTRY)) await registry();
  const out = resolve(option("--out", join(validation(`shot-${stamp()}`), "frame.png")));
  mkdirSync(resolve(out, ".."), { recursive: true });
  const program = await shotProgram();
  const status = JSON.parse(await $`${program} --pack ${PACK} --registry ${REGISTRY} --out ${out} --words ${option("--words", "mode=play")} --frames ${option("--frames", "30")}`.text());
  console.log(JSON.stringify(status, null, 1));
  console.log(out);
} else if (command === "check") {
  const deployed = rest.includes("--dist") ? await dist(Math.round(Number(option("--piece", String(CUT_SIZES.max >> 20))) * (1 << 20))) : null;
  if (!deployed) await build();
  const { chromium } = await import("playwright-core");
  const server = deployed ? serveDist(0) : serve(0);
  const directory = validation(`check-${deployed ? "dist-" : ""}${stamp()}`);
  const seconds = Number(option("--seconds", "5"));
  const browser = await chromium.launch({ channel: "chrome", headless: !rest.includes("--headed"), args: CHROME });
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  const problems: string[] = [];
  // (the deployable directory's host answers no /app.json: the browser writes that one reply to the console)
  page.on("console", (m) => m.type() === "error" && !m.location().url.endsWith("/app.json") && problems.push(m.text()));
  page.on("pageerror", (e) => problems.push(String(e)));
  const expect = (what: string, ok: boolean) => {
    if (!ok) throw new Error(`the page: ${what}`);
  };
  const status = async () => (await page.evaluate("pocketEthertum.status()")) as any;
  const save = async (name: string) => {
    const url = (await page.evaluate("pocketEthertum.capture()")) as string;
    writeFileSync(join(directory, `${name}.png`), Buffer.from(url.slice(url.indexOf(",") + 1), "base64"));
    await page.screenshot({ path: join(directory, `${name}-page.png`) });
  };
  const key = async (code: string, hold = 90) => {
    await page.keyboard.down(code);
    await page.waitForTimeout(hold);
    await page.keyboard.up(code);
    await page.waitForTimeout(250);
  };
  const mode = async (want: string, within = 8000) => {
    await page.waitForFunction(`pocketEthertum.status().mode === ${JSON.stringify(want)}`, undefined, { timeout: within }).catch(() => {});
    const now = (await status()).mode;
    expect(`the flow is "${now}", not "${want}"`, now === want);
  };
  const report: Record<string, any> = { chrome: browser.version(), deployed };
  try {
    await page.goto(`http://127.0.0.1:${server.port}/`);
    // The world behind the title, the interface up, the card gone.
    await page.waitForFunction("window.pocketEthertum && ((pocketEthertum.firstWorld && pocketEthertum.interface?.()) || pocketEthertum.failure)", undefined, { timeout: 120_000 });
    const failure = (await page.evaluate("pocketEthertum.failure")) as string;
    expect(`the start failed: ${failure}`, !failure);
    report.start = await page.evaluate("({ ready: pocketEthertum.ready, interfaceReady: pocketEthertum.interfaceReady, firstFrame: pocketEthertum.firstFrame, firstWorld: pocketEthertum.firstWorld })");
    await mode("title");
    await page.waitForTimeout(1500);
    await save("title");
    // Play is the title's first row: ○ (Z) chooses it.
    await key("KeyZ");
    await mode("play");
    const before = (await status()).player.pos as number[];
    await key("KeyW", 1500);
    const after = (await status()).player.pos as number[];
    const moved = Math.hypot(after[0]! - before[0]!, after[2]! - before[2]!);
    report.walk = { from: before, to: after, metres: moved };
    expect(`the stick did not move the player (${moved.toFixed(2)} m)`, moved > 1);
    // A look to the right with the right stick (L).
    const yaw = (await status()).player.yaw as number;
    await key("KeyL", 600);
    report.look = { from: yaw, to: (await status()).player.yaw };
    expect("the right stick did not turn the view", (await status()).player.yaw !== yaw);
    await save("play");
    // START (Space) opens the menu, and closes it.
    await key("Space");
    await mode("menu");
    await save("menu");
    await key("Space");
    await mode("play");
    // Frames over a while, then what a frame costs without the display.
    const first = await status();
    const from = Date.now();
    await page.waitForTimeout(seconds * 1000);
    const last = await status();
    report.frames = { seconds, made: last.frames - first.frames, perSecond: ((last.frames - first.frames) * 1000) / (Date.now() - from), late: last.late - first.late, cpuMs: last.cpuMs, worldMs: last.worldMs, drawn: last.drawn, adapter: last.adapter, blocks: last.blocks };
    report.burstMs = await page.evaluate("pocketEthertum.burst(300)");
    report.timing = await page.evaluate("pocketEthertum.timing");
    report.problems = problems;
    expect(`the console has errors: ${problems.join(" | ")}`, problems.length === 0);
    report.passed = true;
  } finally {
    writeFileSync(join(directory, "report.json"), JSON.stringify(report, null, 1));
    await browser.close();
    server.stop(true);
  }
  console.log(JSON.stringify(report, null, 1));
  console.log(directory);
} else if (import.meta.main) {
  console.error("bun tools/web.ts build | serve [--dist] | dist | shot | check");
  process.exit(1);
}
