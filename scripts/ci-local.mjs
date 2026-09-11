#!/usr/bin/env node
/**
 * Local equivalent of the checks that must be green before a version tag.
 * A step that cannot run exits the same way as a step that failed — never
 * as a pass.
 */
import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

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

function main() {
  run("script tests", process.execPath, [
    "--test",
    "scripts/check-release.test.mjs",
    "scripts/check-workflows.test.mjs",
  ]);
  run("release identity", process.execPath, ["scripts/check-release.mjs"]);
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
