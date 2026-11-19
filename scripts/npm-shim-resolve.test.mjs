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
