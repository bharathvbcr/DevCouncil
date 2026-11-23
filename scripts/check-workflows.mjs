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
 * Exit codes: 0 workflows are clean · 1 actionlint or a supply-chain
 * rule found problems · 2 the check could not run
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

const LOCKED_VERBS = new Set(["build", "test", "check", "clippy"]);
const MAX_SCAN_CHARS = 2_000_000;

/**
 * Cargo `build`, `test`, `check`, and `clippy` commands that omit `--locked`.
 * The switch counts only as its own argument before a bare `--`. Comments,
 * `cargo fmt`, `cargo run`, and `cargo mutants` are out of this contract.
 * Backslash continuations are joined. A `run:` YAML prefix and one layer of
 * surrounding quotes are unwrapped so a workflow line and a shell line share
 * one scan.
 *
 * @param {string} text
 * @param {string} label
 * @returns {string[]}
 */
export function cargoStepsMissingLocked(text, label) {
  if (typeof text !== "string" || typeof label !== "string" || label.length === 0) {
    throw new TypeError("cargoStepsMissingLocked expects a string and a non-empty label");
  }
  if (text.length > MAX_SCAN_CHARS) {
    throw new RangeError(`cargo script exceeds ${MAX_SCAN_CHARS} characters`);
  }
  const joined = text.replace(/\\\r?\n/g, " ");
  const failures = [];
  const lines = joined.split("\n");
  for (let i = 0; i < lines.length; i++) {
    for (const argv of shellCommands(unwrapRun(lines[i]))) {
      const at = cargoIndex(argv);
      if (at < 0) continue;
      const verb = argv[at + 1];
      if (!LOCKED_VERBS.has(verb)) continue;
      const rest = argv.slice(at + 2);
      const separator = rest.indexOf("--");
      const args = separator === -1 ? rest : rest.slice(0, separator);
      if (!args.includes("--locked")) {
        failures.push(`${label}:${i + 1}: cargo ${verb} omits --locked`);
      }
    }
  }
  return failures;
}

/** @param {string} line */
function unwrapRun(line) {
  let script = line.replace(/\r$/, "").replace(/^\s*(?:-\s*)?run:\s*/, "").trim();
  if (script.length >= 2) {
    const quote = script[0];
    if ((quote === '"' || quote === "'") && script.endsWith(quote)) script = script.slice(1, -1);
  }
  return script;
}

/** @param {string[]} argv */
function cargoIndex(argv) {
  let index = 0;
  while (index < argv.length && /^[A-Za-z_][A-Za-z0-9_]*=/.test(argv[index])) index++;
  while (index < argv.length && (argv[index] === "command" || argv[index] === "exec")) index++;
  return argv[index] === "cargo" ? index : -1;
}

/**
 * Simple commands in one shell line, including commands nested in `$()`
 * and backticks. Quoted text is literal, except that double quotes still
 * expand substitutions.
 *
 * @param {string} script
 * @returns {string[][]}
 */
function shellCommands(script) {
  /** @type {{ op?: boolean, value: string, nested?: string[] }[]} */
  const tokens = [];
  const length = script.length;
  let index = 0;
  while (index < length) {
    const char = script[index];
    if (char === " " || char === "\t" || char === "\r") {
      index++;
      continue;
    }
    if (char === "#") break;
    if (char === "&" && script[index + 1] === "&") {
      tokens.push({ op: true, value: "&&" });
      index += 2;
      continue;
    }
    if (char === "|" && script[index + 1] === "|") {
      tokens.push({ op: true, value: "||" });
      index += 2;
      continue;
    }
    if (char === "|" || char === ";" || char === "&" || char === "(" || char === ")" || char === "{" || char === "}") {
      tokens.push({ op: true, value: char });
      index++;
      continue;
    }
    if (char === "`") {
      const end = script.indexOf("`", index + 1);
      if (end < 0) {
        tokens.push({ value: script.slice(index), nested: [script.slice(index + 1)] });
        break;
      }
      tokens.push({ value: script.slice(index, end + 1), nested: [script.slice(index + 1, end)] });
      index = end + 1;
      continue;
    }
    if (char === "$" && script[index + 1] === "(") {
      const end = matchingParen(script, index + 1);
      if (end < 0) {
        tokens.push({ value: script.slice(index), nested: [script.slice(index + 2)] });
        break;
      }
      tokens.push({ value: script.slice(index, end + 1), nested: [script.slice(index + 2, end)] });
      index = end + 1;
      continue;
    }
    if (char === "'" || char === '"') {
      const quoted = readQuoted(script, index);
      const nested = char === '"' ? substitutions(quoted.value) : [];
      tokens.push({ value: quoted.value, nested });
      index = quoted.next;
      continue;
    }
    let end = index;
    while (end < length) {
      const next = script[end];
      if (" \t\r#|&;(){}`'".includes(next)) break;
      if (next === "$" && script[end + 1] === "(") break;
      if (next === "\\") {
        end += end + 1 < length ? 2 : 1;
        continue;
      }
      end++;
    }
    tokens.push({ value: unescapeWord(script.slice(index, end)) });
    index = end;
  }

  /** @type {string[][]} */
  const commands = [];
  /** @type {string[]} */
  let current = [];
  const flush = () => {
    if (current.length > 0) commands.push(current);
    current = [];
  };
  for (const token of tokens) {
    if (token.op) {
      flush();
      continue;
    }
    for (const inner of token.nested ?? []) {
      for (const nested of shellCommands(inner)) commands.push(nested);
    }
    current.push(token.value);
  }
  flush();
  return commands;
}

/** @param {string} script @param {number} openAt */
function matchingParen(script, openAt) {
  let depth = 0;
  let index = openAt;
  while (index < script.length) {
    const char = script[index];
    if (char === "'" || char === '"') {
      index = readQuoted(script, index).next;
      continue;
    }
    if (char === "(") depth++;
    else if (char === ")") {
      depth--;
      if (depth === 0) return index;
    }
    index++;
  }
  return -1;
}

/** @param {string} script @param {number} start */
function readQuoted(script, start) {
  const quote = script[start];
  let value = "";
  let index = start + 1;
  while (index < script.length) {
    const char = script[index];
    if (quote === '"' && char === "\\") {
      value += script[index + 1] ?? "";
      index += 2;
      continue;
    }
    if (char === quote) return { value, next: index + 1 };
    value += char;
    index++;
  }
  return { value, next: script.length };
}

/** @param {string} value */
function substitutions(value) {
  /** @type {string[]} */
  const inners = [];
  let index = 0;
  while (index < value.length) {
    if (value[index] === "`") {
      const end = value.indexOf("`", index + 1);
      if (end < 0) {
        inners.push(value.slice(index + 1));
        break;
      }
      inners.push(value.slice(index + 1, end));
      index = end + 1;
      continue;
    }
    if (value[index] === "$" && value[index + 1] === "(") {
      const end = matchingParen(value, index + 1);
      if (end < 0) {
        inners.push(value.slice(index + 2));
        break;
      }
      inners.push(value.slice(index + 2, end));
      index = end + 1;
      continue;
    }
    index++;
  }
  return inners;
}

/** @param {string} word */
function unescapeWord(word) {
  let value = "";
  for (let index = 0; index < word.length; index++) {
    if (word[index] === "\\" && index + 1 < word.length) {
      value += word[index + 1];
      index++;
      continue;
    }
    value += word[index];
  }
  return value;
}

/**
 * `rust.yml` compiles `restriction.rs` only as `cfg(test)` inside devmap-cli.
 * A push or pull request that changes only that file must still start the
 * test job. An explicit `paths:` list that misses it does not. No `paths:`
 * key means the event runs for every file, which does cover it. A comment
 * that mentions the glob does not.
 *
 * @param {string} yaml
 * @param {string} label
 * @returns {string[]}
 */
export function testsupportPathsMissing(yaml, label) {
  if (typeof yaml !== "string" || typeof label !== "string" || label.length === 0) {
    throw new TypeError("testsupportPathsMissing expects a string and a non-empty label");
  }
  if (yaml.length > 2_000_000) {
    throw new RangeError("workflow is too large to scan");
  }
  const headEnd = yaml.search(/^jobs\s*:/m);
  const head = headEnd === -1 ? yaml : yaml.slice(0, headEnd);
  const probes = [
    "rust/testsupport/restriction.rs",
    "rust/testsupport/nested/odd 'quote' 名前.rs",
  ];
  // null: that event is not in the workflow. "all": the event has no paths
  // key, so it runs for every file. An array is an explicit paths list, and an
  // empty one matches nothing.
  /** @type {Record<string, string[] | "all" | null>} */
  const events = { push: null, pull_request: null };
  let event = "";
  let inPaths = false;
  for (const line of head.split(/\r?\n/)) {
    const ev = /^  ([A-Za-z0-9_-]+)\s*:/.exec(line);
    if (ev) {
      event = ev[1];
      inPaths = false;
      if (Object.prototype.hasOwnProperty.call(events, event) && events[event] === null) {
        events[event] = "all";
      }
      continue;
    }
    if (!Object.prototype.hasOwnProperty.call(events, event)) continue;
    if (/^    paths\s*:/.test(line)) {
      inPaths = true;
      events[event] = [];
      continue;
    }
    if (!inPaths || !Array.isArray(events[event])) continue;
    const item = /^      -\s*["']?([^"'#\s]+)["']?\s*(#.*)?$/.exec(line);
    if (item) {
      events[event].push(item[1]);
      continue;
    }
    if (/^\s*(#.*)?$/.test(line)) continue;
    inPaths = false;
  }
  const failures = [];
  for (const name of ["push", "pull_request"]) {
    const globs = events[name];
    if (globs === null) {
      failures.push(`${label}: ${name} is absent, so a testsupport-only ${name} does not run`);
      continue;
    }
    if (globs === "all") continue;
    const covers = probes.every((probe) => globs.some((glob) => pathGlob(glob).test(probe)));
    if (!covers) {
      failures.push(`${label}: ${name} paths do not match rust/testsupport/**`);
    }
  }
  return failures;
}

/** @param {string} repoRoot @returns {Record<string, string>} */
export function readCompositeActions(repoRoot) {
  const root = path.join(repoRoot, ".github", "actions");
  if (!existsSync(root)) return {};
  /** @type {Record<string, string>} */
  const out = {};
  for (const dir of readdirSync(root)) {
    const yml = path.join(root, dir, "action.yml");
    if (!existsSync(yml)) continue;
    out[path.posix.join(".github", "actions", dir, "action.yml")] = readFileSync(yml, "utf8");
  }
  return out;
}

/**
 * Bodies of `run:` steps. `defaults: run:` is not a step.
 * @param {string} yaml
 * @returns {string[]}
 */
export function runBodies(yaml) {
  const lines = yaml.split("\n");
  /** @type {string[]} */
  const bodies = [];
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (/^\s*#/.test(line)) continue;
    if (/^\s*defaults\s*:/.test(line)) continue;
    const m = /^(\s*)(?:-\s+)?run:\s*(.*?)\s*$/.exec(line);
    if (!m) continue;
    const indent = m[1].length;
    const rest = m[2].replace(/\s+#.*$/, "");
    if (rest === "|" || rest === "|-" || rest === "|+" || rest === ">" || rest === ">-" || rest === ">+") {
      /** @type {string[]} */
      const chunk = [];
      while (i + 1 < lines.length) {
        const next = lines[i + 1];
        if (next.trim() === "") {
          chunk.push("");
          i++;
          continue;
        }
        const nextIndent = /^ */.exec(next)?.[0].length ?? 0;
        if (nextIndent <= indent) break;
        chunk.push(next);
        i++;
      }
      bodies.push(chunk.join("\n"));
      continue;
    }
    bodies.push(rest.replace(/^['"]|['"]$/g, ""));
  }
  return bodies;
}

/**
 * @param {string} body
 * @returns {string[]}
 */
function actionExpressions(body) {
  /** @type {string[]} */
  const exprs = [];
  let from = 0;
  while (from < body.length) {
    const start = body.indexOf("${{", from);
    if (start === -1) break;
    let depth = 0;
    let end = -1;
    for (let k = start + 2; k < body.length; k++) {
      const ch = body[k];
      if (ch === "{") depth += 1;
      else if (ch === "}") {
        depth -= 1;
        if (depth === 0) {
          end = k + 1;
          break;
        }
      }
    }
    if (end === -1) {
      exprs.push(body.slice(start));
      break;
    }
    exprs.push(body.slice(start, end));
    from = end;
  }
  return exprs;
}

/**
 * Rewrites bracket property access with a literal key to dot access.
 * @param {string} expr
 */
function dotted(expr) {
  return expr.replace(/\[\s*(['"])([A-Za-z0-9_-]+)\1\s*\]/g, ".$2");
}

/**
 * @param {string} body
 * @returns {boolean}
 */
export function pipesCurlToShell(body) {
  const flat = body.replace(/\\\r?\n/g, " ");
  const shellName = String.raw`(?:(?:\/\S+)?\/)?(?:ba|z|da|k|a)?sh\b`;
  // Wrappers that still hand stdin to the shell: sudo [-flags], env [-flags], /usr/bin/env.
  const wrappers = String.raw`(?:(?:(?:\/\S+\/)?sudo|(?:\/\S+\/)?env)\b(?:\s+-\S+)*\s+)*`;
  const fetcher = String.raw`(?:curl|wget)\b`;
  const patterns = [
    // curl ... | [sudo -E] [/usr/bin/env] bash
    new RegExp(String.raw`\b${fetcher}[^|\n]*\|\s*${wrappers}${shellName}`, "i"),
    // bash <(curl ...), source <(curl ...), . <(curl ...)
    new RegExp(String.raw`(?:\b(?:source|eval)\b|(?:^|[\s;&|(])\.|\b${shellName})\s*(?:-\S+\s+)*<\(\s*${fetcher}`, "im"),
    // bash -c "$(curl ...)", eval "$(curl ...)", eval `curl ...`
    new RegExp(
      String.raw`(?:\beval\b|\b${shellName}\s+(?:-\S+\s+)*-[A-Za-z]*c\b)[^\n;|]*?(?:\$\(|\`)\s*${fetcher}`,
      "i",
    ),
  ];
  return patterns.some((pattern) => pattern.test(flat));
}

/** @param {string} yaml @returns {string[]} uses values that are not local and not a 40-hex pin */
export function unpinnedUses(yaml) {
  /** @type {string[]} */
  const bad = [];
  for (const line of yaml.split("\n")) {
    if (line.trim().startsWith("#")) continue;
    const m = /(?:^|\s)uses:\s*['"]?([^'"\s#]+)/.exec(line);
    if (!m) continue;
    const uses = m[1];
    if (uses.startsWith("./")) continue;
    const at = uses.lastIndexOf("@");
    const ref = at === -1 ? "" : uses.slice(at + 1);
    if (!/^[0-9a-f]{40}$/.test(ref)) bad.push(uses);
  }
  return bad;
}

/**
 * @param {string} yaml
 * @returns {boolean}
 */
export function triggersPullRequest(yaml) {
  const jobsAt = yaml.search(/^jobs\s*:/m);
  const head = jobsAt === -1 ? yaml : yaml.slice(0, jobsAt);
  let inOn = false;
  for (const line of head.split("\n")) {
    if (/^\s*#/.test(line)) continue;
    const top = /^(on|permissions|env|concurrency|name|jobs|defaults)\s*:/.exec(line);
    if (top) {
      inOn = top[1] === "on";
      const inline = /^on\s*:\s*\[(.*)\]/.exec(line);
      if (inline && inline[1].split(",").some((part) => part.trim() === "pull_request")) return true;
      continue;
    }
    if (!inOn) continue;
    if (/^\s*-\s*pull_request\s*(?:#.*)?$/.test(line)) return true;
    if (/^\s*pull_request\s*:/.test(line)) return true;
  }
  return false;
}

/**
 * @param {string} section
 * @param {number} keyIndent
 * @returns {{ found: boolean, writeAll: boolean, readAll: boolean, contents: string | null, idToken: string | null }}
 */
function permissionsAt(section, keyIndent) {
  const absent = { found: false, writeAll: false, readAll: false, contents: null, idToken: null };
  const re = new RegExp(`^${" ".repeat(keyIndent)}permissions\\s*:(.*)$`, "m");
  const m = re.exec(section);
  if (!m) return absent;
  const rest = m[1].trim().replace(/\s+#.*$/, "");
  /** @type {string[]} */
  const lines = [];
  if (!rest) {
    const after = section.slice(m.index + m[0].length + 1);
    for (const line of after.split("\n")) {
      if (line.trim() === "") {
        lines.push(line);
        continue;
      }
      const ind = /^ */.exec(line)?.[0].length ?? 0;
      if (ind <= keyIndent) break;
      lines.push(line);
    }
  }
  const text = rest || lines.join("\n");
  const block = { found: true, writeAll: false, readAll: false, contents: null, idToken: null };
  const one = text.trim().replace(/^(['"])(.*)\1$/, "$2");
  if (one === "write-all") {
    block.writeAll = true;
    return block;
  }
  if (one === "read-all") {
    block.readAll = true;
    return block;
  }
  if (one === "{}" || one === "") return block;
  // A flow mapping, { contents: write, id-token: write }, is one line per entry.
  const entries = /^\{.*\}$/s.test(one) ? one.slice(1, -1).split(",") : text.split("\n");
  for (const line of entries) {
    const contents = /^\s*['"]?contents['"]?\s*:\s*['"]?([A-Za-z-]+)['"]?/.exec(line);
    if (contents) block.contents = contents[1];
    const idToken = /^\s*['"]?id-token['"]?\s*:\s*['"]?([A-Za-z-]+)['"]?/.exec(line);
    if (idToken) block.idToken = idToken[1];
  }
  return block;
}

/**
 * @param {{ found: boolean, writeAll: boolean, readAll: boolean, contents: string | null, idToken: string | null }} block
 * @returns {{ contents: string, idToken: string }}
 */
function realizedPermissions(block) {
  if (block.writeAll) return { contents: "write", idToken: "write" };
  if (block.readAll) return { contents: "read", idToken: "read" };
  return { contents: block.contents ?? "none", idToken: block.idToken ?? "none" };
}

/**
 * @param {string} yaml
 * @returns {{ id: string, condition: string, permissions: { found: boolean, writeAll: boolean, readAll: boolean, contents: string | null, idToken: string | null } }[]}
 */
function jobsOf(yaml) {
  const start = yaml.search(/^jobs\s*:/m);
  if (start === -1) return [];
  const lines = yaml.slice(start).split("\n");
  /** @type {{ id: string, lines: string[] }[]} */
  const jobs = [];
  /** @type {{ id: string, lines: string[] } | null} */
  let current = null;
  for (let i = 1; i < lines.length; i++) {
    const job = /^  ([A-Za-z0-9_-]+):\s*(?:#.*)?$/.exec(lines[i]);
    if (job) {
      if (current) jobs.push(current);
      current = { id: job[1], lines: [] };
      continue;
    }
    if (current) current.lines.push(lines[i]);
  }
  if (current) jobs.push(current);
  return jobs.map((job) => {
    const body = job.lines.join("\n");
    let condition = "";
    const linesOfJob = job.lines;
    for (let i = 0; i < linesOfJob.length; i++) {
      const m = /^    if:\s*(.*)$/.exec(linesOfJob[i]);
      if (!m) continue;
      const rest = m[1].trim();
      if (rest === "|" || rest === "|-" || rest === "|+" || rest === ">" || rest === ">-" || rest === ">+") {
        /** @type {string[]} */
        const chunk = [];
        for (let j = i + 1; j < linesOfJob.length; j++) {
          const next = linesOfJob[j];
          if (next.trim() === "") {
            chunk.push(next);
            continue;
          }
          if ((/^ */.exec(next)?.[0].length ?? 0) <= 4) break;
          chunk.push(next);
        }
        condition = chunk.join("\n");
      } else {
        condition = rest;
      }
      break;
    }
    return { id: job.id, condition, permissions: permissionsAt(body, 4) };
  });
}

/**
 * Split `text` on `op` (`&&` or `||`) outside parentheses.
 * @param {string} text
 * @param {"&&" | "||"} op
 * @returns {string[]}
 */
function splitTopLevel(text, op) {
  /** @type {string[]} */
  const parts = [];
  let depth = 0;
  let start = 0;
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (ch === "(") depth += 1;
    else if (ch === ")") depth = Math.max(0, depth - 1);
    else if (depth === 0 && text.startsWith(op, i)) {
      parts.push(text.slice(start, i).trim());
      i += op.length - 1;
      start = i + 1;
    }
  }
  parts.push(text.slice(start).trim());
  return parts.filter(Boolean);
}

/**
 * True only when every top-level `||` alternative excludes a pull request.
 * `&&` binds tighter than `||`, so `a && b || c` is `(a && b) || c` and `c`
 * still runs on a pull request. An alternative excludes it when one of that
 * alternative's own top-level `&&` conjuncts is exactly
 * `github.event_name != 'pull_request'` or `startsWith(github.ref, 'refs/tags/...')`.
 * An `||` inside parentheses is not a top-level alternative.
 * @param {string} condition
 */
function excludesPullRequest(condition) {
  const text = condition.replace(/\$\{\{\s*|\s*\}\}/g, " ").replace(/\s+/g, " ").trim();
  if (!text) return false;
  const exclusion =
    /^(?:github\.event_name\s*!=\s*['"]pull_request['"]|startsWith\(\s*github\.ref\s*,\s*['"]refs\/tags\/[^'"]*['"]\s*\))$/;
  const alternatives = splitTopLevel(text, "||");
  if (alternatives.length === 0) return false;
  return alternatives.every((alternative) =>
    splitTopLevel(alternative, "&&").some((part) => exclusion.test(part)),
  );
}

/**
 * Supply-chain failures for workflow and composite-action YAML.
 * `files` maps a display name to its text. An absent permissions block is not
 * treated as read: on pull_request that is a failure, same as contents: write.
 *
 * @param {Record<string, string>} files
 * @returns {string[]}
 */
export function supplyChainFailures(files) {
  /** @type {string[]} */
  const failures = [];
  for (const [name, yaml] of Object.entries(files)) {
    for (const body of runBodies(yaml)) {
      if (pipesCurlToShell(body)) failures.push(`${name}: run pipes curl output to a shell`);
      // Bracket access reaches the same values: matrix['dist_args'] is matrix.dist_args.
      const exprs = actionExpressions(body).map(dotted);
      if (exprs.some((expr) => /\bmatrix\b/.test(expr))) {
        failures.push(`${name}: matrix expression in run`);
      }
      if (
        exprs.some(
          (expr) =>
            /\bgithub\.ref_name\b/.test(expr) ||
            /\bgithub\.head_ref\b/.test(expr) ||
            /\bgithub\.event\b/.test(expr),
        )
      ) {
        failures.push(`${name}: event expression in run`);
      }
    }
    for (const uses of unpinnedUses(yaml)) failures.push(`${name}: unpinned action ${uses}`);
    for (const match of yaml.matchAll(/@anthropic-ai\/claude-code(?:@([^\s'"]+))?/g)) {
      if (match[1] && /^\d+\.\d+\.\d+$/.test(match[1])) continue;
      failures.push(`${name}: claude-code install is not an exact version`);
    }
    const pulls = triggersPullRequest(yaml);
    const jobsAt = yaml.search(/^jobs\s*:/m);
    const head = jobsAt === -1 ? yaml : yaml.slice(0, jobsAt);
    const workflowPerms = permissionsAt(head, 0);
    if (workflowPerms.found && realizedPermissions(workflowPerms).idToken === "write") {
      failures.push(`${name}: workflow permissions grant id-token: write`);
    }
    for (const job of jobsOf(yaml)) {
      const block = job.permissions.found ? job.permissions : workflowPerms;
      if (!block.found) {
        if (pulls) failures.push(`${name}: job ${job.id} has no permissions on pull_request`);
        continue;
      }
      const perms = realizedPermissions(block);
      const onPullRequest = pulls && !excludesPullRequest(job.condition);
      if (onPullRequest && perms.contents === "write") {
        failures.push(`${name}: job ${job.id} holds contents: write on pull_request`);
      }
      if (onPullRequest && perms.idToken === "write") {
        failures.push(`${name}: job ${job.id} holds id-token: write on pull_request`);
      }
      if (perms.idToken === "write" && job.id !== "publish") {
        failures.push(`${name}: job ${job.id} holds id-token: write`);
      }
    }
  }
  return failures;
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
  // A cargo build, test, check, or clippy that omits --locked rewrites
  // Cargo.lock on the runner and can still exit 0. fmt, run, and mutants
  // are a different contract; ignored-tests.yml already passes the flag.
  for (const name of ["rust.yml", "analysis-plane.yml"]) {
    const yaml = workflows[name];
    if (typeof yaml !== "string") {
      failures.push(`missing workflow ${name}; cargo --locked was not checked`);
      continue;
    }
    failures.push(...cargoStepsMissingLocked(yaml, name));
    if (name === "rust.yml") failures.push(...testsupportPathsMissing(yaml, name));
  }
  const verifyPath = path.join(repoRoot, "rust", "verify.sh");
  if (!existsSync(verifyPath)) {
    failures.push("missing rust/verify.sh; cargo --locked was not checked");
  } else {
    failures.push(...cargoStepsMissingLocked(readFileSync(verifyPath, "utf8"), "rust/verify.sh"));
  }
  failures.push(...supplyChainFailures({ ...workflows, ...readCompositeActions(repoRoot) }));
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