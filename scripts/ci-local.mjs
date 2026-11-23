#!/usr/bin/env node
/**
 * Local equivalent of the checks that must be green before a version tag.
 * A step that cannot run exits the same way as a step that failed — never
 * as a pass.
 *
 *   node scripts/ci-local.mjs                 stop at the first failing step
 *   node scripts/ci-local.mjs --continue      run every step whose needs passed
 *   node scripts/ci-local.mjs --only <step>   run <step> and the steps it needs
 *
 * The two new modes end with a summary block (PASS / FAIL / SKIP / N/A per
 * step) and exit 1 if any step failed, else 2 if any was skipped, else 0.
 * Without a flag the output and exit codes are what they always were.
 */
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  StepStop,
  devmapAvailable,
  expectedRowLabels,
  goModToolchain,
  engineCflags,
  gussetPinCheck,
  localTagState,
  nestedGateProblem,
  parseArgs,
  runSteps,
  tagVerdict,
} from "./ci-local-checks.mjs";

export const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
export const GO_MODULE = path.join(REPO_ROOT, "backend", "go_orchestrator");
export const ACCURACY_MODULE = path.join(REPO_ROOT, "tools", "accuracy");

/**
 * Print `FAIL: <message>` and return the stop for the step to throw.
 *
 * @param {number} code
 * @param {string} message
 * @param {{ cannotRun?: boolean, note?: string }} [opts]
 */
function fail(code, message, opts) {
  console.error(`FAIL: ${message}`);
  return new StepStop(code, message, opts);
}

/**
 * Run one command as the step's work, under the step's env (the base env plus
 * what its declared producers returned).
 *
 * @param {import("./ci-local-checks.mjs").StepContext} ctx
 * @param {string} command
 * @param {string[]} args
 * @param {{ cwd?: string, check?: (log: string) => string | undefined }} [opts]
 */
function exec(ctx, command, args, opts = {}) {
  ctx.header();
  if (opts.check) return execChecked(ctx, command, args, opts);
  const result = spawnSync(command, args, {
    cwd: opts.cwd ?? REPO_ROOT,
    env: ctx.env,
    stdio: "inherit",
    shell: false,
  });
  if (result.error) {
    throw fail(2, `${ctx.label} could not run: ${result.error.message}`, { cannotRun: true });
  }
  if (result.status !== 0) {
    const note = result.signal ? `signal ${result.signal}` : `exit ${result.status}`;
    throw fail(result.status ?? 1, `${ctx.label} (exit ${result.status})`, { note });
  }
}

/**
 * exec() for a nested gate whose exit 0 must be backed by its own output.
 * The output still streams to this terminal (stderr merged into stdout, and
 * through a pipe, as it is in any logged run); `tee` keeps a copy, and the
 * command's status is written to a file because a pipeline reports tee's.
 * After an exit 0, `check(log)` names why the run cannot be believed, or
 * returns undefined.
 *
 * @param {import("./ci-local-checks.mjs").StepContext} ctx
 * @param {string} command
 * @param {string[]} args
 * @param {{ cwd?: string, check: (log: string) => string | undefined }} opts
 */
function execChecked(ctx, command, args, opts) {
  const dir = mkdtempSync(path.join(tmpdir(), "ci-local-run-"));
  const logFile = path.join(dir, "log");
  const statusFile = path.join(dir, "status");
  try {
    const result = spawnSync(
      "sh",
      ["-c", 'st=$1; lg=$2; shift 2; { "$@"; echo $? > "$st"; } 2>&1 | tee "$lg"', "sh", statusFile, logFile, command, ...args],
      { cwd: opts.cwd ?? REPO_ROOT, env: ctx.env, stdio: "inherit", shell: false },
    );
    if (result.error) {
      throw fail(2, `${ctx.label} could not run: ${result.error.message}`, { cannotRun: true });
    }
    const status = existsSync(statusFile) ? Number.parseInt(readFileSync(statusFile, "utf8"), 10) : Number.NaN;
    if (Number.isNaN(status)) {
      throw fail(2, `${ctx.label} did not report an exit status`, { cannotRun: true });
    }
    if (status !== 0) {
      throw fail(status, `${ctx.label} (exit ${status})`, { note: `exit ${status}` });
    }
    const problem = opts.check(existsSync(logFile) ? readFileSync(logFile, "utf8") : "");
    if (problem !== undefined) {
      throw fail(1, `${ctx.label} is not believed: ${problem}`, { note: `unverified: ${problem}` });
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
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
  const verdict = gussetPinCheck(REPO_ROOT, sibling);
  if (!verdict.ok) {
    throw fail(1, verdict.message);
  }
  console.log(`ok: ${verdict.message}`);
}

/**
 * The host links a sibling Gusset checkout and a gitignored staticlib.
 * `go test` without either fails as a missing replace directory, which reads
 * like a Go bug. Name the missing checkout, and build the archive when the
 * checkout is already there.
 *
 * Returns the CGO_CFLAGS that key Go's caches on the archive, for the steps
 * that declare this one in `needs`.
 *
 * @param {import("./ci-local-checks.mjs").StepContext} ctx
 */
function ensureGussetEngine(ctx) {
  const siblingMod = path.resolve(REPO_ROOT, "..", "gusset", "go.mod");
  if (!existsSync(siblingMod)) {
    throw fail(
      2,
      `Go replace expects a Gusset checkout at ${path.dirname(siblingMod)}. ` +
        "CI checks out https://github.com/bharathvbcr/gusset in .github/actions/setup-gusset. " +
        "From a linked worktree (.gitpulse/worktrees/<lane>) run the gate through " +
        "`node scripts/lane-env.mjs -- <command>` (see its --help) instead.",
      { cannotRun: true },
    );
  }
  requireGussetAtPin(path.dirname(siblingMod));
  exec(ctx, "cargo", [
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
    throw fail(1, `${archive} is missing after cargo build`);
  }
  // Go hashes cgo flags, not the libraries they link: key the caches on the archive.
  return { env: { CGO_CFLAGS: engineCflags(archive, ctx.env) } };
}

/**
 * gofmt output differs between Go releases, and CI runs the one go.mod
 * declares (setup-gusset's neighbours use setup-go with go-version-file). A
 * newer local gofmt called a file clean that CI then rejected, so resolve the
 * declared toolchain's own gofmt.
 *
 * @param {import("./ci-local-checks.mjs").StepContext} ctx
 */
function gofmtClean(ctx) {
  ctx.header();
  const toolchain = goModToolchain(readFileSync(path.join(GO_MODULE, "go.mod"), "utf8"));
  if (!toolchain) {
    throw fail(2, "no go or toolchain line in backend/go_orchestrator/go.mod", { cannotRun: true });
  }
  const root = spawnSync("go", ["env", "GOROOT"], {
    cwd: GO_MODULE,
    env: { ...ctx.env, GOTOOLCHAIN: toolchain },
    encoding: "utf8",
    shell: false,
  });
  const goroot = root.status === 0 ? root.stdout.trim() : "";
  if (!goroot) {
    throw fail(2, `could not resolve ${toolchain} (${(root.stderr || "").trim()}); gofmt was not run`, { cannotRun: true });
  }
  const gofmt = path.join(goroot, "bin", process.platform === "win32" ? "gofmt.exe" : "gofmt");
  console.log(`gofmt from ${toolchain}`);
  for (const cwd of [GO_MODULE, ACCURACY_MODULE]) {
    const result = spawnSync(gofmt, ["-l", "."], {
      cwd,
      env: ctx.env,
      encoding: "utf8",
      shell: false,
    });
    if (result.error) {
      throw fail(2, `gofmt could not run: ${result.error.message}`, { cannotRun: true });
    }
    const extra = (result.stdout || "").trim();
    if (result.status !== 0 || extra) {
      if (extra) console.error(extra);
      throw fail(1, `gofmt would rewrite files in ${path.relative(REPO_ROOT, cwd)}`);
    }
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
 *
 * @param {import("./ci-local-checks.mjs").StepContext} ctx
 */
function releaseTagAgrees(ctx) {
  const pkgPath = path.join(REPO_ROOT, "package.json");
  const version = JSON.parse(readFileSync(pkgPath, "utf8")).version;
  if (typeof version !== "string" || version === "") {
    throw fail(2, `no version in ${pkgPath}`, { cannotRun: true });
  }
  const state = localTagState(REPO_ROOT, version);
  const verdict = tagVerdict(state);
  if (verdict === "NOT RUN") {
    // No local tag: nothing to compare with HEAD. The owner creates and pushes
    // the tag at the release commit once every workflow is green.
    ctx.header();
    console.log(`NOT RUN: no local tag ${state.tag}`);
    return { na: `no local tag ${state.tag}` };
  }
  if (verdict === "FAIL" && state.state === "unknown") {
    throw fail(2, state.reason ?? "git could not read the local tag", { cannotRun: true });
  }
  // off-head fails inside dcrelease with the notes-behind-the-tag explanation.
  exec(ctx, dcreleaseFrom(ctx), ["--tag", state.tag, "--tag-at-head"]);
}

/**
 * The binary `build dcrelease` returned. Reachable only through `needs`.
 *
 * @param {import("./ci-local-checks.mjs").StepContext} ctx
 */
function dcreleaseFrom(ctx) {
  const bin = ctx.paths["build dcrelease"]?.dcrelease;
  if (!bin) throw new Error(`${ctx.label} does not declare \`build dcrelease\` in needs`);
  return bin;
}

/**
 * A step that runs one command. The command is data so the tests can read it
 * without running it.
 *
 * @param {string} label
 * @param {string} command
 * @param {string[]} args
 * @param {{ cwd?: string, needs?: string[], requires?: Step["requires"] }} [opts]
 * @returns {Step}
 */
function commandStep(label, command, args, opts = {}) {
  const cwd = opts.cwd ?? REPO_ROOT;
  return { label, needs: opts.needs ?? [], requires: opts.requires, command, args, cwd, run: (ctx) => exec(ctx, command, args, { cwd }) };
}

const VERIFY_ARGS = ["./verify.sh", "--quick"];

/**
 * The rust/verify.sh step, with the directory as a parameter so a test can run
 * it against a fake script.
 *
 * @param {string} cwd the directory holding verify.sh
 * @returns {Step}
 */
export function rustVerifyStep(cwd) {
  /** The steps verify.sh declares, read when the run ends so the script that ran is the one read. */
  const declaredSteps = (/** @type {string} */ dir) => {
    try {
      return expectedRowLabels(readFileSync(path.join(dir, "verify.sh"), "utf8"));
    } catch {
      return undefined;
    }
  };
  return {
    label: "rust verify --quick",
    needs: [],
    command: "bash",
    args: VERIFY_ARGS,
    cwd,
    // In the new modes verify.sh keeps going too (cargo test --no-fail-fast),
    // and prints its rows as a nested block without totals.
    run: (ctx) =>
      exec(ctx, "bash", ctx.mode === "default" ? VERIFY_ARGS : [...VERIFY_ARGS, "--continue", "--nested"], {
        cwd,
        check: (log) => nestedGateProblem(log, { nested: ctx.mode !== "default", labels: declaredSteps(cwd) }),
      }),
  };
}

/**
 * Every ci:local step, in order. `needs` names the steps whose output a step
 * uses; the runner hands a step only its declared producers' `env` and
 * `paths`, and in the new modes runs it only when they passed.
 *
 * @typedef {import("./ci-local-checks.mjs").RunnableStep & { command?: string, args?: string[], cwd?: string }} Step
 * @type {Step[]}
 */
export const STEPS = [
  commandStep("script tests", process.execPath, ["--test", "scripts/check-workflows.test.mjs", "scripts/sync-agent-defs.test.mjs", "scripts/ci-local.test.mjs", "scripts/code-map-bundle.test.mjs"]),
  // The page shell is TypeScript bundled by the pinned Bun; the checked-in bundle must be what it builds.
  commandStep("code map bundle", process.execPath, ["scripts/code-map-bundle.mjs", "check"]),
  commandStep("code map tests", "bun", ["test", "rust/devmap-query/assets/code-map"]),
  commandStep("release gate tests", "go", ["test", "./devcouncil/release", "./cmd/dcrelease", "-count=1"], { cwd: GO_MODULE }),
  {
    label: "build dcrelease",
    needs: [],
    // Built, not `go run`: `go run` reports every non-zero exit as 1, and this
    // gate's 2 ("could not run") must not read as a mismatch.
    run: (ctx) => {
      const binDir = mkdtempSync(path.join(tmpdir(), "devcouncil-dcrelease-"));
      process.on("exit", () => rmSync(binDir, { recursive: true, force: true }));
      const dcrelease = path.join(binDir, process.platform === "win32" ? "dcrelease.exe" : "dcrelease");
      exec(ctx, "go", ["build", "-o", dcrelease, "./cmd/dcrelease"], { cwd: GO_MODULE });
      return { paths: { dcrelease } };
    },
  },
  { label: "release identity", needs: ["build dcrelease"], run: (ctx) => exec(ctx, dcreleaseFrom(ctx), []) },
  { label: "release identity (tag path)", needs: ["build dcrelease"], run: releaseTagAgrees },
  commandStep("workflow lint", process.execPath, ["scripts/check-workflows.mjs"]),
  commandStep("npm pack", "npm", ["run", "pack:check"]),
  commandStep("npm runtime smoke", process.execPath, ["scripts/npm-runtime-smoke.mjs"]),
  { label: "gofmt", needs: [], run: gofmtClean },
  { label: "gusset engine", needs: [], run: ensureGussetEngine },
  commandStep("go host tests", "go", ["test", "./cmd/devcouncil", "-count=1"], { cwd: GO_MODULE, needs: ["gusset engine"] }),
  // The DevMap test selector's end-to-end tests (selected-tests.yml runs them
  // on every push). They are behind the e2e tag, so `./...` below does not
  // compile them, and this is their only run here; the selector's unit tests
  // are in `./...`. They drive the real `devmap` on PATH and fail, not skip,
  // without one; --continue and --only report that as SKIP before starting.
  commandStep("test selector end-to-end", "go", ["test", "-tags", "e2e", "./cmd/dctestselect", "-run", "^TestE2E", "-count=1", "-p", "1", "-timeout", "30m"], {
    cwd: GO_MODULE,
    requires: (env) => (devmapAvailable(env) ? undefined : "devmap not found: not on PATH and MANVI_MAP_BINARY is unset"),
  }),
  // `./...` includes ./cmd/devcouncil and ./gussetfn, which link libgusset.a.
  commandStep("go orchestrator tests", "go", ["test", "./...", "-count=1"], { cwd: GO_MODULE, needs: ["gusset engine"] }),
  // The go/types labeller behind the DevMap corpus record, its own module.
  commandStep("accuracy labeller vet", "go", ["vet", "./..."], { cwd: ACCURACY_MODULE }),
  commandStep("accuracy labeller tests", "go", ["test", "./...", "-count=1"], { cwd: ACCURACY_MODULE }),
  rustVerifyStep(path.join(REPO_ROOT, "rust")),
];

function main() {
  /** @type {ReturnType<typeof parseArgs>} */
  let opts;
  try {
    opts = parseArgs(process.argv.slice(2));
  } catch (e) {
    console.error(`ci:local: ${e instanceof Error ? e.message : String(e)}`);
    process.exit(2);
  }
  /** @type {ReturnType<typeof runSteps>} */
  let result;
  try {
    result = runSteps(STEPS, { ...opts, gate: "DevCouncil ci:local" });
  } catch (e) {
    if (e instanceof StepStop) throw e;
    console.error(`ci:local: ${e instanceof Error ? e.message : String(e)}`);
    process.exit(2);
  }
  if (opts.mode !== "default") {
    process.exitCode = result.code;
    return;
  }
  if (result.code !== 0) process.exit(result.code);
  // Steps that could not run. They are listed at the end and never counted as passes.
  const notRun = result.rows.filter((r) => r.status === "N/A");
  if (notRun.length > 0) {
    console.log(`\nOK: ci:local, with ${notRun.length} step(s) NOT RUN:`);
    for (const r of notRun) console.log(`  - ${r.label}: ${r.note.replace(/^not applicable: /, "")}`);
    return;
  }
  console.log("\nOK: ci:local");
}

// Run only as a program: the tests import STEPS.
const entry = process.argv[1];
if (entry !== undefined && realpathSync(path.resolve(entry)) === realpathSync(fileURLToPath(import.meta.url))) {
  main();
}
