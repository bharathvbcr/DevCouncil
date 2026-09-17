#!/usr/bin/env node
/**
 * Asks every configured MCP host what it serves, and checks `MCP_TOOL_PREFIXES`
 * in `sync-agent-defs.mjs` still covers it.
 *
 * Why this is separate from the sync check: an agent's `tools:` entry is a
 * string, and a string no server serves is not an error any host reports — the
 * agent is simply never handed that tool, and the only symptom is an agent that
 * looks like it is ignoring the instruction telling it to use it. Measured
 * 2026-09-17, all ten grants in this repo and its neighbours named
 * `mcp__plugin_devmap_devmap__*` alone, which is absent from any session where
 * Claude Code resolves the user-scope `devmap` server instead. Those agents had
 * zero DevMap tools and nothing said so.
 *
 * `MCP_TOOL_PREFIXES` is a declaration so `agents:check` can run on a clean
 * clone with no host config and no servers installed. A declaration that nothing
 * confronts with reality is the thing that rots, so this script is the
 * confrontation: it fails when a host serves a family under a prefix the table
 * does not list.
 *
 *   node scripts/mcp-served-tools.mjs            # verify the table, exit 1 on a gap
 *   node scripts/mcp-served-tools.mjs --list     # print every served tool id
 *   node scripts/mcp-served-tools.mjs --json     # machine-readable
 *
 * Exit codes are deliberately three-way, because "I could not ask" and "I asked
 * and it was fine" must not look the same:
 *   0  every live prefix is declared
 *   1  a live prefix is missing from the table
 *   3  nothing could be probed, so nothing was verified
 */

import { existsSync, readFileSync } from "node:fs";
import { spawn } from "node:child_process";
import os from "node:os";
import path from "node:path";
import { MCP_TOOL_PREFIXES, repoRoot } from "./sync-agent-defs.mjs";

const HOME = os.homedir();
const CLAUDE_JSON = path.join(HOME, ".claude.json");
const INSTALLED_PLUGINS = path.join(HOME, ".claude", "plugins", "installed_plugins.json");
const PROTOCOL_VERSION = "2025-06-18";

/** Per-server budget. A hung server must not hang the check. */
const HANDSHAKE_TIMEOUT_MS = 20_000;

function readJson(file) {
  if (!existsSync(file)) return null;
  try {
    return JSON.parse(readFileSync(file, "utf8"));
  } catch {
    return null;
  }
}

/**
 * initialize + tools/list over stdio.
 *
 * Resolves to `{ tools }` or `{ error }` — never rejects, because one
 * unreachable server (an HTTP transport, a plugin whose runtime is missing) must
 * not decide the outcome for the rest.
 */
function handshake(spec) {
  return new Promise((resolve) => {
    if (spec.type && spec.type !== "stdio") {
      resolve({ error: `transport ${spec.type} is not stdio` });
      return;
    }
    if (!spec.command) {
      resolve({ error: "no command" });
      return;
    }
    let child;
    try {
      child = spawn(spec.command, spec.args ?? [], {
        stdio: ["pipe", "pipe", "pipe"],
        env: { ...process.env, ...(spec.env ?? {}), NO_COLOR: "1" },
      });
    } catch (error) {
      resolve({ error: `spawn failed: ${error.message}` });
      return;
    }

    let stdout = "";
    let settled = false;
    const finish = (value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      child.kill("SIGKILL");
      resolve(value);
    };
    const timer = setTimeout(
      () => finish({ error: `timed out after ${HANDSHAKE_TIMEOUT_MS}ms` }),
      HANDSHAKE_TIMEOUT_MS,
    );

    child.on("error", (error) => finish({ error: error.message }));
    child.stderr.on("data", () => {});
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
      for (const line of stdout.split("\n")) {
        if (!line.startsWith("{")) continue;
        let message;
        try {
          message = JSON.parse(line);
        } catch {
          continue; // a partial line; the next chunk completes it
        }
        const tools = message?.result?.tools;
        if (Array.isArray(tools)) {
          finish({ tools: tools.map((t) => t?.name).filter(Boolean).sort() });
          return;
        }
      }
    });
    child.on("close", () => finish({ error: "closed before answering tools/list" }));

    for (const request of [
      {
        jsonrpc: "2.0",
        id: 1,
        method: "initialize",
        params: {
          protocolVersion: PROTOCOL_VERSION,
          capabilities: {},
          clientInfo: { name: "devcouncil-mcp-served-tools", version: "0" },
        },
      },
      { jsonrpc: "2.0", method: "notifications/initialized", params: {} },
      { jsonrpc: "2.0", id: 2, method: "tools/list", params: {} },
    ]) {
      child.stdin.write(`${JSON.stringify(request)}\n`);
    }
  });
}

/** Every (prefix, spec) pair a host would expose, across all registration scopes. */
export function registrations() {
  const found = [];
  const seen = new Set();
  const add = (prefix, spec, origin) => {
    const key = `${prefix}::${spec.command} ${(spec.args ?? []).join(" ")}`;
    if (seen.has(key)) return;
    seen.add(key);
    found.push({ prefix, spec, origin });
  };

  const userScope = readJson(CLAUDE_JSON)?.mcpServers ?? {};
  for (const [server, spec] of Object.entries(userScope)) {
    add(`mcp__${server}__`, spec, "user scope (~/.claude.json)");
  }

  const plugins = readJson(INSTALLED_PLUGINS)?.plugins ?? {};
  for (const [key, entries] of Object.entries(plugins)) {
    const plugin = key.split("@", 1)[0];
    for (const entry of entries ?? []) {
      const servers = readJson(path.join(entry.installPath ?? "", ".mcp.json"))?.mcpServers ?? {};
      for (const [server, spec] of Object.entries(servers)) {
        add(`mcp__plugin_${plugin}_${server}__`, spec, `plugin ${key}`);
      }
    }
  }

  // This repository's own project scope, which uses the bare prefix.
  const project = readJson(path.join(repoRoot, ".mcp.json"))?.mcpServers ?? {};
  for (const [server, spec] of Object.entries(project)) {
    add(`mcp__${server}__`, spec, "project .mcp.json");
  }

  return found;
}

function familyOfTool(tool, declared = MCP_TOOL_PREFIXES) {
  const family = tool.split("_", 1)[0];
  return Object.hasOwn(declared, family) ? family : null;
}

/**
 * Which families a set of served tool IDs proves are reachable, per prefix.
 *
 * Pure, and takes `declared` rather than closing over the module constant, so
 * the comparison below can be tested against a table that is deliberately
 * wrong. A gap detector that is only ever run against a correct table has never
 * been shown to detect anything.
 */
export function observedPrefixes(servedIds, declared = MCP_TOOL_PREFIXES) {
  const live = new Map();
  for (const id of servedIds) {
    const cut = id.lastIndexOf("__");
    if (!id.startsWith("mcp__") || cut <= 4) continue;
    const prefix = id.slice(0, cut + 2);
    const family = familyOfTool(id.slice(cut + 2), declared);
    if (family === null) continue;
    if (!live.has(family)) live.set(family, new Set());
    live.get(family).add(prefix);
  }
  return live;
}

/**
 * Prefixes a host serves that the table does not declare.
 *
 * The reverse direction is returned separately and is never a failure: a
 * declared prefix whose server is not installed *here* is inert inside a grant,
 * and dropping it would break the machine that does have it.
 */
export function comparePrefixes(live, declared = MCP_TOOL_PREFIXES) {
  const gaps = [];
  for (const [family, prefixes] of live) {
    for (const prefix of prefixes) {
      if (!declared[family]?.includes(prefix)) gaps.push({ family, prefix });
    }
  }
  const declaredButAbsent = [];
  for (const [family, prefixes] of Object.entries(declared)) {
    const observed = live.get(family) ?? new Set();
    for (const prefix of prefixes) {
      if (!observed.has(prefix)) declaredButAbsent.push({ family, prefix });
    }
  }
  return { gaps, declaredButAbsent };
}

async function main(argv) {
  const wantJson = argv.includes("--json");
  const wantList = argv.includes("--list");

  const scopes = registrations();
  const probed = [];
  const unreachable = [];
  for (const registration of scopes) {
    const result = await handshake(registration.spec);
    if (result.error) {
      unreachable.push({ ...registration, error: result.error });
    } else {
      probed.push({ ...registration, tools: result.tools });
    }
  }

  const ids = probed.flatMap(({ prefix, tools }) => tools.map((tool) => prefix + tool));
  const live = observedPrefixes(ids);
  const { gaps, declaredButAbsent } = comparePrefixes(live);

  const verified = probed.length > 0;
  const status = !verified ? "not-verified" : gaps.length > 0 ? "gap" : "ok";

  if (wantJson) {
    console.log(
      JSON.stringify(
        {
          status,
          declared: MCP_TOOL_PREFIXES,
          live: Object.fromEntries([...live].map(([f, p]) => [f, [...p].sort()])),
          gaps,
          declared_but_absent_here: declaredButAbsent,
          unreachable: unreachable.map(({ prefix, origin, error }) => ({ prefix, origin, error })),
          served_tool_ids: wantList ? ids.sort() : ids.length,
        },
        null,
        2,
      ),
    );
  } else {
    for (const { prefix, origin, tools } of probed) {
      console.log(`${prefix.padEnd(34)} ${String(tools.length).padStart(3)} tools  <- ${origin}`);
      if (wantList) for (const tool of tools) console.log(`    ${prefix}${tool}`);
    }
    for (const { prefix, origin, error } of unreachable) {
      console.log(`${prefix.padEnd(34)}  unreachable  <- ${origin}: ${error}`);
    }
    console.log("");
    for (const [family, prefixes] of [...live].sort()) {
      console.log(`  ${family.padEnd(11)} served under ${[...prefixes].sort().join(", ")}`);
    }
    for (const { family, prefix } of declaredButAbsent) {
      console.log(`  note: ${family} declares ${prefix}, not installed here (inert, not a gap)`);
    }
  }

  if (!verified) {
    console.error(
      "\nnothing could be probed: no MCP server answered, so MCP_TOOL_PREFIXES was NOT verified.\n" +
        "This is not a pass. Run where the hosts are configured, or fix the unreachable servers above.",
    );
    return 3;
  }
  if (gaps.length > 0) {
    console.error("\nMCP_TOOL_PREFIXES is missing a prefix a host actually serves:");
    for (const { family, prefix } of gaps) {
      console.error(`  ${family}: add "${prefix}" to MCP_TOOL_PREFIXES.${family}`);
    }
    console.error(
      "\nUntil it is added, every agent grant for that family is silently empty in any\n" +
        "session where that prefix is the live one. Add it, then run `npm run agents:sync`.",
    );
    return 1;
  }
  console.log("\nMCP_TOOL_PREFIXES covers every prefix these hosts serve");
  return 0;
}

if (process.argv[1] && path.resolve(process.argv[1]) === path.resolve(new URL(import.meta.url).pathname)) {
  main(process.argv.slice(2)).then((code) => process.exit(code));
}
