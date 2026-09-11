#!/usr/bin/env node
/**
 * Post-publish registry smoke for F-11.
 *
 * Waits until the tagged version is visible on the npm registry, installs it
 * into a clean temp prefix, and verifies `devcouncil --help` / `dev version`.
 *
 * Usage:
 *   node scripts/npm-registry-smoke.mjs [--version 0.4.1] [--timeout-sec 600]
 *
 * Env:
 *   NPM_REGISTRY_SMOKE_VERSION  — package version (default: package.json)
 *   NPM_REGISTRY_SMOKE_TIMEOUT  — seconds to wait (default: 600)
 *   NPM_REGISTRY_SMOKE_PACKAGE  — package name (default: package.json name)
 */
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "..");
const npmCommand = process.platform === "win32" ? "cmd.exe" : "npm";
const npmPrefix = process.platform === "win32" ? ["/d", "/s", "/c", "npm"] : [];

function readPackageJson() {
  return JSON.parse(readFileSync(path.join(repoRoot, "package.json"), "utf-8"));
}

function parseArgs(argv) {
  const out = {
    version: process.env.NPM_REGISTRY_SMOKE_VERSION || "",
    timeoutSec: Number(process.env.NPM_REGISTRY_SMOKE_TIMEOUT || 600),
    packageName: process.env.NPM_REGISTRY_SMOKE_PACKAGE || "",
    help: false,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--help" || arg === "-h") {
      out.help = true;
    } else if (arg === "--version") {
      out.version = argv[++i] || "";
    } else if (arg === "--timeout-sec") {
      out.timeoutSec = Number(argv[++i] || out.timeoutSec);
    } else if (arg === "--package") {
      out.packageName = argv[++i] || "";
    }
  }
  return out;
}

function run(command, args, options = {}) {
  return spawnSync(command, args, {
    cwd: options.cwd ?? repoRoot,
    env: options.env ?? process.env,
    encoding: "utf-8",
    shell: false,
    stdio: options.stdio ?? "pipe",
  });
}

function runNpm(args, options = {}) {
  return run(npmCommand, [...npmPrefix, ...args], options);
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function assertOk(result, label) {
  if (result.status !== 0) {
    throw new Error(
      `${label} failed with exit ${result.status}\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
    );
  }
}

async function waitForRegistry(packageName, version, timeoutSec) {
  const deadline = Date.now() + timeoutSec * 1000;
  let attempt = 0;
  while (Date.now() < deadline) {
    attempt += 1;
    const result = runNpm(["view", `${packageName}@${version}`, "version"], {
      env: { ...process.env, npm_config_fetch_retries: "0" },
    });
    const viewed = String(result.stdout || "").trim();
    if (result.status === 0 && viewed === version) {
      console.log(`registry has ${packageName}@${version} (attempt ${attempt})`);
      return;
    }
    const remaining = Math.max(0, Math.round((deadline - Date.now()) / 1000));
    console.log(
      `waiting for ${packageName}@${version} on registry (attempt ${attempt}, ${remaining}s left)`,
    );
    await sleep(Math.min(15000, 2000 + attempt * 1000));
  }
  throw new Error(
    `Timed out after ${timeoutSec}s waiting for ${packageName}@${version} on the npm registry`,
  );
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.help) {
    console.log(
      "Usage: node scripts/npm-registry-smoke.mjs [--version X.Y.Z] [--timeout-sec N] [--package name]",
    );
    return;
  }

  const pkg = readPackageJson();
  const packageName = args.packageName || pkg.name;
  const version = args.version || pkg.version;
  if (!packageName || !version) {
    throw new Error("package name and version are required");
  }

  await waitForRegistry(packageName, version, args.timeoutSec);

  const prefix = mkdtempSync(path.join(tmpdir(), "devcouncil-registry-smoke-"));
  try {
    const install = runNpm(
      ["install", "-g", `${packageName}@${version}`, `--prefix=${prefix}`],
      { env: { ...process.env, npm_config_update_notifier: "false" } },
    );
    assertOk(install, `npm install -g ${packageName}@${version}`);

    const binDir = path.join(prefix, "bin");
    const help = run(path.join(binDir, "devcouncil"), ["--help"], {
      env: { ...process.env, PATH: `${binDir}${path.delimiter}${process.env.PATH || ""}` },
    });
    const helpText = `${help.stdout || ""}${help.stderr || ""}`;
    if (help.status === 0) {
      if (!helpText.trim()) {
        throw new Error("devcouncil --help produced empty output");
      }
    } else {
      if (!/devcouncil binary not found|requires the devcouncil binary/i.test(helpText)) {
        throw new Error(
          `devcouncil --help failed without naming the missing Go binary\n${helpText}`,
        );
      }
    }

    const versionCmd = run(path.join(binDir, "dev"), ["version"], {
      env: { ...process.env, PATH: `${binDir}${path.delimiter}${process.env.PATH || ""}` },
    });
    const versionOut = `${versionCmd.stdout || ""}${versionCmd.stderr || ""}`;
    if (versionCmd.status === 0) {
      if (!versionOut.includes(version.split(".")[0])) {
        console.warn(`warning: version output did not clearly include ${version}: ${versionOut}`);
      }
    } else if (!/devcouncil binary not found|requires the devcouncil binary/i.test(versionOut)) {
      throw new Error(`dev version failed unexpectedly\n${versionOut}`);
    }

    console.log(`registry smoke passed for ${packageName}@${version}`);
  } finally {
    rmSync(prefix, { recursive: true, force: true });
  }
}

main().catch((err) => {
  console.error(err instanceof Error ? err.message : String(err));
  process.exit(1);
});
