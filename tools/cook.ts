// Cooks the reference's art into the device pack.
//
//   bun tools/cook.ts [--out DIR] [--no-preview]
//
// Runs `ether-cook` (crates/ether-cook) on the pinned reference in
// ref/ethertum and writes `ether.pack`, `receipt.json` and, unless
// `--no-preview`, pictures decoded back from the pack, under
// `.pocket-build/pack/` (ignored by Git). The same sources cook to the
// same bytes: the receipt's sha256 names the pack.

import { $ } from "bun";
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

const ROOT = resolve(import.meta.dir, "..");
const REF = resolve(ROOT, "ref/ethertum");

export const PACK_DIR = resolve(ROOT, ".pocket-build/pack");

export interface Receipt {
  pack: { file: string; sha256: string; bytes: number };
  version: number;
  reference: string;
}

function value(argv: string[], flag: string, fallback: string): string {
  const i = argv.indexOf(flag);
  return i >= 0 && argv[i + 1] ? argv[i + 1]! : fallback;
}

/** Cooks the pack and returns its receipt. */
export async function cook(argv: string[] = []): Promise<Receipt> {
  if (!existsSync(resolve(REF, "assets/baked/atlas_diff.png"))) {
    throw new Error("ref/ethertum is not checked out: git submodule update --init ref/ethertum");
  }
  const out = resolve(value(argv, "--out", PACK_DIR));
  const extra = argv.includes("--no-preview") ? ["--no-preview"] : [];
  await $`cargo run --release -q -p ether-cook -- --ref ${REF} --out ${out} ${extra}`.cwd(ROOT);
  return JSON.parse(readFileSync(resolve(out, "receipt.json"), "utf8"));
}

if (import.meta.main) {
  const receipt = await cook(Bun.argv.slice(2));
  console.log(`cook: ${receipt.pack.file} ${receipt.pack.bytes} bytes, sha256 ${receipt.pack.sha256}`);
}
