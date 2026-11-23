#!/usr/bin/env node
/**
 * The input stamp for libgusset.a: what a built archive was built from, so a
 * consumer can tell whether it still matches a checkout's sources.
 *
 *   node scripts/gusset-engine-stamp.mjs begin <archive-dir> <gusset-dir>   # deletes the old stamp, prints the inputs
 *   node scripts/gusset-engine-stamp.mjs write <archive-dir> <gusset-dir> < inputs-from-begin
 *
 * A build is: begin (delete the old stamp, record the inputs), build, write
 * (only if the inputs are unchanged by the build and the archive exists). A
 * failed or raced stamp therefore leaves NO stamp, never the previous one.
 * Whoever builds the archive writes <archive-dir>/libgusset.stamp.json:
 * ensureGussetEngine in ci-local.mjs and rust/gusset-engine/cgo-env.sh (which
 * calls this CLI, so there is one computation). scripts/lane-env.mjs compares
 * it with the lane's own inputs. The inputs are the committed git trees of
 * rust/gusset-engine and every path dependency under the repository, read from
 * the Cargo.toml files (rust/dc-glob today; its own path deps too), the blob of
 * rust-toolchain.toml, and the sibling gusset's HEAD. Cargo.lock is inside the
 * engine tree, so it is covered. "dirty" is any uncommitted or untracked change
 * under those inputs or in the gusset sibling (git status --untracked-files=all,
 * which ignores status.showUntrackedFiles); a dirty stamp is never reusable.
 */
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync, realpathSync, renameSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const ENGINE = "rust/gusset-engine";
export const STAMP_NAME = "libgusset.stamp.json";

/** A known way the inputs cannot be computed; anything else thrown is a bug. */
export class InputError extends Error {}

function git(cwd, ...args) {
  const r = spawnSync("git", ["-C", cwd, ...args], { encoding: "utf8" });
  if (r.status !== 0) throw new InputError(`git ${args.join(" ")} (in ${cwd}): ${r.stderr.trim() || "failed"}`);
  return r.stdout.trim();
}

/**
 * Every `path = "..."` in a manifest that feeds the build, with its kind:
 * "dep" for dependency tables ([dependencies], dev-/build-, [workspace.dependencies],
 * [target.*.dependencies], their [dependencies.name] forms, [patch.*] and
 * [replace]) and "target" for [lib], [[bin]], [[example]], [[test]], [[bench]].
 *
 * @returns {Generator<{ kind: "dep" | "target", table: string, path: string }>}
 */
function* manifestPaths(toml) {
  const depTable = /^(?:workspace\.|target\..+\.)?(?:dev-|build-)?dependencies(?:\..+)?$|^patch\..+$|^replace(?:\..+)?$/;
  const targetTable = /^(?:lib|bin|example|test|bench)$/;
  let kind;
  let table = "";
  for (const line of toml.split("\n")) {
    const header = /^\s*\[{1,2}\s*([^\]]+?)\s*\]{1,2}\s*(?:#.*)?$/.exec(line);
    if (header) {
      table = header[1];
      kind = depTable.test(table) ? "dep" : targetTable.test(table) ? "target" : undefined;
      continue;
    }
    if (!kind) continue;
    for (const m of line.matchAll(/\bpath\s*=\s*"([^"]+)"/g)) yield { kind, table, path: m[1] };
  }
}

/** Repository-relative engine inputs: the engine, its path dependencies, the toolchain file. */
export function enginePaths(root) {
  const out = [ENGINE];
  const queue = [ENGINE];
  while (queue.length) {
    const rel = queue.shift();
    const toml = path.join(root, rel, "Cargo.toml");
    if (!existsSync(toml)) throw new InputError(`${toml} is missing (a path dependency of the engine)`);
    for (const entry of manifestPaths(readFileSync(toml, "utf8"))) {
      const crateDir = path.resolve(root, rel);
      const abs = path.resolve(crateDir, entry.path);
      if (entry.kind === "target") {
        // A target source outside the crate directory is an input the stamp cannot see.
        if (!abs.startsWith(crateDir + path.sep)) {
          throw new InputError(`${toml} [${entry.table}] path ${entry.path} resolves outside the crate directory; the engine stamp cannot cover it`);
        }
        continue;
      }
      const dep = path.relative(root, abs).split(path.sep).join("/");
      if (dep.startsWith("..") || path.isAbsolute(dep)) {
        // Outside the repository it must be the sibling gusset the manifests
        // expect (<root>/../gusset, resolved the way cargo does); its commit is
        // recorded instead. Anything else is an input the stamp cannot see.
        const sibling = path.resolve(root, "..", "gusset");
        if (abs !== sibling && !abs.startsWith(sibling + path.sep)) {
          throw new InputError(`${toml} has a path dependency ${entry.path} outside the repository and outside ${sibling}; the engine stamp cannot cover it`);
        }
        continue;
      }
      if (out.includes(dep)) continue;
      out.push(dep);
      queue.push(dep);
    }
  }
  if (existsSync(path.join(root, "rust-toolchain.toml"))) out.push("rust-toolchain.toml");
  return out;
}

/**
 * @param {string} root repository (or linked worktree) root
 * @param {string} gussetDir the sibling gusset checkout
 * @returns {{ stamp: { version: 1, trees: Record<string, string>, gusset: string, dirty: boolean }, dirtyFiles: string[] }}
 */
export function engineInputs(root, gussetDir) {
  const paths = enginePaths(root);
  const trees = Object.fromEntries(paths.map((p) => [p, git(root, "rev-parse", `HEAD:${p}`)]));
  const dirtyFiles = [
    ...git(root, "status", "--porcelain", "--untracked-files=all", "--", ...paths).split("\n").filter(Boolean),
    ...git(gussetDir, "status", "--porcelain", "--untracked-files=all").split("\n").filter(Boolean).map((l) => `gusset: ${l}`),
  ];
  return { stamp: { version: 1, trees, gusset: git(gussetDir, "rev-parse", "HEAD"), dirty: dirtyFiles.length > 0 }, dirtyFiles };
}

export const stampFile = (dir) => path.join(dir, STAMP_NAME);

function putStamp(archiveDir, stamp) {
  const tmp = `${stampFile(archiveDir)}.${process.pid}.tmp`;
  writeFileSync(tmp, `${JSON.stringify(stamp, null, 2)}\n`);
  renameSync(tmp, stampFile(archiveDir));
}

/** Compute and write the stamp for the current inputs (no build in between). */
export function writeStamp(root, gussetDir, archiveDir) {
  const { stamp } = engineInputs(root, gussetDir);
  putStamp(archiveDir, stamp);
  return stamp;
}

/**
 * Start a build: delete any previous stamp, then record the inputs. Returns
 * the inputs, or undefined (with the reason) if they cannot be computed.
 *
 * @returns {{ before?: ReturnType<typeof engineInputs>, error?: string }}
 */
export function beginStamp(root, gussetDir, archiveDir) {
  rmSync(stampFile(archiveDir), { force: true });
  try {
    return { before: engineInputs(root, gussetDir) };
  } catch (e) {
    return { error: e.message };
  }
}

/**
 * Finish a build: write the stamp only if the archive exists and the inputs
 * equal the ones `beginStamp` saw. Never throws; otherwise no stamp exists.
 *
 * @returns {{ written: boolean, reason: string }}
 */
export function finishStamp(root, gussetDir, archiveDir, before) {
  rmSync(stampFile(archiveDir), { force: true });
  if (!before) return { written: false, reason: "the inputs could not be recorded before the build" };
  if (!existsSync(path.join(archiveDir, "libgusset.a"))) return { written: false, reason: `no libgusset.a in ${archiveDir}` };
  try {
    const after = engineInputs(root, gussetDir);
    if (JSON.stringify(after.stamp) !== JSON.stringify(before.stamp)) {
      return { written: false, reason: "the engine inputs changed while the build ran" };
    }
    putStamp(archiveDir, after.stamp);
    return { written: true, reason: after.stamp.dirty ? "dirty stamp (never reused by lane-env)" : "ok" };
  } catch (e) {
    return { written: false, reason: e.message };
  }
}

/**
 * Run `build` between begin and finish. A stamp problem is reported through
 * `warn` and never fails the build; it leaves the archive unstamped.
 */
export function stampedBuild(root, gussetDir, archiveDir, build, warn = console.warn) {
  const { before, error } = beginStamp(root, gussetDir, archiveDir);
  build();
  const r = finishStamp(root, gussetDir, archiveDir, before);
  if (!r.written) warn(`engine stamp not written (${error ?? r.reason}); lane-env.mjs will refuse this archive as unstamped`);
  return r;
}

/**
 * Does the archive in `dir` match the inputs? Anything unreadable, missing,
 * dirty or different is a refusal with the reason.
 *
 * @returns {{ ok: boolean, reason: string }}
 */
export function stampVerdict(inputs, dir) {
  const file = stampFile(dir);
  if (!existsSync(file)) return { ok: false, reason: `${dir} has an archive with no stamp (built before stamping): rebuild to stamp it` };
  let have;
  try {
    have = JSON.parse(readFileSync(file, "utf8"));
  } catch (e) {
    return { ok: false, reason: `${file} is unreadable (${e.message}): rebuild to stamp it` };
  }
  if (have.dirty) return { ok: false, reason: `the archive in ${dir} was built from uncommitted sources; a dirty build is never reused` };
  if (inputs.stamp.dirty) return { ok: false, reason: "the lane's engine inputs have uncommitted changes" };
  const want = inputs.stamp;
  const diffs = [
    ...new Set([...Object.keys(want.trees), ...Object.keys(have.trees ?? {})]),
  ].filter((k) => want.trees[k] !== have.trees?.[k]);
  if (have.gusset !== want.gusset) diffs.push("gusset commit");
  if (have.version !== want.version) diffs.push("stamp version");
  if (diffs.length) return { ok: false, reason: `the archive in ${dir} was built from other inputs (${diffs.join(", ")} differ)` };
  return { ok: true, reason: "stamp matches" };
}

const entry = process.argv[1];
if (entry !== undefined && realpathSync(path.resolve(entry)) === realpathSync(fileURLToPath(import.meta.url))) {
  const [cmd, archiveDir, gusset] = process.argv.slice(2);
  if (!["begin", "write"].includes(cmd) || !archiveDir || !gusset) {
    console.error("usage: gusset-engine-stamp.mjs begin <archive-dir> <gusset-dir> | write <archive-dir> <gusset-dir>  (the output of begin on stdin)");
    process.exit(2);
  }
  const root = realpathSync(git(path.dirname(fileURLToPath(import.meta.url)), "rev-parse", "--show-toplevel"));
  if (cmd === "begin") {
    const { before, error } = beginStamp(root, path.resolve(gusset), archiveDir);
    if (!before) {
      console.error(`gusset-engine-stamp: ${error}`);
      process.exit(1);
    }
    console.log(JSON.stringify(before));
  } else {
    const r = finishStamp(root, path.resolve(gusset), archiveDir, JSON.parse(readFileSync(0, "utf8")));
    console.error(`gusset-engine-stamp: ${r.written ? `wrote ${stampFile(archiveDir)} (${r.reason})` : `no stamp written: ${r.reason}`}`);
    process.exit(r.written ? 0 : 3);
  }
}
