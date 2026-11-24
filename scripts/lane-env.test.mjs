import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import {
  copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { describe, it, after } from "node:test";
import { fileURLToPath } from "node:url";
import { engineInputs, stampedBuild, writeStamp } from "./gusset-engine-stamp.mjs";
import { chooseEngine, laneGoWork, locate, plan } from "./lane-env.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const GIT_ENV = {
  ...process.env,
  GIT_CONFIG_GLOBAL: "/dev/null",
  GIT_CONFIG_SYSTEM: "/dev/null",
  GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@example.invalid",
  GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@example.invalid",
};
const roots = [];
after(() => roots.forEach((r) => rmSync(r, { recursive: true, force: true })));

function git(cwd, ...args) {
  const r = spawnSync("git", ["-C", cwd, ...args], { encoding: "utf8", env: GIT_ENV });
  assert.equal(r.status, 0, `git ${args.join(" ")}: ${r.stderr}`);
  return r.stdout.trim();
}
const put = (file, text) => { mkdirSync(path.dirname(file), { recursive: true }); writeFileSync(file, text); };

/** A gusset repo, a main DevCouncil checkout with an engine archive, and one linked lane. */
function fixture({ archive = true, stamp = true } = {}) {
  const top = realpathSync(mkdtempSync(path.join(tmpdir(), "lane-env-")));
  roots.push(top);
  const gusset = path.join(top, "gusset");
  put(path.join(gusset, "go.mod"), "module github.com/bharathvbcr/gusset\n");
  git(top, "init", "-q", gusset);
  git(gusset, "add", "."); git(gusset, "commit", "-qm", "g");
  const pin = git(gusset, "rev-parse", "HEAD");
  const main = path.join(top, "DevCouncil");
  git(top, "init", "-q", main);
  put(path.join(main, ".gitignore"), "target/\n.gitpulse/\n");
  put(path.join(main, "go.work"), "go 1.27.4\n\nuse (\n\t./backend/go_orchestrator\n\t./tools/accuracy\n)\n");
  put(path.join(main, "backend/go_orchestrator/go.mod"), "module x\n");
  put(path.join(main, "tools/accuracy/go.mod"), "module y\n");
  put(path.join(main, ".github/actions/setup-gusset/action.yml"), `        ref: ${pin}\n`);
  put(path.join(main, "rust/gusset-engine/Cargo.toml"),
    '[workspace]\n\n[dependencies]\ngusset_core = { package = "gusset", path = "../../../gusset/crates/gusset" }\ndc-glob = { path = "../dc-glob" }\n');
  put(path.join(main, "rust/gusset-engine/Cargo.lock"), "# lock\n");
  put(path.join(main, "rust/dc-glob/Cargo.toml"), "[package]\n");
  put(path.join(main, "rust/dc-glob/src/lib.rs"), "// glob\n");
  put(path.join(main, "rust-toolchain.toml"), "[toolchain]\nchannel = \"1.100\"\n");
  put(path.join(main, "rust/gusset-engine/src/lib.rs"), "// engine\n");
  mkdirSync(path.join(main, "scripts"), { recursive: true });
  for (const f of ["lane-env.mjs", "ci-local-checks.mjs", "gusset-engine-stamp.mjs"]) copyFileSync(path.join(HERE, f), path.join(main, "scripts", f));
  git(main, "add", "."); git(main, "commit", "-qm", "m");
  if (archive) {
    const dir = path.join(main, "rust/gusset-engine/target/release");
    put(path.join(dir, "libgusset.a"), "main-archive");
    if (stamp) writeStamp(main, gusset, dir);
  }
  const lane = path.join(main, ".gitpulse/worktrees/lane");
  git(main, "worktree", "add", "-q", "-b", "lane", lane);
  return { top, gusset, pin, main, lane, scripts: (root) => path.join(root, "scripts") };
}

describe("locate", () => {
  const f = fixture();
  it("main checkout at the top and in a subdirectory is not linked (git-common-dir comes back relative)", () => {
    for (const cwd of [f.main, path.join(f.main, "backend/go_orchestrator")]) {
      const loc = locate(cwd, f.scripts(f.main));
      assert.equal(loc.linked, false, cwd);
      assert.equal(loc.root, f.main);
    }
  });
  it("a linked worktree at the top and in a subdirectory is linked, root is the lane, main is found", () => {
    for (const cwd of [f.lane, path.join(f.lane, "backend/go_orchestrator")]) {
      const loc = locate(cwd, f.scripts(f.lane));
      assert.equal(loc.linked, true, cwd);
      assert.equal(loc.root, f.lane);
      assert.equal(loc.mainRoot, f.main);
    }
  });
  it("resolves a symlinked cwd", () => {
    const link = path.join(f.top, "link");
    symlinkSync(path.join(f.lane, "backend"), link);
    const loc = locate(link, f.scripts(f.lane));
    assert.equal(loc.linked, true);
    assert.equal(loc.root, f.lane);
  });
  it("refuses a cwd outside the script's checkout", () => {
    assert.throws(() => locate(f.main, f.scripts(f.lane)), /outside the checkout/);
  });
});

describe("laneGoWork", () => {
  it("is the tracked go.work at absolute paths plus the gusset replace", () => {
    const f = fixture();
    assert.equal(
      laneGoWork(f.lane, f.gusset),
      `go 1.27.4\n\nuse (\n\t${f.lane}/backend/go_orchestrator\n\t${f.lane}/tools/accuracy\n)\n\nreplace github.com/bharathvbcr/gusset => ${f.gusset}\n`,
    );
  });
  it("refuses a path with whitespace by name", () => {
    const f = fixture();
    assert.throws(() => laneGoWork(f.lane, "/a b/gusset"), /gusset checkout .* whitespace/);
  });
});

describe("engine choice by input stamp", () => {
  const opts = (f, extra = {}) => ({ scriptDir: f.scripts(f.lane), env: { ...process.env, CGO_LDFLAGS: "", ...extra } });
  const mainDir = (f) => path.join(f.main, "rust/gusset-engine/target/release");
  const laneDir = (f) => path.join(f.lane, "rust/gusset-engine/target/release");
  const edit = (f, rel, text = "// edited\n") => put(path.join(f.lane, rel), text);
  const commit = (f) => { git(f.lane, "add", "."); git(f.lane, "commit", "-qm", "edit"); };
  const refused = (f, re, extra) => assert.throws(() => plan(f.lane, opts(f, extra)), re);

  it("main checkout: no environment at all", () => {
    const f = fixture();
    assert.deepEqual(plan(f.main, { scriptDir: f.scripts(f.main) }).env, {});
  });
  it("a matching stamp is accepted, and the hash covers that archive", () => {
    const f = fixture();
    const p = plan(f.lane, opts(f));
    assert.equal(p.engine.dir, mainDir(f));
    assert.match(p.env.CGO_LDFLAGS, new RegExp(`^-L${mainDir(f)}`));
    assert.equal(p.env.GOWORK, path.join(f.main, ".git/worktrees/lane/devcouncil-lane/go.work"));
    const h = (x) => /SHA256=(\w+)/.exec(x.env.CGO_CFLAGS)[1];
    const first = h(p);
    put(path.join(mainDir(f), "libgusset.a"), "rebuilt");
    assert.notEqual(h(plan(f.lane, opts(f))), first);
  });
  it("the lane's own matching archive is preferred over main's", () => {
    const f = fixture();
    put(path.join(laneDir(f), "libgusset.a"), "lane-archive");
    writeStamp(f.lane, f.gusset, laneDir(f));
    assert.equal(plan(f.lane, opts(f)).engine.dir, laneDir(f));
  });
  it("an engine-adjacent path dependency (dc-glob) changed and committed in the lane is refused", () => {
    const f = fixture();
    edit(f, "rust/dc-glob/src/lib.rs"); commit(f);
    refused(f, /no libgusset\.a matches.*rust\/dc-glob differ.*cgo-env\.sh.*ci:local -- --only "gusset engine"/s);
  });
  it("dc-glob changed but uncommitted is refused", () => {
    const f = fixture();
    edit(f, "rust/dc-glob/src/lib.rs");
    refused(f, /uncommitted changes.*rust\/dc-glob/s);
  });
  it("an untracked engine file is refused even with status.showUntrackedFiles=no", () => {
    const f = fixture();
    git(f.main, "config", "status.showUntrackedFiles", "no");
    edit(f, "rust/gusset-engine/src/new.rs", "// new\n");
    assert.equal(git(f.lane, "status", "--porcelain", "--", "rust/gusset-engine"), "", "the repo config hides it from plain status");
    refused(f, /uncommitted changes.*new\.rs/s);
  });
  it("a lane's own archive gone stale (inputs changed after the stamp) is refused", () => {
    const f = fixture();
    put(path.join(laneDir(f), "libgusset.a"), "lane-archive");
    writeStamp(f.lane, f.gusset, laneDir(f));
    edit(f, "rust/gusset-engine/src/lib.rs"); commit(f);
    refused(f, /the lane's own archive: .*other inputs.*main's archive: .*other inputs/s);
  });
  it("an archive built from a dirty tree is never reused, even after the changes are committed", () => {
    const f = fixture();
    put(path.join(laneDir(f), "libgusset.a"), "lane-archive");
    edit(f, "rust/gusset-engine/src/lib.rs");
    assert.equal(writeStamp(f.lane, f.gusset, laneDir(f)).dirty, true);
    commit(f);
    refused(f, /the lane's own archive: .*built from uncommitted sources/s);
  });
  it("an unstamped archive is refused with 'rebuild to stamp it'", () => {
    const f = fixture({ stamp: false });
    refused(f, /main's archive: .*no stamp.*rebuild to stamp it/s);
  });
  it("fails when there is no archive anywhere", () => {
    const f = fixture({ archive: false });
    refused(f, /no libgusset\.a matches.*none in .*ci:local/s);
  });
  it("a stale CGO_LDFLAGS -L to a different libgusset.a (an earlier eval) is refused", () => {
    const f = fixture();
    const stale = path.join(f.top, "stale");
    put(path.join(stale, "libgusset.a"), "stale");
    refused(f, /already holds -L.*stale/, { CGO_LDFLAGS: `-L${stale}` });
  });
  it("CGO_LDFLAGS naming the chosen archive, or an unrelated -L, is fine and kept", () => {
    const f = fixture();
    assert.equal(plan(f.lane, opts(f, { CGO_LDFLAGS: `-L${mainDir(f)}` })).env.CGO_LDFLAGS, `-L${mainDir(f)}`);
    const other = path.join(f.top, "other-libs");
    mkdirSync(other);
    assert.equal(plan(f.lane, opts(f, { CGO_LDFLAGS: `-L${other}` })).env.CGO_LDFLAGS, `-L${mainDir(f)} -L${other}`);
  });
  it("--engine-dir / DEVCOUNCIL_GUSSET_ENGINE_DIR bypass the stamp with a warning, even when dirty", () => {
    const f = fixture();
    edit(f, "rust/dc-glob/src/lib.rs");
    const mine = path.join(f.top, "mine");
    put(path.join(mine, "libgusset.a"), "mine");
    const p = plan(f.lane, { ...opts(f), engineDir: mine });
    assert.equal(p.engine.dir, mine);
    assert.match(p.engine.warnings[0], /WARNING.*without checking/);
    assert.equal(plan(f.lane, opts(f, { DEVCOUNCIL_GUSSET_ENGINE_DIR: mine })).engine.dir, mine);
    assert.throws(() => plan(f.lane, { ...opts(f), engineDir: path.join(f.top, "none") }), /has no libgusset\.a/);
    const stale = path.join(f.top, "stale");
    put(path.join(stale, "libgusset.a"), "stale");
    assert.throws(() => plan(f.lane, { ...opts(f, { CGO_LDFLAGS: `-L${stale}` }), engineDir: mine }), /already holds -L.*stale/);
  });
  it("fails by name, not with a stack, when GUSSET_DIR does not exist", () => {
    const f = fixture();
    refused(f, /no gusset checkout at .*nope.*GUSSET_DIR/, { GUSSET_DIR: path.join(f.top, "nope") });
  });
  it("fails when the sibling gusset is not at the pin", () => {
    const f = fixture();
    put(path.join(f.gusset, "x"), "x");
    git(f.gusset, "add", "."); git(f.gusset, "commit", "-qm", "drift");
    refused(f, /CI pins/);
  });
});

/** A fixture whose main has cgo-env.sh and a fake cargo; returns how to run it. */
function cgoFixture() {
  const f = fixture({ archive: false });
  const bin = path.join(f.top, "bin");
  put(path.join(bin, "cargo"), `#!/bin/sh
while [ $# -gt 0 ]; do [ "$1" = --target-dir ] && td="$2"; shift; done
mkdir -p "$td/release" && echo fake > "$td/release/libgusset.a"
[ -n "$FAKE_CARGO_BREAK_GIT" ] && rm -rf "$FAKE_CARGO_BREAK_GIT/.git"
[ -n "$FAKE_CARGO_APPEND" ] && echo changed >> "$FAKE_CARGO_APPEND"
exit 0
`);
  spawnSync("chmod", ["+x", path.join(bin, "cargo")]);
  const sh = path.join(f.main, "rust/gusset-engine/cgo-env.sh");
  copyFileSync(path.join(HERE, "../rust/gusset-engine/cgo-env.sh"), sh);
  mkdirSync(path.join(f.gusset, "crates/gusset"), { recursive: true });
  writeFileSync(path.join(f.gusset, "crates/gusset/Cargo.toml"), "[package]\n");
  git(f.gusset, "add", "."); git(f.gusset, "commit", "-qm", "crate");
  put(path.join(f.main, ".github/actions/setup-gusset/action.yml"), `        ref: ${git(f.gusset, "rev-parse", "HEAD")}\n`);
  git(f.main, "add", "."); git(f.main, "commit", "-qm", "sh");
  git(f.lane, "reset", "-q", "--hard", git(f.main, "rev-parse", "HEAD"));
  const releaseDir = path.join(f.main, "rust/gusset-engine/target/release");
  const run = (extra = {}, args = []) =>
    spawnSync("bash", [sh, ...args], { encoding: "utf8", cwd: f.main, env: { ...GIT_ENV, PATH: `${bin}:${process.env.PATH}`, ...extra } });
  return { f, run, releaseDir, stamp: path.join(releaseDir, "libgusset.stamp.json"),
    lanePlan: () => plan(f.lane, { scriptDir: f.scripts(f.lane), env: { ...process.env, CGO_LDFLAGS: "" } }) };
}

describe("cgo-env.sh stamps what it builds (fake cargo)", () => {
  it("writes a stamp lane-env accepts; --no-build leaves the stamp alone", () => {
    const c = cgoFixture();
    const r = c.run();
    assert.equal(r.status, 0, r.stderr);
    const stamp = JSON.parse(readFileSync(c.stamp, "utf8"));
    assert.equal(stamp.dirty, false);
    assert.deepEqual(Object.keys(stamp.trees).sort(), ["rust-toolchain.toml", "rust/dc-glob", "rust/gusset-engine"]);
    assert.equal(c.lanePlan().engine.dir, c.releaseDir);
    rmSync(c.stamp);
    assert.equal(c.run({}, ["--no-build"]).status, 0);
    assert.equal(existsSync(c.stamp), false, "--no-build does not stamp");
  });
  it("records the gusset cargo built against (../../../gusset), not GUSSET_DIR", () => {
    const c = cgoFixture();
    const other = path.join(c.f.top, "other-gusset");
    put(path.join(other, "go.mod"), "module other\n");
    git(c.f.top, "init", "-q", other); git(other, "add", "."); git(other, "commit", "-qm", "o");
    const r = c.run({ GUSSET_DIR: other });
    assert.equal(r.status, 0, r.stderr);
    assert.equal(JSON.parse(readFileSync(c.stamp, "utf8")).gusset, git(c.f.gusset, "rev-parse", "HEAD"));
  });
  it("a failed stamp write leaves no stamp, not the previous one", () => {
    const c = cgoFixture();
    assert.equal(c.run().status, 0);
    assert.equal(existsSync(c.stamp), true);
    const r = c.run({ FAKE_CARGO_BREAK_GIT: c.f.gusset });
    assert.equal(r.status, 0, "the archive is fine; the stamp is not fatal");
    assert.equal(existsSync(c.stamp), false);
    assert.match(r.stderr, /no engine stamp was written/);
  });
  it("inputs changing during the build write no stamp", () => {
    const c = cgoFixture();
    assert.equal(c.run().status, 0);
    const r = c.run({ FAKE_CARGO_APPEND: path.join(c.f.main, "rust/dc-glob/src/lib.rs") });
    assert.equal(r.status, 0);
    assert.equal(existsSync(c.stamp), false);
    assert.match(r.stderr, /inputs changed while the build ran/);
  });
});

describe("stampedBuild (what ensureGussetEngine runs around cargo)", () => {
  const setup = () => {
    const f = fixture({ archive: false });
    const dir = path.join(f.main, "rust/gusset-engine/target/release");
    const build = (extra = () => {}) => () => { put(path.join(dir, "libgusset.a"), "built"); extra(); };
    return { f, dir, build, stamp: path.join(dir, "libgusset.stamp.json") };
  };
  it("stamps a clean build", () => {
    const x = setup();
    const r = stampedBuild(x.f.main, x.f.gusset, x.dir, x.build(), () => assert.fail("no warning expected"));
    assert.equal(r.written, true);
    assert.equal(JSON.parse(readFileSync(x.stamp, "utf8")).gusset, git(x.f.gusset, "rev-parse", "HEAD"));
  });
  it("drops the previous stamp, and warns without throwing when inputs change mid-build", () => {
    const x = setup();
    stampedBuild(x.f.main, x.f.gusset, x.dir, x.build());
    const warnings = [];
    const r = stampedBuild(x.f.main, x.f.gusset, x.dir, x.build(() => put(path.join(x.f.main, "rust/dc-glob/src/lib.rs"), "// raced\n")), (m) => warnings.push(m));
    assert.equal(r.written, false);
    assert.equal(existsSync(x.stamp), false);
    assert.match(warnings[0], /changed while the build ran.*unstamped/);
  });
  it("a stamp that cannot be computed warns, does not throw, and leaves no stamp", () => {
    const x = setup();
    stampedBuild(x.f.main, x.f.gusset, x.dir, x.build());
    const warnings = [];
    const r = stampedBuild(x.f.main, x.f.gusset, x.dir, x.build(() => rmSync(path.join(x.f.gusset, ".git"), { recursive: true })), (m) => warnings.push(m));
    assert.equal(r.written, false);
    assert.equal(existsSync(x.stamp), false);
    assert.equal(warnings.length, 1);
  });
  it("a failing build propagates and leaves no stale stamp", () => {
    const x = setup();
    stampedBuild(x.f.main, x.f.gusset, x.dir, x.build());
    assert.throws(() => stampedBuild(x.f.main, x.f.gusset, x.dir, () => { throw new Error("cargo failed"); }), /cargo failed/);
    assert.equal(existsSync(x.stamp), false);
  });
});

describe("path dependencies outside the repository", () => {
  it("a refused input surfaces as one lane-env line and exit 1, not a stack", () => {
    const f = fixture();
    put(path.join(f.lane, "rust/gusset-engine/Cargo.toml"), '[workspace]\n[dependencies]\nevil = { path = "../../../evil" }\n');
    git(f.lane, "add", "."); git(f.lane, "commit", "-qm", "evil dep");
    const r = spawnSync(process.execPath, [path.join(f.lane, "scripts/lane-env.mjs"), "--check"], { cwd: f.lane, encoding: "utf8", env: GIT_ENV });
    assert.equal(r.status, 1);
    assert.match(r.stderr, /^lane-env: .*path dependency[^\n]*\n$/);
  });
  it("a refused input names the escape hatch", () => {
    const f = fixture();
    put(path.join(f.lane, "rust/gusset-engine/Cargo.toml"), '[workspace]\n[dependencies]\nevil = { path = "../../../evil" }\n');
    git(f.lane, "add", "."); git(f.lane, "commit", "-qm", "evil dep");
    const r = spawnSync(process.execPath, [path.join(f.lane, "scripts/lane-env.mjs"), "--check"], { cwd: f.lane, encoding: "utf8", env: GIT_ENV });
    assert.equal(r.status, 1);
    assert.match(r.stderr, /Cargo\.toml:\d+ has a path dependency[^\n]*--engine-dir <dir> or DEVCOUNCIL_GUSSET_ENGINE_DIR=<dir>/);
  });
  const commitManifest = (f, toml) => {
    put(path.join(f.lane, "rust/gusset-engine/Cargo.toml"), `[workspace]\n${toml}`);
    git(f.lane, "add", "."); git(f.lane, "commit", "-qm", "manifest");
  };
  const GUSSET = '[dependencies.gusset_core]\npath = "../../../gusset/crates/gusset"\n';
  it("[lib]/[[bin]]/[[test]] paths must stay inside the crate; inside is fine", () => {
    const f = fixture();
    commitManifest(f, `[lib]\npath = "src/lib.rs"\n[[bin]]\nname = "b"\npath = "src/bin/b.rs"\n[dependencies]\ndc-glob = { path = "../dc-glob" }\n${GUSSET}`);
    assert.deepEqual(Object.keys(engineInputs(f.lane, f.gusset).stamp.trees).sort(), ["rust-toolchain.toml", "rust/dc-glob", "rust/gusset-engine"]);
    for (const table of ["[lib]", "[[bin]]", "[[test]]", "[[example]]", "[[bench]]"]) {
      commitManifest(f, `${table}\npath = "../dc-glob/src/lib.rs"\n${GUSSET}`);
      assert.throws(() => engineInputs(f.lane, f.gusset), new RegExp(`\\[${table.replace(/[[\]]/g, "")}\\] path \\.\\./dc-glob/src/lib\\.rs resolves outside the crate`), table);
    }
  });
  it("[patch.*] and [replace] paths count as path dependencies under the outside-repo rule", () => {
    const f = fixture();
    commitManifest(f, `[patch.crates-io]\nfoo = { path = "../dc-glob" }\n${GUSSET}`);
    assert.ok("rust/dc-glob" in engineInputs(f.lane, f.gusset).stamp.trees);
    commitManifest(f, `[patch.crates-io]\nfoo = { path = "../../../evil" }\n${GUSSET}`);
    assert.throws(() => engineInputs(f.lane, f.gusset), /path dependency \.\.\/\.\.\/\.\.\/evil outside/);
    commitManifest(f, `[replace."foo:1.0.0"]\npath = "../../../evil"\n${GUSSET}`);
    assert.throws(() => engineInputs(f.lane, f.gusset), /path dependency .* outside/);
  });
  it("only known input errors become lane-env lines; a bug keeps its stack", () => {
    const f = fixture();
    assert.throws(() => chooseEngine({ root: null, mainRoot: f.main }, { env: {}, gusset: f.gusset }), TypeError);
  });
  it("are refused unless they are under the sibling gusset", () => {
    const f = fixture();
    put(path.join(f.lane, "rust/gusset-engine/Cargo.toml"),
      '[workspace]\n\n[dependencies]\ngusset_core = { package = "gusset", path = "../../../gusset/crates/gusset" }\nevil = { path = "../../../evil" }\n');
    git(f.lane, "add", "."); git(f.lane, "commit", "-qm", "evil dep");
    assert.throws(() => plan(f.lane, { scriptDir: f.scripts(f.lane), env: { ...process.env, CGO_LDFLAGS: "" } }),
      /path dependency \.\.\/\.\.\/\.\.\/evil outside the repository and outside/);
  });
});

describe("command line", () => {
  const cli = (f, args, cwd = f.lane) =>
    spawnSync(process.execPath, [path.join(f.lane, "scripts/lane-env.mjs"), ...args], { cwd, encoding: "utf8", env: GIT_ENV });
  const laneDir = (f) => path.join(f.main, ".git/worktrees/lane/devcouncil-lane");

  it("--check prints the plan and writes nothing; the default form writes go.work atomically", () => {
    const f = fixture();
    const c = cli(f, ["--check"], path.join(f.lane, "backend"));
    assert.equal(c.status, 0, c.stderr);
    assert.match(c.stdout, /linked worktree/);
    assert.equal(existsSync(laneDir(f)), false);
    const e = cli(f, []);
    assert.equal(e.status, 0, e.stderr);
    assert.match(e.stdout, /^export GOWORK='.*devcouncil-lane\/go\.work'$/m);
    assert.match(readFileSync(path.join(laneDir(f), "go.work"), "utf8"), /replace github\.com\/bharathvbcr\/gusset =>/);
    assert.deepEqual(
      [...new Set(readdirNames(laneDir(f)))], ["go.work"], "no temp file is left behind");
  });
  it("in the main checkout, from a subdirectory, prints nothing and runs the command as is", () => {
    const f = fixture();
    const r = spawnSync(process.execPath, [path.join(f.main, "scripts/lane-env.mjs"), "--", process.execPath, "-e", "console.log(process.env.GOWORK ?? 'unset')"],
      { cwd: path.join(f.main, "backend"), encoding: "utf8", env: { ...GIT_ENV, GOWORK: "" } });
    assert.equal(r.status, 0, r.stderr);
    assert.equal(r.stdout.trim(), "");
    assert.equal(cli(f, [], f.main).status, 1, "the lane copy refuses a cwd outside its checkout");
  });
  it("propagates the child's exit code", () => {
    const f = fixture();
    assert.equal(cli(f, ["--", process.execPath, "-e", "process.exit(7)"]).status, 7);
  });
  it("on SIGTERM the child is signalled and the wrapper dies of that signal, not exit 0", async () => {
    const f = fixture();
    const child = spawn(process.execPath, [path.join(f.lane, "scripts/lane-env.mjs"), "--", process.execPath, "-e",
      "console.log('ready');setTimeout(()=>{},20000)"], { cwd: f.lane, env: GIT_ENV });
    await new Promise((res) => child.stdout.on("data", (d) => String(d).includes("ready") && res()));
    const ended = new Promise((res) => child.on("exit", (code, signal) => res({ code, signal })));
    child.kill("SIGTERM");
    assert.deepEqual(await ended, { code: null, signal: "SIGTERM" });
  });
  it("a child killed by a signal makes the wrapper die of the same signal", () => {
    const f = fixture();
    const r = cli(f, ["--", process.execPath, "-e", "process.kill(process.pid,'SIGKILL')"]);
    assert.equal(r.signal, "SIGKILL");
  });
});

function readdirNames(dir) {
  return spawnSync("ls", [dir], { encoding: "utf8" }).stdout.split("\n").filter(Boolean);
}
