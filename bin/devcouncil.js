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

// Recursion bound. Each shim passes depth+1 to the binary it spawns; a shim
// reached through a chain of itself (DEVCOUNCIL_BIN or PATH resolving back to
// a shim) climbs one level per hop and stops at MAX_SHIM_DEPTH. Legitimate
// nesting (agent sessions, hooks, verify commands calling the shim again) is
// far shallower. Rule: only a plain run of decimal digits counts; a missing,
// empty, negative, fractional, exponent or otherwise non-numeric value is 0.
const SHIM_DEPTH_ENV = "DEVCOUNCIL_SHIM_DEPTH";
const MAX_SHIM_DEPTH = 16;

function shimDepth(env = process.env) {
  const raw = env[SHIM_DEPTH_ENV];
  return typeof raw === "string" && /^\d+$/.test(raw) ? Number(raw) : 0;
}

// The depth the spawned binary inherits.
function childShimDepthEnv(env = process.env) {
  return { [SHIM_DEPTH_ENV]: String(shimDepth(env) + 1) };
}

function shimRecursionProblem(env = process.env) {
  const depth = shimDepth(env);
  return depth >= MAX_SHIM_DEPTH
    ? `${SHIM_DEPTH_ENV}=${depth} reaches the limit of ${MAX_SHIM_DEPTH}: the devcouncil npm shim keeps launching itself. Likely cause: DEVCOUNCIL_BIN or PATH resolves back to a shim instead of the native devcouncil binary.`
    : null;
}

// On win32 a git-bash style HOME (/c/Users/u, which path.win32 calls rooted)
// is not a usable path for the native binary, so only a drive-letter or UNC
// HOME wins over USERPROFILE.
function localBinDir(env = process.env, pathMod = path) {
  let home = env.HOME || env.USERPROFILE || "";
  if (pathMod === path.win32 && !/^([A-Za-z]:[\\/]|\\\\)/.test(env.HOME || "")) {
    home = env.USERPROFILE || "";
  }
  return home ? pathMod.join(home, ".local", "bin") : "";
}

// Windows PATH semantics: ';' separates entries except inside double quotes,
// and the quotes are not part of the entry.
function splitWin32Path(raw) {
  const entries = [];
  let cur = "";
  let quoted = false;
  for (const ch of raw) {
    if (ch === '"') {
      quoted = !quoted;
    } else if (ch === ";" && !quoted) {
      entries.push(cur);
      cur = "";
    } else {
      cur += ch;
    }
  }
  entries.push(cur);
  return entries;
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

// Why an explicit override cannot be the native binary, or null if it can.
function overrideProblem(file, win32, opts = {}) {
  if (win32 && !/\.(exe|com)$/i.test(file)) {
    return "on Windows it must be a .exe or .com file";
  }
  if ((opts.isSelf ?? isSelf)(file)) {
    return "it is this npm shim itself, which would respawn forever";
  }
  if ((opts.looksLikeNodeShim ?? looksLikeNodeShim)(file)) {
    return "it is a node script or wrapper, not the native devcouncil binary";
  }
  return null;
}

// Find the native binary `name` on PATH (then ~/.local/bin). Only PATH entries
// are searched: never the current directory, which Windows' own lookup would
// try first. On win32 only `.exe` / `.com` count; an extensionless file there
// is an npm sh wrapper (next to `.cmd` / `.ps1`), which Windows cannot exec
// and which re-enters this shim. `opts` exists so tests can pin the platform,
// environment and filesystem probe instead of patching globals.
function resolveBin(name, envKey, opts = {}) {
  const platform = opts.platform ?? process.platform;
  const env = opts.env ?? process.env;
  const isNative = opts.isNative ?? isNativeBin;
  const win32 = platform === "win32";
  const pathMod = win32 ? path.win32 : path.posix;
  const override = env[envKey];
  if (override) {
    const problem = overrideProblem(override, win32, opts);
    if (problem) {
      throw new Error(`${envKey}=${override} is not usable: ${problem}`);
    }
    return override;
  }
  const rawPath = (win32 ? env.PATH ?? env.Path : env.PATH) || "";
  const dirs = (win32 ? splitWin32Path(rawPath) : rawPath.split(pathMod.delimiter))
    .filter(Boolean)
    // A relative Windows PATH entry resolves against the cwd.
    .filter((dir) => !win32 || pathMod.isAbsolute(dir));
  const local = localBinDir(env, pathMod);
  if (local) {
    dirs.push(local);
  }
  const exts = win32 ? [".exe", ".com"] : [""];
  for (const dir of dirs) {
    for (const ext of exts) {
      const candidate = pathMod.join(dir, name + ext);
      if (isNative(candidate)) {
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
    env: { ...spawnEnv(), ...childShimDepthEnv() },
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

function main() {
  const args = process.argv.slice(2);

  const recursion = shimRecursionProblem();
  if (recursion) {
    fail(recursion);
  }
  let goBin;
  try {
    goBin = resolveBin("devcouncil", "DEVCOUNCIL_BIN");
  } catch (err) {
    fail(err.message);
  }
  if (!goBin) {
    missing(
      "devcouncil",
      WIN32
        ? "Build: go -C backend/go_orchestrator build -o %USERPROFILE%\\.local\\bin\\devcouncil.exe ./cmd/devcouncil\nOr run scripts\\install.ps1, which builds it and runs `devcouncil install host`."
        : "Build: go -C backend/go_orchestrator build -o ~/.local/bin/devcouncil ./cmd/devcouncil\nAlso: ln -sf devcouncil ~/.local/bin/dev"
    );
  }

  const result = run(goBin, args);
  if (result.error) {
    fail(`Failed to start devcouncil: ${result.error.message}`);
  }
  process.exit(result.status ?? 1);
}

// Run as a program only; `require` (the unit tests) gets the exports.
if (require.main === module) {
  main();
}

module.exports = {
  resolveBin,
  isNativeBin,
  isSelf,
  looksLikeNodeShim,
  shimRecursionProblem,
  shimDepth,
  childShimDepthEnv,
  SHIM_DEPTH_ENV,
  MAX_SHIM_DEPTH,
};
