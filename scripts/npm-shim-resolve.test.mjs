import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const shimPath = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "bin", "devcouncil.js");
const { resolveBin } = require(shimPath);

// A fake Windows PATH: the npm layout (sh wrapper, .cmd, .ps1) ahead of the exe.
const NPM = "C:\\Users\\u\\AppData\\Roaming\\npm";
const GO = "C:\\tools\\go-bin";
function win32(files, extra = {}) {
  const present = new Set(files);
  return {
    platform: "win32",
    env: { PATH: [NPM, GO].join(";"), ...extra },
    isNative: (candidate) => present.has(candidate),
  };
}

test("win32 skips the npm sh wrapper, .cmd and .ps1 and returns the exe", () => {
  const opts = win32([
    NPM + "\\devcouncil",
    NPM + "\\devcouncil.cmd",
    NPM + "\\devcouncil.ps1",
    GO + "\\devcouncil.exe",
  ]);
  assert.equal(resolveBin("devcouncil", "DEVCOUNCIL_BIN", opts), GO + "\\devcouncil.exe");
});

test("win32 accepts only .exe and .com, preferring .exe within a directory", () => {
  assert.equal(
    resolveBin("devcouncil", "DEVCOUNCIL_BIN", win32([GO + "\\devcouncil.com", GO + "\\devcouncil.exe"])),
    GO + "\\devcouncil.exe",
  );
  assert.equal(
    resolveBin("devcouncil", "DEVCOUNCIL_BIN", win32([GO + "\\devcouncil.com"])),
    GO + "\\devcouncil.com",
  );
});

test("win32 with only an extensionless wrapper resolves nothing", () => {
  assert.equal(resolveBin("devcouncil", "DEVCOUNCIL_BIN", win32([NPM + "\\devcouncil"])), null);
});

test("win32 never probes the current directory or a bare name; only PATH entries", () => {
  const probed = [];
  const opts = win32([], {});
  opts.isNative = (candidate) => {
    probed.push(candidate);
    return false;
  };
  assert.equal(resolveBin("devcouncil", "DEVCOUNCIL_BIN", opts), null);
  assert.deepEqual(probed, [
    NPM + "\\devcouncil.exe",
    NPM + "\\devcouncil.com",
    GO + "\\devcouncil.exe",
    GO + "\\devcouncil.com",
  ]);
});

test("win32 strips quotes around a PATH entry and reads Path", () => {
  const opts = win32([GO + "\\devcouncil.exe"]);
  opts.env = { Path: '"' + GO + '"' };
  assert.equal(resolveBin("devcouncil", "DEVCOUNCIL_BIN", opts), GO + "\\devcouncil.exe");
});

test("the override env var wins on every platform", () => {
  const opts = win32([]);
  opts.env.DEVCOUNCIL_BIN = "D:\\custom\\devcouncil.exe";
  assert.equal(resolveBin("devcouncil", "DEVCOUNCIL_BIN", opts), "D:\\custom\\devcouncil.exe");
});

test("posix still accepts the extensionless binary", () => {
  const opts = {
    platform: "linux",
    env: { PATH: "/a:/b" },
    isNative: (candidate) => candidate === "/b/devcouncil",
  };
  assert.equal(resolveBin("devcouncil", "DEVCOUNCIL_BIN", opts), "/b/devcouncil");
});

// The next two use the real filesystem and POSIX paths; win32 resolution is
// covered above through the injected probe.
const posixOnly = { skip: process.platform === "win32" && "POSIX-path fixture" };

test("the shim itself (a symlink to it on PATH) is never returned", posixOnly, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "dc-shim-"));
  try {
    const shimDir = path.join(dir, "shim");
    const realDir = path.join(dir, "real");
    mkdirSync(shimDir);
    mkdirSync(realDir);
    symlinkSync(shimPath, path.join(shimDir, "devcouncil"));
    writeFileSync(path.join(realDir, "devcouncil"), "\x7fELF-not-a-node-script\n");
    const got = resolveBin("devcouncil", "DEVCOUNCIL_BIN", {
      platform: "linux",
      env: { PATH: [shimDir, realDir].join(path.delimiter) },
    });
    assert.equal(got, path.join(realDir, "devcouncil"));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("a node-script wrapper is skipped even when extensionless", posixOnly, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "dc-shim-"));
  try {
    writeFileSync(path.join(dir, "devcouncil"), "#!/usr/bin/env node\n");
    assert.equal(
      resolveBin("devcouncil", "DEVCOUNCIL_BIN", { platform: "linux", env: { PATH: dir } }),
      null,
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});


// ---- adversarial cases ----
function probeAll(opts) {
  const probed = [];
  opts.isNative = (c) => { probed.push(c); return false; };
  return probed;
}

test("adv: win32 empty PATH entries (;;) are dropped, so cwd is never probed", () => {
  const opts = win32([], {});
  opts.env = { PATH: ";;" + GO + ";" };
  const probed = probeAll(opts);
  resolveBin("devcouncil", "DEVCOUNCIL_BIN", opts);
  assert.deepEqual(probed, [GO + "\\devcouncil.exe", GO + "\\devcouncil.com"]);
});

test("adv: win32 trailing backslash, forward slashes and UNC entries normalise", () => {
  const opts = win32([], {});
  opts.env = { PATH: [GO + "\\", "C:/fwd/bin", "\\\\srv\\share\\bin"].join(";") };
  const probed = probeAll(opts);
  resolveBin("devcouncil", "DEVCOUNCIL_BIN", opts);
  assert.deepEqual(probed.filter((p) => p.endsWith(".exe")), [
    GO + "\\devcouncil.exe",
    "C:\\fwd\\bin\\devcouncil.exe",
    "\\\\srv\\share\\bin\\devcouncil.exe",
  ]);
});

test("adv: win32 PATH wins over Path when both are present", () => {
  const opts = win32([GO + "\\devcouncil.exe", "C:\\other\\devcouncil.exe"]);
  opts.env = { PATH: GO, Path: "C:\\other" };
  assert.equal(resolveBin("devcouncil", "DEVCOUNCIL_BIN", opts), GO + "\\devcouncil.exe");
});

test("adv: win32 probes only lowercase .exe/.com names (case-insensitive match is the filesystem's job)", () => {
  const opts = win32([GO + "\\devcouncil.EXE"]);
  assert.equal(resolveBin("devcouncil", "DEVCOUNCIL_BIN", opts), null);
});

test("adv: win32 does not consult PATHEXT (.bat/.cmd/.ps1 never resolve)", () => {
  const opts = win32([GO + "\\devcouncil.bat", GO + "\\devcouncil.cmd", GO + "\\devcouncil.ps1"], { PATHEXT: ".COM;.EXE;.BAT;.CMD" });
  assert.equal(resolveBin("devcouncil", "DEVCOUNCIL_BIN", opts), null);
});

test("adv: win32 ~/.local/bin uses HOME before USERPROFILE and is searched last", () => {
  const opts = win32([], { USERPROFILE: "C:\\Users\\u" });
  const probed = probeAll(opts);
  resolveBin("devcouncil", "DEVCOUNCIL_BIN", opts);
  assert.equal(probed.at(-2), "C:\\Users\\u\\.local\\bin\\devcouncil.exe");
  const opts2 = win32([], { HOME: "D:\\h", USERPROFILE: "C:\\Users\\u" });
  const probed2 = probeAll(opts2);
  resolveBin("devcouncil", "DEVCOUNCIL_BIN", opts2);
  assert.equal(probed2.at(-2), "D:\\h\\.local\\bin\\devcouncil.exe");
});

const posixReal = { skip: process.platform === "win32" && "POSIX fixture" };

test("adv: a directory named devcouncil is not a native binary", posixReal, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "dc-shim-"));
  try {
    mkdirSync(path.join(dir, "devcouncil"));
    assert.equal(resolveBin("devcouncil", "DEVCOUNCIL_BIN", { platform: "linux", env: { PATH: dir } }), null);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("adv: posix shim first on PATH, real binary later: shim skipped, real returned", posixReal, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "dc-shim-"));
  try {
    const a = path.join(dir, "a");
    const b = path.join(dir, "b");
    mkdirSync(a);
    mkdirSync(b);
    writeFileSync(path.join(a, "devcouncil"), "#!/usr/bin/env node\n" + "x".repeat(5_000_000));
    writeFileSync(path.join(b, "devcouncil"), "\x7fELF\n");
    assert.equal(
      resolveBin("devcouncil", "DEVCOUNCIL_BIN", { platform: "linux", env: { PATH: [a, b].join(":") } }),
      path.join(b, "devcouncil"),
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("adv: requiring the shim does not run main (no exit, no spawn)", () => {
  assert.equal(typeof require(shimPath).resolveBin, "function");
});

// --- fix round 1: override validation, recursion guard, win32 PATH/HOME ---
const { shimRecursionProblem, shimDepth, childShimDepthEnv, SHIM_DEPTH_ENV, MAX_SHIM_DEPTH } = require(shimPath);

test("override naming the shim itself is refused with an error naming the variable", posixOnly, () => {
  assert.throws(
    () => resolveBin("devcouncil", "DEVCOUNCIL_BIN", { platform: "linux", env: { DEVCOUNCIL_BIN: shimPath } }),
    /DEVCOUNCIL_BIN.*shim/,
  );
});

test("override naming a symlink to the shim is refused", posixOnly, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "dc-ovr-"));
  try {
    const link = path.join(dir, "devcouncil-link");
    symlinkSync(shimPath, link);
    assert.throws(
      () => resolveBin("devcouncil", "DEVCOUNCIL_BIN", { platform: "linux", env: { DEVCOUNCIL_BIN: link } }),
      /DEVCOUNCIL_BIN/,
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("override naming a node-shebang file is refused; a native file is used", posixOnly, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "dc-ovr-"));
  try {
    const nodeShim = path.join(dir, "wrapper");
    const native = path.join(dir, "native");
    writeFileSync(nodeShim, "#!/usr/bin/env node\n");
    writeFileSync(native, "\x7fELF-not-a-node-script\n");
    assert.throws(
      () => resolveBin("devcouncil", "DEVCOUNCIL_BIN", { platform: "linux", env: { DEVCOUNCIL_BIN: nodeShim } }),
      /DEVCOUNCIL_BIN.*node/,
    );
    assert.equal(
      resolveBin("devcouncil", "DEVCOUNCIL_BIN", { platform: "linux", env: { DEVCOUNCIL_BIN: native } }),
      native,
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("win32 override must be .exe or .com", () => {
  const env = (v) => ({ DEVCOUNCIL_BIN: v });
  assert.throws(
    () => resolveBin("devcouncil", "DEVCOUNCIL_BIN", { platform: "win32", env: env("C:\\x\\devcouncil.cmd") }),
    /DEVCOUNCIL_BIN.*\.exe/,
  );
  assert.throws(
    () => resolveBin("devcouncil", "DEVCOUNCIL_BIN", { platform: "win32", env: env("C:\\x\\devcouncil") }),
    /DEVCOUNCIL_BIN/,
  );
  assert.equal(
    resolveBin("devcouncil", "DEVCOUNCIL_BIN", { platform: "win32", env: env("C:\\x\\devcouncil.exe") }),
    "C:\\x\\devcouncil.exe",
  );
});

test("recursion guard allows depth 0..15 and refuses 16 and above", () => {
  assert.equal(MAX_SHIM_DEPTH, 16);
  assert.equal(shimRecursionProblem({}), null);
  for (const d of ["0", "1", "15"]) {
    assert.equal(shimRecursionProblem({ [SHIM_DEPTH_ENV]: d }), null, d);
  }
  for (const d of ["16", "17", "99999999999999999999"]) {
    const msg = shimRecursionProblem({ [SHIM_DEPTH_ENV]: d });
    assert.match(msg, /DEVCOUNCIL_SHIM_DEPTH=/, d);
    assert.match(msg, /DEVCOUNCIL_BIN or PATH/, d);
  }
  assert.match(shimRecursionProblem({ [SHIM_DEPTH_ENV]: "16" }), /=16 /);
});

test("shimDepth: only plain decimal digits count; anything else is 0", () => {
  assert.equal(shimDepth({}), 0);
  for (const bad of ["", "abc", "-3", "1e9", "1.5", " 3", "0x10", "+4"]) {
    assert.equal(shimDepth({ [SHIM_DEPTH_ENV]: bad }), 0, JSON.stringify(bad));
    assert.equal(shimRecursionProblem({ [SHIM_DEPTH_ENV]: bad }), null, JSON.stringify(bad));
  }
  assert.equal(shimDepth({ [SHIM_DEPTH_ENV]: "7" }), 7);
});

test("the child env carries depth+1 (garbage counts as 0)", () => {
  assert.deepEqual(childShimDepthEnv({}), { [SHIM_DEPTH_ENV]: "1" });
  assert.deepEqual(childShimDepthEnv({ [SHIM_DEPTH_ENV]: "15" }), { [SHIM_DEPTH_ENV]: "16" });
  assert.deepEqual(childShimDepthEnv({ [SHIM_DEPTH_ENV]: "-3" }), { [SHIM_DEPTH_ENV]: "1" });
});

test("win32 drops relative PATH entries (they would probe the cwd)", () => {
  const probed = [];
  const opts = {
    platform: "win32",
    env: { PATH: [".", "bin", "..\\up", GO].join(";") },
    isNative: (c) => (probed.push(c), false),
  };
  resolveBin("devcouncil", "DEVCOUNCIL_BIN", opts);
  assert.deepEqual(probed, [GO + "\\devcouncil.exe", GO + "\\devcouncil.com"]);
});

test("win32 PATH splitting honours double quotes around a semicolon", () => {
  const probed = [];
  const opts = {
    platform: "win32",
    env: { PATH: '"C:\\a;b";' + GO },
    isNative: (c) => (probed.push(c), false),
  };
  resolveBin("devcouncil", "DEVCOUNCIL_BIN", opts);
  assert.deepEqual(probed, [
    "C:\\a;b\\devcouncil.exe",
    "C:\\a;b\\devcouncil.com",
    GO + "\\devcouncil.exe",
    GO + "\\devcouncil.com",
  ]);
});

test("win32 ~/.local/bin falls back to USERPROFILE when HOME is not win32-absolute", () => {
  const probed = [];
  const opts = {
    platform: "win32",
    env: { PATH: "", HOME: "/c/Users/u", USERPROFILE: "C:\\Users\\u" },
    isNative: (c) => (probed.push(c), false),
  };
  resolveBin("devcouncil", "DEVCOUNCIL_BIN", opts);
  assert.deepEqual(probed, [
    "C:\\Users\\u\\.local\\bin\\devcouncil.exe",
    "C:\\Users\\u\\.local\\bin\\devcouncil.com",
  ]);
});
