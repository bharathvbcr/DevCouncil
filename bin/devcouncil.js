#!/usr/bin/env node

const { spawnSync } = require("node:child_process");
const { existsSync } = require("node:fs");
const path = require("node:path");

const packageRoot = path.resolve(__dirname, "..");
const pyproject = path.join(packageRoot, "pyproject.toml");

function run(command, args) {
  return spawnSync(command, args, {
    cwd: process.cwd(),
    stdio: "inherit",
    shell: process.platform === "win32",
    env: process.env,
  });
}

function fail(message) {
  console.error(message);
  process.exit(1);
}

function ensureUv() {
  const check = spawnSync("uv", ["--version"], {
    stdio: "ignore",
    shell: process.platform === "win32",
  });

  if (check.status === 0) {
    return;
  }

  fail(
    [
      "DevCouncil requires uv to run from the npm package.",
      "Install uv first:",
      "  macOS/Linux: curl -LsSf https://astral.sh/uv/install.sh | sh",
      '  Windows:     powershell -ExecutionPolicy ByPass -c "irm https://astral.sh/uv/install.ps1 | iex"',
      "",
      "Then rerun:",
      "  devcouncil --help",
    ].join("\n")
  );
}

if (!existsSync(pyproject)) {
  fail(
    "DevCouncil npm package is missing pyproject.toml. Reinstall the package and try again."
  );
}

ensureUv();

const args = process.argv.slice(2);
const result = run("uv", ["run", "--project", packageRoot, "devcouncil", ...args]);

if (result.error) {
  fail(`Failed to start DevCouncil: ${result.error.message}`);
}

process.exit(result.status ?? 1);
