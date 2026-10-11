import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, it } from "node:test";
import {
  goModToolchain,
  gussetPin,
  gussetPinVerdict,
  localTagState,
  tagVerdict,
} from "./ci-local-checks.mjs";

const PIN = "984209b542885b10b43e3d9aaf2073fd871e9eb9";

function repo() {
  const dir = mkdtempSync(path.join(tmpdir(), "ci-local-test-"));
  const git = (...args) => {
    const r = spawnSync("git", ["-C", dir, ...args], { encoding: "utf8" });
    assert.equal(r.status, 0, r.stderr);
    return r.stdout.trim();
  };
  git("init", "-q");
  git("config", "user.email", "t@example.com");
  git("config", "user.name", "t");
  git("config", "commit.gpgsign", "false");
  git("config", "tag.gpgsign", "false");
  writeFileSync(path.join(dir, "a"), "1");
  git("add", "a");
  git("commit", "-q", "-m", "one");
  return { dir, git, cleanup: () => rmSync(dir, { recursive: true, force: true }) };
}

describe("ci:local release tag step", () => {
  it("is NOT RUN, never a pass, when there is no local tag", () => {
    const r = repo();
    try {
      const s = localTagState(r.dir, "0.2.4");
      assert.equal(s.state, "absent");
      assert.equal(tagVerdict(s), "NOT RUN");
    } finally {
      r.cleanup();
    }
  });

  it("FAILs for a local tag that is not at HEAD", () => {
    const r = repo();
    try {
      r.git("tag", "v0.2.4");
      writeFileSync(path.join(r.dir, "a"), "2");
      r.git("commit", "-q", "-am", "two");
      const s = localTagState(r.dir, "0.2.4");
      assert.equal(s.state, "off-head");
      assert.equal(tagVerdict(s), "FAIL");
    } finally {
      r.cleanup();
    }
  });

  it("PASSes for a local tag at HEAD, annotated or lightweight", () => {
    const r = repo();
    try {
      r.git("tag", "-a", "-m", "release", "v0.2.4");
      const s = localTagState(r.dir, "0.2.4");
      assert.equal(s.state, "at-head");
      assert.equal(tagVerdict(s), "PASS");
    } finally {
      r.cleanup();
    }
  });

  it("FAILs when git cannot answer", () => {
    const s = localTagState("/nonexistent/for/ci-local", "0.2.4");
    assert.equal(s.state, "unknown");
    assert.equal(tagVerdict(s), "FAIL");
  });
});

describe("ci:local Gusset pin", () => {
  it("reads the pinned ref from setup-gusset", () => {
    assert.equal(gussetPin(`with:\n  ref: ${PIN}\n  path: x\n`), PIN);
    assert.equal(gussetPin("ref: main\n"), undefined);
  });

  it("FAILs, not warns, when the sibling is off the pin", () => {
    const v = gussetPinVerdict(PIN, "92c205609f02ab116a12844a074a8b424c121359");
    assert.equal(v.ok, false);
    assert.match(v.message, /pins 984209b54288/);
  });

  it("FAILs when the pin or the sibling HEAD cannot be read", () => {
    assert.equal(gussetPinVerdict(undefined, PIN).ok, false);
    assert.equal(gussetPinVerdict(PIN, "").ok, false);
  });

  it("passes only at the pin", () => {
    assert.equal(gussetPinVerdict(PIN, PIN).ok, true);
  });
});

describe("ci:local go toolchain", () => {
  it("names the toolchain go.mod declares", () => {
    assert.equal(goModToolchain("module m\n\ngo 1.26.6\n"), "go1.26.6");
    assert.equal(goModToolchain("module m\n\ngo 1.26.0\ntoolchain go1.26.6\n"), "go1.26.6");
    assert.equal(goModToolchain("module m\n"), undefined);
  });
});

describe("rust/gusset-engine/check-pin.sh (used by rust/verify.sh)", () => {
  const script = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "rust", "gusset-engine", "check-pin.sh");

  function fixture(pinned) {
    const sibling = repo();
    const head = sibling.git("rev-parse", "HEAD");
    const action = path.join(sibling.dir, "..", `action-${path.basename(sibling.dir)}.yml`);
    writeFileSync(action, `with:\n  ref: ${pinned ?? head}\n`);
    return { sibling, action, head };
  }
  const run = (action, dir) => spawnSync("bash", [script, action, dir], { encoding: "utf8" });

  it("passes at the pin", () => {
    const f = fixture();
    try {
      assert.equal(run(f.action, f.sibling.dir).status, 0);
    } finally {
      rmSync(f.action, { force: true });
      f.sibling.cleanup();
    }
  });

  it("fails, not warns, off the pin", () => {
    const f = fixture(PIN);
    try {
      const r = run(f.action, f.sibling.dir);
      assert.equal(r.status, 1);
      assert.match(r.stderr, /CI pins 984209b54288/);
    } finally {
      rmSync(f.action, { force: true });
      f.sibling.cleanup();
    }
  });

  it("exits 2 when the pin or the sibling cannot be read", () => {
    const f = fixture();
    try {
      assert.equal(run("/nonexistent/action.yml", f.sibling.dir).status, 2);
      assert.equal(run(f.action, "/nonexistent/gusset").status, 2);
    } finally {
      rmSync(f.action, { force: true });
      f.sibling.cleanup();
    }
  });
});

describe("rust/dc-sparse-encode/ci-test.sh", () => {
  const script = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "rust", "dc-sparse-encode", "ci-test.sh");
  const run = (env) =>
    spawnSync("bash", [script], {
      encoding: "utf8",
      env: { ...process.env, PATH: `/usr/bin:/bin:${process.env.PATH}`, ...env },
    });
  const ok = { DC_SPARSE_METAL_PROBE: "true", DC_SPARSE_SDK_VERSION: "echo 26.1", DC_SPARSE_GPU_PROBE: "echo 'Metal Support: Metal 4'" };

  it("prints NOT RUN, and exits 0, when the Metal toolchain is absent", () => {
    const r = run({ ...ok, DC_SPARSE_METAL_PROBE: "false" });
    assert.equal(r.status, 0);
    assert.match(r.stdout, /^NOT RUN: dc-sparse-encode tests: no Metal toolchain/m);
  });

  it("prints NOT RUN for an SDK older than 26 and for a runner with no Metal device", () => {
    const sdk = run({ ...ok, DC_SPARSE_SDK_VERSION: "echo 15.5" });
    assert.equal(sdk.status, 0);
    assert.match(sdk.stdout, /NOT RUN: .*SDK 15\.5/);
    const gpu = run({ ...ok, DC_SPARSE_GPU_PROBE: "echo 'no display'" });
    assert.equal(gpu.status, 0);
    assert.match(gpu.stdout, /NOT RUN: .*no Metal-capable device/);
  });

  it("fails, not NOT RUN, when the SDK version cannot be read", () => {
    const r = run({ ...ok, DC_SPARSE_SDK_VERSION: "echo unreadable" });
    assert.equal(r.status, 2);
    assert.doesNotMatch(r.stdout, /NOT RUN/);
  });

  it("runs the locked tests once every probe passes, and a failure there fails", () => {
    const fake = mkdtempSync(path.join(tmpdir(), "fake-cargo-"));
    try {
      writeFileSync(path.join(fake, "cargo"), '#!/bin/sh\necho "cargo $*"\nexit "${FAKE_CARGO_EXIT:-0}"\n', { mode: 0o755 });
      const base = { ...ok, PATH: `${fake}:/usr/bin:/bin` };
      const pass = run(base);
      assert.equal(pass.status, 0);
      assert.match(pass.stdout, /cargo test --locked/);
      assert.doesNotMatch(pass.stdout, /NOT RUN/);
      const fail = run({ ...base, FAKE_CARGO_EXIT: "101" });
      assert.notEqual(fail.status, 0);
    } finally {
      rmSync(fake, { recursive: true, force: true });
    }
  });
});
