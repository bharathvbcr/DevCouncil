#!/usr/bin/env node
/**
 * The input stamp for libgusset.a: what a built archive was built from, so a
 * consumer can tell whether it still matches a checkout's sources.
 *
 *   node scripts/gusset-engine-stamp.mjs write <archive-dir> [<gusset-dir>]
 *
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
import { existsSync, readFileSync, realpathSync, renameSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const ENGINE = "rust/gusset-engine";
export const STAMP_NAME = "libgusset.stamp.json";

function git(cwd, ...args) {
  const r = spawnSync("git", ["-C", cwd, ...args], { encoding: "utf8" });
  if (r.status !== 0) throw new Error(`git ${args.join(" ")} (in ${cwd}): ${r.stderr.trim() || "failed"}`);
  return r.stdout.trim();
}

/** Repository-relative engine inputs: the engine, its path dependencies, the toolchain file. */
export function enginePaths(root) {
  const out = [ENGINE];
  const queue = [ENGINE];
  while (queue.length) {
    const rel = queue.shift();
    const toml = path.join(root, rel, "Cargo.toml");
    if (!existsSync(toml)) throw new Error(`${toml} is missing (a path dependency of the engine)`);
    for (const m of readFileSync(toml, "utf8").matchAll(/\bpath\s*=\s*"([^"]+)"/g)) {
      const dep = path.relative(root, path.resolve(root, rel, m[1])).split(path.sep).join("/");
      // Outside the repository is the sibling gusset: its commit is recorded instead.
      if (dep.startsWith("..") || path.isAbsolute(dep) || out.includes(dep)) continue;
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

export function writeStamp(root, gussetDir, archiveDir) {
  const { stamp } = engineInputs(root, gussetDir);
  const tmp = `${stampFile(archiveDir)}.${process.pid}.tmp`;
  writeFileSync(tmp, `${JSON.stringify(stamp, null, 2)}\n`);
  renameSync(tmp, stampFile(archiveDir));
  return stamp;
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
  if (cmd !== "write" || !archiveDir) {
    console.error("usage: gusset-engine-stamp.mjs write <archive-dir> [<gusset-dir>]");
    process.exit(2);
  }
  const root = realpathSync(git(path.dirname(fileURLToPath(import.meta.url)), "rev-parse", "--show-toplevel"));
  const s = writeStamp(root, path.resolve(gusset ?? process.env.GUSSET_DIR ?? path.join(root, "..", "gusset")), archiveDir);
  console.log(`gusset-engine-stamp: wrote ${stampFile(archiveDir)}${s.dirty ? " (DIRTY: not reusable by lane-env)" : ""}`);
}
