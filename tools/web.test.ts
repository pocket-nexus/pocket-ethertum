// A device with a package is a device the game's page plays as (Pocket Studio's rule, `docs/publishing.md` "The
// devices a site plays as"): the page's `pocket-devices`, the player's devices and `runsOn` in wgpu/page/main.js,
// and the targets the release builds a package for are the same list. `bun test tools/web.test.ts`.

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

const root = join(import.meta.dir, "..");
const page = readFileSync(join(root, "wgpu/page/index.html"), "utf8");
const script = readFileSync(join(root, "wgpu/page/main.js"), "utf8");
const vita = readFileSync(join(root, "tools/vita.ts"), "utf8");

/** The targets a package is built for: the PS Vita's `.vpk` (`bun tools/vita.ts vpk`). */
const RELEASED = vita.includes("vita-pack-vpk") || vita.includes("vpk") ? ["vita"] : [];

test("the page plays as every device the release has a package for", () => {
  const meta = page.match(/<meta name="pocket-devices" content="([^"]*)">/)?.[1]?.split(/\s+/).filter(Boolean) ?? [];
  const devices = [...script.matchAll(/\{ id: "([a-z0-9-]+)", label:/g)].map((m) => m[1]);
  const runsOn = JSON.parse((script.match(/runsOn: (\[[^\]]*\])/)?.[1] ?? "[]").replace(/'/g, '"'));
  expect(RELEASED).toEqual(["vita"]);
  expect(meta).toEqual(RELEASED);
  expect(devices).toEqual(RELEASED);
  expect(runsOn).toEqual(RELEASED);
});

test("the page's pack is the one the build serves and the dist names", () => {
  expect(page).toContain(`<meta name="pocket-pack" content="ether.pack">`);
  expect(page).toContain(`src="main.js"`);
});
