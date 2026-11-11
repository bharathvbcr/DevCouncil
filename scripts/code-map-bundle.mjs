#!/usr/bin/env node
// Builds, or drift-checks, the checked-in code-map page bundle.
//
// The page shell lives in rust/devmap-query/assets/code-map/src (TypeScript, no
// npm dependencies). Bun's built-in bundler turns the entry into one IIFE that
// the Rust renderers inline, so the checked-in bytes must be exactly what this
// build produces. `build` writes them; `check` rebuilds to a temp path and
// byte-compares, failing loudly on any drift or on a Bun that is not the pinned
// one (a different Bun can emit different bytes for the same source).
//
// Bun does not type-check. The type gate is GitPulse's tsc over its vendored
// copy of this directory (see docs/devmap/README.md).

import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
export const ENTRY = "rust/devmap-query/assets/code-map/src/page/entry.ts";
export const BUNDLE = "rust/devmap-query/assets/code-map.js.bundle";
export const BUN_VERSION_FILE = ".bun-version";

export function pinnedBunVersion(read = (path) => readFileSync(path, "utf8")) {
  const pinned = read(join(root, BUN_VERSION_FILE)).trim();
  if (!/^\d+\.\d+\.\d+$/.test(pinned)) {
    throw new Error(`${BUN_VERSION_FILE} must hold one exact Bun version like 1.4.4, found ${JSON.stringify(pinned)}`);
  }
  return pinned;
}

/** Throws unless the Bun on PATH is exactly the pinned one. */
export function assertBunVersion(actual, pinned) {
  if (actual.trim() !== pinned) {
    throw new Error(
      `the code-map bundle is built with Bun ${pinned} (${BUN_VERSION_FILE}) but Bun ${actual.trim()} is on PATH; ` +
        "a different Bun can emit different bytes, so refusing to build or compare.",
    );
  }
}

function bunVersion() {
  try {
    return execFileSync("bun", ["--version"], { encoding: "utf8" });
  } catch (error) {
    throw new Error(`bun is required to build the code-map bundle and was not found on PATH: ${error.message}`);
  }
}

export function bundleTo(outfile) {
  execFileSync(
    "bun",
    ["build", ENTRY, "--target=browser", "--format=iife", "--minify", `--outfile=${outfile}`],
    { cwd: root, stdio: ["ignore", "inherit", "inherit"] },
  );
}

function main(mode) {
  assertBunVersion(bunVersion(), pinnedBunVersion());
  const target = join(root, BUNDLE);
  if (mode === "build") {
    bundleTo(target);
    console.log(`wrote ${BUNDLE} (${readFileSync(target).length} bytes)`);
    return;
  }
  if (mode === "check") {
    const scratch = mkdtempSync(join(tmpdir(), "code-map-"));
    try {
      const rebuilt = join(scratch, "code-map.js.bundle");
      bundleTo(rebuilt);
      if (!existsSync(target)) throw new Error(`${BUNDLE} is missing; run \`npm run build:code-map\``);
      const fresh = readFileSync(rebuilt);
      const checkedIn = readFileSync(target);
      if (!fresh.equals(checkedIn)) {
        throw new Error(
          `${BUNDLE} is stale: the checked-in bundle is ${checkedIn.length} bytes and a fresh build is ${fresh.length}. ` +
            "Run `npm run build:code-map` and commit the result.",
        );
      }
      console.log(`${BUNDLE} matches a fresh build (${fresh.length} bytes)`);
    } finally {
      rmSync(scratch, { recursive: true, force: true });
    }
    return;
  }
  throw new Error(`usage: code-map-bundle.mjs build|check (got ${JSON.stringify(mode)})`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main(process.argv[2]);
  } catch (error) {
    console.error(`code-map-bundle: ${error.message}`);
    process.exit(1);
  }
}
