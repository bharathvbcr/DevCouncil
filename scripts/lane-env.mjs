#!/usr/bin/env node
/**
 * Make Go tests that link Gusset (cmd/devcouncil, gussetfn, ...) run from a
 * linked git worktree such as .gitpulse/worktrees/<lane>.
 *
 * backend/go_orchestrator/go.mod replaces github.com/bharathvbcr/gusset with
 * ../../../gusset, and gussetfn links rust/gusset-engine/target/release/libgusset.a.
 * From a linked worktree the replace resolves under .gitpulse/worktrees/ and
 * the lane has no archive. This script writes a go.work OUTSIDE the worktree
 * (in its private git dir; nothing is tracked or ignored-by-rule) whose
 * replace points at the main checkout's sibling gusset, and selects an engine
 * archive. go.mod, CI and the main checkout are untouched.
 *
 *   node scripts/lane-env.mjs -- go -C backend/go_orchestrator test ./cmd/devcouncil/... -count=1
 *   eval "$(node scripts/lane-env.mjs)"      # export GOWORK / CGO_* in this shell
 *   node scripts/lane-env.mjs --check        # print the plan, write nothing
 *   node scripts/lane-env.mjs --engine-dir <dir> -- <cmd>   # dir holds libgusset.a
 *
 * Run it from anywhere inside the worktree. In the main checkout (not a linked
 * worktree) it does nothing: no exports, and with -- it runs the command as is.
 * Environment: GUSSET_DIR overrides the gusset checkout (default
 * <main checkout>/../gusset); the checkout must be at the setup-gusset pin.
 *
 * Which libgusset.a is linked, first match wins:
 *   1. --engine-dir, or a -L dir in a CGO_LDFLAGS you already set (it must hold libgusset.a);
 *   2. the lane's own rust/gusset-engine/target/release (build it with
 *      rust/gusset-engine/cgo-env.sh, which is what a lane that edits the engine needs);
 *   3. the main checkout's archive, only if the lane's committed AND working-tree
 *      rust/gusset-engine equals main's HEAD tree, main's engine tree is clean, and
 *      the archive is not older than its sources;
 *   4. otherwise it fails and names the build command. It never links an engine
 *      that does not match the lane's sources.
 * CGO_CFLAGS carries the hash of the archive chosen, so Go's caches follow it.
 *
 * The linker prints "ld: warning: search path ... not found" for the lane's
 * own (absent) rust/gusset-engine/target/release, and for gusset's own
 * target/gusset fallback. Both are harmless: the archive is found through -L.
 * (The second also appears in the main checkout.)
 */
import { spawn, spawnSync } from "node:child_process";
import {
  existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, renameSync, rmSync, statSync, writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { engineCflags, gussetPinCheck } from "./ci-local-checks.mjs";

const SCRIPT_DIR = path.dirname(fileURLToPath(import.meta.url));
const ENGINE = path.join("rust", "gusset-engine");
const ARCHIVE = "libgusset.a";

class LaneError extends Error {}

function git(cwd, ...args) {
  const r = spawnSync("git", ["-C", cwd, ...args], { encoding: "utf8" });
  if (r.status !== 0) throw new LaneError(`git ${args.join(" ")}: ${r.stderr.trim() || "failed"}`);
  return r.stdout.trim();
}

function noSpaces(p, what) {
  if (/\s/.test(p)) throw new LaneError(`${what} ${JSON.stringify(p)} contains whitespace, which cgo and go.work cannot carry`);
  return p;
}

/**
 * Where this script's checkout is, and whether it is a linked worktree.
 * `cwd` must be inside it (symlinks resolved).
 *
 * @param {string} cwd
 * @param {string} [scriptDir]
 * @returns {{ linked: boolean, root: string, mainRoot: string, gitDir: string }}
 */
export function locate(cwd, scriptDir = SCRIPT_DIR) {
  const root = realpathSync(git(scriptDir, "rev-parse", "--show-toplevel"));
  const here = realpathSync(cwd);
  if (here !== root && !here.startsWith(root + path.sep)) {
    throw new LaneError(`${here} is outside the checkout this script belongs to (${root}); run the copy of scripts/lane-env.mjs from the worktree you are in`);
  }
  const [gitDirRaw, commonRaw] = git(root, "rev-parse", "--path-format=absolute", "--git-dir", "--git-common-dir").split("\n");
  const gitDir = realpathSync(gitDirRaw);
  const common = realpathSync(commonRaw);
  return { linked: gitDir !== common, root, mainRoot: path.dirname(common), gitDir };
}

/**
 * The go.work for a lane: the tracked go.work's modules at absolute paths plus
 * the gusset replace, produced by `go work edit`.
 *
 * @param {string} root
 * @param {string} gussetDir
 * @returns {string}
 */
export function laneGoWork(root, gussetDir) {
  const tracked = path.join(root, "go.work");
  const j = spawnSync("go", ["work", "edit", "-json", tracked], { encoding: "utf8" });
  if (j.status !== 0) throw new LaneError(`go work edit -json ${tracked}: ${j.stderr.trim() || "failed"}`);
  const { Go: goVersion, Use: use = [] } = JSON.parse(j.stdout);
  if (!goVersion || use.length === 0) throw new LaneError(`${tracked} has no go line or use directories`);
  const dir = mkdtempSync(path.join(tmpdir(), "lane-gowork-"));
  try {
    const tmp = path.join(dir, "go.work");
    writeFileSync(tmp, `go ${goVersion}\n`);
    const edits = [
      ...use.map((u) => `-use=${noSpaces(path.resolve(root, u.DiskPath), "module directory")}`),
      `-replace=github.com/bharathvbcr/gusset=${noSpaces(gussetDir, "gusset checkout")}`,
    ];
    const e = spawnSync("go", ["work", "edit", ...edits, tmp], { encoding: "utf8" });
    if (e.status !== 0) throw new LaneError(`go work edit: ${e.stderr.trim() || "failed"}`);
    return readFileSync(tmp, "utf8");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

function newestMtime(dir, files) {
  return Math.max(0, ...files.map((f) => statSync(path.join(dir, f), { throwIfNoEntry: false })?.mtimeMs ?? 0));
}

/**
 * Pick the engine archive directory per the rules in the header.
 *
 * @param {{ root: string, mainRoot: string }} loc
 * @param {{ engineDir?: string, env: Record<string, string | undefined> }} opts
 * @returns {{ dir: string, why: string }}
 */
export function chooseEngine(loc, { engineDir, env }) {
  const holds = (d) => existsSync(path.join(d, ARCHIVE));
  if (engineDir) {
    const d = path.resolve(engineDir);
    if (!holds(d)) throw new LaneError(`--engine-dir ${d} has no ${ARCHIVE}`);
    return { dir: d, why: "--engine-dir" };
  }
  for (const m of (env.CGO_LDFLAGS || "").matchAll(/(?:^|\s)-L(\S+)/g)) {
    if (holds(m[1])) return { dir: m[1], why: "CGO_LDFLAGS" };
  }
  if (env.CGO_LDFLAGS?.includes("-L")) {
    throw new LaneError(`CGO_LDFLAGS names -L directories but none holds ${ARCHIVE}`);
  }
  const own = path.join(loc.root, ENGINE, "target", "release");
  if (holds(own)) return { dir: own, why: "the lane's own archive" };

  const build =
    `build one in the lane: rust/gusset-engine/cgo-env.sh --export (run from ${loc.root}); ` +
    `or, if the lane does not touch rust/gusset-engine, build main's: (cd ${loc.mainRoot} && npm run ci:local -- --only "gusset engine")`;
  const mainDir = path.join(loc.mainRoot, ENGINE, "target", "release");
  if (!holds(mainDir)) throw new LaneError(`no ${ARCHIVE} in the lane or in ${mainDir}; ${build}`);
  const laneTree = git(loc.root, "rev-parse", `HEAD:${ENGINE}`);
  const mainTree = git(loc.mainRoot, "rev-parse", `HEAD:${ENGINE}`);
  if (laneTree !== mainTree) {
    throw new LaneError(`the lane's ${ENGINE} (${laneTree.slice(0, 12)}) differs from main's (${mainTree.slice(0, 12)}), so main's archive does not match it; ${build}`);
  }
  for (const [where, dir] of [["lane", loc.root], ["main", loc.mainRoot]]) {
    if (git(dir, "status", "--porcelain", "--", ENGINE)) {
      throw new LaneError(`the ${where}'s ${ENGINE} has uncommitted changes, so main's archive may not match it; ${build}`);
    }
  }
  const sources = git(loc.mainRoot, "ls-files", "--", ENGINE).split("\n").filter(Boolean);
  const archiveMtime = statSync(path.join(mainDir, ARCHIVE)).mtimeMs;
  if (newestMtime(loc.mainRoot, sources) > archiveMtime) {
    throw new LaneError(`main's ${ARCHIVE} is older than main's ${ENGINE} sources; ${build}`);
  }
  return { dir: mainDir, why: "main's archive (same engine tree)" };
}

/**
 * Compute the environment and go.work content for this checkout. Writes nothing.
 *
 * @param {string} cwd
 * @param {{ engineDir?: string, env?: Record<string, string | undefined>, scriptDir?: string }} [opts]
 */
export function plan(cwd, opts = {}) {
  const env = opts.env ?? process.env;
  const loc = locate(cwd, opts.scriptDir);
  if (!loc.linked) return { loc, env: {} };
  const wanted = path.resolve(env.GUSSET_DIR || path.join(loc.mainRoot, "..", "gusset"));
  if (!existsSync(path.join(wanted, "go.mod"))) throw new LaneError(`no gusset checkout at ${wanted} (set GUSSET_DIR)`);
  const gusset = realpathSync(wanted);
  const pin = gussetPinCheck(loc.root, gusset);
  if (!pin.ok) throw new LaneError(pin.message);
  const engine = chooseEngine(loc, { engineDir: opts.engineDir, env });
  noSpaces(engine.dir, "engine directory");
  const archive = path.join(engine.dir, ARCHIVE);
  const goWork = path.join(loc.gitDir, "devcouncil-lane", "go.work");
  return {
    loc,
    engine,
    goWork,
    goWorkContent: laneGoWork(loc.root, gusset),
    env: {
      GOWORK: goWork,
      CGO_LDFLAGS: env.CGO_LDFLAGS?.includes(`-L${engine.dir}`) ? env.CGO_LDFLAGS : `-L${engine.dir}${env.CGO_LDFLAGS ? ` ${env.CGO_LDFLAGS}` : ""}`,
      CGO_CFLAGS: engineCflags(archive, env),
    },
  };
}

/** Write the go.work atomically (temp file in the same directory, then rename). */
function materialize(p) {
  if (!p.goWork) return;
  if (existsSync(p.goWork) && readFileSync(p.goWork, "utf8") === p.goWorkContent) return;
  mkdirSync(path.dirname(p.goWork), { recursive: true });
  const tmp = `${p.goWork}.${process.pid}.tmp`;
  writeFileSync(tmp, p.goWorkContent);
  renameSync(tmp, p.goWork);
}

/** Run a command, forwarding SIGINT/SIGTERM to it; resolves how it ended. */
function run(cmd, env) {
  return new Promise((resolve) => {
    const child = spawn(cmd[0], cmd.slice(1), { stdio: "inherit", env: { ...process.env, ...env } });
    const forward = (sig) => () => child.kill(sig);
    const handlers = ["SIGINT", "SIGTERM"].map((s) => [s, forward(s)]);
    for (const [s, h] of handlers) process.on(s, h);
    const done = (r) => {
      for (const [s, h] of handlers) process.off(s, h);
      resolve(r);
    };
    child.on("error", (e) => {
      console.error(`lane-env: ${cmd[0]}: ${e.message}`);
      done({ code: 127 });
    });
    child.on("exit", (code, signal) => done({ code: code ?? 1, signal }));
  });
}

async function main(argv) {
  if (argv.includes("--help") || argv.includes("-h")) {
    const src = readFileSync(fileURLToPath(import.meta.url), "utf8");
    console.log(/\/\*\*([\s\S]*?)\*\//.exec(src)[1].replace(/^ ?\* ?/gm, "").trim());
    return { code: 0 };
  }
  const dd = argv.indexOf("--");
  const flags = dd < 0 ? argv : argv.slice(0, dd);
  const cmd = dd < 0 ? [] : argv.slice(dd + 1);
  let check = false;
  let engineDir;
  const bad = [];
  for (let i = 0; i < flags.length; i++) {
    if (flags[i] === "--check") check = true;
    else if (flags[i] === "--engine-dir" && flags[i + 1]) engineDir = flags[++i];
    else bad.push(flags[i]);
  }
  if (dd >= 0 && cmd.length === 0) bad.push("--");
  if (bad.length) {
    console.error(`lane-env: unknown or incomplete arguments: ${bad.join(" ")} (see --help)`);
    return { code: 2 };
  }
  let p;
  try {
    p = plan(process.cwd(), { engineDir });
  } catch (e) {
    if (!(e instanceof LaneError)) throw e;
    console.error(`lane-env: ${e.message}`);
    return { code: 1 };
  }
  if (check) {
    console.log(p.loc.linked
      ? `linked worktree ${p.loc.root}\nGOWORK=${p.env.GOWORK}\nengine: ${p.engine.dir} (${p.engine.why})`
      : "main checkout: nothing to do");
    return { code: 0 };
  }
  materialize(p);
  if (cmd.length) return run(cmd, p.env);
  const q = (s) => `'${s.replaceAll("'", "'\\''")}'`;
  for (const [k, v] of Object.entries(p.env)) console.log(`export ${k}=${q(v)}`);
  return { code: 0 };
}

// Run only as a program: the tests import plan, locate and laneGoWork.
const entry = process.argv[1];
if (entry !== undefined && realpathSync(path.resolve(entry)) === realpathSync(fileURLToPath(import.meta.url))) {
  const { code, signal } = await main(process.argv.slice(2));
  if (signal) process.kill(process.pid, signal);
  process.exitCode = code;
}
