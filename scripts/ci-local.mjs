#!/usr/bin/env node
/**
 * Local equivalent of the checks that must be green before a version tag.
 * A step that cannot run exits the same way as a step that failed — never
 * as a pass.
 */
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const GO_MODULE = path.join(REPO_ROOT, "backend", "go_orchestrator");

/**
 * @param {string} label
 * @param {string} command
 * @param {string[]} args
 * @param {{ cwd?: string, env?: NodeJS.ProcessEnv }} [opts]
 */
function run(label, command, args, opts = {}) {
  console.log(`\n== ${label} ==`);
  const result = spawnSync(command, args, {
    cwd: opts.cwd ?? REPO_ROOT,
    env: opts.env ?? process.env,
    stdio: "inherit",
    shell: false,
  });
  if (result.error) {
    console.error(`FAIL: ${label} could not run: ${result.error.message}`);
    process.exit(2);
  }
  if (result.status !== 0) {
    console.error(`FAIL: ${label} (exit ${result.status})`);
    process.exit(result.status ?? 1);
  }
}

/**
 * rust/gusset-engine/Cargo.lock records the Gusset version at the revision
 * setup-gusset pins. A sibling at another revision with another version
 * makes the `--locked` build below fail with "cannot update the lock file",
 * which reads like a stale DevCouncil lock. Measured 2026-09-27: a sibling
 * at Gusset v0.0.2 against the 0.0.1 pin. Say which it is before building.
 *
 * @param {string} sibling
 */
function warnIfGussetOffPin(sibling) {
  const actionPath = path.join(REPO_ROOT, ".github", "actions", "setup-gusset", "action.yml");
  const pin = existsSync(actionPath)
    ? /^\s*ref:\s*([0-9a-f]{40})\s*$/m.exec(readFileSync(actionPath, "utf8"))?.[1]
    : undefined;
  const head = spawnSync("git", ["-C", sibling, "rev-parse", "HEAD"], { encoding: "utf8", shell: false });
  const sha = head.status === 0 ? head.stdout.trim() : "";
  if (pin && sha && sha !== pin) {
    console.warn(
      `WARN: Gusset checkout ${sibling} is at ${sha.slice(0, 12)}, but CI pins ${pin.slice(0, 12)} ` +
        "(.github/actions/setup-gusset/action.yml). If the --locked build below fails on the " +
        `gusset entry, check out the pin (git -C ${sibling} checkout ${pin.slice(0, 12)}); ` +
        "the DevCouncil lock is not what moved.",
    );
  }
}

/**
 * The host links a sibling Gusset checkout and a gitignored staticlib.
 * `go test` without either fails as a missing replace directory, which reads
 * like a Go bug. Name the missing checkout, and build the archive when the
 * checkout is already there.
 */
function ensureGussetEngine() {
  const siblingMod = path.resolve(REPO_ROOT, "..", "gusset", "go.mod");
  if (!existsSync(siblingMod)) {
    console.error(
      `FAIL: Go replace expects a Gusset checkout at ${path.dirname(siblingMod)}. ` +
        "CI checks out https://github.com/bharathvbcr/gusset in .github/actions/setup-gusset.",
    );
    process.exit(2);
  }
  warnIfGussetOffPin(path.dirname(siblingMod));
  run("gusset engine", "cargo", [
    "build",
    "--release",
    "--locked",
    "--manifest-path",
    "rust/gusset-engine/Cargo.toml",
    // Where gussetfn's #cgo -L and the hash below look, whatever
    // CARGO_TARGET_DIR says.
    "--target-dir",
    "rust/gusset-engine/target",
  ]);
  const archive = path.join(REPO_ROOT, "rust", "gusset-engine", "target", "release", "libgusset.a");
  if (!existsSync(archive)) {
    console.error(`FAIL: ${archive} is missing after cargo build`);
    process.exit(1);
  }
  // Key Go's caches on the archive, as rust/gusset-engine/cgo-env.sh does:
  // Go hashes cgo flags, not the libraries they link, so a rebuilt archive
  // is otherwise not relinked and a cached "ok" is replayed against the old one.
  const sum = createHash("sha256").update(readFileSync(archive)).digest("hex").slice(0, 32);
  // `go env` reports the process env, a `go env -w` setting, or Go's default.
  const goEnv = spawnSync("go", ["env", "CGO_CFLAGS"], { encoding: "utf8", shell: false });
  const current = goEnv.status === 0 ? goEnv.stdout.trim() : (process.env.CGO_CFLAGS || "-O2 -g");
  const base = current
    .replace(/(^| )-DDEVCOUNCIL_GUSSET_ENGINE_SHA256=\S*/g, "")
    .trim();
  process.env.CGO_CFLAGS = `${base ? `${base} ` : ""}-DDEVCOUNCIL_GUSSET_ENGINE_SHA256=${sum}`;
}

function gofmtClean() {
  console.log("\n== gofmt ==");
  const result = spawnSync("gofmt", ["-l", "."], {
    cwd: GO_MODULE,
    encoding: "utf8",
    shell: false,
  });
  if (result.error) {
    console.error(`FAIL: gofmt could not run: ${result.error.message}`);
    process.exit(2);
  }
  const extra = (result.stdout || "").trim();
  if (result.status !== 0 || extra) {
    if (extra) console.error(extra);
    console.error("FAIL: gofmt would rewrite files");
    process.exit(1);
  }
}

/**
 * The tag path, which `release.yml` takes on a push and which nothing here
 * exercised before: on a tag push CI runs `cmd/dcrelease --tag <ref>`, while
 * the bare form below is only ever the pull_request path.
 *
 * `--tag-at-head` also checks the one release invariant CI *structurally
 * cannot* see. `release.yml` publishes `docs/releases/<tag>.md` from the
 * tagged commit, so if the tag is behind the notes, the stale notes ship and
 * every gate passes on them — they are internally consistent at that commit.
 * The only place to catch it is here, before the push.
 */
function releaseTagAgrees(dcrelease) {
  const pkgPath = path.join(REPO_ROOT, "package.json");
  const version = JSON.parse(readFileSync(pkgPath, "utf8")).version;
  if (typeof version !== "string" || version === "") {
    console.error(`FAIL: no version in ${pkgPath}`);
    process.exit(2);
  }
  run("release identity (tag path)", dcrelease, ["--tag", `v${version}`, "--tag-at-head"]);
}

function main() {
  run("script tests", process.execPath, ["--test", "scripts/check-workflows.test.mjs"]);
  run("release gate tests", "go", ["test", "./devcouncil/release", "./cmd/dcrelease", "-count=1"], {
    cwd: GO_MODULE,
  });
  // Built, not `go run`: `go run` reports every non-zero exit as 1, and this
  // gate's 2 ("could not run") must not read as a mismatch.
  const binDir = mkdtempSync(path.join(tmpdir(), "devcouncil-dcrelease-"));
  process.on("exit", () => rmSync(binDir, { recursive: true, force: true }));
  const dcrelease = path.join(binDir, process.platform === "win32" ? "dcrelease.exe" : "dcrelease");
  run("build dcrelease", "go", ["build", "-o", dcrelease, "./cmd/dcrelease"], { cwd: GO_MODULE });
  run("release identity", dcrelease, []);
  releaseTagAgrees(dcrelease);
  run("workflow lint", process.execPath, ["scripts/check-workflows.mjs"]);
  run("npm pack", "npm", ["run", "pack:check"]);
  run("npm runtime smoke", process.execPath, ["scripts/npm-runtime-smoke.mjs"]);
  gofmtClean();
  ensureGussetEngine();
  run("go host tests", "go", ["test", "./cmd/devcouncil", "-count=1"], {
    cwd: GO_MODULE,
  });
  run("go orchestrator tests", "go", ["test", "./...", "-count=1"], {
    cwd: GO_MODULE,
  });
  run("rust verify --quick", "bash", ["./verify.sh", "--quick"], {
    cwd: path.join(REPO_ROOT, "rust"),
  });
  console.log("\nOK: ci:local");
}

main();
