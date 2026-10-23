#!/usr/bin/env bun
// The game's page on Pocket Studio: listing/listing.json's words, and its clips, stills, share card and cover.
//
//   bun tools/listing.ts            → dist/listing/ (listing.json and its files), for `pocket-studio listing dist/listing`
//   bun tools/listing.ts --upload   the same, then sent with the Studio's command line ($POCKET_STUDIO_CLI, or
//                                   `pocket-studio`): the listing, and the cover on the game's case
//
// A clip is the browser build (`bun tools/web.ts build`) drawing one held frame after another in Chrome, the
// interface off: each entry's `take` names where the eye is and how it moves (yaw, pitch and the daytime from
// their first value to their last over the clip; `strokes`: the brush breaks or places between two times, every
// eighth of a second or every `every` seconds). Two sixtieths of a second of the game a frame, thirty frames a second, H.264 with no sound
// track. A still with `still` is a picture captured on a device (`from`), taken as it is. The card is one held
// frame of `cardTake` at 1200 × 630. Nothing here is committed: dist/ is ignored.

import { $ } from "bun";
import { existsSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { CHROME, serve } from "./web.ts";

const ROOT = resolve(import.meta.dir, "..");
const SOURCE = join(ROOT, "listing/listing.json");
const OUT = join(ROOT, "dist/listing");
const FRAMES = join(ROOT, ".pocket-build/listing/frames");
const WIDTH = 960;
const HEIGHT = 544;
const RATE = 30;
const MIB = 1 << 20;
/** Pocket Studio's limits (`LISTING_LIMITS` in its src/core/listing.ts). */
const LIMIT = { clip: 12 * MIB, still: 2 * MIB, card: MIB, cover: 2 * MIB, total: 96 * MIB, seconds: [6, 30], media: [2, 12], tagline: 120, paragraph: 600, paragraphs: [1, 6], caption: 140 };
const CARD = { width: 1200, height: 630 };

type Range = [number, number];
interface Take {
  seconds: number;
  pos: [number, number, number];
  pitch: Range;
  yaw: Range;
  daytime: Range;
  strokes?: [number, number, "break" | "place", number?][];
}
interface Entry {
  kind: "video" | "image";
  file: string;
  poster?: string;
  take?: Take;
  still?: string;
  from?: string;
  caption: string;
}
interface Source {
  tagline: string;
  description: string[];
  translations?: { ja?: { tagline?: string; description?: string[]; captions?: Record<string, string> } };
  media: Entry[];
  card: string;
  cardTake: { pos: [number, number, number]; pitch: number; yaw: number; daytime: number };
  cover: string;
}

const source = JSON.parse(readFileSync(SOURCE, "utf8")) as Source;
const fail = (what: string) => {
  throw new Error(`listing: ${what}`);
};
// The words first: the Studio refuses what breaks its limits, and so does this.
if (source.tagline.length < 1 || [...source.tagline].length > LIMIT.tagline) fail("the tagline is 1 to 120 characters");
if (source.description.length < LIMIT.paragraphs[0]! || source.description.length > LIMIT.paragraphs[1]!) fail("the description is 1 to 6 paragraphs");
for (const p of [...source.description, ...(source.translations?.ja?.description ?? [])]) if ([...p].length > LIMIT.paragraph) fail(`a paragraph is over 600 characters: ${p.slice(0, 40)}…`);
if (source.media.length < LIMIT.media[0]! || source.media.length > LIMIT.media[1]!) fail("2 to 12 clips and pictures");
for (const e of source.media) if ([...e.caption].length > LIMIT.caption) fail(`${e.file}'s caption is over 140 characters`);

const lerp = (r: Range, t: number) => r[0] + (r[1] - r[0]) * t;
const words = (take: Take, t: number) => `mode=play fly=1 pos=${take.pos.join(",")} yaw=${lerp(take.yaw, t)} pitch=${lerp(take.pitch, t)} daytime=${lerp(take.daytime, t)}`;

rmSync(OUT, { recursive: true, force: true });
mkdirSync(OUT, { recursive: true });

const site = join(ROOT, ".pocket-build/web/site/pkg/ether_wgpu_bg.wasm");
if (!existsSync(site)) await $`bun ${join(ROOT, "tools/web.ts")} build`;
const { chromium } = await import("playwright-core");
const server = serve(0);
const browser = await chromium.launch({ channel: "chrome", headless: true, args: CHROME });
const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
const drawn: Record<string, string> = {};
try {
  await page.goto(`http://127.0.0.1:${server.port}/?interface=off`);
  await page.waitForFunction("window.pocketEthertum && (pocketEthertum.firstWorld || pocketEthertum.failure)", undefined, { timeout: 120_000 });
  const failure = (await page.evaluate("pocketEthertum.failure")) as string;
  if (failure) fail(`the page: ${failure}`);
  await page.evaluate("pocketEthertum.hold()");
  /** A held frame after `count` sixtieths of a second with `words`, as PNG bytes. */
  const take = async (w: string, count: number) => {
    const url = (await page.evaluate(`pocketEthertum.take(${JSON.stringify(w)}, ${count})`)) as string;
    return Buffer.from(url.slice(url.indexOf(",") + 1), "base64");
  };
  /** The eye at `w`, held until the world around it is meshed. */
  const settle = async (w: string) => {
    await take(w, 2);
    for (let i = 0; i < 400; i++) {
      await take(w, 10);
      if ((await page.evaluate("pocketEthertum.status().settled")) as boolean) break;
    }
    await take(w, 30);
  };
  for (const entry of source.media) {
    if (!entry.take) continue;
    const take_ = entry.take;
    const frames = Math.round(take_.seconds * RATE);
    const dir = join(FRAMES, entry.file.replace(/\.mp4$/, ""));
    rmSync(dir, { recursive: true, force: true });
    mkdirSync(dir, { recursive: true });
    await settle(words(take_, 0));
    for (let k = 0; k < frames; k++) {
      const t = k / Math.max(frames - 1, 1);
      const seconds = k / RATE;
      let w = words(take_, t);
      // (a stroke on the window's first frame, then every `every` seconds: an eighth by default, four frames)
      const stroke = take_.strokes?.find(([from, to]) => seconds >= from && seconds < to);
      if (stroke && (k - Math.ceil(stroke[0] * RATE)) % Math.round((stroke[3] ?? 0.125) * RATE) === 0) w += ` stroke=${stroke[2]}`;
      writeFileSync(join(dir, `${String(k).padStart(4, "0")}.png`), await take(w, 2));
    }
    await $`ffmpeg -v error -y -framerate ${RATE} -i ${join(dir, "%04d.png")} -c:v libx264 -preset slow -crf 20 -maxrate 6M -bufsize 12M -profile:v high -pix_fmt yuv420p -g ${RATE * 2} -movflags +faststart -an -map_metadata -1 ${join(OUT, entry.file)}`;
    if (entry.poster) await $`ffmpeg -v error -y -i ${join(dir, `${String(Math.floor(frames / 3)).padStart(4, "0")}.png`)} -q:v 2 ${join(OUT, entry.poster)}`;
    drawn[entry.file] = "browser";
  }
  // The share card: one held frame, at the card's width, cut to its height.
  const c = source.cardTake;
  const cw = `mode=play fly=1 pos=${c.pos.join(",")} yaw=${c.yaw} pitch=${c.pitch} daytime=${c.daytime}`;
  await settle(cw);
  mkdirSync(FRAMES, { recursive: true });
  writeFileSync(join(FRAMES, "card.png"), await take(cw, 2));
  const tall = Math.round((HEIGHT * CARD.width) / WIDTH);
  await $`ffmpeg -v error -y -i ${join(FRAMES, "card.png")} -vf ${`scale=${CARD.width}:${tall}:flags=lanczos,crop=${CARD.width}:${CARD.height}:0:${Math.round((tall - CARD.height) / 2)}`} -q:v 3 ${join(OUT, source.card)}`;
} finally {
  await browser.close();
  server.stop(true);
}

// The stills a device captured, taken as they are.
for (const entry of source.media) {
  if (!entry.still) continue;
  const from = join(ROOT, entry.still);
  if (!existsSync(from)) fail(`${entry.still} is missing: it is a capture from the device (bun tools/vita.ts capture)`);
  await $`ffmpeg -v error -y -i ${from} -q:v 2 ${join(OUT, entry.file)}`;
  drawn[entry.file] = entry.from ?? "vita";
}

// What each file is, read from the file.
const probe = async (file: string) => JSON.parse(await $`ffprobe -v error -print_format json -show_streams -show_format ${file}`.text());
const media = [];
let total = 0;
for (const entry of source.media) {
  const file = join(OUT, entry.file);
  const p = await probe(file);
  const stream = p.streams[0];
  const bytes = statSync(file).size;
  total += bytes;
  if (entry.kind === "video") {
    const seconds = Math.round(Number(p.format.duration));
    if (bytes > LIMIT.clip) fail(`${entry.file} is ${bytes} bytes (at most ${LIMIT.clip})`);
    if (seconds < LIMIT.seconds[0]! || seconds > LIMIT.seconds[1]!) fail(`${entry.file} is ${seconds} s (6 to 30)`);
    if (p.streams.length !== 1) fail(`${entry.file} has a sound track or more than one stream`);
    const poster = join(OUT, entry.poster!);
    total += statSync(poster).size;
    media.push({ kind: "video", file: entry.file, poster: entry.poster, width: stream.width, height: stream.height, seconds, from: drawn[entry.file], caption: entry.caption });
  } else {
    if (bytes > LIMIT.still) fail(`${entry.file} is ${bytes} bytes (at most ${LIMIT.still})`);
    media.push({ kind: "image", file: entry.file, width: stream.width, height: stream.height, from: drawn[entry.file], caption: entry.caption });
  }
}
const cardBytes = statSync(join(OUT, source.card)).size;
if (cardBytes > LIMIT.card) fail(`the card is ${cardBytes} bytes (at most ${LIMIT.card})`);
total += cardBytes;
if (total > LIMIT.total) fail(`the listing is ${total} bytes (at most ${LIMIT.total})`);
const listing = { tagline: source.tagline, description: source.description, media, card: source.card, ...(source.translations ? { translations: source.translations } : {}) };
writeFileSync(join(OUT, "listing.json"), JSON.stringify(listing, null, 1));
const cover = join(ROOT, source.cover);
if (statSync(cover).size > LIMIT.cover) fail(`the cover is over ${LIMIT.cover} bytes`);
console.log(JSON.stringify({ directory: OUT, files: media.length * 2 + 2, bytes: total, media: media.map((m) => `${m.file} ${m.width}x${m.height}${"seconds" in m ? ` ${m.seconds}s` : ""} ${m.from}`) }, null, 1));

if (process.argv.includes("--upload")) {
  const cli = (process.env.POCKET_STUDIO_CLI ?? "pocket-studio").split(" ");
  await $`${cli} listing ${OUT}`.cwd(ROOT);
  await $`${cli} cover ${cover}`.cwd(ROOT);
}
