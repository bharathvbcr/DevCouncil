#!/usr/bin/env node
/**
 * Release identity gate. Every producer of a vX.Y.Z tag reads a different
 * file: the Git tag, package.json, package-lock.json, rust workspace
 * version, Cargo.lock, and the Go `Version` stamp. A tag that names 0.2.0
 * while those files still say something else ships the wrong product under
 * the right name. This check fails before any build minutes are spent.
 *
 * It also owns the GitHub-release invariants that CI cannot see until a
 * tag is pushed: notes come from docs/releases/vX.Y.Z.md (never through
 * a 48 KB environment variable), npm-publish.yml must not create a
 * GitHub Release (cargo-dist's release.yml is the one owner), and
 * cargo-dist must not package test-helper binaries.
 *
 * Exit codes: 0 identity holds · 1 mismatch · 2 the check could not run.
 */
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

export const DIST_PACKAGE = "devmap-cli";

export const REQUIRED_ARCHIVES = Object.freeze([
  "devmap-cli-x86_64-unknown-linux-gnu.tar.xz",
  "devmap-cli-aarch64-apple-darwin.tar.xz",
  "devmap-cli-x86_64-apple-darwin.tar.xz",
  "devmap-cli-x86_64-pc-windows-msvc.zip",
]);

/**
 * @param {string} tag
 * @returns {{ ok: true, version: string } | { ok: false, reason: string }}
 */
export function parseTag(tag) {
  if (!tag.startsWith("v")) {
    return {
      ok: false,
      reason: `tag ${JSON.stringify(tag)} must start with "v"`,
    };
  }
  const version = tag.slice(1);
  if (!/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(version)) {
    return {
      ok: false,
      reason: `tag ${JSON.stringify(tag)} must be v<major>.<minor>.<patch> with no suffix`,
    };
  }
  return { ok: true, version };
}

/**
 * Whether a release tag names the commit whose notes would actually ship.
 *
 * `release.yml` checks out the *tagged* commit and publishes
 * `docs/releases/<tag>.md` from it. So notes edited after the tag was cut are
 * invisible to the release, and **no CI gate can see the discrepancy**: at the
 * tagged commit the old notes are internally consistent, mention the version,
 * and sit under the size cap, so every check passes while the wrong text ships.
 * Measured 2026-09-14 — `v0.2.2` named a commit whose notes were 277 lines
 * against 653 at `HEAD`.
 *
 * This is therefore a pre-push check by construction, not an oversight that it
 * runs only locally. A tag that does not exist yet is fine: pushing creates it
 * at `HEAD`.
 *
 * @param {{ tag: string, tagSha: string | null, headSha: string }} input
 * @returns {{ ok: true } | { ok: false, reason: string }}
 */
export function tagNamesHead({ tag, tagSha, headSha }) {
  if (!tagSha) return { ok: true };
  if (tagSha === headSha) return { ok: true };
  const short = (/** @type {string} */ sha) => sha.slice(0, 12);
  return {
    ok: false,
    reason:
      `tag ${tag} names ${short(tagSha)} but HEAD is ${short(headSha)}; ` +
      `the release would publish docs/releases/${tag}.md as it was at ` +
      `${short(tagSha)}. Retag at HEAD, or push a different tag.`,
  };
}

/**
 * @param {string} source
 * @param {string} section  e.g. "[workspace.package]" or "[package]"
 */
export function parseTomlSectionVersion(source, section) {
  const lines = source.split(/\r?\n/);
  let inSection = false;
  for (const rawLine of lines) {
    const line = rawLine.replace(/#.*$/, "").trim();
    if (/^\[[^\]]+\]$/.test(line)) {
      inSection = line === section;
      continue;
    }
    if (!inSection) continue;
    const match = /^version\s*=\s*"([^"]*)"/.exec(line);
    if (match) return match[1];
  }
  return null;
}

/**
 * @param {string} source
 * @param {string} crate
 */
export function parseCargoLockVersion(source, crate) {
  const blocks = source.split(/^\[\[package\]\]\s*$/m).slice(1);
  for (const block of blocks) {
    const upToNextSection = block.split(/^\[/m)[0];
    const name = /^name\s*=\s*"([^"]*)"/m.exec(upToNextSection);
    if (!name || name[1] !== crate) continue;
    const version = /^version\s*=\s*"([^"]*)"/m.exec(upToNextSection);
    return version ? version[1] : null;
  }
  return null;
}

/** @param {string} source */
export function parseGoVersion(source) {
  const match = /^const Version = "([^"]+)"/m.exec(source);
  return match ? match[1] : null;
}

/** @param {string} source */
export function parseNpmVersion(source) {
  const payload = JSON.parse(source);
  if (!payload || typeof payload.version !== "string" || payload.version === "") {
    return null;
  }
  return payload.version;
}

/** @param {string} source */
export function parseDistWorkspaceTargets(source) {
  const match = /targets\s*=\s*\[([\s\S]*?)\]/.exec(source);
  if (!match) return [];
  const inside = match[1];
  const stringMatches = inside.matchAll(/"([^"]+)"/g);
  return Array.from(stringMatches, (m) => m[1]);
}

/**
 * cargo-dist treats a binary package as dist-able unless it sets
 * `[package.metadata.dist] dist = false`. Default is true.
 *
 * @param {string} source
 */
export function packageIsDistable(source) {
  const lines = source.split(/\r?\n/);
  let inDist = false;
  for (const rawLine of lines) {
    const line = rawLine.replace(/#.*$/, "").trim();
    if (/^\[[^\]]+\]$/.test(line)) {
      inDist = line === "[package.metadata.dist]";
      continue;
    }
    if (!inDist) continue;
    const match = /^dist\s*=\s*(true|false)\b/.exec(line);
    if (match) return match[1] === "true";
  }
  return true;
}

/**
 * The `[workspace] members` list, in declaration order.
 *
 * @param {string} source
 * @returns {string[]}
 */
export function parseWorkspaceMembers(source) {
  const match = /^\[workspace\]([\s\S]*?)(?=^\[)/m.exec(source);
  if (!match) return [];
  const members = /members\s*=\s*\[([\s\S]*?)\]/.exec(match[1]);
  if (!members) return [];
  return Array.from(members[1].matchAll(/"([^"]+)"/g), (m) => m[1]);
}

/**
 * Workspace members that do not take `version.workspace = true`.
 *
 * Every per-binary version check compares a program's answer against its own
 * `CARGO_PKG_VERSION`, so a crate that swapped inheritance for a literal would
 * keep passing all of them while shipping a different number than the product.
 * Inheritance is what makes `[workspace.package] version` the single owner, so
 * it is asserted here rather than assumed: this is the only place that sees
 * every crate at once.
 *
 * A member whose manifest is missing is reported too — an unbuildable
 * workspace must not read as "every crate inherits".
 *
 * @param {string} rustRoot
 * @param {readonly string[]} members
 * @returns {string[]} one human-readable reason per offending member
 */
export function membersNotInheritingVersion(rustRoot, members) {
  /** @type {string[]} */
  const offenders = [];
  for (const member of members) {
    const manifestPath = path.join(rustRoot, member, "Cargo.toml");
    if (!existsSync(manifestPath)) {
      offenders.push(`${member} is a workspace member with no Cargo.toml`);
      continue;
    }
    const source = readFileSync(manifestPath, "utf8");
    const own = parseTomlSectionVersion(source, "[package]");
    if (own != null) {
      offenders.push(
        `${member} declares its own version ${JSON.stringify(own)} instead of version.workspace = true`,
      );
      continue;
    }
    if (!/^\s*version\.workspace\s*=\s*true\s*$/m.test(source)) {
      offenders.push(`${member} sets no version: it must take version.workspace = true`);
    }
  }
  return offenders;
}

/**
 * Packages under `rustRoot` that cargo would build a binary for: they have
 * `src/main.rs` or at least one `src/bin/*.rs`. A library crate with no
 * binary is not dist-able even when `dist` is left at the default.
 *
 * @param {string} rustRoot
 * @returns {{ name: string, distable: boolean, dir: string }[]}
 */
export function binaryPackages(rustRoot) {
  if (!existsSync(rustRoot)) return [];
  const entries = readdirSync(rustRoot, { withFileTypes: true });
  /** @type {{ name: string, distable: boolean, dir: string }[]} */
  const found = [];
  for (const entry of entries) {
    if (!entry.isDirectory()) continue;
    const dir = path.join(rustRoot, entry.name);
    const manifestPath = path.join(dir, "Cargo.toml");
    if (!existsSync(manifestPath)) continue;
    const hasMain = existsSync(path.join(dir, "src", "main.rs"));
    const binDir = path.join(dir, "src", "bin");
    const hasBin =
      existsSync(binDir) &&
      readdirSync(binDir).some((name) => name.endsWith(".rs"));
    if (!hasMain && !hasBin) continue;
    const source = readFileSync(manifestPath, "utf8");
    const nameMatch = /^name\s*=\s*"([^"]+)"/m.exec(source);
    found.push({
      name: nameMatch ? nameMatch[1] : entry.name,
      distable: packageIsDistable(source),
      dir,
    });
  }
  return found.sort((a, b) => a.name.localeCompare(b.name));
}

/** @param {string} yaml */
export function npmPublishCreatesGitHubRelease(yaml) {
  return /^\s*gh release create\b/m.test(yaml);
}

/** @param {string} yaml */
export function npmPublishSkipsExistingVersion(yaml) {
  return (
    yaml.includes("npm ping") &&
    yaml.includes("published=false") &&
    yaml.includes("npm view")
  );
}

/** @param {string} yaml */
export function releasePutsAnnouncementBodyInEnv(yaml) {
  return /^\s*ANNOUNCEMENT_BODY:/.test(yaml);
}

/** @param {string} yaml */
export function releaseUsesNotesFile(yaml) {
  return yaml.includes("docs/releases/") && yaml.includes("--notes-file");
}

/** @param {string} yaml */
export function releaseIsIdempotent(yaml) {
  return (
    yaml.includes("gh release view") &&
    yaml.includes("gh release edit") &&
    yaml.includes("gh release upload")
  );
}

/**
 * Gaps in the path that runs when cargo-dist has already opened the tag.
 *
 * `host --steps=release` creates the GitHub Release before this step, with
 * cargo-dist's own changelog. The shell then uses `set -euo pipefail`. If
 * `gh release upload` runs first and fails — the usual retry error is an
 * asset that already exists — the notes edit never runs, and the release
 * that users see is the changelog, not `docs/releases/<tag>.md`.
 *
 * The existing-release branch must edit the notes first, then re-upload
 * with `--clobber`.
 *
 * @param {string} yaml
 * @returns {string[]}
 */
export function releaseNotesPublishGaps(yaml) {
  const marker = yaml.includes("if gh release view") ? "if gh release view" : "gh release view";
  const afterView = yaml.split(marker)[1] ?? "";
  const elseAt = afterView.search(/\n\s*else\b/);
  const branch = elseAt >= 0 ? afterView.slice(0, elseAt) : afterView;
  const editAt = branch.search(/gh release edit\b[^\n]*--notes-file/);
  const uploadAt = branch.search(/gh release upload\b[^\n]*--clobber/);
  /** @type {string[]} */
  const errors = [];
  if (!yaml.includes("gh release view")) {
    errors.push(
      "existing-release path must `gh release view` before editing notes; without it a second publish runs `gh release create` and fails after cargo-dist has already opened the tag",
    );
    return errors;
  }
  if (editAt < 0) {
    errors.push(
      "existing-release path must `gh release edit` with --notes-file; cargo-dist opens the release first, so this is the path that publishes docs/releases",
    );
  }
  if (uploadAt < 0) {
    errors.push(
      "existing-release path must `gh release upload --clobber`; without --clobber a retry fails after cargo-dist has already attached the archives",
    );
  }
  if (editAt >= 0 && uploadAt >= 0 && editAt > uploadAt) {
    errors.push(
      "existing-release path uploads before editing notes; a failed upload skips the notes edit and the release keeps cargo-dist's changelog",
    );
  }
  return errors;
}

/**
 * @param {string} dir
 * @param {readonly string[]} required
 * @returns {string[]}
 */
export function missingArchives(dir, required = REQUIRED_ARCHIVES) {
  if (!existsSync(dir)) {
    return [...required];
  }
  /** @type {string[]} */
  const names = [];
  const walk = (current) => {
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      const full = path.join(current, entry.name);
      if (entry.isDirectory()) walk(full);
      else names.push(entry.name);
    }
  };
  const st = statSync(dir);
  if (st.isDirectory()) walk(dir);
  const have = new Set(names);
  return required.filter((name) => !have.has(name));
}

/**
 * @param {string} root
 */
export function defaultSources(root) {
  return {
    packagePath: path.join(root, "package.json"),
    packageLockPath: path.join(root, "package-lock.json"),
    cargoTomlPath: path.join(root, "rust", "Cargo.toml"),
    cargoLockPath: path.join(root, "rust", "Cargo.lock"),
    // `devcouncil/version`, not `cmd/devcouncil`: the constant moved into a
    // library so `devcouncil/mcp` could import it, and this gate must read
    // the owner rather than the alias that `package main` keeps for printing.
    goVersionPath: path.join(root, "backend", "go_orchestrator", "devcouncil", "version", "version.go"),
    rustRoot: path.join(root, "rust"),
    distWorkspacePath: path.join(root, "dist-workspace.toml"),
    npmPublishPath: path.join(root, ".github", "workflows", "npm-publish.yml"),
    releaseWorkflowPath: path.join(root, ".github", "workflows", "release.yml"),
    notesDir: path.join(root, "docs", "releases"),
  };
}

/**
 * @param {ReturnType<typeof defaultSources>} sources
 * @param {{ tag?: string, artifactsDir?: string, skipWorkflows?: boolean }} [opts]
 */
export function inspectRelease(sources, opts = {}) {
  /** @type {string[]} */
  const errors = [];
  const read = (file, label) => {
    if (!existsSync(file)) {
      errors.push(`missing ${label}: ${file}`);
      return null;
    }
    return readFileSync(file, "utf8");
  };

  const pkg = read(sources.packagePath, "package.json");
  const lock = read(sources.packageLockPath, "package-lock.json");
  const cargoToml = read(sources.cargoTomlPath, "rust/Cargo.toml");
  const cargoLock = read(sources.cargoLockPath, "rust/Cargo.lock");
  const goSrc = read(sources.goVersionPath, "version.go");

  const versions = {
    "package.json": pkg ? parseNpmVersion(pkg) : null,
    "package-lock.json": lock ? parseNpmVersion(lock) : null,
    "rust/Cargo.toml [workspace.package]": cargoToml
      ? parseTomlSectionVersion(cargoToml, "[workspace.package]")
      : null,
    [`rust/Cargo.lock ${DIST_PACKAGE}`]: cargoLock
      ? parseCargoLockVersion(cargoLock, DIST_PACKAGE)
      : null,
    "version.go": goSrc ? parseGoVersion(goSrc) : null,
  };

  const present = Object.values(versions).filter((v) => typeof v === "string" && v !== "");
  const identity = present[0] ?? null;
  for (const [label, value] of Object.entries(versions)) {
    if (value == null) {
      errors.push(`${label} has no version`);
    } else if (identity && value !== identity) {
      errors.push(`${label} is ${JSON.stringify(value)}, expected ${JSON.stringify(identity)}`);
    }
  }

  if (opts.tag) {
    const parsed = parseTag(opts.tag);
    if (!parsed.ok) {
      errors.push(parsed.reason);
    } else if (identity && parsed.version !== identity) {
      errors.push(
        `tag ${JSON.stringify(opts.tag)} names ${JSON.stringify(parsed.version)}, product version is ${JSON.stringify(identity)}`,
      );
    }
  }

  if (identity) {
    const notesPath = path.join(sources.notesDir, `v${identity}.md`);
    if (!existsSync(notesPath)) {
      errors.push(`missing release notes: ${notesPath}`);
    } else {
      const notes = readFileSync(notesPath, "utf8");
      if (!notes.trim()) {
        errors.push(`release notes are empty: ${notesPath}`);
      } else if (!notes.includes(identity)) {
        errors.push(`release notes do not mention ${identity}: ${notesPath}`);
      } else if (notes.length > 100_000) {
        // GitHub's release-body limit is 1 MB; stay well under it so notes
        // never have to travel through GITHUB_ENV (48 KB cap).
        errors.push(`release notes exceed 100000 characters: ${notesPath}`);
      }
    }
  }

  if (cargoToml) {
    const members = parseWorkspaceMembers(cargoToml);
    if (members.length === 0) {
      errors.push("rust/Cargo.toml declares no workspace members");
    } else {
      for (const reason of membersNotInheritingVersion(sources.rustRoot, members)) {
        errors.push(reason);
      }
    }
  }

  if (existsSync(sources.rustRoot)) {
    const extras = binaryPackages(sources.rustRoot).filter(
      (pkg) => pkg.distable && pkg.name !== DIST_PACKAGE,
    );
    if (extras.length > 0) {
      errors.push(
        `cargo-dist would ship helper/analysis binaries: ${extras.map((p) => p.name).join(", ")}. Set [package.metadata.dist] dist = false on each.`,
      );
    }
    const distPkg = binaryPackages(sources.rustRoot).find((pkg) => pkg.name === DIST_PACKAGE);
    if (!distPkg) {
      errors.push(`binary package ${DIST_PACKAGE} was not found under rust/`);
    } else if (!distPkg.distable) {
      errors.push(`${DIST_PACKAGE} has dist = false; GitHub Releases would have no archives`);
    }
  }

  if (sources.distWorkspacePath && existsSync(sources.distWorkspacePath)) {
    const distToml = readFileSync(sources.distWorkspacePath, "utf8");
    const targets = parseDistWorkspaceTargets(distToml);
    if (targets.length === 0) {
      errors.push("dist-workspace.toml declares no targets");
    } else {
      for (const target of targets) {
        const expectedArchive = target.includes("windows")
          ? `${DIST_PACKAGE}-${target}.zip`
          : `${DIST_PACKAGE}-${target}.tar.xz`;
        if (!REQUIRED_ARCHIVES.includes(expectedArchive)) {
          errors.push(
            `dist-workspace.toml target ${target} (${expectedArchive}) is missing from REQUIRED_ARCHIVES`,
          );
        }
      }
      for (const req of REQUIRED_ARCHIVES) {
        const hasTarget = targets.some((t) => {
          const expected = t.includes("windows")
            ? `${DIST_PACKAGE}-${t}.zip`
            : `${DIST_PACKAGE}-${t}.tar.xz`;
          return expected === req;
        });
        if (!hasTarget) {
          errors.push(`REQUIRED_ARCHIVES entry ${req} is not declared in dist-workspace.toml`);
        }
      }
    }
  }

  if (!opts.skipWorkflows) {
    const npmYaml = read(sources.npmPublishPath, "npm-publish.yml");
    const relYaml = read(sources.releaseWorkflowPath, "release.yml");
    if (npmYaml) {
      if (npmPublishCreatesGitHubRelease(npmYaml)) {
        errors.push(
          "npm-publish.yml runs `gh release create`; a tag would race cargo-dist's release.yml and one of them would fail",
        );
      }
      if (!npmPublishSkipsExistingVersion(npmYaml)) {
        errors.push(
          "npm-publish.yml does not skip versions already on the registry; republishing 0.2.0 would fail the whole tag",
        );
      }
    }
    if (relYaml) {
      if (releasePutsAnnouncementBodyInEnv(relYaml)) {
        errors.push(
          "release.yml puts ANNOUNCEMENT_BODY in the environment; GitHub caps env values at 48 KB and the notes would fail after the builds",
        );
      }
      if (!releaseUsesNotesFile(relYaml)) {
        errors.push("release.yml does not publish docs/releases/ via --notes-file");
      }
      if (!releaseIsIdempotent(relYaml)) {
        errors.push(
          "release.yml is not idempotent (needs gh release view/edit/upload); a second create on an existing tag fails after the artifacts are built",
        );
      }
      for (const gap of releaseNotesPublishGaps(relYaml)) {
        errors.push(gap);
      }
    }
  }

  if (opts.artifactsDir) {
    const missing = missingArchives(opts.artifactsDir);
    if (missing.length > 0) {
      errors.push(`required release archives missing: ${missing.join(", ")}`);
    }
  }

  return { version: identity, errors };
}

function parseArgs(argv) {
  const out = { tag: "", artifactsDir: "", root: REPO_ROOT, help: false };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--help" || arg === "-h") out.help = true;
    else if (arg === "--tag") out.tag = argv[++i] || "";
    else if (arg === "--artifacts") out.artifactsDir = argv[++i] || "";
    else if (arg === "--root") out.root = argv[++i] || out.root;
    else {
      throw new Error(`unknown argument: ${arg}`);
    }
  }
  return out;
}

export function main(argv = process.argv.slice(2)) {
  let args;
  try {
    args = parseArgs(argv);
  } catch (err) {
    console.error(`check-release: ${err instanceof Error ? err.message : String(err)}`);
    return 2;
  }
  if (args.help) {
    console.log(
      "Usage: node scripts/check-release.mjs [--tag vX.Y.Z] [--artifacts DIR] [--root DIR]",
    );
    return 0;
  }
  const result = inspectRelease(defaultSources(args.root), {
    tag: args.tag || undefined,
    artifactsDir: args.artifactsDir || undefined,
  });
  if (result.errors.length > 0) {
    for (const error of result.errors) {
      console.error(`FAIL: ${error}`);
    }
    return 1;
  }
  console.log(`OK: product version ${result.version} is consistent`);
  return 0;
}

const invokedPath = process.argv[1] ? path.resolve(process.argv[1]) : "";
if (invokedPath === fileURLToPath(import.meta.url)) {
  process.exitCode = main();
}
