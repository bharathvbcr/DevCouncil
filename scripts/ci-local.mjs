#!/usr/bin/env node
/**
 * Local equivalent of the checks that must be green before a version tag.
 * A step that cannot run exits the same way as a step that failed — never
 * as a pass.
 */
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { tagNamesHead } from "./check-release.mjs";

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

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

function gofmtClean() {
  console.log("\n== gofmt ==");
  const result = spawnSync("gofmt", ["-l", "."], {
    cwd: path.join(REPO_ROOT, "backend", "go_orchestrator"),
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
 * Resolve a git rev to a commit sha, or null when it does not exist.
 * A git that cannot run at all is exit 2, never a pass.
 * @param {string} rev
 * @returns {string | null}
 */
function revParse(rev) {
  const result = spawnSync("git", ["rev-parse", "-q", "--verify", `${rev}^{commit}`], {
    cwd: REPO_ROOT,
    encoding: "utf8",
    shell: false,
  });
  if (result.error) {
    console.error(`FAIL: git rev-parse could not run: ${result.error.message}`);
    process.exit(2);
  }
  // Exit 1 with no output is "no such rev", which is a legitimate answer.
  const sha = (result.stdout || "").trim();
  return sha === "" ? null : sha;
}

/**
 * The tag path, which `release.yml` takes on a push and which nothing here
 * exercised before: on a tag push CI runs `check-release.mjs --tag <ref>`,
 * while the bare form below is only ever the pull_request path.
 *
 * It also checks the one release invariant CI *structurally cannot* see.
 * `release.yml` publishes `docs/releases/<tag>.md` from the tagged commit, so
 * if the tag is behind the notes, the stale notes ship and every gate passes
 * on them — they are internally consistent at that commit. The only place to
 * catch it is here, before the push.
 */
function releaseTagAgrees() {
  console.log("\n== release tag ==");
  const pkgPath = path.join(REPO_ROOT, "package.json");
  const version = JSON.parse(readFileSync(pkgPath, "utf8")).version;
  if (typeof version !== "string" || version === "") {
    console.error(`FAIL: no version in ${pkgPath}`);
    process.exit(2);
  }
  const tag = `v${version}`;
  const headSha = revParse("HEAD");
  if (!headSha) {
    console.error("FAIL: could not resolve HEAD");
    process.exit(2);
  }
  const verdict = tagNamesHead({ tag, tagSha: revParse(`refs/tags/${tag}`), headSha });
  if (!verdict.ok) {
    console.error(`FAIL: ${verdict.reason}`);
    process.exit(1);
  }
  console.log(`OK: ${tag} would publish the notes at HEAD`);
  run("release identity (tag path)", process.execPath, [
    "scripts/check-release.mjs",
    "--tag",
    tag,
  ]);
}

function main() {
  run("script tests", process.execPath, [
    "--test",
    "scripts/check-release.test.mjs",
    "scripts/check-workflows.test.mjs",
  ]);
  run("release identity", process.execPath, ["scripts/check-release.mjs"]);
  releaseTagAgrees();
  run("workflow lint", process.execPath, ["scripts/check-workflows.mjs"]);
  run("npm pack", "npm", ["run", "pack:check"]);
  run("npm runtime smoke", process.execPath, ["scripts/npm-runtime-smoke.mjs"]);
  gofmtClean();
  run("go host tests", "go", ["test", "./cmd/devcouncil", "-count=1"], {
    cwd: path.join(REPO_ROOT, "backend", "go_orchestrator"),
  });
  run("go orchestrator tests", "go", ["test", "./...", "-count=1"], {
    cwd: path.join(REPO_ROOT, "backend", "go_orchestrator"),
  });
  run("rust verify --quick", "bash", ["./verify.sh", "--quick"], {
    cwd: path.join(REPO_ROOT, "rust"),
  });
  console.log("\nOK: ci:local");
}

main();
