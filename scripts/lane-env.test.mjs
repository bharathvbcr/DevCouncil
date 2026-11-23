import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import {
  copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, utimesSync, writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { describe, it, after } from "node:test";
import { fileURLToPath } from "node:url";
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
const past = (file) => { const t = new Date(Date.now() - 3600_000); utimesSync(file, t, t); };

/** A gusset repo, a main DevCouncil checkout with an engine archive, and one linked lane. */
function fixture({ archive = true } = {}) {
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
  put(path.join(main, "rust/gusset-engine/Cargo.toml"), "[package]\n");
  put(path.join(main, "rust/gusset-engine/src/lib.rs"), "// engine\n");
  mkdirSync(path.join(main, "scripts"), { recursive: true });
  for (const f of ["lane-env.mjs", "ci-local-checks.mjs"]) copyFileSync(path.join(HERE, f), path.join(main, "scripts", f));
  git(main, "add", "."); git(main, "commit", "-qm", "m");
  for (const f of ["rust/gusset-engine/Cargo.toml", "rust/gusset-engine/src/lib.rs"]) past(path.join(main, f));
  if (archive) put(path.join(main, "rust/gusset-engine/target/release/libgusset.a"), "main-archive");
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

describe("chooseEngine / plan", () => {
  const opts = (f, extra = {}) => ({ scriptDir: f.scripts(f.lane), env: { ...process.env, CGO_LDFLAGS: "", ...extra } });
  const mainDir = (f) => path.join(f.main, "rust/gusset-engine/target/release");

  it("main checkout: no environment at all", () => {
    const f = fixture();
    assert.deepEqual(plan(f.main, { scriptDir: f.scripts(f.main) }).env, {});
  });
  it("(b) uses main's archive when the engine trees match, and hashes that archive", () => {
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
  it("(a) prefers the lane's own archive over main's", () => {
    const f = fixture();
    const own = path.join(f.lane, "rust/gusset-engine/target/release");
    put(path.join(own, "libgusset.a"), "lane-archive");
    const p = plan(f.lane, opts(f));
    assert.equal(p.engine.dir, own);
    assert.match(p.engine.why, /lane's own/);
  });
  it("(c) fails, naming the build command, when the lane's engine differs from main's", () => {
    const f = fixture();
    put(path.join(f.lane, "rust/gusset-engine/src/lib.rs"), "// edited\n");
    git(f.lane, "commit", "-qam", "engine edit");
    assert.throws(() => plan(f.lane, opts(f)), /differs from main's.*cgo-env\.sh.*ci:local -- --only "gusset engine"/s);
  });
  it("(c) fails on an uncommitted engine edit in the lane", () => {
    const f = fixture();
    put(path.join(f.lane, "rust/gusset-engine/src/lib.rs"), "// dirty\n");
    assert.throws(() => plan(f.lane, opts(f)), /lane's rust\/gusset-engine has uncommitted/);
  });
  it("(c) fails when main's archive is older than main's engine sources", () => {
    const f = fixture();
    const t = new Date(Date.now() - 7200_000);
    utimesSync(path.join(mainDir(f), "libgusset.a"), t, t);
    assert.throws(() => plan(f.lane, opts(f)), /older than main's rust\/gusset-engine sources/);
  });
  it("(c) fails when there is no archive anywhere", () => {
    const f = fixture({ archive: false });
    assert.throws(() => plan(f.lane, opts(f)), /no libgusset\.a in the lane or in .*ci:local/s);
  });
  it("a caller's --engine-dir or CGO_LDFLAGS -L wins, even over a mismatched tree", () => {
    const f = fixture();
    put(path.join(f.lane, "rust/gusset-engine/src/lib.rs"), "// edited\n");
    git(f.lane, "commit", "-qam", "engine edit");
    const mine = path.join(f.top, "mine");
    put(path.join(mine, "libgusset.a"), "mine");
    assert.equal(plan(f.lane, { ...opts(f), engineDir: mine }).engine.dir, mine);
    const p = plan(f.lane, opts(f, { CGO_LDFLAGS: `-L${mine}` }));
    assert.equal(p.engine.dir, mine);
    assert.equal(p.env.CGO_LDFLAGS, `-L${mine}`);
    assert.throws(() => chooseEngine({ root: f.lane, mainRoot: f.main }, { engineDir: path.join(f.top, "none"), env: {} }), /has no libgusset\.a/);
  });
  it("fails by name, not with a stack, when GUSSET_DIR does not exist", () => {
    const f = fixture();
    assert.throws(() => plan(f.lane, opts(f, { GUSSET_DIR: path.join(f.top, "nope") })), /no gusset checkout at .*nope.*GUSSET_DIR/);
  });
  it("fails when the sibling gusset is not at the pin", () => {
    const f = fixture();
    put(path.join(f.gusset, "x"), "x");
    git(f.gusset, "add", "."); git(f.gusset, "commit", "-qm", "drift");
    assert.throws(() => plan(f.lane, opts(f)), /CI pins/);
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
