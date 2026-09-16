#!/usr/bin/env node
/**
 * Renders the other agent-definition copies from `.claude/agents/<name>.md`.
 *
 * The three definitions used to exist three times, hand-synced. The drift was
 * not theoretical: two of the three still told agents to change files "only via
 * `devcouncil_write_file` / `devcouncil_apply_patch`" and to run tests "via
 * `devcouncil_run_command`", and to fetch critique cards from
 * `devcouncil_live_review` / `devcouncil_live_cards`. This host serves none of
 * those — they belonged to the retired Python host — so an agent following the
 * stale copy spent its first move calling into an error.
 *
 * Source of truth : .claude/agents/<name>.md
 * Generated       : .codex/agents/<name>.toml                     (Codex agent role)
 *                   .devcouncil/claude-plugin/devcouncil/agents/<name>.md  (plugin)
 *
 * Usage:
 *   node scripts/sync-agent-defs.mjs             # write the generated copies
 *   node scripts/sync-agent-defs.mjs --check     # fail on drift, write nothing
 *   node scripts/sync-agent-defs.mjs --list      # show what would be synced
 *
 * Two deliberate choices, both of which a future reader will otherwise try to
 * "fix":
 *
 * 1. The frontmatter parser folds YAML continuation lines rather than rejecting
 *    them. DevCouncil's definitions wrap `tools:` over a dozen lines, and that is
 *    a plain multi-line scalar — valid YAML that folds to one space-joined
 *    string. Refusing it would mean reformatting three correct files to suit the
 *    script.
 *
 * 2. Facts about a *host* live in that host's renderer, not in the shared body.
 *    "There is no `gitpulse` CLI" and "the `devmap` CLI answers the same
 *    questions" are true of the Codex role because it may run without the MCP
 *    tools loaded; they are not role-specific, so they are emitted by the TOML
 *    renderer for all three roles rather than duplicated into each source body.
 *
 * The plugin copy is byte-identical to the source and carries no generated
 * banner, because the whole file becomes the agent's prompt and a banner would
 * be read as an instruction.
 *
 * Not a target: /Users/…/GenoThermalTargeting/.claude/agents/. It is a different
 * repository, and its implementer carries a local "consult the advisor tool"
 * step. It consumes the plugin copy and keeps that one delta by hand.
 */

import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
export const repoRoot = path.resolve(__dirname, "..");

export const CLAUDE_AGENTS_DIR = path.join(".claude", "agents");
export const CODEX_AGENTS_DIR = path.join(".codex", "agents");
export const PLUGIN_AGENTS_DIR = path.join(
  ".devcouncil",
  "claude-plugin",
  "devcouncil",
  "agents",
);

export const BANNER = "# GENERATED FILE — DO NOT EDIT.";

/**
 * Host facts the Codex role needs and the Claude role does not. Emitted by the
 * TOML renderer so the shared body stays host-neutral.
 */
export const CODEX_HOST_NOTES = `## On this host

A Codex agent role has no \`tools\` allowlist field — only name, description and
developer_instructions — so nothing mechanically restricts which tools you reach
for. The \`devcouncil_*\` names above are the whole restriction; honour them.

Where the instructions name \`Edit\`, \`Write\`, \`Read\` or \`Bash\`, those are the
Claude host's tools. Use this host's equivalent editing and shell tools. The
point stands either way: the writes are yours to make, because the tools that
once made them through DevCouncil are retired and nothing gates a write now.

When the MCP tools are not loaded, the \`devmap\` CLI answers the same questions
(\`devmap search\`, \`devmap explore <name>\`, \`devmap impact\`, \`devmap affected
<target>\`). GitPulse Insights is served over MCP only — there is no \`gitpulse\`
CLI, so when it is unavailable, report the situation as unestablished rather than
guessing at it.`;

/** Thrown for every rejected input, so callers can fail closed on one type. */
export class AgentDefError extends Error {}

/**
 * Splits `---\n...\n---\n` frontmatter from the markdown body.
 *
 * Deliberately strict: a definition we cannot parse is an error, never a
 * silently-empty agent. Continuation lines — lines that open with whitespace —
 * fold into the preceding key, which is what YAML does with a plain multi-line
 * scalar and what these files rely on.
 */
export function parseAgentMarkdown(source, label = "<source>") {
  if (!source.startsWith("---\n") && !source.startsWith("---\r\n")) {
    throw new AgentDefError(`${label}: must begin with '---' YAML frontmatter`);
  }
  const normalized = source.replace(/\r\n/g, "\n");
  const closing = normalized.indexOf("\n---\n", 3);
  if (closing === -1) {
    throw new AgentDefError(`${label}: frontmatter is never closed by a '---' line`);
  }

  const frontmatter = normalized.slice(4, closing + 1);
  const body = normalized.slice(closing + 5).trim();
  if (body === "") {
    throw new AgentDefError(`${label}: body is empty; an agent with no instructions is a bug`);
  }

  const fields = new Map();
  let current = null;
  for (const rawLine of frontmatter.split("\n")) {
    if (rawLine.trim() === "" || rawLine.trimStart().startsWith("#")) continue;
    if (/^\s/.test(rawLine)) {
      if (current === null) {
        throw new AgentDefError(`${label}: frontmatter opens with a continuation line: ${rawLine}`);
      }
      fields.set(current, `${fields.get(current)} ${rawLine.trim()}`.trim());
      continue;
    }
    const separator = rawLine.indexOf(":");
    if (separator === -1) {
      throw new AgentDefError(`${label}: frontmatter line is not 'key: value': ${rawLine}`);
    }
    const key = rawLine.slice(0, separator).trim();
    if (fields.has(key)) {
      throw new AgentDefError(`${label}: duplicate frontmatter key '${key}'`);
    }
    fields.set(key, rawLine.slice(separator + 1).trim());
    current = key;
  }

  const name = fields.get("name") ?? "";
  const description = fields.get("description") ?? "";
  if (!name) throw new AgentDefError(`${label}: frontmatter must define a non-empty 'name'`);
  if (!/^[a-z0-9][a-z0-9-]*$/.test(name)) {
    throw new AgentDefError(`${label}: name '${name}' must be lowercase kebab-case`);
  }
  if (!description) {
    throw new AgentDefError(`${label}: frontmatter must define a non-empty 'description'`);
  }

  const tools = (fields.get("tools") ?? "")
    .split(",")
    .map((t) => t.trim())
    .filter(Boolean);

  return { name, description, tools, body, fields };
}

/** Escapes a value for a TOML basic string on one line. */
export function tomlBasic(value) {
  return `"${value.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
}

/**
 * Escapes a value for a TOML multi-line basic string.
 *
 * Only the sequences that would end the string early are escaped — a run of
 * three or more quotes, and a trailing quote — so the rendered instructions stay
 * readable instead of turning every inner quotation into `\"`.
 */
export function tomlMultiline(value) {
  let escaped = value.replace(/\\/g, "\\\\");
  // TOML permits one or two consecutive quotes inside `"""`; a third would close
  // the string. Escaping every third quote of a run leaves runs of 1 and 2
  // untouched and makes any longer run safe, whatever its length.
  escaped = escaped.replace(/"+/g, (run) =>
    run
      .split("")
      .map((quote, index) => ((index + 1) % 3 === 0 ? '\\"' : quote))
      .join(""),
  );
  // A value ending in a quote would abut the delimiter and make four.
  if (escaped.endsWith('"')) escaped = `${escaped.slice(0, -1)}\\"`;
  return `"""\n${escaped}"""`;
}

/** Renders the Codex `.toml` role for one parsed definition. */
export function renderCodexToml(def, sourceRelative) {
  const instructions = `${def.body}\n\n${CODEX_HOST_NOTES}`;
  return [
    `${BANNER} Source: ${sourceRelative}`,
    "# Regenerate with: npm run agents:sync",
    `name = ${tomlBasic(def.name)}`,
    `description = ${tomlBasic(def.description)}`,
    // tomlMultiline opens with `"""\n`, and TOML trims that first newline, so the
    // instructions must not add one of their own or the body starts blank.
    `developer_instructions = ${tomlMultiline(instructions)}`,
    "",
  ].join("\n");
}

/** The plugin copy is the source, byte for byte. */
export function renderPluginMarkdown(source) {
  return source;
}

/** Definition basenames present in the source directory, sorted. */
export function agentNames(directory) {
  if (!existsSync(directory)) return [];
  return readdirSync(directory)
    .filter((f) => f.endsWith(".md"))
    .map((f) => f.slice(0, -3))
    .sort();
}

/**
 * Computes every generated artifact without touching the filesystem, so
 * `--check` and a real write can never disagree about what should be there.
 */
export function plan(root = repoRoot) {
  const sourceDir = path.join(root, CLAUDE_AGENTS_DIR);
  const names = agentNames(sourceDir);
  if (names.length === 0) {
    throw new AgentDefError(
      `no agent definitions under ${CLAUDE_AGENTS_DIR}; refusing to treat an absent source as "nothing to do"`,
    );
  }
  const outputs = [];
  for (const name of names) {
    const sourceRelative = path.join(CLAUDE_AGENTS_DIR, `${name}.md`);
    const source = readFileSync(path.join(root, sourceRelative), "utf8");
    const def = parseAgentMarkdown(source, sourceRelative);
    if (def.name !== name) {
      throw new AgentDefError(`${sourceRelative}: frontmatter name '${def.name}' != filename`);
    }
    outputs.push({
      relative: path.join(CODEX_AGENTS_DIR, `${name}.toml`),
      contents: renderCodexToml(def, sourceRelative),
    });
    outputs.push({
      relative: path.join(PLUGIN_AGENTS_DIR, `${name}.md`),
      contents: renderPluginMarkdown(source),
    });
  }
  return { names, outputs };
}

/**
 * Compares the plan against what is on disk.
 *
 * A missing target is drift, not a skip: the point of `--check` is to fail when
 * a copy does not match its source, and "the file is not there" is the most
 * complete way for it not to match. A checker that cannot run must never report
 * the same result as one that ran and passed.
 */
export function drift(root = repoRoot) {
  const { outputs } = plan(root);
  const changed = [];
  for (const output of outputs) {
    const absolute = path.join(root, output.relative);
    const actual = existsSync(absolute) ? readFileSync(absolute, "utf8") : null;
    if (actual !== output.contents) {
      changed.push({ relative: output.relative, missing: actual === null });
    }
  }
  return changed;
}

export function write(root = repoRoot) {
  const { outputs } = plan(root);
  const written = [];
  for (const output of outputs) {
    const absolute = path.join(root, output.relative);
    mkdirSync(path.dirname(absolute), { recursive: true });
    const actual = existsSync(absolute) ? readFileSync(absolute, "utf8") : null;
    if (actual === output.contents) continue;
    writeFileSync(absolute, output.contents);
    written.push(output.relative);
  }
  return written;
}

function main(argv) {
  const root = process.env.DEVCOUNCIL_AGENT_SYNC_ROOT ?? repoRoot;
  try {
    if (argv.includes("--list")) {
      const { names, outputs } = plan(root);
      console.log(`source: ${CLAUDE_AGENTS_DIR} (${names.length})`);
      for (const output of outputs) console.log(`  -> ${output.relative}`);
      return 0;
    }
    if (argv.includes("--check")) {
      const changed = drift(root);
      if (changed.length === 0) {
        console.log("agent definitions are in sync");
        return 0;
      }
      for (const { relative, missing } of changed) {
        console.error(`${missing ? "missing" : "stale"}: ${relative}`);
      }
      console.error("\nRun `npm run agents:sync` to regenerate.");
      return 1;
    }
    const written = write(root);
    if (written.length === 0) console.log("agent definitions already in sync");
    for (const relative of written) console.log(`wrote ${relative}`);
    return 0;
  } catch (error) {
    if (error instanceof AgentDefError) {
      console.error(`agent definition error: ${error.message}`);
      return 2;
    }
    throw error;
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  process.exit(main(process.argv.slice(2)));
}
