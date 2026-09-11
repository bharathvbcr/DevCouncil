#!/usr/bin/env node
/**
 * Smoke the published npm shape: a JS shim that execs Go `devcouncil`.
 * `map` / `graph` / `ast` are forwarded to that binary (which execs `devmap`).
 * Never requires Python or uv.
 */
import { spawnSync, execFileSync } from "node:child_process";
import {
  chmodSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "..");
const npmCommand = process.platform === "win32" ? "cmd.exe" : "npm";
const npmPrefix = process.platform === "win32" ? ["/d", "/s", "/c", "npm"] : [];

function run(command, args, options = {}) {
  return spawnSync(command, args, {
    cwd: options.cwd ?? repoRoot,
    env: options.env ?? process.env,
    encoding: "utf-8",
    shell: options.shell ?? false,
  });
}

function runNpm(args, options = {}) {
  return run(npmCommand, [...npmPrefix, ...args], options);
}

function assertOk(result, label) {
  if (result.status !== 0) {
    throw new Error(
      `${label} failed with exit ${result.status}\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
    );
  }
}

function assertIncludes(text, expected, label) {
  if (!text.includes(expected)) {
    throw new Error(`${label} did not include ${JSON.stringify(expected)}\n${text}`);
  }
}

function assertPackedFile(packMetadata, pathName) {
  const files = packMetadata.files ?? [];
  if (!files.some((file) => file.path === pathName)) {
    throw new Error(`npm package is missing required file ${pathName}`);
  }
}

function assertNotPacked(packMetadata, pathName) {
  const files = packMetadata.files ?? [];
  if (files.some((file) => file.path === pathName || file.path.startsWith(pathName))) {
    throw new Error(`npm package still ships ${pathName}`);
  }
}

function smokeNpmEnv(extra) {
  return {
    ...process.env,
    npm_config_ignore_scripts: "true",
    npm_config_allow_scripts: "",
    ...(extra || {}),
  };
}

function shimEnv(extra) {
  const env = { ...process.env, ...extra };
  if (!Object.prototype.hasOwnProperty.call(extra, "DEVCOUNCIL_BIN")) {
    delete env.DEVCOUNCIL_BIN;
  }
  if (!Object.prototype.hasOwnProperty.call(extra, "DEVMAP_BIN")) {
    delete env.DEVMAP_BIN;
  }
  return env;
}

function main() {
  const packed = runNpm(["pack", "--dry-run", "--json"], { env: smokeNpmEnv() });
  assertOk(packed, "npm pack --dry-run --json");
  const parsed = JSON.parse(packed.stdout.trim());
  const metadata = Array.isArray(parsed) ? parsed[0] : parsed;
  assertPackedFile(metadata, "bin/devcouncil.js");
  assertPackedFile(metadata, "README.md");
  assertPackedFile(metadata, "LICENSE");
  assertNotPacked(metadata, "pyproject.toml");
  assertNotPacked(metadata, "uv.lock");
  assertNotPacked(metadata, "src/devcouncil/cli/main.py");
  assertNotPacked(metadata, "scripts/build-week-demo.sh");
  assertNotPacked(metadata, "examples/build-week-demo/calc.py");

  const prefix = mkdtempSync(path.join(tmpdir(), "devcouncil-npm-smoke-"));
  try {
    const tarball = execFileSync("npm", ["pack", "--pack-destination", prefix], {
      cwd: repoRoot,
      encoding: "utf-8",
    }).trim().split("\n").filter(Boolean).at(-1);
    const tarballPath = path.join(prefix, path.basename(tarball.trim()));
    const install = runNpm(
      ["install", "-g", tarballPath, `--prefix=${prefix}`],
      { env: smokeNpmEnv({ npm_config_update_notifier: "false" }) },
    );
    assertOk(install, "npm install packed tarball");

    const binDir = path.join(prefix, "bin");
    const shim = path.join(binDir, "devcouncil");
    const nodeDir = path.dirname(process.execPath);
    const isolatedPath = `${binDir}${path.delimiter}${nodeDir}${path.delimiter}${path.join(prefix, "empty-path")}`;
    mkdirSync(path.join(prefix, "empty-path"), { recursive: true });

    const missing = run(process.execPath, [shim, "--help"], {
      env: shimEnv({ PATH: isolatedPath, HOME: prefix, USERPROFILE: prefix }),
    });
    if (missing.status === 0) {
      throw new Error("shim --help succeeded without a Go binary");
    }
    const missingText = `${missing.stdout || ""}${missing.stderr || ""}`;
    assertIncludes(missingText, "requires the devcouncil binary", "missing-binary message");
    if (/\buv\b/.test(missingText.toLowerCase()) && missingText.toLowerCase().includes("requires uv")) {
      throw new Error(`shim still requires uv:\n${missingText}`);
    }

    const fakeGo = path.join(prefix, "fake-devcouncil");
    const argvFile = path.join(prefix, "go-argv.txt");
    writeFileSync(
      fakeGo,
      [
        "#!/bin/sh",
        'if [ -n "$FAKE_ARGV_FILE" ]; then',
        '  : > "$FAKE_ARGV_FILE"',
        '  for a in "$@"; do printf \'%s\\n\' "$a" >> "$FAKE_ARGV_FILE"; done',
        "fi",
        'if [ "$1" = "version" ] || [ "$1" = "--version" ] || [ "$1" = "-V" ]; then echo \'devcouncil 0.2.0\'; exit 0; fi',
        "echo 'devcouncil — DevCouncil host binary (Phase 7)' >&2",
        "exit 0",
        "",
      ].join("\n"),
    );
    chmodSync(fakeGo, 0o755);

    const localBin = path.join(prefix, ".local", "bin");
    mkdirSync(localBin, { recursive: true });
    const homeGo = path.join(localBin, "devcouncil");
    writeFileSync(homeGo, readFileSync(fakeGo));
    chmodSync(homeGo, 0o755);

    const viaHome = run(process.execPath, [shim, "--help"], {
      env: shimEnv({ PATH: isolatedPath, HOME: prefix, USERPROFILE: prefix }),
    });
    assertOk(viaHome, "shim --help via ~/.local/bin (skipping npm node shim on PATH)");
    assertIncludes(
      `${viaHome.stdout || ""}${viaHome.stderr || ""}`,
      "devcouncil",
      "help via ~/.local/bin",
    );

    const helped = run(process.execPath, [shim, "--help"], {
      env: shimEnv({ PATH: isolatedPath, DEVCOUNCIL_BIN: fakeGo, HOME: prefix }),
    });
    assertOk(helped, "shim --help with DEVCOUNCIL_BIN");
    const helpText = `${helped.stdout || ""}${helped.stderr || ""}`;
    assertIncludes(helpText, "devcouncil", "help via Go binary");

    const versioned = run(process.execPath, [path.join(binDir, "dev"), "version"], {
      env: shimEnv({ PATH: isolatedPath, DEVCOUNCIL_BIN: fakeGo, HOME: prefix }),
    });
    assertOk(versioned, "dev version via shim");
    assertIncludes(
      `${versioned.stdout || ""}${versioned.stderr || ""}`,
      "devcouncil",
      "dev version output",
    );

    const mapped = run(process.execPath, [shim, "map", "--json", "paths"], {
      env: shimEnv({
        PATH: isolatedPath,
        DEVCOUNCIL_BIN: fakeGo,
        HOME: prefix,
        FAKE_ARGV_FILE: argvFile,
      }),
    });
    assertOk(mapped, "shim forwards map to Go");
    const forwarded = readFileSync(argvFile, "utf8");
    assertIncludes(forwarded, "map", "map reaches Go");
    assertIncludes(forwarded, "--json", "leading --json reaches Go");
    assertIncludes(forwarded, "paths", "paths reaches Go");

    console.log("npm runtime smoke passed");
  } finally {
    rmSync(prefix, { recursive: true, force: true });
  }
}

main();
