import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, it } from "node:test";
import {
  StepStop,
  chainOf,
  devmapAvailable,
  exitCode,
  goModToolchain,
  gussetPin,
  gussetPinVerdict,
  expectedRowLabels,
  libtestFailures,
  localTagState,
  nestedGateProblem,
  parseArgs,
  runSteps,
  summarize,
  tagVerdict,
} from "./ci-local-checks.mjs";
import { GO_MODULE, STEPS, rustVerifyStep } from "./ci-local.mjs";

const PIN = "320c56c27e1098f7e0a26e9854ff80c4f4354ef9";

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
    assert.match(v.message, /pins 320c56c27e10/);
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
      assert.match(r.stderr, /CI pins 320c56c27e10/);
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

  it("checks formatting before the probes, so a NOT RUN runner still fails on unformatted code", () => {
    const fake = mkdtempSync(path.join(tmpdir(), "fake-cargo-"));
    try {
      writeFileSync(path.join(fake, "cargo"), '#!/bin/sh\necho "cargo $*"\n[ "$1" = fmt ] && exit "${FAKE_FMT_EXIT:-0}"\nexit 0\n', { mode: 0o755 });
      const base = { ...ok, DC_SPARSE_METAL_PROBE: "false", PATH: `${fake}:/usr/bin:/bin` };
      const clean = run(base);
      assert.equal(clean.status, 0);
      assert.match(clean.stdout, /cargo fmt -- --check/);
      assert.match(clean.stdout, /NOT RUN/);
      assert.doesNotMatch(clean.stdout, /cargo test/);
      const dirty = run({ ...base, FAKE_FMT_EXIT: "1" });
      assert.equal(dirty.status, 1);
      assert.doesNotMatch(dirty.stdout, /NOT RUN/);
    } finally {
      rmSync(fake, { recursive: true, force: true });
    }
  });

  it("appends the NOT RUN reason to GITHUB_STEP_SUMMARY, and an unset or unwritable one never fails", () => {
    const fake = mkdtempSync(path.join(tmpdir(), "fake-cargo-"));
    try {
      writeFileSync(path.join(fake, "cargo"), "#!/bin/sh\nexit 0\n", { mode: 0o755 });
      const base = { ...ok, PATH: `${fake}:/usr/bin:/bin` };
      const summary = path.join(fake, "summary.md");
      const cases = [
        [{ DC_SPARSE_METAL_PROBE: "false" }, /no Metal toolchain/],
        [{ DC_SPARSE_SDK_VERSION: "echo 15.5" }, /SDK 15\.5/],
        [{ DC_SPARSE_GPU_PROBE: "echo 'no display'" }, /no Metal-capable device/],
      ];
      for (const [probe, reason] of cases) {
        writeFileSync(summary, "");
        const r = run({ ...base, ...probe, GITHUB_STEP_SUMMARY: summary });
        assert.equal(r.status, 0);
        assert.match(readFileSync(summary, "utf8"), reason);
        assert.match(readFileSync(summary, "utf8"), /NOT RUN/);
      }
      const env = { ...base, DC_SPARSE_METAL_PROBE: "false" };
      const unset = { ...process.env, ...env };
      delete unset.GITHUB_STEP_SUMMARY;
      assert.equal(spawnSync("bash", [script], { encoding: "utf8", env: unset }).status, 0);
      assert.equal(run({ ...env, GITHUB_STEP_SUMMARY: "/nonexistent-dir/summary.md" }).status, 0);
    } finally {
      rmSync(fake, { recursive: true, force: true });
    }
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

describe("test-support ownership", () => {
  it("has exactly one definition of restriction_holds, in rust/testsupport", () => {
    const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
    const r = spawnSync("git", ["-C", root, "grep", "-l", "fn restriction_holds", "--", "rust/*.rs"], { encoding: "utf8" });
    assert.equal(r.status, 0, r.stderr);
    assert.deepEqual(r.stdout.trim().split("\n"), ["rust/testsupport/restriction.rs"]);
  });
});

describe("ci:local runs the DevMap test selector's tests", () => {
  const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
  const selectorSteps = () => STEPS.filter((s) => (s.args ?? []).includes("./cmd/dctestselect"));

  it("has a step that runs the selector's end-to-end tests under the e2e tag, from the Go module", () => {
    const step = STEPS.find((s) => s.label === "test selector end-to-end");
    assert.ok(step, "ci-local.mjs has no `test selector end-to-end` step");
    assert.equal(step.command, "go");
    assert.equal(step.cwd, GO_MODULE);
    const timeout = /^(\d+)m$/.exec(step.args.at(-1) ?? "");
    assert.deepEqual(step.args.slice(0, -1), ["test", "-tags", "e2e", "./cmd/dctestselect", "-run", "^TestE2E", "-count=1", "-p", "1", "-timeout"]);
    assert.ok(timeout, `the last argument must be a -timeout in minutes, got ${step.args.at(-1)}`);
    // go test's default package timeout is 10 minutes; the whole-crate e2e test
    // bounds itself at 25, which only works if -timeout is longer than that.
    assert.ok(Number(timeout[1]) > 25, `-timeout ${timeout[1]}m must exceed the 25-minute in-test bound`);
  });

  it("does not run the selector twice: the e2e tests are tagged out of ./..., so only this step runs them", () => {
    assert.deepEqual(selectorSteps().map((s) => s.label), ["test selector end-to-end"]);
    for (const file of ["e2e_test.go"]) {
      const head = readFileSync(path.join(root, "backend", "go_orchestrator", "cmd", "dctestselect", file), "utf8").split("\n")[0];
      assert.equal(head, "//go:build e2e", `${file} must carry the e2e build tag`);
    }
  });

  it("names a package that exists, and a workflow that runs the selector", () => {
    const goMain = readFileSync(path.join(root, "backend", "go_orchestrator", "cmd", "dctestselect", "main.go"), "utf8");
    assert.match(goMain, /^package main$/m);
    const workflow = readFileSync(path.join(root, ".github", "workflows", "selected-tests.yml"), "utf8");
    for (const phase of ["plan", "--phase selected", "--phase full", "audit", "-tags e2e"]) {
      assert.ok(workflow.includes(phase), `selected-tests.yml does not run dctestselect ${phase}`);
    }
  });
});

/**
 * A runnable step for the runner tests. `behaviour` is what its run does:
 * "pass", "fail" (exit 3), "cannot" (could not run, exit 2), "na", or a
 * function of the context.
 */
function fake(label, needs = [], behaviour = "pass", extra = {}) {
  return {
    label,
    needs,
    ...extra,
    run: (ctx) => {
      ctx.header();
      ran.push(label);
      if (typeof behaviour === "function") return behaviour(ctx);
      if (behaviour === "fail") throw new StepStop(3, `${label} broke`);
      if (behaviour === "cannot") throw new StepStop(2, `${label} could not start`, { cannotRun: true });
      if (behaviour === "na") return { na: `${label} has nothing to do` };
      return undefined;
    },
  };
}
/** @type {string[]} */
let ran = [];

function quietRun(steps, opts) {
  ran = [];
  const out = [];
  let t = 0;
  const result = runSteps(steps, {
    gate: "test gate",
    baseEnv: { PATH: "/nowhere" },
    log: (s) => out.push(s),
    // Every step takes 1.5 s of fake time: the runner reads the clock once
    // as a step starts and once as it ends.
    now: () => (t += 1500),
    ...opts,
  });
  return { ...result, out: out.join("\n"), ran: [...ran] };
}

const row = (rows, label) => rows.find((r) => r.label === label);

describe("ci:local --continue", () => {
  it("runs every step whose needs passed, after a failure", () => {
    const r = quietRun(
      [fake("a", [], "fail"), fake("b", ["a"]), fake("c"), fake("d", ["c"])],
      { mode: "continue" },
    );
    assert.deepEqual(r.ran, ["a", "c", "d"]);
    assert.deepEqual(r.rows.map((x) => [x.label, x.status]), [["a", "FAIL"], ["b", "SKIP"], ["c", "PASS"], ["d", "PASS"]]);
  });

  it("makes a dependent of a FAIL or of a SKIP a SKIP, naming the dependency", () => {
    const r = quietRun([fake("a", [], "fail"), fake("b", ["a"]), fake("c", ["b"]), fake("d", ["c"])], { mode: "continue" });
    assert.equal(row(r.rows, "b").note, "not run: needs a");
    assert.equal(row(r.rows, "c").note, "not run: needs b");
    assert.equal(row(r.rows, "d").note, "not run: needs c");
    assert.deepEqual(r.ran, ["a"]);
    assert.match(r.out, /^SKIP: c \(not run: needs b\)$/m);
  });

  it("names the first prerequisite that did not pass when a step needs several", () => {
    const r = quietRun([fake("a"), fake("b", [], "fail"), fake("c", ["a", "b"])], { mode: "continue" });
    assert.equal(row(r.rows, "c").note, "not run: needs b");
  });

  it("skips a step whose N/A producer produced nothing", () => {
    const r = quietRun([fake("a", [], "na"), fake("b", ["a"])], { mode: "continue" });
    assert.equal(row(r.rows, "a").status, "N/A");
    assert.equal(row(r.rows, "b").status, "SKIP");
  });

  it("reports a step that could not start as SKIP with its reason, and its dependents as SKIP", () => {
    const r = quietRun([fake("engine", [], "cannot"), fake("host", ["engine"]), fake("other")], { mode: "continue" });
    assert.equal(row(r.rows, "engine").status, "SKIP");
    assert.equal(row(r.rows, "engine").note, "not run: engine could not start");
    assert.equal(row(r.rows, "host").note, "not run: needs engine");
    assert.equal(r.code, 2);
  });

  it("SKIPs a step whose external requirement is missing, before running it", () => {
    const r = quietRun([fake("e2e", [], "pass", { requires: () => "devmap not found" }), fake("next")], { mode: "continue" });
    assert.deepEqual(r.ran, ["next"]);
    assert.equal(row(r.rows, "e2e").note, "not run: devmap not found");
  });

  it("prints a start marker for every step, run or skipped", () => {
    const r = quietRun([fake("a", [], "fail"), fake("b", ["a"]), fake("c")], { mode: "continue" });
    assert.deepEqual([...r.out.matchAll(/^== (.+) ==$/gm)].map((m) => m[1]), ["a", "b", "c"]);
  });
});

describe("ci:local step outputs go only to declared dependents", () => {
  const producer = fake("producer", [], () => ({ env: { CGO_CFLAGS: "-DHASH=1" }, paths: { bin: "/tmp/x/bin" } }));
  const seen = {};
  const consumer = (label, needs) =>
    fake(label, needs, (ctx) => {
      seen[label] = { cflags: ctx.env.CGO_CFLAGS, paths: ctx.paths };
    });

  for (const mode of ["default", "continue"]) {
    it(`hands env and paths to a step that needs the producer, and not to one that does not (${mode})`, () => {
      quietRun([producer, consumer("declared", ["producer"]), consumer("undeclared", [])], { mode });
      assert.equal(seen.declared.cflags, "-DHASH=1");
      assert.deepEqual(seen.declared.paths, { producer: { bin: "/tmp/x/bin" } });
      assert.equal(seen.undeclared.cflags, undefined);
      assert.deepEqual(seen.undeclared.paths, {});
    });
  }

  it("never writes the producer's env into process.env", () => {
    const before = process.env.CGO_CFLAGS;
    quietRun([producer, consumer("declared", ["producer"])], { mode: "default" });
    assert.equal(process.env.CGO_CFLAGS, before);
  });

  it("fails loudly when release identity runs without build dcrelease's output", () => {
    const step = STEPS.find((s) => s.label === "release identity");
    assert.throws(
      () => step.run({ label: step.label, mode: "default", env: {}, paths: {}, header: () => {} }),
      /does not declare `build dcrelease` in needs/,
    );
  });
});

describe("ci:local --only", () => {
  const steps = () => [fake("a"), fake("b", ["a"]), fake("c", ["b"]), fake("d"), fake("e", ["a"])];

  it("runs exactly the step and its needs chain, in step order", () => {
    assert.deepEqual(chainOf(steps(), "c"), ["a", "b", "c"]);
    const r = quietRun(steps(), { mode: "only", only: "c" });
    assert.deepEqual(r.ran, ["a", "b", "c"]);
    assert.deepEqual(r.rows.map((x) => x.label), ["a", "b", "c"]);
    assert.match(r.out, /3 passed · 0 failed · 0 skipped · 0 not applicable of 3 steps/);
  });

  it("refuses a step that does not exist, listing the steps", () => {
    assert.throws(() => chainOf(steps(), "nope"), /no step "nope"; steps are:\n {2}a\n/);
  });

  it("chains the real steps: go host tests needs gusset engine, nothing else", () => {
    assert.deepEqual(chainOf(STEPS, "go host tests"), ["gusset engine", "go host tests"]);
    assert.deepEqual(chainOf(STEPS, "release identity (tag path)"), ["build dcrelease", "release identity (tag path)"]);
    assert.deepEqual(chainOf(STEPS, "test selector end-to-end"), ["test selector end-to-end"]);
  });
});

describe("ci:local exit codes", () => {
  it("is 1 if any step failed, else 2 if any was skipped, else 0; N/A never changes it", () => {
    assert.equal(exitCode([{ status: "PASS" }, { status: "N/A" }]), 0);
    assert.equal(exitCode([{ status: "PASS" }, { status: "SKIP" }, { status: "N/A" }]), 2);
    assert.equal(exitCode([{ status: "SKIP" }, { status: "FAIL" }]), 1);
    assert.equal(exitCode([]), 0);
  });

  it("returns them from a --continue run", () => {
    assert.equal(quietRun([fake("a"), fake("b", [], "na")], { mode: "continue" }).code, 0);
    assert.equal(quietRun([fake("a"), fake("b", [], "pass", { requires: () => "absent" })], { mode: "continue" }).code, 2);
    assert.equal(quietRun([fake("a", [], "fail"), fake("b", ["a"])], { mode: "continue" }).code, 1);
  });
});

describe("ci:local default mode", () => {
  it("stops at the first failure with that step's exit code, and prints no summary", () => {
    const r = quietRun([fake("a"), fake("b", [], "fail"), fake("c")], { mode: "default" });
    assert.equal(r.code, 3);
    assert.deepEqual(r.ran, ["a", "b"]);
    assert.doesNotMatch(r.out, /──|passed ·|SKIP/);
  });

  it("keeps a could-not-run stop as its exit code, not a SKIP", () => {
    const r = quietRun([fake("a", [], "cannot"), fake("b")], { mode: "default" });
    assert.equal(r.code, 2);
    assert.deepEqual(r.ran, ["a"]);
  });

  it("does not consult a step's requirement: the step runs and its own failure stands", () => {
    const r = quietRun([fake("e2e", [], "fail", { requires: () => "devmap not found" })], { mode: "default" });
    assert.deepEqual(r.ran, ["e2e"]);
    assert.equal(r.code, 3);
  });

  it("prints a step's header only when the step asks for it, as before", () => {
    const r = quietRun([{ label: "quiet", needs: [], run: () => {} }, fake("loud")], { mode: "default" });
    assert.deepEqual([...r.out.matchAll(/^== (.+) ==$/gm)].map((m) => m[1]), ["loud"]);
  });
});

describe("ci:local arguments", () => {
  it("parses the modes, and refuses anything else", () => {
    assert.deepEqual(parseArgs([]), { mode: "default" });
    assert.deepEqual(parseArgs(["--continue"]), { mode: "continue" });
    assert.deepEqual(parseArgs(["--only", "gofmt"]), { mode: "only", only: "gofmt" });
    assert.deepEqual(parseArgs(["--only=go host tests"]), { mode: "only", only: "go host tests" });
    assert.deepEqual(parseArgs(["--continue", "--only", "gofmt"]), { mode: "only", only: "gofmt" });
    assert.throws(() => parseArgs(["--only"]), /--only needs a step name/);
    assert.throws(() => parseArgs(["--only", "a", "--only", "b"]), /--only given twice/);
    assert.throws(() => parseArgs(["--keep-going"]), /unknown argument "--keep-going"/);
  });
});

describe("ci:local summary block", () => {
  const rows = [
    { label: "script tests", status: "PASS", ms: 12340, note: "" },
    { label: "test selector end-to-end", status: "FAIL", ms: 4000, note: "exit 1" },
    { label: "go host tests", status: "SKIP", ms: 0, note: "not run: needs gusset engine" },
    { label: "release identity (tag path)", status: "N/A", ms: 0, note: "not applicable: no local tag v0.2.5" },
  ];

  it("has GitPulse's ci:local shape, with N/A kept apart from SKIP", () => {
    const expected = [
      "",
      `── DevCouncil ci:local ${"─".repeat(37)}`,
      "",
      `  PASS  script tests${" ".repeat(30)}   12.3s`,
      `  FAIL  test selector end-to-end${" ".repeat(18)}    4.0s  (exit 1)`,
      `  SKIP  go host tests${" ".repeat(39)}(not run: needs gusset engine)`,
      `  N/A   release identity (tag path)${" ".repeat(25)}(not applicable: no local tag v0.2.5)`,
      "",
      "  1 passed · 1 failed · 1 skipped · 1 not applicable of 4 steps",
      "",
    ].join("\n");
    assert.equal(summarize("DevCouncil ci:local", rows), expected);
  });

  it("matches the row and totals patterns failset reads", () => {
    const lines = summarize("DevCouncil ci:local", rows).split("\n");
    assert.ok(lines.some((l) => /^ {2}(PASS|FAIL) {2}(.+?)\s+\d+\.\d+s(?: {2}\(.*\))?$/.test(l)));
    assert.ok(lines.some((l) => /^ {2}SKIP {2}(.+?)\s{2,}\(.*\)$/.test(l)));
    assert.ok(lines.some((l) => /^\s+\d+ passed · \d+ failed · \d+ skipped · \d+ not applicable of \d+ steps$/.test(l)));
  });

  it("prints a nested block indented, with no totals line", () => {
    const nested = summarize("rust/verify.sh", rows, { nested: true }).split("\n");
    assert.equal(nested[1], `  ── rust/verify.sh ${"─".repeat(42)}`);
    assert.equal(nested[3], `    PASS  script tests${" ".repeat(30)}   12.3s`);
    assert.ok(!nested.some((l) => /passed ·/.test(l)));
  });

  it("is printed by a --continue run, after the steps", () => {
    const r = quietRun([fake("a"), fake("b", [], "fail")], { mode: "continue" });
    assert.match(r.out, /── test gate ─+\n\n {2}PASS {2}a {45}1\.5s\n {2}FAIL {2}b {45}1\.5s {2}\(exit 3\)\n\n {2}1 passed · 1 failed · 0 skipped · 0 not applicable of 2 steps\n$/);
  });
});

describe("ci:local declared dependencies", () => {
  it("pins every edge, each to an earlier step", () => {
    const edges = Object.fromEntries(STEPS.filter((s) => s.needs.length > 0).map((s) => [s.label, s.needs]));
    assert.deepEqual(edges, {
      "release identity": ["build dcrelease"],
      "release identity (tag path)": ["build dcrelease"],
      // `go test ./...` includes ./cmd/devcouncil and ./gussetfn, which link libgusset.a.
      "go host tests": ["gusset engine"],
      "go orchestrator tests": ["gusset engine"],
      // Built from this checkout's source, so it links libgusset.a too.
      "spec gate": ["gusset engine"],
    });
    const labels = STEPS.map((s) => s.label);
    assert.equal(new Set(labels).size, labels.length);
  });

  it("gates every specs/NNN-* spec under --strict with the source-built CLI, never skipping", () => {
    const step = STEPS.find((s) => s.label === "spec gate");
    assert.ok(step, "ci:local has no `spec gate` step");
    const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
    assert.equal(step.command, "go");
    assert.equal(step.cwd, GO_MODULE);
    assert.deepEqual(step.args, ["run", "./cmd/devcouncil", "spec", "validate", "--all", "--strict", "--project-root", repoRoot]);
    // A spec gate that could not run must fail, not report N/A.
    assert.equal(step.requires, undefined);
  });

  it("asks for devmap before the selector's end-to-end tests", () => {
    const step = STEPS.find((s) => s.label === "test selector end-to-end");
    const empty = mkdtempSync(path.join(tmpdir(), "no-devmap-"));
    try {
      assert.match(step.requires({ PATH: empty }), /^devmap not found/);
      assert.equal(step.requires({ PATH: empty, MANVI_MAP_BINARY: "/set/but/missing" }), undefined);
      writeFileSync(path.join(empty, "devmap"), "#!/bin/sh\n", { mode: 0o755 });
      assert.equal(step.requires({ PATH: empty }), undefined);
      assert.equal(devmapAvailable({ PATH: `/nowhere${path.delimiter}${empty}` }), true);
      chmodSync(path.join(empty, "devmap"), 0o644);
      assert.equal(devmapAvailable({ PATH: empty }), false);
    } finally {
      rmSync(empty, { recursive: true, force: true });
    }
  });
});

describe("rust/verify.sh --continue", { skip: process.platform === "win32" }, () => {
  const script = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "rust", "verify.sh");
  const which = (name) => spawnSync("/bin/sh", ["-c", `command -v ${name}`], { encoding: "utf8" }).stdout.trim();
  const bash = which("bash");

  /**
   * verify.sh against a fake cargo, with a PATH holding only the tools it
   * needs: no `go`, so the host fixture fails unless DEVCOUNCIL_BIN is set.
   * The fake cargo logs every call and exits FAKE_CARGO_EXIT when its
   * arguments contain FAKE_CARGO_FAIL.
   */
  function verify(args, env = {}) {
    const dir = mkdtempSync(path.join(tmpdir(), "verify-continue-"));
    const bin = path.join(dir, "bin");
    mkdirSync(bin);
    for (const tool of ["dirname", "mktemp", "rm", "date", "perl", "gdate", "cat", "sh"]) {
      const at = which(tool);
      if (at) symlinkSync(at, path.join(bin, tool));
    }
    const log = path.join(dir, "cargo.log");
    writeFileSync(
      path.join(bin, "cargo"),
      '#!/bin/sh\necho "cargo $*" >> "$FAKE_LOG"\nif [ -n "${FAKE_CARGO_FAIL:-}" ]; then\n  case "$*" in *"$FAKE_CARGO_FAIL"*) exit "${FAKE_CARGO_EXIT:-1}" ;; esac\nfi\nexit 0\n',
      { mode: 0o755 },
    );
    const host = path.join(dir, "devcouncil");
    writeFileSync(host, "");
    try {
      const r = spawnSync(bash, [script, ...args], {
        encoding: "utf8",
        env: { PATH: bin, FAKE_LOG: log, HOME: dir, ...env, ...(env.DEVCOUNCIL_BIN === "host" ? { DEVCOUNCIL_BIN: host } : {}) },
      });
      let calls = [];
      try {
        calls = readFileSync(log, "utf8").trim().split("\n");
      } catch {
        // no cargo call
      }
      return { status: r.status, stdout: r.stdout, stderr: r.stderr, calls };
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  }

  /** The rows of verify.sh's block, as [status, label, note]. */
  const rowsOf = (stdout, indent = "  ") => {
    const re = new RegExp(`^${indent}(PASS|FAIL|SKIP|N/A) +(.+?) +(?:\\d+\\.\\ds)?(?: {2}\\((.*)\\))?$`);
    return stdout
      .split("\n")
      .map((l) => re.exec(l))
      .filter(Boolean)
      .map((m) => [m[1], m[2], m[3] ?? ""]);
  };

  it("keeps going past a failed step, runs cargo test with --no-fail-fast, and ends with the summary", () => {
    const r = verify(["--quick", "--continue"], { DEVCOUNCIL_BIN: "host", FAKE_CARGO_FAIL: "fmt" });
    assert.equal(r.status, 1, r.stdout + r.stderr);
    assert.ok(r.calls.includes("cargo test --locked --workspace --no-fail-fast"), r.calls.join("\n"));
    assert.deepEqual(rowsOf(r.stdout), [
      ["FAIL", "1/8 format", "exit 1"],
      ["PASS", "2/8 clippy (-D warnings)", ""],
      ["PASS", "3/8 tests — host fixture", ""],
      ["PASS", "3/8 tests", ""],
      ["N/A", "release worker panic recovery (executable, not test profile)", "not applicable: quick mode"],
      ["N/A", "4/8 determinism — two clean builds must produce identical graph digests", "not applicable: quick mode"],
      ["N/A", "5/8 self-build gates — DevCouncil repo, release", "not applicable: quick mode"],
      ["N/A", "6/8 growth gate — repeated builds must plateau", "not applicable: quick mode"],
      ["N/A", "7/8 incremental-vs-cold equivalence — a rebuild must not drift", "not applicable: quick mode"],
      ["N/A", "8/8 mutation testing (optional)", "not applicable: quick mode"],
    ]);
    assert.match(r.stdout, /^ {2}3 passed · 1 failed · 0 skipped · 6 not applicable of 10 steps$/m);
  });

  it("prints the same block ci-local's summarize() prints for those rows", () => {
    const r = verify(["--quick", "--continue"], { DEVCOUNCIL_BIN: "host", FAKE_CARGO_FAIL: "clippy" });
    const start = r.stdout.indexOf("\n── rust/verify.sh");
    assert.ok(start >= 0, r.stdout);
    const block = r.stdout.slice(start);
    const rows = block
      .split("\n")
      .map((l) => /^ {2}(PASS|FAIL|SKIP|N\/A) {1,2} (.+?) +(?:(\d+\.\d)s)?(?: {2}\((.*)\))?$/.exec(l))
      .filter(Boolean)
      .map((m) => ({ status: m[1], label: m[2], ms: m[3] ? Number(m[3]) * 1000 : 0, note: m[4] ?? "" }));
    assert.equal(rows.length, 10);
    assert.equal(block, `${summarize("rust/verify.sh", rows)}\n`);
  });

  it("SKIPs 3/8 tests, naming the host fixture, when the fixture failed; nested prints no totals", () => {
    const r = verify(["--quick", "--continue", "--nested"], {});
    assert.equal(r.status, 1, r.stdout + r.stderr);
    assert.match(r.stderr, /GATE FAIL: no `go` on PATH/);
    assert.ok(!r.calls.some((c) => c.startsWith("cargo test")), r.calls.join("\n"));
    const rows = rowsOf(r.stdout, "    ");
    assert.deepEqual(rows.slice(0, 4), [
      ["PASS", "1/8 format", ""],
      ["PASS", "2/8 clippy (-D warnings)", ""],
      ["FAIL", "3/8 tests — host fixture", "exit 1"],
      ["SKIP", "3/8 tests", "not run: needs 3/8 tests — host fixture"],
    ]);
    assert.match(r.stdout, /^ {2}── rust\/verify\.sh ─+$/m);
    assert.doesNotMatch(r.stdout, /passed ·/);
  });

  it("SKIPs steps 5-7 when step 4's release build failed, and still runs the steps after", () => {
    const r = verify(["--continue"], { DEVCOUNCIL_BIN: "host", FAKE_CARGO_FAIL: "devmap-cli" });
    assert.equal(r.status, 1, r.stdout + r.stderr);
    assert.ok(!r.calls.some((c) => c.includes("devmap-soak")), r.calls.join("\n"));
    assert.deepEqual(rowsOf(r.stdout).slice(4), [
      ["PASS", "release worker panic recovery (executable, not test profile)", ""],
      ["FAIL", "4/8 determinism — two clean builds must produce identical graph digests", "exit 1"],
      ["SKIP", "5/8 self-build gates — DevCouncil repo, release", "not run: needs 4/8 determinism — two clean builds must produce identical graph digests"],
      ["SKIP", "6/8 growth gate — repeated builds must plateau", "not run: needs 4/8 determinism — two clean builds must produce identical graph digests"],
      ["SKIP", "7/8 incremental-vs-cold equivalence — a rebuild must not drift", "not run: needs 4/8 determinism — two clean builds must produce identical graph digests"],
      ["N/A", "8/8 mutation testing (optional)", "not applicable: pass --mutants to run it"],
    ]);
  });

  // Characterization: the default mode, unchanged. These pass against the
  // script before --continue existed too.
  it("without --continue, still stops at the first failure with cargo's exit code", () => {
    const r = verify(["--quick"], { DEVCOUNCIL_BIN: "host", FAKE_CARGO_FAIL: "clippy", FAKE_CARGO_EXIT: "101" });
    assert.equal(r.status, 101);
    assert.deepEqual(r.calls, ["cargo fmt --all -- --check", "cargo clippy --locked --workspace --all-targets -- -D warnings"]);
    assert.doesNotMatch(r.stdout, /host fixture|──/);
  });

  it("without --continue, runs cargo test without --no-fail-fast and ends as before", () => {
    const r = verify(["--quick"], { DEVCOUNCIL_BIN: "host" });
    assert.equal(r.status, 0, r.stdout + r.stderr);
    assert.ok(r.calls.includes("cargo test --locked --workspace"), r.calls.join("\n"));
    assert.ok(!r.calls.some((c) => c.includes("--no-fail-fast")));
    assert.match(r.stdout, /QUICK GATES GREEN; the full gate suite was NOT RUN/);
    assert.doesNotMatch(r.stdout, /──/);
  });

  // 2026-11-17: macOS /bin/bash 3.2 reads the bytes of "$rule─" as part of the
  // variable name, so summary() died under `set -u`, and the EXIT trap's own
  // status replaced the crash's. The base gate then printed "PASS rust verify
  // --quick" and "0 failed" with two failing test targets. This harness gives
  // verify.sh no LANG, so it ran in the C locale and never saw any of it.
  it("prints its summary and exits 1 when a step failed, in a UTF-8 locale", () => {
    const r = verify(["--quick", "--continue"], { DEVCOUNCIL_BIN: "host", FAKE_CARGO_FAIL: "fmt", LANG: "en_US.UTF-8" });
    assert.doesNotMatch(r.stderr, /unbound variable/, r.stderr);
    assert.equal(r.status, 1, r.stdout + r.stderr);
    assert.match(r.stdout, /^ {2}3 passed · 1 failed · 0 skipped · 6 not applicable of 10 steps$/m);
  });

  it("never reads a bare variable followed by a multibyte character (bash 3.2 takes its bytes as part of the name)", () => {
    const bad = readFileSync(script, "utf8")
      .split("\n")
      .map((line, i) => [i + 1, line])
      .filter(([, line]) => /\$[A-Za-z_][A-Za-z0-9_]*[^\x00-\x7F]/.test(line));
    assert.deepEqual(bad, [], "write ${name} before a non-ASCII character");
  });

  it("keeps the exit status of the run when its EXIT trap cleans up", () => {
    const trap = readFileSync(script, "utf8")
      .split("\n")
      .find((line) => /^trap .* EXIT$/.test(line));
    assert.ok(trap, "verify.sh installs no EXIT trap");
    assert.match(trap, /exit "\$/, `an EXIT trap that does not re-exit with the incoming status replaces it with its own: ${trap}`);
  });
});

// 2026-11-17: the merge-train base gate printed "PASS rust verify --quick" and
// "0 failed" beside "3/8 tests failed (exit 101)". verify.sh's summary() had
// died and its EXIT trap had turned the crash into exit 0, and ci-local, which
// read only the exit status, believed it. A nested gate's exit 0 is a claim;
// these tests run the verify step against fake scripts that make it falsely.
describe("ci:local does not believe a nested gate's exit status alone", { skip: process.platform === "win32" }, () => {
  const BLOCK = [
    "",
    "  ── rust/verify.sh " + "─".repeat(42),
    "",
    "    PASS  1/8 format                                   0.1s",
    "    PASS  3/8 tests                                    2.0s",
    "    N/A   4/8 determinism                  (not applicable: quick mode)",
    "",
    "",
  ].join("\n");

  /** Run the verify step against a fake verify.sh whose body is `body`; returns the row and the exit code. */
  function runFake(body, mode) {
    const dir = mkdtempSync(path.join(tmpdir(), "ci-local-fake-verify-"));
    try {
      writeFileSync(path.join(dir, "verify.sh"), body, { mode: 0o755 });
      const lines = [];
      const res = runSteps([rustVerifyStep(dir)], { mode, gate: "t", log: (s) => lines.push(s) });
      return { row: res.rows[0], code: res.code, out: lines.join("\n") };
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  }
  const script = (...lines) => ["#!/usr/bin/env bash", ...lines, ""].join("\n");
  const printBlock = (block) => `printf '%b' '${block.replace(/\n/g, "\\n")}'`;

  it("accepts a complete block and exit 0 (control: the fake is a faithful verify.sh)", () => {
    const r = runFake(script("echo steps", printBlock(BLOCK)), "continue");
    assert.equal(r.row.status, "PASS", r.out);
    assert.equal(r.code, 0);
  });

  it("FAILs, never PASSes, when verify.sh exits 0 after its steps but printed no summary block", () => {
    const r = runFake(script("echo '== 1/8 format =='", "echo 'test t ... ok'", "exit 0"), "continue");
    assert.equal(r.row.status, "FAIL", r.out);
    assert.match(r.row.note, /no summary block/);
    assert.equal(r.code, 1);
  });

  it("FAILs on a malformed block: a header with no rows, or rows with no terminating blank line", () => {
    const headerOnly = BLOCK.split("\n").slice(0, 3).join("\n") + "\n";
    assert.equal(runFake(script(printBlock(headerOnly)), "continue").row.status, "FAIL");
    const cut = BLOCK.trimEnd();
    assert.equal(runFake(script(printBlock(cut)), "continue").row.status, "FAIL");
    const junk = BLOCK.replace("    PASS  3/8 tests", "    garbage 3/8 tests");
    assert.equal(runFake(script(printBlock(junk)), "continue").row.status, "FAIL");
  });

  it("FAILs when exit 0 sits beside a FAIL row", () => {
    const r = runFake(script(printBlock(BLOCK.replace("    PASS  3/8 tests", "    FAIL  3/8 tests"))), "continue");
    assert.equal(r.row.status, "FAIL", r.out);
    assert.match(r.row.note, /FAIL row/);
  });

  it("FAILs when exit 0 sits beside a SKIP row (verify.sh exits 2 whenever a row is SKIP)", () => {
    const r = runFake(script(printBlock(BLOCK.replace("    PASS  3/8 tests", "    SKIP  3/8 tests"))), "continue");
    assert.equal(r.row.status, "FAIL", r.out);
    assert.match(r.row.note, /SKIP row/);
  });

  it("FAILs when the log holds libtest FAILED lines but every row is PASS", () => {
    const r = runFake(
      script("echo 'test store::tests::roundtrip ... FAILED'", "echo 'test result: FAILED. 7 passed; 1 failed; 0 ignored'", printBlock(BLOCK)),
      "continue",
    );
    assert.equal(r.row.status, "FAIL", r.out);
    assert.match(r.row.note, /libtest/);
  });

  it("in the default mode requires the QUICK GATES GREEN line, not just exit 0", () => {
    assert.equal(runFake(script("echo steps", "exit 0"), "default").code, 1);
    const ok = runFake(script("printf '\\nQUICK GATES GREEN; the full gate suite was NOT RUN\\n'"), "default");
    assert.equal(ok.code, 0, ok.out);
    const failed = runFake(script("echo 'test a ... FAILED'", "printf '\\nQUICK GATES GREEN; the full gate suite was NOT RUN\\n'"), "default");
    assert.equal(failed.code, 1);
  });

  it("still reports a non-zero exit as its own FAIL with that code", () => {
    const r = runFake(script("echo boom", "exit 101"), "continue");
    assert.equal(r.row.status, "FAIL");
    assert.equal(r.row.note, "exit 101");
    assert.equal(runFake(script("exit 101"), "default").code, 101);
  });
});

describe("libtestFailures and nestedGateProblem", () => {
  const block = ["", "  ── rust/verify.sh " + "─".repeat(42), "", "    PASS  1/8 format    0.1s", "", ""].join("\n");

  it("reads cargo's FAILED lines, through ANSI colour, and nothing else", () => {
    const log = ["test a::ok ... ok", "test a::bad ... FAILED", "\u001b[31mtest result: FAILED. 1 passed; 1 failed\u001b[0m", "test result: ok. 3 passed", "failures:", "    a::bad"].join("\n");
    assert.deepEqual(libtestFailures(log), ["test a::bad ... FAILED", "test result: FAILED. 1 passed; 1 failed"]);
    assert.deepEqual(libtestFailures("test x ... ignored\n3/8 tests failed (exit 101)"), []);
  });

  it("believes a complete nested block, and a default-mode run that reached its green line", () => {
    assert.equal(nestedGateProblem(block, { nested: true }), undefined);
    assert.equal(nestedGateProblem("QUICK GATES GREEN; the full gate suite was NOT RUN\n", { nested: false }), undefined);
  });

  it("does not believe an empty log in either mode", () => {
    assert.match(nestedGateProblem("", { nested: true }), /no summary block/);
    assert.match(nestedGateProblem("", { nested: false }), /never printed/);
  });
});

// The 2026-11-17 crash needed bash 3.2 (/bin/bash on macOS) and a UTF-8
// locale; the harness above runs the first bash on PATH with no LANG. This
// matrix runs the gate under every distinct bash among PATH's and /bin/bash,
// with and without LANG=en_US.UTF-8.
//
// CI (.github/workflows/ci.yml, `npm run test:scripts` on ubuntu-latest) has
// one bash (5.x) and may have no en_US.UTF-8 locale installed, so there the
// matrix collapses to the default row and the bash 3.2 rows only run on a Mac.
// Covering them in CI means adding a macos job that runs `npm run test:scripts`
// (macos runners ship /bin/bash 3.2.57): a workflow change this test file does
// not make.
describe("rust/verify.sh across bash and locale", { skip: process.platform === "win32" }, () => {
  const script = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "rust", "verify.sh");
  const which = (name) => spawnSync("/bin/sh", ["-c", `command -v ${name}`], { encoding: "utf8" }).stdout.trim();
  const real = (p) => spawnSync("/bin/sh", ["-c", `cd "$(dirname "${p}")" && pwd -P`], { encoding: "utf8" }).stdout.trim() + "/" + path.basename(p);
  const shells = [...new Set([which("bash"), "/bin/bash"].filter((p) => p && spawnSync(p, ["-c", "exit 0"]).status === 0).map(real))];
  const locales = [undefined, "en_US.UTF-8"];

  for (const shell of shells) {
    for (const lang of locales) {
      it(`prints its block and exits 1 for a failed step: ${shell}, LANG=${lang ?? "(unset)"}`, () => {
        const dir = mkdtempSync(path.join(tmpdir(), "verify-matrix-"));
        try {
          const bin = path.join(dir, "bin");
          mkdirSync(bin);
          for (const tool of ["dirname", "mktemp", "rm", "date", "perl", "gdate", "cat", "sh"]) {
            const at = which(tool);
            if (at) symlinkSync(at, path.join(bin, tool));
          }
          writeFileSync(path.join(bin, "cargo"), '#!/bin/sh\ncase "$*" in *fmt*) exit 1 ;; esac\nexit 0\n', { mode: 0o755 });
          const host = path.join(dir, "devcouncil");
          writeFileSync(host, "");
          const r = spawnSync(shell, [script, "--quick", "--continue", "--nested"], {
            encoding: "utf8",
            env: { PATH: bin, HOME: dir, DEVCOUNCIL_BIN: host, ...(lang ? { LANG: lang, LC_ALL: lang } : {}) },
          });
          assert.doesNotMatch(r.stderr, /unbound variable/, r.stderr);
          assert.equal(r.status, 1, r.stdout + r.stderr);
          assert.match(nestedGateProblem(r.stdout, { nested: true }), /FAIL row: FAIL +1\/8 format/);
          assert.match(r.stdout, /^ {2}── rust\/verify\.sh ─+$/m);
          // The real script's rows are exactly the steps it declares.
          const labels = expectedRowLabels(readFileSync(script, "utf8"));
          assert.equal(labels.length, 10);
          assert.doesNotMatch(nestedGateProblem(r.stdout, { nested: true, labels }), /row\(s\) but the script declares/);
        } finally {
          rmSync(dir, { recursive: true, force: true });
        }
      });
    }
  }
});

// Attack round (ft-7fdb1ab4 lane C): adversarial logs for the verify step's
// verdict. Each case runs the step against a fake verify.sh.
describe("ci:local nested-gate verdict, adversarial logs", { skip: process.platform === "win32" }, () => {
  const rule = "─".repeat(42);
  const ROWS = ["    PASS  1/8 format                                   0.1s", "    PASS  3/8 tests                                    2.0s"];
  const block = (rows = ROWS) => ["", "  ── rust/verify.sh " + rule, "", ...rows, "", ""].join("\n");
  const q = (s) => "'" + s.replace(/'/g, "'\\''") + "'";
  const emit = (s) => "printf '%s' " + q(s);
  function runFake(body, mode = "continue") {
    const dir = mkdtempSync(path.join(tmpdir(), "ci-local-adv-"));
    try {
      writeFileSync(path.join(dir, "verify.sh"), ["#!/usr/bin/env bash", ...body, ""].join("\n"), { mode: 0o755 });
      const lines = [];
      const res = runSteps([rustVerifyStep(dir)], { mode, gate: "t", log: (s) => lines.push(s) });
      return { row: res.rows[0], code: res.code, out: lines.join("\n") };
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  }



  // Line endings are normalised (CRLF and bare CR) before anything is matched,
  // so a gate that ends its lines with \r is read the way one ending in \n is.
  it("reads a CRLF summary block as the block it is, and still judges its rows", () => {
    assert.equal(runFake([emit(block().replace(/\n/g, "\r\n"))]).row.status, "PASS");
    assert.equal(runFake([emit(block(["    FAIL  3/8 tests    2.0s"]).replace(/\n/g, "\r\n"))]).row.status, "FAIL");
  });

  it("BREAK: FAILs when an early PASS block is followed by a later FAIL block", () => {
    const later = block(["    FAIL  3/8 tests                                    2.0s"]);
    const r = runFake([emit(block()), "echo more steps", emit(later)]);
    assert.equal(r.row.status, "FAIL", r.out);
  });

  it("FAILs on two summary blocks even when both are clean: verify.sh prints exactly one", () => {
    const r = runFake([emit(block()), emit(block())]);
    assert.equal(r.row.status, "FAIL", r.out);
    assert.match(r.row.note, /2 summary blocks/);
  });

  it("BREAK: FAILs on a libtest FAILED line that carries a carriage return", () => {
    const r = runFake([emit("test a::b ... FAILED\r\n"), emit(block())]);
    assert.equal(r.row.status, "FAIL", r.out);
    assert.equal(runFake([emit("test a::b ... FAILED\r"), emit(block())]).row.status, "FAIL");
  });

  it("FAILs on cargo's compile and target failures beside exit 0 and a valid block", () => {
    for (const line of [
      "error: could not compile `dc-verify` (test \"json_contract\") due to 1 previous error",
      "error: 2 targets failed:",
      "error: 1 target failed:",
      "error: test failed, to rerun pass `-p dc-verify --lib`",
    ]) {
      const r = runFake([emit(line + "\n"), emit(block())]);
      assert.equal(r.row.status, "FAIL", line + "\n" + r.out);
    }
  });

  // The step list is knowable: verify.sh declares one L_NAME="label" per step.
  it("compares the block's rows with the steps the script declares", () => {
    const decl = ['L_A="1/2 alpha"', 'L_B="2/2 beta"'];
    const rows = (...r) => block(r);
    const good = rows("    PASS  1/2 alpha        0.1s", "    PASS  2/2 beta         0.1s");
    assert.equal(runFake([...decl, emit(good)]).row.status, "PASS");
    const missing = rows("    PASS  1/2 alpha        0.1s");
    const r = runFake([...decl, emit(missing)]);
    assert.equal(r.row.status, "FAIL", r.out);
    assert.match(r.row.note, /1 row\(s\) but the script declares 2 steps/);
    const wrong = rows("    PASS  1/2 alpha        0.1s", "    PASS  2/2 gamma        0.1s");
    assert.match(runFake([...decl, emit(wrong)]).row.note, /row 2 is not "2\/2 beta"/);
  });

  it("reads ANSI colour around the block, its FAIL row and the libtest lines", () => {
    const esc = "\u001b[1m";
    const coloured = block().replace("  ── rust", "  " + esc + "── rust").replace("    PASS  1/8", "    \u001b[32mPASS\u001b[0m  1/8");
    assert.equal(runFake([emit(coloured)]).row.status, "PASS");
    const failRow = block(["    \u001b[31mFAIL\u001b[0m  3/8 tests    2.0s"]);
    assert.equal(runFake([emit(failRow)]).row.status, "FAIL");
    assert.equal(runFake([emit("\u001b[31mtest a ... FAILED\u001b[0m\n"), emit(block())]).row.status, "FAIL");
  });

  it("believes a block written to stderr only (stderr is merged into the captured log)", () => {
    assert.equal(runFake([emit(block()) + " >&2"]).row.status, "PASS");
  });

  it("FAILs a block valid in itself when a libtest FAILED line follows it", () => {
    assert.equal(runFake([emit(block()), "echo 'test late::one ... FAILED'"]).row.status, "FAIL");
  });

  it("does not FAIL a passing test whose name contains FAILED", () => {
    const r = runFake([emit("test it_reports_FAILED_rows ... ok\ntest result: ok. 1 passed; 0 failed\n"), emit(block())]);
    assert.equal(r.row.status, "PASS", r.out);
  });

  it("reads unicode and box-drawing in step names and notes", () => {
    const rows = ["    PASS  3/8 tests — host fixture │ ✓                  0.0s", "    N/A   4/8 determinism — ünïcödé  (not applicable: quick mode)"];
    assert.equal(runFake([emit(block(rows))]).row.status, "PASS");
  });

  it("FAILs a signal-killed nested gate", () => {
    const r = runFake([emit(block()), "kill -9 $$"]);
    assert.equal(r.row.status, "FAIL", r.out);
    assert.match(r.row.note, /exit 137/);
  });

  it("FAILs, closed, when a stderr line lands inside the block", () => {
    const parts = block().split("\n");
    const r = runFake([emit(parts.slice(0, 4).join("\n") + "\n"), "echo 'warning: noise' >&2", emit(parts.slice(4).join("\n"))]);
    assert.equal(r.row.status, "FAIL", r.out);
  });

  it("scans a 3 MiB single-line log", () => {
    const line = "test " + "a ".repeat(1_500_000);
    assert.equal(nestedGateProblem(line + "\n" + block(), { nested: true }), undefined);
    assert.deepEqual(libtestFailures(line + " ... FAILED"), [line + " ... FAILED"]);
  });

  it("FAILs when the default-mode green line sits beside a libtest FAILED line", () => {
    const r = runFake(["echo 'test result: FAILED. 0 passed; 1 failed'", "printf '\\nQUICK GATES GREEN; the full gate suite was NOT RUN\\n'"], "default");
    assert.equal(r.code, 1, r.out);
  });
});

// 2026-11-19: on /bin/bash 3.2.57 a top-level `set -u` abort enters the EXIT
// trap with $?=0, so a trap that only re-exits with its entry status exits 0
// over a crash. verify.sh sets COMPLETED=1 as the last statement of a run that
// finished, and its trap turns a 0 without it into 1. This drives the real
// script into a top-level set -u abort without cargo: a `perl` that prints a
// word instead of a clock reading makes the step timer's arithmetic read an
// unbound variable (`date` fails, so now_ns falls through to perl).
describe("rust/verify.sh completion flag", { skip: process.platform === "win32" }, () => {
  const script = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "rust", "verify.sh");
  const which = (name) => spawnSync("/bin/sh", ["-c", `command -v ${name}`], { encoding: "utf8" }).stdout.trim();
  const shells = [...new Set(["/bin/bash", which("bash")].filter((p) => p && spawnSync(p, ["-c", "exit 0"]).status === 0))];

  for (const shell of shells) {
    it(`exits non-zero when a top-level set -u abort enters the EXIT trap: ${shell}`, () => {
      const dir = mkdtempSync(path.join(tmpdir(), "verify-abort-"));
      try {
        const bin = path.join(dir, "bin");
        mkdirSync(bin);
        for (const tool of ["dirname", "mktemp", "rm", "cat", "sh"]) symlinkSync(which(tool), path.join(bin, tool));
        writeFileSync(path.join(bin, "date"), "#!/bin/sh\nexit 1\n", { mode: 0o755 });
        writeFileSync(path.join(bin, "perl"), "#!/bin/sh\necho bogus\n", { mode: 0o755 });
        writeFileSync(path.join(bin, "cargo"), "#!/bin/sh\nexit 0\n", { mode: 0o755 });
        const host = path.join(dir, "devcouncil");
        writeFileSync(host, "");
        const r = spawnSync(shell, [script, "--quick", "--continue"], {
          encoding: "utf8",
          env: { PATH: bin, HOME: dir, DEVCOUNCIL_BIN: host },
        });
        assert.match(r.stderr, /unbound variable/, "the fixture must reach a set -u abort: " + r.stderr);
        assert.notEqual(r.status, 0, r.stdout + r.stderr);
        assert.doesNotMatch(r.stdout, /passed ·/);
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    });
  }

  it("sets COMPLETED=1 before each successful exit, and the trap fails a bare 0", () => {
    const src = readFileSync(script, "utf8");
    assert.match(src, /^COMPLETED=0$/m);
    assert.match(src, /\[ "\$COMPLETED" -ne 1 \]/);
  });
});

// The false-red guard: the real verify.sh, green, must pass every check the
// runner applies to a nested gate's output (its block, its declared steps),
// in both modes and under each bash and locale. Without this the checks could
// reject the script's own good output and nothing here would say so.
describe("ci:local believes the real rust/verify.sh when it is green", { skip: process.platform === "win32" }, () => {
  const realScript = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "rust", "verify.sh");
  const which = (name) => spawnSync("/bin/sh", ["-c", `command -v ${name}`], { encoding: "utf8" }).stdout.trim();
  const shells = [...new Set(["/bin/bash", which("bash")].filter((p) => p && spawnSync(p, ["-c", "exit 0"]).status === 0))];

  for (const shell of shells) {
    for (const lang of [undefined, "en_US.UTF-8"]) {
      for (const mode of ["default", "continue"]) {
        it(`${mode} mode: PASS: ${shell}, LANG=${lang ?? "(unset)"}`, () => {
          const dir = mkdtempSync(path.join(tmpdir(), "verify-green-"));
          try {
            const bin = path.join(dir, "bin");
            mkdirSync(bin);
            for (const tool of ["dirname", "mktemp", "rm", "date", "perl", "gdate", "cat", "sh", "tee"]) {
              const at = which(tool);
              if (at) symlinkSync(at, path.join(bin, tool));
            }
            symlinkSync(shell, path.join(bin, "bash"));
            writeFileSync(path.join(bin, "cargo"), "#!/bin/sh\nexit 0\n", { mode: 0o755 });
            const rust = path.join(dir, "rust");
            mkdirSync(rust);
            writeFileSync(path.join(rust, "verify.sh"), readFileSync(realScript), { mode: 0o755 });
            const host = path.join(dir, "devcouncil");
            writeFileSync(host, "");
            const env = { PATH: bin, HOME: dir, DEVCOUNCIL_BIN: host, ...(lang ? { LANG: lang, LC_ALL: lang } : {}) };
            const lines = [];
            const res = runSteps([rustVerifyStep(rust)], { mode, gate: "t", baseEnv: env, log: (s) => lines.push(s) });
            assert.equal(res.rows[0].status, "PASS", res.rows[0].note + "\n" + lines.join("\n"));
            assert.equal(res.code, 0);
          } finally {
            rmSync(dir, { recursive: true, force: true });
          }
        });
      }
    }
  }

  it("the real script's own green nested output has no problem, with its declared labels", () => {
    const labels = expectedRowLabels(readFileSync(realScript, "utf8"));
    assert.equal(labels.length, 10);
    const dir = mkdtempSync(path.join(tmpdir(), "verify-green-out-"));
    try {
      const bin = path.join(dir, "bin");
      mkdirSync(bin);
      for (const tool of ["dirname", "mktemp", "rm", "date", "perl", "gdate", "cat", "sh"]) {
        const at = which(tool);
        if (at) symlinkSync(at, path.join(bin, tool));
      }
      writeFileSync(path.join(bin, "cargo"), "#!/bin/sh\nexit 0\n", { mode: 0o755 });
      const host = path.join(dir, "devcouncil");
      writeFileSync(host, "");
      const r = spawnSync("/bin/bash", [realScript, "--quick", "--continue", "--nested"], { encoding: "utf8", env: { PATH: bin, HOME: dir, DEVCOUNCIL_BIN: host } });
      assert.equal(r.status, 0, r.stdout + r.stderr);
      assert.equal(nestedGateProblem(r.stdout + r.stderr, { nested: true, labels }), undefined);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
