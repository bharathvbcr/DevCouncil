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
import {
  goModToolchain,
  gussetPin,
  gussetPinVerdict,
  localTagState,
  tagVerdict,
} from "./ci-local-checks.mjs";

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const GO_MODULE = path.join(REPO_ROOT, "backend", "go_orchestrator");

/** Steps that could not run. They are listed at the end and never counted as passes. */
const notRun = [];

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
 * CI builds Gusset at the revision setup-gusset pins, and
 * rust/gusset-engine/Cargo.lock records that revision's version. A sibling at
 * any other revision either fails the `--locked` build below with "cannot
 * update the lock file" (which reads like a stale DevCouncil lock) or, worse,
 * builds green here against code CI never sees. Fail before building.
 *
 * @param {string} sibling
 */
function requireGussetAtPin(sibling) {
  const actionPath = path.join(REPO_ROOT, ".github", "actions", "setup-gusset", "action.yml");
  const pin = existsSync(actionPath) ? gussetPin(readFileSync(actionPath, "utf8")) : undefined;
  const head = spawnSync("git", ["-C", sibling, "rev-parse", "HEAD"], { encoding: "utf8", shell: false });
  const verdict = gussetPinVerdict(pin, head.status === 0 ? head.stdout.trim() : "");
  if (!verdict.ok) {
    console.error(`FAIL: ${verdict.message}`);
    process.exit(1);
  }
  console.log(`ok: ${verdict.message}`);
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
  requireGussetAtPin(path.dirname(siblingMod));
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

/**
 * gofmt output differs between Go releases, and CI runs the one go.mod
 * declares (setup-gusset's neighbours use setup-go with go-version-file). A
 * newer local gofmt called a file clean that CI then rejected, so resolve the
 * declared toolchain's own gofmt.
 */
function gofmtClean() {
  console.log("\n== gofmt ==");
  const toolchain = goModToolchain(readFileSync(path.join(GO_MODULE, "go.mod"), "utf8"));
  if (!toolchain) {
    console.error("FAIL: no go or toolchain line in backend/go_orchestrator/go.mod");
    process.exit(2);
  }
  const root = spawnSync("go", ["env", "GOROOT"], {
    cwd: GO_MODULE,
    env: { ...process.env, GOTOOLCHAIN: toolchain },
    encoding: "utf8",
    shell: false,
  });
  const goroot = root.status === 0 ? root.stdout.trim() : "";
  if (!goroot) {
    console.error(`FAIL: could not resolve ${toolchain} (${(root.stderr || "").trim()}); gofmt was not run`);
    process.exit(2);
  }
  const gofmt = path.join(goroot, "bin", process.platform === "win32" ? "gofmt.exe" : "gofmt");
  console.log(`gofmt from ${toolchain}`);
  const result = spawnSync(gofmt, ["-l", "."], {
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
  const state = localTagState(REPO_ROOT, version);
  const verdict = tagVerdict(state);
  if (verdict === "NOT RUN") {
    // No local tag: nothing to compare with HEAD. The owner creates and pushes
    // the tag at the release commit once every workflow is green.
    console.log(`\n== release identity (tag path) ==\nNOT RUN: no local tag ${state.tag}`);
    notRun.push(`release identity (tag path): no local tag ${state.tag}`);
    return;
  }
  if (verdict === "FAIL" && state.state === "unknown") {
    console.error(`FAIL: ${state.reason}`);
    process.exit(2);
  }
  // off-head fails inside dcrelease with the notes-behind-the-tag explanation.
  run("release identity (tag path)", dcrelease, ["--tag", state.tag, "--tag-at-head"]);
}

function main() {
  run("script tests", process.execPath, ["--test", "scripts/check-workflows.test.mjs", "scripts/sync-agent-defs.test.mjs", "scripts/ci-local.test.mjs"]);
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
  if (notRun.length > 0) {
    console.log(`\nOK: ci:local, with ${notRun.length} step(s) NOT RUN:`);
    for (const line of notRun) console.log(`  - ${line}`);
    return;
  }
  console.log("\nOK: ci:local");
}

main();
