#!/usr/bin/env node
/**
 * Lint the GitHub Actions workflow files with actionlint.
 *
 * `release.yml` runs only on a version tag, so a syntax error there stays
 * invisible until a release is cut. actionlint is a separate binary rather
 * than an npm dependency. A missing binary exits 2 (checker could not run)
 * and is reported differently from workflows that were checked and found
 * faulty, which exits 1.
 *
 * Exit codes: 0 workflows are clean · 1 actionlint found problems ·
 * 2 the check could not run
 */
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const WORKFLOW_DIR = path.join(REPO_ROOT, ".github", "workflows");

export const INSTALL_HINT =
  "actionlint is not installed. Install it with `brew install actionlint`, or see " +
  "https://github.com/rhysd/actionlint/blob/main/docs/install.md";

/**
 * @param {string} dir
 * @returns {string[]}
 */
export function workflowFiles(dir) {
  if (!existsSync(dir)) return [];
  return readdirSync(dir)
    .filter((name) => name.endsWith(".yml") || name.endsWith(".yaml"))
    .sort();
}

/**
 * True when `yaml` declares a workflow-level `permissions:` map (before `jobs:`).
 * Job-only permissions leave the rest of the file on GitHub's default token.
 *
 * @param {string} yaml
 */
export function hasWorkflowLevelPermissions(yaml) {
  const jobsAt = yaml.search(/^jobs\s*:/m);
  const head = jobsAt === -1 ? yaml : yaml.slice(0, jobsAt);
  return /^permissions\s*:/m.test(head);
}

/**
 * @param {string} dir
 * @returns {string[]}
 */
export function workflowsMissingPermissions(dir) {
  return workflowFiles(dir).filter((name) => {
    const text = readFileSync(path.join(dir, name), "utf8");
    return !hasWorkflowLevelPermissions(text);
  });
}

/**
 * @param {{ status: number | null, error?: Error }} result
 * @returns {{ code: number, message: string | null }}
 */
export function interpret(result) {
  if (result.error && /** @type {NodeJS.ErrnoException} */ (result.error).code === "ENOENT") {
    return { code: 2, message: INSTALL_HINT };
  }
  if (result.error) return { code: 2, message: `actionlint could not run: ${result.error.message}` };
  if (result.status === 0) return { code: 0, message: null };
  if (result.status === 1) return { code: 1, message: "actionlint reported problems" };
  return { code: 2, message: `actionlint exited with ${result.status}` };
}

/**
 * Workspace members as `{ dir, name }`, read from rust/Cargo.toml's `members`
 * and each member's own manifest.
 *
 * @param {string} rustDir
 * @returns {{ dir: string, name: string }[]}
 */
export function workspaceMembers(rustDir) {
  const root = readFileSync(path.join(rustDir, "Cargo.toml"), "utf8");
  const list = /^members\s*=\s*\[([^\]]*)\]/m.exec(root)?.[1] ?? "";
  const dirs = [...list.matchAll(/"([^"]+)"/g)].map((m) => m[1]);
  return dirs.map((dir) => {
    const manifest = readFileSync(path.join(rustDir, dir, "Cargo.toml"), "utf8");
    const name = /^name\s*=\s*"([^"]+)"/m.exec(manifest)?.[1];
    if (!name) throw new Error(`no package name in ${dir}/Cargo.toml`);
    return { dir, name };
  });
}

/** @param {string} glob @returns {RegExp} */
function pathGlob(glob) {
  const body = glob
    .replace(/[.+^${}()|[\]\\]/g, "\\$&")
    .replace(/\*\*/g, "\0")
    .replace(/\*/g, "[^/]*")
    .replace(/\0/g, ".*");
  return new RegExp(`^${body}$`);
}

/**
 * The `paths:` globs of a workflow's push and pull_request triggers. A
 * workflow with no `paths:` filter runs on every push, so it covers any path.
 *
 * @param {string} yaml
 * @returns {string[] | "all"}
 */
export function triggerPaths(yaml) {
  const head = yaml.slice(0, yaml.search(/^jobs\s*:/m) === -1 ? undefined : yaml.search(/^jobs\s*:/m));
  const globs = [];
  let inPaths = false;
  for (const line of head.split("\n")) {
    if (/^\s*paths\s*:/.test(line)) {
      inPaths = true;
      continue;
    }
    const item = /^\s*-\s*["']?([^"'#\s]+)["']?\s*(#.*)?$/.exec(line);
    if (inPaths && item) {
      globs.push(item[1]);
      continue;
    }
    if (!/^\s*(#.*)?$/.test(line)) inPaths = false;
  }
  return globs.length === 0 ? "all" : globs;
}

/**
 * Packages a workflow's cargo test commands name: `-p NAME`, a `for crate in`
 * list that feeds `-p "$crate"`, or `--workspace` less its `--exclude`s.
 *
 * @param {string} yaml
 * @param {string[]} allNames
 * @returns {Set<string>}
 */
export function testedPackages(yaml, allNames) {
  const text = yaml.replace(/\\\n/g, " ");
  const tested = new Set();
  for (const raw of text.split("\n")) {
    const line = raw.trim();
    if (line.startsWith("#")) continue;
    const loop = /^for crate in (.*?);\s*do\b/.exec(line);
    if (loop) for (const name of loop[1].split(/\s+/).filter(Boolean)) tested.add(name);
    if (!/\bcargo test\b/.test(line) || /\s--(test|example|doc|lib|bin)\b/.test(line)) continue;
    for (const m of line.matchAll(/\s-p\s+([A-Za-z0-9_-]+)/g)) tested.add(m[1]);
    if (/\s--workspace\b/.test(line)) {
      const excluded = new Set([...line.matchAll(/\s--exclude\s+([A-Za-z0-9_-]+)/g)].map((m) => m[1]));
      for (const name of allNames) if (!excluded.has(name)) tested.add(name);
    }
  }
  return tested;
}

/**
 * Workspace members no workflow both triggers on and tests. A crate in the
 * rust/ workspace that no `cargo test` command names, or that is named only by
 * workflows whose `paths:` filter does not match its directory, is changed and
 * merged with no CI saying a word: the green check never looked at it. `tested`
 * is false when no workflow names it at all.
 *
 * @param {{ dir: string, name: string }[]} members
 * @param {Record<string, string>} workflows file name to yaml
 * @returns {{ name: string, tested: boolean }[]}
 */
export function membersOutOfScope(members, workflows) {
  const names = members.map((m) => m.name);
  const bad = [];
  for (const m of members) {
    const probe = `rust/${m.dir}/src/lib.rs`;
    let triggered = false;
    let tested = false;
    for (const yaml of Object.values(workflows)) {
      const paths = triggerPaths(yaml);
      const covers = paths === "all" || paths.some((g) => pathGlob(g).test(probe));
      const names_ = testedPackages(yaml, names);
      triggered ||= covers && names_.has(m.name);
      tested ||= names_.has(m.name);
    }
    if (!triggered) bad.push({ name: m.name, tested });
  }
  return bad;
}

/**
 * Workflows that name a Rust channel instead of reading rust-toolchain.toml.
 *
 * @param {Record<string, string>} workflows
 * @returns {string[]}
 */
export function workflowsNamingAChannel(workflows) {
  return Object.entries(workflows)
    .filter(([, yaml]) => /^\s*(?:-\s*)?uses:\s*dtolnay\/rust-toolchain@/m.test(yaml))
    .map(([name]) => name)
    .sort();
}

/**
 * Workflows without a concurrency group.
 *
 * @param {Record<string, string>} workflows
 * @returns {string[]}
 */
export function workflowsMissingConcurrency(workflows) {
  return Object.entries(workflows)
    .filter(([, yaml]) => !/^concurrency\s*:/m.test(yaml.slice(0, Math.max(0, yaml.search(/^jobs\s*:/m)))))
    .map(([name]) => name)
    .sort();
}

/** @param {string} dir @returns {Record<string, string>} */
export function readWorkflows(dir) {
  return Object.fromEntries(workflowFiles(dir).map((n) => [n, readFileSync(path.join(dir, n), "utf8")]));
}

/**
 * Structural checks that need no actionlint. Returns the failures.
 * @param {string} repoRoot
 * @returns {string[]}
 */
export function structuralFailures(repoRoot) {
  const workflows = readWorkflows(path.join(repoRoot, ".github", "workflows"));
  const failures = [];
  const channel = workflowsNamingAChannel(workflows);
  if (channel.length > 0) {
    failures.push(`workflows name a Rust channel instead of using .github/actions/setup-rust: ${channel.join(", ")}`);
  }
  const noConcurrency = workflowsMissingConcurrency(workflows);
  if (noConcurrency.length > 0) {
    failures.push(`workflows without a top-level concurrency group: ${noConcurrency.join(", ")}`);
  }
  // Every workflow but release.yml and npm-publish.yml is Rust CI; the scope
  // check reads all of them, because any workflow may be the one that tests a crate.
  const out = membersOutOfScope(workspaceMembers(path.join(repoRoot, "rust")), workflows);
  for (const m of out) {
    failures.push(
      `workspace member ${m.name} is not both triggered and tested by any workflow ` +
        "(add its path to a workflow's paths filter and name it in a cargo test command)",
    );
  }
  return failures;
}

export function main() {
  const files = workflowFiles(WORKFLOW_DIR);
  if (files.length === 0) {
    console.error(`FAIL: no workflow files found in ${WORKFLOW_DIR} — nothing was checked`);
    return 2;
  }
  const missing = workflowsMissingPermissions(WORKFLOW_DIR);
  if (missing.length > 0) {
    console.error(
      `FAIL: workflows missing a top-level permissions block: ${missing.join(", ")}`,
    );
    return 1;
  }
  const structural = structuralFailures(REPO_ROOT);
  if (structural.length > 0) {
    for (const f of structural) console.error(`FAIL: ${f}`);
    return 1;
  }
  const result = spawnSync("actionlint", ["-color"], { cwd: REPO_ROOT, stdio: "inherit" });
  const { code, message } = interpret(result);
  if (code === 0) {
    console.log(`OK: ${files.length} workflow files lint clean (${files.join(", ")})`);
    return 0;
  }
  console.error(`FAIL: ${message}`);
  return code;
}

const invokedPath = process.argv[1] ? path.resolve(process.argv[1]) : "";
if (invokedPath === fileURLToPath(import.meta.url)) {
  process.exitCode = main();
}
