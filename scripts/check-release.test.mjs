import assert from "node:assert/strict";
import { existsSync, mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { after, describe, it } from "node:test";
import {
  DIST_PACKAGE,
  REQUIRED_ARCHIVES,
  binaryPackages,
  defaultSources,
  inspectRelease,
  missingArchives,
  npmPublishCreatesGitHubRelease,
  npmPublishSkipsExistingVersion,
  packageIsDistable,
  parseCargoLockVersion,
  parseGoVersion,
  parseTag,
  parseTomlSectionVersion,
  releaseIsIdempotent,
  releasePutsAnnouncementBodyInEnv,
  releaseUsesNotesFile,
} from "./check-release.mjs";

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
/** @type {string[]} */
const tempDirs = [];

after(() => {
  for (const dir of tempDirs) rmSync(dir, { recursive: true, force: true });
});

/**
 * @param {string} prefix
 * @param {Partial<{ pkg: string, rust: string, go: string, notes: boolean, distStore: boolean }>} versions
 */
function scratchTree(prefix, versions = {}) {
  const v = {
    pkg: "0.2.0",
    rust: "0.2.0",
    go: "0.2.0",
    notes: true,
    distStore: false,
    ...versions,
  };
  const dir = mkdtempSync(path.join(tmpdir(), `devcouncil-rel-${prefix}-`));
  tempDirs.push(dir);
  mkdirSync(path.join(dir, "backend", "go_orchestrator", "cmd", "devcouncil"), { recursive: true });
  mkdirSync(path.join(dir, "rust", "devmap-cli", "src"), { recursive: true });
  mkdirSync(path.join(dir, "rust", "devmap-store", "src", "bin"), { recursive: true });
  mkdirSync(path.join(dir, "docs", "releases"), { recursive: true });
  mkdirSync(path.join(dir, ".github", "workflows"), { recursive: true });

  writeFileSync(path.join(dir, "package.json"), JSON.stringify({ name: "devcouncil", version: v.pkg }));
  writeFileSync(
    path.join(dir, "package-lock.json"),
    JSON.stringify({ name: "devcouncil", version: v.pkg, lockfileVersion: 3, packages: { "": { version: v.pkg } } }),
  );
  writeFileSync(
    path.join(dir, "rust", "Cargo.toml"),
    `[workspace]\nmembers = ["devmap-cli", "devmap-store"]\n\n[workspace.package]\nversion = "${v.rust}"\n`,
  );
  writeFileSync(
    path.join(dir, "rust", "Cargo.lock"),
    `[[package]]\nname = "${DIST_PACKAGE}"\nversion = "${v.rust}"\n`,
  );
  writeFileSync(
    path.join(dir, "backend", "go_orchestrator", "cmd", "devcouncil", "version.go"),
    `package main\n\nconst Version = "${v.go}"\n`,
  );
  writeFileSync(
    path.join(dir, "rust", "devmap-cli", "Cargo.toml"),
    `[package]\nname = "${DIST_PACKAGE}"\nversion.workspace = true\n`,
  );
  writeFileSync(path.join(dir, "rust", "devmap-cli", "src", "main.rs"), "fn main() {}\n");
  writeFileSync(
    path.join(dir, "rust", "devmap-store", "Cargo.toml"),
    `[package]\nname = "devmap-store"\nversion.workspace = true\n${
      v.distStore ? "" : "[package.metadata.dist]\ndist = false\n"
    }`,
  );
  writeFileSync(path.join(dir, "rust", "devmap-store", "src", "bin", "hold_uncommitted_child.rs"), "fn main() {}\n");
  if (v.notes) {
    writeFileSync(path.join(dir, "docs", "releases", `v${v.pkg}.md`), `# DevCouncil v${v.pkg}\n\nNative host.\n`);
  }
  writeFileSync(
    path.join(dir, ".github", "workflows", "npm-publish.yml"),
    [
      "name: npm",
      "jobs:",
      "  publish:",
      "    steps:",
      "      - run: npm ping",
      "      - run: npm view devcouncil@$VERSION version",
      "      - run: echo published=false >> $GITHUB_OUTPUT",
    ].join("\n"),
  );
  writeFileSync(
    path.join(dir, ".github", "workflows", "release.yml"),
    [
      "name: Release",
      "jobs:",
      "  host:",
      "    steps:",
      '      - run: NOTES_FILE="docs/releases/${RELEASE_TAG}.md"',
      "      - run: gh release view \"$RELEASE_TAG\"",
      "      - run: gh release upload \"$RELEASE_TAG\" artifacts/* --clobber",
      "      - run: gh release edit \"$RELEASE_TAG\" --notes-file \"$NOTES_FILE\"",
    ].join("\n"),
  );
  return dir;
}

describe("parseTag", () => {
  it("accepts a three-part tag and rejects suffixes", () => {
    assert.deepEqual(parseTag("v0.2.0"), { ok: true, version: "0.2.0" });
    assert.equal(parseTag("0.2.0").ok, false);
    assert.equal(parseTag("v0.2.0-rc.1").ok, false);
  });
});

describe("parsers", () => {
  it("reads workspace.package rather than a dependency version", () => {
    const source = `[workspace.package]\nversion = "0.2.0"\n\n[workspace.dependencies]\nrusqlite = { version = "0.40.2" }\n`;
    assert.equal(parseTomlSectionVersion(source, "[workspace.package]"), "0.2.0");
    assert.equal(parseTomlSectionVersion(source, "[package]"), null);
  });

  it("reads the named crate from Cargo.lock, not the next package", () => {
    const source = `[[package]]\nname = "clap"\nversion = "4.5.0"\n\n[[package]]\nname = "devmap-cli"\nversion = "0.2.0"\n`;
    assert.equal(parseCargoLockVersion(source, "devmap-cli"), "0.2.0");
    assert.equal(parseCargoLockVersion(source, "missing"), null);
  });

  it("reads the Go product stamp and not another Version constant", () => {
    assert.equal(parseGoVersion('package main\n\nconst Version = "0.2.0"\n'), "0.2.0");
    assert.equal(parseGoVersion("const SchemaVersion = 9\n"), null);
  });

  it("treats missing dist metadata as dist-able, and dist = false as not", () => {
    assert.equal(packageIsDistable("[package]\nname = \"x\"\n"), true);
    assert.equal(packageIsDistable("[package.metadata.dist]\ndist = false\n"), false);
    assert.equal(packageIsDistable("[package.metadata.dist]\ndist = true\n"), true);
  });
});

describe("missingArchives", () => {
  it("names every required archive when the directory is empty, and nothing when all four are present", () => {
    const empty = mkdtempSync(path.join(tmpdir(), "devcouncil-art-empty-"));
    tempDirs.push(empty);
    assert.deepEqual(missingArchives(empty), [...REQUIRED_ARCHIVES]);

    const full = mkdtempSync(path.join(tmpdir(), "devcouncil-art-full-"));
    tempDirs.push(full);
    for (const name of REQUIRED_ARCHIVES) {
      writeFileSync(path.join(full, name), "x");
    }
    assert.deepEqual(missingArchives(full), []);
  });

  it("does not treat a capped sample as complete: a linux archive is not the set", () => {
    const dir = mkdtempSync(path.join(tmpdir(), "devcouncil-art-one-"));
    tempDirs.push(dir);
    writeFileSync(path.join(dir, REQUIRED_ARCHIVES[0]), "x");
    assert.deepEqual(missingArchives(dir), REQUIRED_ARCHIVES.slice(1));
  });

  it("requires the Windows MSVC zip by name so a missing grammar build cannot ship", () => {
    assert.ok(REQUIRED_ARCHIVES.includes("devmap-cli-x86_64-pc-windows-msvc.zip"));
  });
});

describe("inspectRelease", () => {
  it("accepts a consistent tree", () => {
    const root = scratchTree("ok");
    const result = inspectRelease(defaultSources(root));
    assert.deepEqual(result.errors, []);
    assert.equal(result.version, "0.2.0");
  });

  it("fails when package.json drifts from the Go stamp", () => {
    const root = scratchTree("drift", { pkg: "0.4.2" });
    const result = inspectRelease(defaultSources(root));
    assert.ok(result.errors.some((e) => e.includes("0.4.2") && e.includes("0.2.0")));
  });

  it("fails when the notes file for this version is missing", () => {
    const root = scratchTree("notes", { notes: false });
    const result = inspectRelease(defaultSources(root));
    assert.ok(result.errors.some((e) => e.includes("missing release notes")));
  });

  it("fails when the tag names a different version", () => {
    const root = scratchTree("tag");
    const result = inspectRelease(defaultSources(root), { tag: "v0.4.2" });
    assert.ok(result.errors.some((e) => e.includes("v0.4.2")));
  });

  it("fails when a helper binary package is still dist-able", () => {
    const root = scratchTree("helper", { distStore: true });
    const extras = binaryPackages(path.join(root, "rust")).filter((pkg) => pkg.distable);
    assert.ok(extras.some((pkg) => pkg.name === "devmap-store"));
    const result = inspectRelease(defaultSources(root));
    assert.ok(result.errors.some((e) => e.includes("devmap-store")));
  });
});

describe("this repository's workflows", () => {
  const npmYaml = readWorkflow("npm-publish.yml");
  const relYaml = readWorkflow("release.yml");

  it("does not let npm-publish.yml create a GitHub Release (that races cargo-dist)", () => {
    assert.equal(npmPublishCreatesGitHubRelease(npmYaml), false);
  });

  it("skips npm publish when the version is already on the registry", () => {
    assert.equal(npmPublishSkipsExistingVersion(npmYaml), true);
  });

  it("does not put the announcement body in the environment", () => {
    assert.equal(releasePutsAnnouncementBodyInEnv(relYaml), false);
  });

  it("publishes docs/releases via --notes-file and is idempotent on an existing tag", () => {
    assert.equal(releaseUsesNotesFile(relYaml), true);
    assert.equal(releaseIsIdempotent(relYaml), true);
  });

  it("does not keep or attach judge-release-notes", () => {
    assert.equal(existsSync(path.join(REPO_ROOT, "docs", "judge-release-notes.md")), false);
    assert.equal(npmYaml.includes("judge-release-notes"), false);
    assert.equal(relYaml.includes("judge-release-notes"), false);
  });
});

describe("this repository's cargo-dist packages", () => {
  it("only dist-ables the CLI package, so hold_uncommitted_child cannot ship", () => {
    const packages = binaryPackages(path.join(REPO_ROOT, "rust"));
    const distable = packages.filter((pkg) => pkg.distable).map((pkg) => pkg.name);
    assert.deepEqual(distable, [DIST_PACKAGE]);
    assert.ok(packages.some((pkg) => pkg.name === "devmap-store" && pkg.distable === false));
  });
});

/** @param {string} name */
function readWorkflow(name) {
  return readFileSync(path.join(REPO_ROOT, ".github", "workflows", name), "utf8");
}
