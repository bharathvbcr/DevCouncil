#!/usr/bin/env node
/**
 * Make Go tests that link Gusset (cmd/devcouncil, gussetfn, ...) run from a
 * linked git worktree such as .gitpulse/worktrees/<lane>.
 *
 * backend/go_orchestrator/go.mod replaces github.com/bharathvbcr/gusset with
 * ../../../gusset, and gussetfn links rust/gusset-engine/target/release/libgusset.a.
 * From a linked worktree both resolve under .gitpulse/worktrees/ and do not
 * exist. This script writes a go.work OUTSIDE the worktree (in its private
 * git dir, so nothing is tracked or ignored-by-rule) whose replace points at
 * the main checkout's sibling gusset, and points the linker at the main
 * checkout's libgusset.a. go.mod, CI and the main checkout are untouched.
 *
 *   node scripts/lane-env.mjs -- go -C backend/go_orchestrator test ./cmd/devcouncil/... -count=1
 *   eval "$(node scripts/lane-env.mjs)"      # export GOWORK / CGO_* in this shell
 *   node scripts/lane-env.mjs --check        # report, change nothing, exit 1 if unusable
 *
 * Environment: GUSSET_DIR overrides the gusset checkout (default
 * <main checkout>/../gusset). In the main checkout (not a linked worktree) the
 * script does nothing: prints no exports, and with -- runs the command as is.
 *
 * The archive is the MAIN checkout's rust/gusset-engine/target/release/libgusset.a;
 * build it there (cargo build --release --locked in rust/gusset-engine, or
 * node scripts/ci-local.mjs --only "gusset engine"). It is not rebuilt from the
 * lane's engine sources, so a lane that edits rust/gusset-engine must build its
 * own archive and set CGO_LDFLAGS itself.
 */
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";

const HELP = readFileSync(new URL(import.meta.url), "utf8").match(/\/\*\*([\s\S]*?)\*\//)[1]
  .replace(/^ ?\* ?/gm, "").trim();

function git(cwd, ...args) {
  const r = spawnSync("git", ["-C", cwd, ...args], { encoding: "utf8" });
  if (r.status !== 0) throw new Error(`git ${args.join(" ")}: ${r.stderr.trim() || "failed"}`);
  return r.stdout.trim();
}

/** @returns {{linked: boolean, root: string, mainRoot: string, gitDir: string}} */
function locate(cwd) {
  const root = git(cwd, "rev-parse", "--show-toplevel");
  const gitDir = path.resolve(root, git(cwd, "rev-parse", "--git-dir"));
  const common = path.resolve(root, git(cwd, "rev-parse", "--git-common-dir"));
  return { linked: gitDir !== common, root, mainRoot: path.dirname(common), gitDir };
}

/** Absolute-path copy of the tracked go.work, plus the gusset replace. */
function laneGoWork(root, gussetDir) {
  const tracked = readFileSync(path.join(root, "go.work"), "utf8");
  const goLine = tracked.match(/^go\s+\S+$/m)?.[0];
  const uses = [...tracked.matchAll(/^\s*(\.{1,2}\/\S*)\s*$/gm)].map((m) => path.resolve(root, m[1]));
  if (!goLine || uses.length === 0) throw new Error("go.work has no go line or use directories");
  return `${goLine}\n\nuse (\n${uses.map((u) => `\t${u}\n`).join("")})\n\nreplace github.com/bharathvbcr/gusset => ${gussetDir}\n`;
}

function plan() {
  const loc = locate(process.cwd());
  if (!loc.linked) return { loc, env: {} };
  const gusset = path.resolve(process.env.GUSSET_DIR || path.join(loc.mainRoot, "..", "gusset"));
  if (!existsSync(path.join(gusset, "go.mod"))) {
    throw new Error(`no gusset checkout at ${gusset} (set GUSSET_DIR)`);
  }
  const libdir = path.join(loc.mainRoot, "rust", "gusset-engine", "target", "release");
  const archive = path.join(libdir, "libgusset.a");
  if (!existsSync(archive)) {
    throw new Error(`${archive} is missing: build it in the main checkout (cargo build --release --locked --manifest-path rust/gusset-engine/Cargo.toml --target-dir rust/gusset-engine/target)`);
  }
  const dir = path.join(loc.gitDir, "devcouncil-lane");
  const work = path.join(dir, "go.work");
  mkdirSync(dir, { recursive: true });
  const content = laneGoWork(loc.root, gusset);
  if (!existsSync(work) || readFileSync(work, "utf8") !== content) writeFileSync(work, content);
  // Same cache key as rust/gusset-engine/cgo-env.sh and ci-local.mjs.
  const sum = createHash("sha256").update(readFileSync(archive)).digest("hex").slice(0, 32);
  const cur = spawnSync("go", ["env", "CGO_CFLAGS"], { encoding: "utf8" });
  const base = (cur.status === 0 ? cur.stdout.trim() : "-O2 -g")
    .replace(/(^| )-DDEVCOUNCIL_GUSSET_ENGINE_SHA256=\S*/g, "").trim();
  return {
    loc,
    env: {
      GOWORK: work,
      CGO_LDFLAGS: `-L${libdir}${process.env.CGO_LDFLAGS ? ` ${process.env.CGO_LDFLAGS}` : ""}`,
      CGO_CFLAGS: `${base ? `${base} ` : ""}-DDEVCOUNCIL_GUSSET_ENGINE_SHA256=${sum}`,
    },
  };
}

function main(argv) {
  if (argv.includes("--help") || argv.includes("-h")) {
    console.log(HELP);
    return 0;
  }
  const dd = argv.indexOf("--");
  const flags = dd < 0 ? argv : argv.slice(0, dd);
  const cmd = dd < 0 ? [] : argv.slice(dd + 1);
  const unknown = flags.filter((a) => a !== "--check");
  if (unknown.length || (dd >= 0 && cmd.length === 0)) {
    console.error(`lane-env: unknown or incomplete arguments: ${[...unknown, ...(dd >= 0 && !cmd.length ? ["--"] : [])].join(" ")}\n(see --help)`);
    return 2;
  }
  let p;
  try {
    p = plan();
  } catch (e) {
    console.error(`lane-env: ${e.message}`);
    return 1;
  }
  if (flags.includes("--check")) {
    console.log(p.loc.linked ? `linked worktree ${p.loc.root}: GOWORK=${p.env.GOWORK}` : "main checkout: nothing to do");
    return 0;
  }
  if (cmd.length) {
    const r = spawnSync(cmd[0], cmd.slice(1), { stdio: "inherit", env: { ...process.env, ...p.env } });
    if (r.error) {
      console.error(`lane-env: ${cmd[0]}: ${r.error.message}`);
      return 127;
    }
    return r.status ?? 1;
  }
  const q = (s) => `'${s.replaceAll("'", "'\\''")}'`;
  for (const [k, v] of Object.entries(p.env)) console.log(`export ${k}=${q(v)}`);
  return 0;
}

process.exitCode = main(process.argv.slice(2));
