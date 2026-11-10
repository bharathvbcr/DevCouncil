import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { assertBunVersion, pinnedBunVersion } from "./code-map-bundle.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

test("the pin is one exact Bun version", () => {
  assert.equal(pinnedBunVersion(() => "1.4.4\n"), "1.4.4");
  for (const bad of ["", "1.4", "^1.4.4", "latest", "1.4.4-canary"]) {
    assert.throws(() => pinnedBunVersion(() => bad), /exact Bun version/, JSON.stringify(bad));
  }
});

test("a different Bun is refused rather than compared", () => {
  assertBunVersion("1.4.4\n", "1.4.4");
  assert.throws(() => assertBunVersion("1.4.5", "1.4.4"), /different Bun can emit different bytes/);
});

test("CI installs the Bun the repository pins, from a SHA-pinned action", () => {
  const ci = readFileSync(path.join(root, ".github", "workflows", "ci.yml"), "utf8");
  assert.match(ci, /uses: oven-sh\/setup-bun@[0-9a-f]{40} # v\d/);
  assert.match(ci, /bun-version-file: \.bun-version/);
  assert.match(ci, /npm run check:code-map/);
  assert.match(ci, /npm run test:code-map/);
  assert.match(pinnedBunVersion(), /^\d+\.\d+\.\d+$/);
});
