#!/usr/bin/env node

/**
 * npm `dev` / `devcouncil` shim.
 *
 * Live commands belong to the Go `devcouncil` binary, including `map` /
 * `graph` / `ast` (which that binary execs `devmap` for). This file never
 * invokes Python or uv, and it is not a second owner of any command.
 */

const { spawnSync } = require("node:child_process");
const {
  closeSync,
  existsSync,
  openSync,
  readSync,
  realpathSync,
  statSync,
} = require("node:fs");
const path = require("node:path");

const WIN32 = process.platform === "win32";

function localBinDir() {
  const home = process.env.HOME || process.env.USERPROFILE || "";
  return home ? path.join(home, ".local", "bin") : "";
}

function isSelf(file) {
  try {
    return realpathSync(file) === realpathSync(__filename);
  } catch {
    return path.resolve(file) === path.resolve(__filename);
  }
}

function looksLikeNodeShim(file) {
  const lower = file.toLowerCase();
  if (
    lower.endsWith(".js") ||
    lower.endsWith(".cmd") ||
    lower.endsWith(".bat") ||
    lower.endsWith(".ps1")
  ) {
    return true;
  }
  let fd;
  try {
    fd = openSync(file, "r");
    const buf = Buffer.alloc(160);
    const n = readSync(fd, buf, 0, 160, 0);
    const line = buf.slice(0, n).toString("utf8").split(/\r?\n/, 1)[0] || "";
    if (line.startsWith("#!") && /\bnode\b/i.test(line)) {
      return true;
    }
  } catch {
    return false;
  } finally {
    if (fd !== undefined) {
      closeSync(fd);
    }
  }
  return false;
}

function isNativeBin(file) {
  if (!file || !existsSync(file)) {
    return false;
  }
  try {
    if (statSync(file).isDirectory()) {
      return false;
    }
  } catch {
    return false;
  }
  if (isSelf(file) || looksLikeNodeShim(file)) {
    return false;
  }
  return true;
}

function resolveBin(name, envKey) {
  const override = process.env[envKey];
  if (override) {
    return override;
  }
  const dirs = (process.env.PATH || "").split(path.delimiter).filter(Boolean);
  const local = localBinDir();
  if (local) {
    dirs.push(local);
  }
  const exts = WIN32 ? [".exe", ".com", ""] : [""];
  for (const dir of dirs) {
    for (const ext of exts) {
      const candidate = path.join(dir, name + ext);
      if (isNativeBin(candidate)) {
        return candidate;
      }
    }
  }
  return null;
}

function spawnEnv() {
  const env = { ...process.env };
  const local = localBinDir();
  if (local && existsSync(local)) {
    const current = env.PATH || env.Path || "";
    const parts = current.split(path.delimiter);
    if (!parts.includes(local)) {
      env.PATH = `${local}${path.delimiter}${current}`;
      if (WIN32) {
        env.Path = env.PATH;
      }
    }
  }
  return env;
}

function run(command, args) {
  return spawnSync(command, args, {
    cwd: process.cwd(),
    stdio: "inherit",
    shell: false,
    env: spawnEnv(),
  });
}

function fail(message) {
  console.error(message);
  process.exit(1);
}

function missing(name, hint) {
  fail(
    [
      `DevCouncil requires the ${name} binary on PATH (or ~/.local/bin).`,
      hint,
    ].join("\n")
  );
}

const args = process.argv.slice(2);

const goBin = resolveBin("devcouncil", "DEVCOUNCIL_BIN");
if (!goBin) {
  missing(
    "devcouncil",
    WIN32
      ? "Build: go -C backend/go_orchestrator build -o %USERPROFILE%\\.local\\bin\\devcouncil.exe ./cmd/devcouncil\nAlso copy it to %USERPROFILE%\\.local\\bin\\dev.exe (scripts/install.ps1)."
      : "Build: go -C backend/go_orchestrator build -o ~/.local/bin/devcouncil ./cmd/devcouncil\nAlso: ln -sf devcouncil ~/.local/bin/dev"
  );
}

const result = run(goBin, args);
if (result.error) {
  fail(`Failed to start devcouncil: ${result.error.message}`);
}
process.exit(result.status ?? 1);
