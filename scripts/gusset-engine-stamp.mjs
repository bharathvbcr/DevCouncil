#!/usr/bin/env node
/**
 * The input stamp for libgusset.a: what a built archive was built from, so a
 * consumer can tell whether it still matches a checkout's sources.
 *
 *   node scripts/gusset-engine-stamp.mjs begin <archive-dir> <gusset-dir>   # deletes the old stamp, prints the inputs
 *   node scripts/gusset-engine-stamp.mjs write <archive-dir> <gusset-dir> < inputs-from-begin
 *
 * A build is: begin (delete the old stamp, record the inputs), build, write
 * (only if the inputs are unchanged by the build and the archive exists). A
 * failed or raced stamp therefore leaves NO stamp, never the previous one.
 * Whoever builds the archive writes <archive-dir>/libgusset.stamp.json:
 * ensureGussetEngine in ci-local.mjs and rust/gusset-engine/cgo-env.sh (which
 * calls this CLI, so there is one computation). scripts/lane-env.mjs compares
 * it with the lane's own inputs. The inputs are the committed git trees of
 * rust/gusset-engine and every path dependency under the repository, read from
 * the Cargo.toml files (rust/dc-glob today; its own path deps too), the blob of
 * rust-toolchain.toml, the blob of the workspace root manifest (rust/Cargo.toml)
 * when a crate inherits version, license, lints or a dependency from it, and the
 * sibling gusset's HEAD. Manifests are parsed as TOML (parseToml) and classified
 * by structure; a manifest that cannot be read, or a path key the stamp cannot
 * classify, is refused (InputError) rather than skipped. Cargo.lock is inside the
 * engine tree, so it is covered. "dirty" is any uncommitted or untracked change
 * under those inputs or in the gusset sibling (git status --untracked-files=all,
 * which ignores status.showUntrackedFiles); a dirty stamp is never reusable.
 *
 * The one rule that closes the class: the stamp is a tree hash plus a dirty
 * flag, so it is sound for every file under an input that is in HEAD or shows
 * as dirty, and engineInputs refuses when anything under an input is neither:
 * an ignored file (other than a crate's target/), an index-flagged file
 * (assume-unchanged, skip-worktree), a gitlink or a symlink. That is what keeps
 * cargo's auto-discovery (build.rs, src/lib.rs, src/bin/*), workspace globs and
 * mod/include!/#[path] from reaching a file the stamp does not see. A refusal
 * names the files and the --engine-dir escape.
 *
 * NOT covered, by design: source-level paths that leave the crate (include!,
 * #[path] or include_str! pointing outside the input directories) or into the
 * engine's own target/ (ENGINE_TARGET, the one ignored tree that is tolerated); .cargo/config.toml
 * at the repository, a parent or the user level; RUSTFLAGS and other
 * environment; the installed toolchain beyond the pinned channel in
 * rust-toolchain.toml; symlinks inside the sibling gusset (only its HEAD and
 * dirty state are recorded).
 *
 * Every git child runs with a bounded timeout (a timeout is a refusal, never a
 * pass) and with fsmonitor disabled through its environment, deliberately: a
 * wedged fsmonitor daemon must not be able to hang lane-env.
 */
import { spawnSync } from "node:child_process";
import { existsSync, lstatSync, readFileSync, realpathSync, renameSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const ENGINE = "rust/gusset-engine";
/** Where the engine build puts its output (cgo-env.sh and ci-local.mjs pass --target-dir <engine>/target); the only ignored tree the stamp tolerates. */
export const ENGINE_TARGET = `${ENGINE}/target`;
export const STAMP_NAME = "libgusset.stamp.json";

/** A known way the inputs cannot be computed; anything else thrown is a bug. */
export class InputError extends Error {}

const gitTimeoutMs = () => Number(process.env.GUSSET_STAMP_GIT_TIMEOUT_MS) || 30_000;

/** The child environment: the caller's, plus core.fsmonitor=false (appended after any GIT_CONFIG_* pairs). */
function gitEnv() {
  const n = Number(process.env.GIT_CONFIG_COUNT) || 0;
  return { ...process.env, GIT_CONFIG_COUNT: String(n + 1), [`GIT_CONFIG_KEY_${n}`]: "core.fsmonitor", [`GIT_CONFIG_VALUE_${n}`]: "false" };
}

/** Run git; a timeout or a failure to start is an InputError, so it can never read as a pass. */
function run(cwd, args) {
  const ms = gitTimeoutMs();
  const r = spawnSync("git", ["-C", cwd, ...args], { encoding: "utf8", env: gitEnv(), timeout: ms, killSignal: "SIGKILL" });
  if (r.error || r.signal) {
    throw new InputError(`git ${args.join(" ")} (in ${cwd}) did not finish within ${ms} ms (${r.error?.code ?? r.signal}); refused rather than assumed clean`);
  }
  return r;
}

function git(cwd, ...args) {
  const r = run(cwd, args);
  if (r.status !== 0) throw new InputError(`git ${args.join(" ")} (in ${cwd}): ${r.stderr.trim() || "failed"}`);
  return r.stdout.trim();
}

class Raw {
  constructor(text) {
    this.text = text;
  }
}
const isTable = (v) => v !== null && typeof v === "object" && !Array.isArray(v) && !(v instanceof Raw);
const newTable = () => Object.create(null);
/** table -> 1-based line of its header or opening brace, for refusal messages */
const LINES = new WeakMap();
const where = (label, t) => (LINES.has(t) ? `${label}:${LINES.get(t)}` : label);
const isSpace = (c) => c === " " || c === "\t" || c === "\r" || c === "\n";
const ESCAPES = { e: "\x1b", b: "\b", t: "\t", n: "\n", f: "\f", r: "\r", '"': '"', "\\": "\\" };
const BARE_KEY = /[A-Za-z0-9_-]+/y;
const BARE_VALUE = /[^\s,\]}#]+/y;

/**
 * A TOML 1.0 reader (tables, arrays of tables, dotted and quoted keys, basic and
 * literal strings in both single- and multi-line form, inline tables, arrays).
 * Scalars other than strings and booleans come back as Raw. It exists so the
 * stamp classifies manifests by structure, never by the spelling of a line, and
 * it throws an InputError on anything it cannot read completely.
 */
export function parseToml(text, label) {
  const src = text.startsWith("\uFEFF") ? text.slice(1) : text;
  let i = 0;
  const mk = () => {
    const t = newTable();
    LINES.set(t, src.slice(0, i).split("\n").length);
    return t;
  };
  const fail = (why) => {
    throw new InputError(`${label}: cannot read this manifest (${why}, line ${src.slice(0, i).split("\n").length}); the engine stamp refuses a manifest it cannot parse completely`);
  };
  const peek = (s) => src.startsWith(s, i);
  const ws = () => {
    while (src[i] === " " || src[i] === "\t") i++;
  };
  const comment = () => {
    if (src[i] === "#") while (i < src.length && src[i] !== "\n") i++;
  };
  const space = () => {
    for (;;) {
      ws();
      comment();
      if (src[i] === "\n" || src[i] === "\r") i++;
      else return;
    }
  };
  const eol = () => {
    ws();
    comment();
    if (i < src.length && src[i] !== "\n" && src[i] !== "\r") fail("unexpected text after a value or header");
  };
  const basic = (multi) => {
    const q = multi ? '"""' : '"';
    i += q.length;
    if (multi) {
      if (src[i] === "\r") i++;
      if (src[i] === "\n") i++;
    }
    let out = "";
    for (;;) {
      if (i >= src.length) fail("unterminated string");
      if (peek(q)) {
        if (!multi) {
          i++;
          return out;
        }
        let extra = 0;
        while (extra < 2 && src[i + 3 + extra] === '"') extra++;
        i += 3 + extra;
        return out + '"'.repeat(extra);
      }
      const c = src[i];
      if (c === "\n" && !multi) fail("newline in a single-line string");
      if (c !== "\\") {
        out += c;
        i++;
        continue;
      }
      const e = src[i + 1];
      if (e === "u" || e === "U") {
        const len = e === "u" ? 4 : 8;
        const hex = src.slice(i + 2, i + 2 + len);
        const cp = Number.parseInt(hex, 16);
        if (hex.length !== len || !/^[0-9a-fA-F]+$/.test(hex) || cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff)) fail("bad unicode escape");
        out += String.fromCodePoint(cp);
        i += 2 + len;
      } else if (multi && e !== undefined && isSpace(e)) {
        i++;
        while (i < src.length && isSpace(src[i])) i++;
      } else if (e !== undefined && Object.hasOwn(ESCAPES, e)) {
        out += ESCAPES[e];
        i += 2;
      } else {
        fail("unknown escape in a string");
      }
    }
  };
  const literal = (multi) => {
    const q = multi ? "'''" : "'";
    i += q.length;
    if (multi) {
      if (src[i] === "\r") i++;
      if (src[i] === "\n") i++;
    }
    const end = src.indexOf(q, i);
    if (end < 0) fail("unterminated string");
    let text = src.slice(i, end);
    if (!multi && /[\r\n]/.test(text)) fail("newline in a single-line string");
    i = end + q.length;
    for (let extra = 0; multi && extra < 2 && src[i] === "'"; extra++, i++) text += "'";
    return text;
  };
  const key = () => {
    if (src[i] === '"') return basic(false);
    if (src[i] === "'") return literal(false);
    BARE_KEY.lastIndex = i;
    const m = BARE_KEY.exec(src);
    if (!m) fail("expected a key");
    i += m[0].length;
    return m[0];
  };
  const keyPath = () => {
    const keys = [key()];
    for (;;) {
      ws();
      if (src[i] !== ".") return keys;
      i++;
      ws();
      keys.push(key());
    }
  };
  const assign = (table, keys, v) => {
    let t = table;
    for (const k of keys.slice(0, -1)) {
      if (t[k] === undefined) t[k] = mk();
      if (!isTable(t[k])) fail("a dotted key runs through a value that is not a table");
      t = t[k];
    }
    const last = keys.at(-1);
    if (Object.hasOwn(t, last)) fail("duplicate key");
    t[last] = v;
  };
  const value = () => {
    const c = src[i];
    if (c === '"') return basic(peek('"""'));
    if (c === "'") return literal(peek("'''"));
    if (c === "[") {
      i++;
      const arr = [];
      for (;;) {
        space();
        if (src[i] === "]") break;
        arr.push(value());
        space();
        if (src[i] === ",") i++;
        else if (src[i] !== "]") fail("expected , or ] in an array");
      }
      i++;
      return arr;
    }
    if (c === "{") {
      const t = mk();
      i++;
      for (;;) {
        space();
        if (src[i] === "}") break;
        const keys = keyPath();
        ws();
        if (src[i] !== "=") fail("expected = in an inline table");
        i++;
        space();
        assign(t, keys, value());
        space();
        if (src[i] === ",") i++;
        else if (src[i] !== "}") fail("expected , or } in an inline table");
      }
      i++;
      return t;
    }
    BARE_VALUE.lastIndex = i;
    const m = BARE_VALUE.exec(src);
    if (!m) fail("expected a value");
    i += m[0].length;
    if (/^\d{4}-\d\d-\d\d$/.test(m[0]) && src[i] === " ") {
      // RFC 3339 allows a space between the date and the time
      BARE_VALUE.lastIndex = i + 1;
      const time = /^\d\d:\d\d/.test(src.slice(i + 1, i + 6)) ? BARE_VALUE.exec(src) : null;
      if (time?.index === i + 1) {
        i += 1 + time[0].length;
        return new Raw(`${m[0]} ${time[0]}`);
      }
    }
    return m[0] === "true" ? true : m[0] === "false" ? false : new Raw(m[0]);
  };

  const top = mk();
  const headers = new WeakSet();
  let cur = top;
  for (;;) {
    space();
    if (i >= src.length) return top;
    if (src[i] === "[") {
      const isArray = src[i + 1] === "[";
      i += isArray ? 2 : 1;
      ws();
      const keys = keyPath();
      ws();
      const close = isArray ? "]]" : "]";
      if (!peek(close)) fail("unterminated table header");
      i += close.length;
      eol();
      let t = top;
      keys.forEach((k, j) => {
        if (isArray && j === keys.length - 1) {
          if (t[k] === undefined) t[k] = [];
          if (!Array.isArray(t[k])) fail("an array of tables redefines a table");
          t[k].push(mk());
          t = t[k].at(-1);
          return;
        }
        if (t[k] === undefined) t[k] = mk();
        t = Array.isArray(t[k]) ? t[k].at(-1) : t[k];
        if (!isTable(t)) fail("a table header runs through a value that is not a table");
      });
      if (!isArray) {
        if (headers.has(t)) fail("a table header is defined twice");
        headers.add(t);
      }
      cur = t;
      continue;
    }
    const keys = keyPath();
    ws();
    if (src[i] !== "=") fail("expected =");
    i++;
    ws();
    assign(cur, keys, value());
    eol();
  }
}

const DEP_TABLES = ["dependencies", "dev-dependencies", "build-dependencies", "dev_dependencies", "build_dependencies"];
const TARGET_TABLES = ["lib", "bin", "example", "test", "bench"];

/**
 * Everything in a parsed manifest that names a path or a workspace to inherit
 * from, classified by structure:
 *   "dep"    a dependency (every [dependencies]-like table, at the top level,
 *            under [target.*] and under [workspace], plus [patch.*.*] and
 *            [replace.*]); an entry that is just { workspace = true } is an
 *            "inherit" of that name from the workspace root;
 *   "target" [lib], [[bin]], [[example]], [[test]], [[bench]] paths and
 *            [package] build, which must stay inside the crate.
 * Any other table holding a key named "path" is refused: a path the stamp
 * cannot classify is an input it cannot see, so it must not be skipped.
 *
 * @returns {{ entries: Array<{ kind: "dep" | "target", table: string, key?: string, path?: string, inherit?: string, at: string }>, inherits: boolean, isRoot: boolean, workspace?: string }}
 */
export function manifestRefs(doc, label) {
  const entries = [];
  const consumed = new Set();
  const pathOf = (o, table, key = "path") => {
    if (typeof o[key] !== "string" || o[key] === "") throw new InputError(`${label} [${table}] ${key} is not a non-empty string; the engine stamp cannot cover it`);
    return o[key];
  };
  const dependency = (spec, table) => {
    if (!isTable(spec)) return;
    if ("path" in spec) {
      consumed.add(spec);
      entries.push({ kind: "dep", table, path: pathOf(spec, table), at: where(label, spec) });
    }
  };
  const depTable = (t, table) => {
    if (!isTable(t)) return;
    for (const [name, spec] of Object.entries(t)) {
      dependency(spec, table);
      if (isTable(spec) && spec.workspace === true) entries.push({ kind: "dep", table, inherit: name, at: where(label, spec) });
    }
  };
  for (const name of DEP_TABLES) depTable(doc[name], name);
  depTable(doc.workspace?.dependencies, "workspace.dependencies");
  for (const [cfg, t] of Object.entries(isTable(doc.target) ? doc.target : {})) {
    for (const name of DEP_TABLES) depTable(t?.[name], `target.${cfg}.${name}`);
  }
  for (const [registry, t] of Object.entries(isTable(doc.patch) ? doc.patch : {})) {
    for (const spec of Object.values(isTable(t) ? t : {})) dependency(spec, `patch.${registry}`);
  }
  for (const spec of Object.values(isTable(doc.replace) ? doc.replace : {})) dependency(spec, "replace");
  for (const name of TARGET_TABLES) {
    for (const t of Array.isArray(doc[name]) ? doc[name] : [doc[name]]) {
      if (!isTable(t) || !("path" in t)) continue;
      consumed.add(t);
      entries.push({ kind: "target", table: name, path: pathOf(t, name), at: where(label, t) });
    }
  }
  const build = doc.package?.build;
  if (typeof build === "string") entries.push({ kind: "target", table: "package", key: "build", path: build, at: where(label, doc.package) });
  else if (build !== undefined && typeof build !== "boolean") throw new InputError(`${label} [package] build is neither a path nor a boolean; the engine stamp cannot cover it`);

  let inherits = false;
  const scan = (node, where_) => {
    if (Array.isArray(node)) return node.forEach((n, k) => scan(n, `${where_}[${k}]`));
    if (!isTable(node)) return;
    // [package.metadata] and [workspace.metadata] are free-form data cargo never builds from.
    if (where_ === "package.metadata" || where_ === "workspace.metadata") return;
    if ("path" in node && !consumed.has(node)) {
      throw new InputError(`${where(label, node)}: a path key at ${where_ || "the top level"} is not in a table the engine stamp classifies (dependencies, patch, replace, lib, bin, example, test, bench); it cannot be covered`);
    }
    if (node.workspace === true) inherits = true;
    for (const [k, v] of Object.entries(node)) scan(v, where_ ? `${where_}.${k}` : k);
  };
  scan(doc, "");
  const workspace = doc.package?.workspace;
  if (workspace !== undefined && typeof workspace !== "string") throw new InputError(`${label} [package] workspace is not a path; the engine stamp cannot cover it`);
  return { entries, inherits, isRoot: isTable(doc.workspace), workspace };
}

const posix = (p) => p.split(path.sep).join("/");

/**
 * A [workspace] that lists members, default-members or exclude entries outside
 * its own crate directory builds crates whose sources the stamp does not hash.
 * An entry is judged by its fixed prefix (before the first glob character).
 */
/**
 * A path the stamp relies on must be in HEAD: the stamp hashes HEAD's trees and
 * `git status` does not report ignored files, so an ignored or untracked file
 * could change what is built while the stamp stays identical.
 */
function committed(root, abs, what) {
  for (let d = abs; d !== root && d.startsWith(root + path.sep); d = path.dirname(d)) {
    if (lstatSync(d, { throwIfNoEntry: false })?.isSymbolicLink()) throw new InputError(`${path.relative(root, d)} is a symlink under the engine inputs; the engine stamp hashes only its link text, not what it points to`);
  }
  const rel = path.relative(root, abs).split(path.sep).join("/");
  if (run(root, ["cat-file", "-e", `HEAD:${rel}`]).status !== 0) {
    throw new InputError(`${what} ${rel} is not committed in HEAD (it is ignored or untracked); the engine stamp cannot cover it`);
  }
}

/** Nothing under a crate's target/ is a source: it is build output (and ignored). */
function refuseBuildOutput(abs, crateDir, what) {
  const target = path.join(crateDir, "target");
  if (abs === target || abs.startsWith(target + path.sep)) throw new InputError(`${what} points into ${target}, which is build output; the engine stamp cannot cover it`);
}

function workspaceMembersInside(ws, crateDir, toml, root) {
  if (!isTable(ws)) return;
  for (const name of ["members", "default-members", "exclude"]) {
    if (ws[name] === undefined) continue;
    const at = where(toml, ws);
    if (!Array.isArray(ws[name]) || ws[name].some((m) => typeof m !== "string")) throw new InputError(`${at} [workspace] ${name} is not a list of strings; the engine stamp cannot cover it`);
    for (const raw of ws[name]) {
      // A glob before a .. ("*/../../evil") hides the escape from the prefix check.
      if (raw.split(/[\\/]/).includes("..")) throw new InputError(`${at} [workspace] ${name} entry ${raw} has a .. segment; the engine stamp cannot cover where it leads`);
      // Judged in normal form: "./*/x" is "*/x", and "././t*/x" is "t*/x".
      const member = raw.split(/[\\/]+/).filter((seg) => seg !== "" && seg !== ".").join("/") || ".";
      const prefix = member.split(/[*?[{]/)[0];
      const isGlob = prefix !== member;
      // A glob is judged by the directory it starts in. One in the first path
      // segment (*/x, t*/x) can expand into the crate's own target/, so it is refused.
      const dirPrefix = isGlob ? prefix.slice(0, prefix.lastIndexOf("/") + 1) : prefix;
      if (isGlob && dirPrefix === "") throw new InputError(`${at} [workspace] ${name} glob ${member} starts with a glob in its first path segment, which can expand into the crate's target/; the engine stamp cannot cover it`);
      const abs = path.resolve(crateDir, dirPrefix || ".");
      if (abs !== crateDir && !abs.startsWith(crateDir + path.sep)) {
        throw new InputError(`${at} [workspace] ${name} entry ${member} resolves outside the crate directory; the engine stamp cannot cover it`);
      }
      refuseBuildOutput(abs, crateDir, `${at} [workspace] ${name} entry ${member}`);
      if (name === "exclude" || abs === crateDir) continue;
      if (!isGlob) {
        committed(root, path.join(abs, "Cargo.toml"), `${at} [workspace] ${name} entry ${member}: its Cargo.toml`);
      } else if (run(root, ["check-ignore", "-q", "--", path.relative(root, abs).split(path.sep).join("/")]).status === 0) {
        throw new InputError(`${at} [workspace] ${name} glob ${member} starts in an ignored directory; members found there are not in HEAD and the engine stamp cannot cover them`);
      }
    }
  }
}

/**
 * The workspace root a crate inherits from: itself if it declares [workspace],
 * the directory [package] workspace names, or the nearest ancestor directory
 * (inside the repository) whose Cargo.toml declares one.
 *
 * @returns {{ dir: string, doc: object }}
 */
function workspaceRootOf(root, crateDir, refs, toml) {
  const inside = (d) => d === root || d.startsWith(root + path.sep);
  const candidates = [];
  if (refs.workspace !== undefined) {
    const named = path.resolve(crateDir, refs.workspace);
    candidates.push(path.basename(named) === "Cargo.toml" ? path.dirname(named) : named);
  } else {
    for (let d = path.dirname(crateDir); inside(d); d = path.dirname(d)) candidates.push(d);
  }
  for (const dir of candidates) {
    if (!inside(dir)) throw new InputError(`${toml} names a workspace root ${dir} outside the repository; the engine stamp cannot cover it`);
    const manifest = path.join(dir, "Cargo.toml");
    if (!existsSync(manifest)) continue;
    const doc = parseToml(readFileSync(manifest, "utf8"), manifest);
    if (isTable(doc.workspace)) return { dir, doc };
  }
  throw new InputError(`${toml} inherits from a workspace root but none was found inside the repository; the engine stamp cannot cover it`);
}

/**
 * Repository-relative engine inputs: the engine, its path dependencies, the
 * toolchain file, and the workspace root manifest of every crate that inherits
 * from one (version, license, lints, dependencies: all live in that file).
 */
export function enginePaths(root) {
  const out = [ENGINE];
  const queue = [ENGINE];
  while (queue.length) {
    const rel = queue.shift();
    const crateDir = path.resolve(root, rel);
    const toml = path.join(crateDir, "Cargo.toml");
    if (lstatSync(crateDir, { throwIfNoEntry: false })?.isSymbolicLink()) throw new InputError(`${rel} is a symlink under the engine inputs; the engine stamp hashes only its link text, not what it points to`);
    if (!existsSync(toml)) throw new InputError(`${toml} is missing (a path dependency of the engine)`);
    const doc = parseToml(readFileSync(toml, "utf8"), toml);
    const refs = manifestRefs(doc, toml);
    if (rel === ENGINE) {
      // An engine inside an outer workspace takes its [patch], [profile] and
      // Cargo.lock from that root, none of which the stamp could see.
      if (!refs.isRoot) throw new InputError(`${toml} must declare its own [workspace] (an empty one is enough): without it cargo builds the engine under an outer workspace root whose [patch], [profile] and Cargo.lock the engine stamp cannot cover`);
      if (!existsSync(path.join(crateDir, "Cargo.lock"))) throw new InputError(`${path.join(crateDir, "Cargo.lock")} is missing: the engine's lockfile must sit in the engine directory so the stamp covers it`);
      committed(root, path.join(crateDir, "Cargo.lock"), `${toml}: the engine's lockfile`);
    }
    workspaceMembersInside(doc.workspace, crateDir, toml, root);
    let inheritedFrom;
    if (refs.inherits || refs.entries.some((e) => e.inherit !== undefined)) {
      inheritedFrom = refs.isRoot ? { dir: crateDir, doc } : workspaceRootOf(root, crateDir, refs, toml);
      if (inheritedFrom.dir !== crateDir) {
        const rootToml = posix(path.relative(root, path.join(inheritedFrom.dir, "Cargo.toml")));
        if (!out.includes(rootToml)) out.push(rootToml);
      }
    }
    for (const entry of refs.entries) {
      let { path: entryPath } = entry;
      let base = crateDir;
      if (entry.inherit !== undefined) {
        const declared = inheritedFrom.doc.workspace?.dependencies?.[entry.inherit];
        if (declared === undefined) throw new InputError(`${toml} inherits dependency ${entry.inherit}, which is not declared in the workspace.dependencies of ${inheritedFrom.dir}/Cargo.toml`);
        if (!isTable(declared) || !("path" in declared)) continue;
        if (typeof declared.path !== "string" || declared.path === "") throw new InputError(`${inheritedFrom.dir}/Cargo.toml workspace.dependencies.${entry.inherit} path is not a non-empty string; the engine stamp cannot cover it`);
        entryPath = declared.path;
        base = inheritedFrom.dir;
      }
      const abs = path.resolve(base, entryPath);
      if (entry.kind === "target") {
        // A target source outside the crate directory is an input the stamp cannot see.
        if (!abs.startsWith(crateDir + path.sep)) {
          throw new InputError(`${entry.at} [${entry.table}] ${entry.key ?? "path"} ${entry.path} resolves outside the crate directory; the engine stamp cannot cover it`);
        }
        refuseBuildOutput(abs, crateDir, `${entry.at} [${entry.table}] ${entry.key ?? "path"} ${entry.path}`);
        committed(root, abs, `${entry.at} [${entry.table}] ${entry.key ?? "path"}`);
        continue;
      }
      const dep = posix(path.relative(root, abs));
      if (dep.startsWith("..") || path.isAbsolute(dep)) {
        // Outside the repository it must be the sibling gusset the manifests
        // expect (<root>/../gusset, resolved the way cargo does); its commit is
        // recorded instead. Anything else is an input the stamp cannot see.
        const sibling = path.resolve(root, "..", "gusset");
        if (abs !== sibling && !abs.startsWith(sibling + path.sep)) {
          throw new InputError(`${entry.at} has a path dependency ${entryPath} outside the repository and outside ${sibling}; the engine stamp cannot cover it`);
        }
        continue;
      }
      if (out.includes(dep)) continue;
      committed(root, path.join(abs, "Cargo.toml"), `${entry.at} path dependency ${entryPath}: its Cargo.toml`);
      out.push(dep);
      queue.push(dep);
    }
  }
  if (existsSync(path.join(root, "rust-toolchain.toml"))) out.push("rust-toolchain.toml");
  return out;
}

/**
 * Refuse when anything under an input is neither in HEAD nor shown by git
 * status: ignored files (a crate's own target/ is build output and is skipped)
 * and index-flagged files (assume-unchanged, skip-worktree), whose edits git
 * status does not report.
 */
function unhashedFiles(root, paths) {
  const listed = (args) => run(root, ["ls-files", "-z", ...args, "--", ...paths]).stdout.split("\0").filter(Boolean);
  const ignored = listed(["-o", "-i", "--exclude-standard", "--directory"]).filter((f) => f !== `${ENGINE_TARGET}/`);
  const flagged = listed(["-v"]).filter((l) => /^[a-zS] /.test(l)).map((l) => l.slice(2));
  const refuse = (what, files) => {
    const shown = files.slice(0, 3).join(", ");
    throw new InputError(`${files.length} ${what} under the engine inputs (${shown}${files.length > 3 ? `, and ${files.length - 3} more` : ""}): the engine stamp covers only files in HEAD or shown dirty, so these could change the build unseen. Remove or commit them`);
  };
  if (ignored.length) refuse("ignored file(s)", ignored);
  if (flagged.length) refuse("index-flagged (assume-unchanged or skip-worktree) file(s)", flagged);
}

/**
 * @param {string} root repository (or linked worktree) root
 * @param {string} gussetDir the sibling gusset checkout
 * @returns {{ stamp: { version: 1, trees: Record<string, string>, gusset: string, dirty: boolean }, dirtyFiles: string[] }}
 */
export function engineInputs(root, gussetDir) {
  const paths = enginePaths(root);
  const trees = Object.fromEntries(paths.map((p) => [p, git(root, "rev-parse", `HEAD:${p}`)]));
  for (const line of git(root, "ls-tree", "-r", "HEAD", "--", ...paths).split("\n").filter(Boolean)) {
    // Refused rather than followed: a link inside the inputs (a member directory, a
    // path dependency, Cargo.lock) can lead outside the crate and the repository,
    // and the tree hash covers only its link text.
    if (line.startsWith("160000 ")) throw new InputError(`${line.split("\t")[1]} is a gitlink (submodule) under the engine inputs; its contents are not in the stamp's tree hash`);
    if (line.startsWith("120000 ")) throw new InputError(`${line.split("\t")[1]} is a symlink under the engine inputs; the engine stamp hashes only its link text, not what it points to`);
  }
  unhashedFiles(root, paths);
  const dirtyFiles = [
    ...git(root, "status", "--porcelain", "--untracked-files=all", "--", ...paths).split("\n").filter(Boolean),
    ...git(gussetDir, "status", "--porcelain", "--untracked-files=all").split("\n").filter(Boolean).map((l) => `gusset: ${l}`),
  ];
  return { stamp: { version: 1, trees, gusset: git(gussetDir, "rev-parse", "HEAD"), dirty: dirtyFiles.length > 0 }, dirtyFiles };
}

export const stampFile = (dir) => path.join(dir, STAMP_NAME);

function putStamp(archiveDir, stamp) {
  const tmp = `${stampFile(archiveDir)}.${process.pid}.tmp`;
  writeFileSync(tmp, `${JSON.stringify(stamp, null, 2)}\n`);
  renameSync(tmp, stampFile(archiveDir));
}

/** Compute and write the stamp for the current inputs (no build in between). */
export function writeStamp(root, gussetDir, archiveDir) {
  const { stamp } = engineInputs(root, gussetDir);
  putStamp(archiveDir, stamp);
  return stamp;
}

/**
 * Start a build: delete any previous stamp, then record the inputs. Returns
 * the inputs, or undefined (with the reason) if they cannot be computed.
 *
 * @returns {{ before?: ReturnType<typeof engineInputs>, error?: string }}
 */
export function beginStamp(root, gussetDir, archiveDir) {
  rmSync(stampFile(archiveDir), { force: true });
  try {
    return { before: engineInputs(root, gussetDir) };
  } catch (e) {
    return { error: e.message };
  }
}

/**
 * Finish a build: write the stamp only if the archive exists and the inputs
 * equal the ones `beginStamp` saw. Never throws; otherwise no stamp exists.
 *
 * @returns {{ written: boolean, reason: string }}
 */
export function finishStamp(root, gussetDir, archiveDir, before) {
  rmSync(stampFile(archiveDir), { force: true });
  if (!before) return { written: false, reason: "the inputs could not be recorded before the build" };
  if (!existsSync(path.join(archiveDir, "libgusset.a"))) return { written: false, reason: `no libgusset.a in ${archiveDir}` };
  try {
    const after = engineInputs(root, gussetDir);
    if (JSON.stringify(after.stamp) !== JSON.stringify(before.stamp)) {
      return { written: false, reason: "the engine inputs changed while the build ran" };
    }
    putStamp(archiveDir, after.stamp);
    return { written: true, reason: after.stamp.dirty ? "dirty stamp (never reused by lane-env)" : "ok" };
  } catch (e) {
    return { written: false, reason: e.message };
  }
}

/**
 * Run `build` between begin and finish. A stamp problem is reported through
 * `warn` and never fails the build; it leaves the archive unstamped.
 */
export function stampedBuild(root, gussetDir, archiveDir, build, warn = console.warn) {
  const { before, error } = beginStamp(root, gussetDir, archiveDir);
  build();
  const r = finishStamp(root, gussetDir, archiveDir, before);
  if (!r.written) warn(`engine stamp not written (${error ?? r.reason}); lane-env.mjs will refuse this archive as unstamped`);
  return r;
}

/**
 * Does the archive in `dir` match the inputs? Anything unreadable, missing,
 * dirty or different is a refusal with the reason.
 *
 * @returns {{ ok: boolean, reason: string }}
 */
export function stampVerdict(inputs, dir) {
  const file = stampFile(dir);
  if (!existsSync(file)) return { ok: false, reason: `${dir} has an archive with no stamp (built before stamping): rebuild to stamp it` };
  let have;
  try {
    have = JSON.parse(readFileSync(file, "utf8"));
  } catch (e) {
    return { ok: false, reason: `${file} is unreadable (${e.message}): rebuild to stamp it` };
  }
  if (have.dirty) return { ok: false, reason: `the archive in ${dir} was built from uncommitted sources; a dirty build is never reused` };
  if (inputs.stamp.dirty) return { ok: false, reason: "the lane's engine inputs have uncommitted changes" };
  const want = inputs.stamp;
  const diffs = [
    ...new Set([...Object.keys(want.trees), ...Object.keys(have.trees ?? {})]),
  ].filter((k) => want.trees[k] !== have.trees?.[k]);
  if (have.gusset !== want.gusset) diffs.push("gusset commit");
  if (have.version !== want.version) diffs.push("stamp version");
  if (diffs.length) return { ok: false, reason: `the archive in ${dir} was built from other inputs (${diffs.join(", ")} differ)` };
  return { ok: true, reason: "stamp matches" };
}

const entry = process.argv[1];
if (entry !== undefined && realpathSync(path.resolve(entry)) === realpathSync(fileURLToPath(import.meta.url))) {
  const [cmd, archiveDir, gusset] = process.argv.slice(2);
  if (!["begin", "write"].includes(cmd) || !archiveDir || !gusset) {
    console.error("usage: gusset-engine-stamp.mjs begin <archive-dir> <gusset-dir> | write <archive-dir> <gusset-dir>  (the output of begin on stdin)");
    process.exit(2);
  }
  const root = realpathSync(git(path.dirname(fileURLToPath(import.meta.url)), "rev-parse", "--show-toplevel"));
  if (cmd === "begin") {
    const { before, error } = beginStamp(root, path.resolve(gusset), archiveDir);
    if (!before) {
      console.error(`gusset-engine-stamp: ${error}`);
      process.exit(1);
    }
    console.log(JSON.stringify(before));
  } else {
    const r = finishStamp(root, path.resolve(gusset), archiveDir, JSON.parse(readFileSync(0, "utf8")));
    console.error(`gusset-engine-stamp: ${r.written ? `wrote ${stampFile(archiveDir)} (${r.reason})` : `no stamp written: ${r.reason}`}`);
    process.exit(r.written ? 0 : 3);
  }
}
