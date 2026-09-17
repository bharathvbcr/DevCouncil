import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { describe, it } from "node:test";
import {
  AgentDefError,
  BANNER,
  CLAUDE_AGENTS_DIR,
  CODEX_AGENTS_DIR,
  CODEX_HOST_NOTES,
  MCP_TOOL_PREFIXES,
  PLUGIN_AGENTS_DIR,
  drift,
  expandTools,
  parseAgentMarkdown,
  pinnedGrants,
  plan,
  renderCodexToml,
  rewriteToolsBlock,
  splitToolId,
  tomlMultiline,
  write,
} from "./sync-agent-defs.mjs";
import { comparePrefixes, observedPrefixes } from "./mcp-served-tools.mjs";

/** A definition shaped like the real ones: wrapped `description` and `tools`. */
const WRAPPED = `---
name: devcouncil-reviewer
description: Reviews the working-tree diff against DevCouncil policy and the code
  graph. Use for a structural, policy-aware code review before merge.
tools: Read, Grep, Glob, Bash, mcp__devcouncil__devcouncil_get_diff,
  mcp__devcouncil__devcouncil_policy_check_write,
  mcp__plugin_devmap_devmap__devmap_impact
---

You are the DevCouncil reviewer subagent.

Use \`devcouncil_get_diff\` for the change set.
`;

function scratchRepo(files) {
  const root = mkdtempSync(path.join(tmpdir(), "agent-defs-"));
  for (const [relative, contents] of Object.entries(files)) {
    const absolute = path.join(root, relative);
    mkdirSync(path.dirname(absolute), { recursive: true });
    writeFileSync(absolute, contents);
  }
  return root;
}

describe("parseAgentMarkdown", () => {
  it("folds YAML continuation lines instead of rejecting them", () => {
    const def = parseAgentMarkdown(WRAPPED, "reviewer.md");
    assert.equal(def.name, "devcouncil-reviewer");
    assert.equal(
      def.description,
      "Reviews the working-tree diff against DevCouncil policy and the code graph. " +
        "Use for a structural, policy-aware code review before merge.",
    );
    assert.deepEqual(def.tools, [
      "Read",
      "Grep",
      "Glob",
      "Bash",
      "mcp__devcouncil__devcouncil_get_diff",
      "mcp__devcouncil__devcouncil_policy_check_write",
      "mcp__plugin_devmap_devmap__devmap_impact",
    ]);
    assert.match(def.body, /^You are the DevCouncil reviewer subagent\./);
  });

  it("rejects a definition it cannot parse rather than emitting an empty agent", () => {
    assert.throws(() => parseAgentMarkdown("no frontmatter", "x.md"), AgentDefError);
    assert.throws(() => parseAgentMarkdown("---\nname: x\n", "x.md"), AgentDefError);
    assert.throws(() => parseAgentMarkdown("---\nname: x\n---\n\n", "x.md"), AgentDefError);
    assert.throws(() => parseAgentMarkdown("---\nname: X_Y\ndescription: d\n---\n\nb\n", "x.md"), AgentDefError);
    assert.throws(() => parseAgentMarkdown("---\nname: x\n---\n\nb\n", "x.md"), AgentDefError);
    assert.throws(
      () => parseAgentMarkdown("---\nname: x\nname: y\ndescription: d\n---\n\nb\n", "x.md"),
      AgentDefError,
    );
    assert.throws(() => parseAgentMarkdown("---\n  orphan: 1\n---\n\nb\n", "x.md"), AgentDefError);
  });
});

describe("tomlMultiline", () => {
  it("keeps one and two consecutive quotes as they are", () => {
    assert.ok(tomlMultiline('say "nothing exists" here').includes('say "nothing exists" here'));
    assert.ok(tomlMultiline('a ""b').includes('a ""b'));
  });

  it("breaks up any run that would close the string early", () => {
    for (const run of ['"""', '""""', '"""""', '""""""']) {
      const rendered = tomlMultiline(`x${run}y`);
      const inner = rendered.slice(4, -3);
      assert.ok(!/(?<!\\)"{3}/.test(inner), `run of ${run.length} left a live delimiter: ${inner}`);
    }
  });

  it("escapes a trailing quote so it cannot abut the delimiter", () => {
    assert.ok(tomlMultiline('ends with "').endsWith('\\""""'));
  });

  it("escapes backslashes", () => {
    assert.ok(tomlMultiline("a\\b").includes("a\\\\b"));
  });
});

describe("renderCodexToml", () => {
  const def = parseAgentMarkdown(WRAPPED, "reviewer.md");
  const rendered = renderCodexToml(def, path.join(CLAUDE_AGENTS_DIR, "devcouncil-reviewer.md"));

  it("marks the file generated and names its source", () => {
    assert.ok(rendered.startsWith(BANNER));
    assert.ok(rendered.includes(path.join(CLAUDE_AGENTS_DIR, "devcouncil-reviewer.md")));
    assert.ok(rendered.includes("npm run agents:sync"));
  });

  it("carries the body through unchanged", () => {
    assert.ok(rendered.includes("You are the DevCouncil reviewer subagent."));
    assert.ok(rendered.includes("Use `devcouncil_get_diff` for the change set."));
  });

  it("adds the host facts the shared body deliberately omits", () => {
    assert.ok(rendered.includes("there is no `gitpulse`"));
    assert.ok(rendered.includes("no `tools` allowlist field"));
    assert.ok(rendered.includes(CODEX_HOST_NOTES));
  });

  it("emits no `tools` key, because a Codex role has no such field", () => {
    assert.ok(!/^tools\s*=/m.test(rendered));
  });
});

describe("plan / write / drift", () => {
  it("writes both targets and then reports no drift", () => {
    const root = scratchRepo({ [path.join(CLAUDE_AGENTS_DIR, "devcouncil-reviewer.md")]: WRAPPED });
    try {
      const { names, outputs } = plan(root);
      assert.deepEqual(names, ["devcouncil-reviewer"]);
      // Three, not two: WRAPPED pins `devmap_impact` to one of its server's
      // three prefixes, so the source is a target as well. Claude Code reads
      // the source directly, so it is the copy that most needs widening.
      assert.deepEqual(
        outputs.map((o) => o.relative).sort(),
        [
          path.join(CLAUDE_AGENTS_DIR, "devcouncil-reviewer.md"),
          path.join(CODEX_AGENTS_DIR, "devcouncil-reviewer.toml"),
          path.join(PLUGIN_AGENTS_DIR, "devcouncil-reviewer.md"),
        ].sort(),
      );

      assert.equal(drift(root).length, 3, "nothing is generated yet, so every target is drift");
      assert.equal(write(root).length, 3);
      assert.deepEqual(drift(root), []);
      assert.equal(write(root).length, 0, "a second run rewrites nothing");

      const plugin = readFileSync(
        path.join(root, PLUGIN_AGENTS_DIR, "devcouncil-reviewer.md"),
        "utf8",
      );
      const source = readFileSync(
        path.join(root, CLAUDE_AGENTS_DIR, "devcouncil-reviewer.md"),
        "utf8",
      );
      assert.equal(plugin, source, "the plugin copy is the source, byte for byte");
      assert.notEqual(source, WRAPPED, "the pinned grant in the source was widened in place");
      assert.ok(
        plugin.includes("mcp__devmap__devmap_impact"),
        "the copy must carry the widened grant, not the pinned one it was written from",
      );
      assert.ok(!plugin.includes(BANNER), "a banner would be read as part of the agent's prompt");
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("reports a hand-edited copy as stale", () => {
    const root = scratchRepo({ [path.join(CLAUDE_AGENTS_DIR, "devcouncil-reviewer.md")]: WRAPPED });
    try {
      write(root);
      const target = path.join(root, CODEX_AGENTS_DIR, "devcouncil-reviewer.toml");
      writeFileSync(target, `${readFileSync(target, "utf8")}\n# hand edit\n`);
      const changed = drift(root);
      assert.equal(changed.length, 1);
      assert.equal(changed[0].relative, path.join(CODEX_AGENTS_DIR, "devcouncil-reviewer.toml"));
      assert.equal(changed[0].missing, false);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("reports a deleted copy as missing rather than skipping it", () => {
    const root = scratchRepo({ [path.join(CLAUDE_AGENTS_DIR, "devcouncil-reviewer.md")]: WRAPPED });
    try {
      write(root);
      rmSync(path.join(root, PLUGIN_AGENTS_DIR, "devcouncil-reviewer.md"));
      const changed = drift(root);
      assert.equal(changed.length, 1);
      assert.equal(changed[0].missing, true);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("refuses an absent source instead of calling it 'nothing to do'", () => {
    const root = scratchRepo({ "README.md": "no agents here\n" });
    try {
      assert.throws(() => plan(root), AgentDefError);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("refuses a definition whose frontmatter name disagrees with its filename", () => {
    const root = scratchRepo({
      [path.join(CLAUDE_AGENTS_DIR, "devcouncil-verifier.md")]: WRAPPED,
    });
    try {
      assert.throws(() => plan(root), AgentDefError);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});

describe("MCP grant prefix expansion", () => {
  it("splits a plugin tool id on its LAST separator, not its first", () => {
    // `mcp__plugin_gitpulse_gitpulse__` contains a `__` of its own. Splitting on
    // the first would call the server "plugin" and the tool
    // "gitpulse_gitpulse__devmap_impact", and every downstream family lookup
    // would miss.
    assert.deepEqual(splitToolId("mcp__plugin_gitpulse_gitpulse__devmap_impact"), {
      prefix: "mcp__plugin_gitpulse_gitpulse__",
      tool: "devmap_impact",
    });
    assert.deepEqual(splitToolId("mcp__devmap__devmap_impact"), {
      prefix: "mcp__devmap__",
      tool: "devmap_impact",
    });
    assert.equal(splitToolId("Read"), null, "a built-in carries no prefix");
    assert.equal(splitToolId("mcp__"), null, "a truncated id is not a split");
    assert.equal(splitToolId("mcp__x"), null, "an id with no closing separator is not a split");
  });

  it("widens one prefix to every prefix its server is served under", () => {
    const expanded = expandTools(["Read", "mcp__plugin_devmap_devmap__devmap_impact"]);
    assert.deepEqual(expanded, [
      "Read",
      "mcp__devmap__devmap_impact",
      "mcp__plugin_devmap_devmap__devmap_impact",
      "mcp__plugin_gitpulse_gitpulse__devmap_impact",
    ]);
    assert.deepEqual(pinnedGrants(expanded), [], "the widened grant is not pinned");
  });

  it("reports which prefix a pinned grant is missing", () => {
    const pinned = pinnedGrants(["mcp__devmap__devmap_search"]);
    assert.equal(pinned.length, 1);
    assert.equal(pinned[0].tool, "devmap_search");
    assert.deepEqual(pinned[0].missing, [
      "mcp__plugin_devmap_devmap__",
      "mcp__plugin_gitpulse_gitpulse__",
    ]);
  });

  it("is idempotent, so a synced grant never reports drift again", () => {
    const once = expandTools(["Bash", "mcp__devmap__devmap_search"]);
    assert.deepEqual(expandTools(once), once);
    assert.deepEqual(expandTools([...once].reverse()), once, "order of input does not matter");
  });

  it("does not drop an MCP tool from an unknown family", () => {
    // Widening must never narrow. A server this script has no prefix table for
    // still has to reach the agent it was granted to.
    const expanded = expandTools(["mcp__somethingelse__frobnicate", "mcp__devmap__devmap_search"]);
    assert.ok(expanded.includes("mcp__somethingelse__frobnicate"));
    assert.ok(expanded.includes("mcp__plugin_gitpulse_gitpulse__devmap_search"));
  });

  it("de-duplicates rather than emitting a tool twice", () => {
    const expanded = expandTools([
      "Read",
      "Read",
      "mcp__devmap__devmap_search",
      "mcp__plugin_devmap_devmap__devmap_search",
    ]);
    assert.equal(new Set(expanded).size, expanded.length, `duplicates in ${expanded}`);
  });

  it("rewrites only the tools block, leaving a folded description untouched", () => {
    const rewritten = rewriteToolsBlock(WRAPPED, expandTools(["Read", "mcp__devmap__devmap_impact"]));
    assert.ok(
      rewritten.includes(
        "description: Reviews the working-tree diff against DevCouncil policy and the code\n  graph.",
      ),
      "the folded description must survive byte for byte",
    );
    assert.ok(rewritten.endsWith("Use `devcouncil_get_diff` for the change set.\n"));
  });

  it("round-trips: what it writes parses back to exactly what it was given", () => {
    const expanded = expandTools([
      "Read",
      "Grep",
      "Glob",
      "Bash",
      "mcp__devcouncil__devcouncil_get_diff",
      "mcp__plugin_devmap_devmap__devmap_impact",
      "mcp__plugin_gitpulse_gitpulse__gitpulse_insights",
    ]);
    const rewritten = rewriteToolsBlock(WRAPPED, expanded);
    const reparsed = parseAgentMarkdown(rewritten, "<round-trip>");
    assert.deepEqual(reparsed.tools, expanded);
    assert.ok(
      !reparsed.tools.some((t) => t === ""),
      "a dangling comma would parse back as an empty tool name",
    );
  });

  it("wraps long grants onto continuation lines the parser folds", () => {
    const many = expandTools([
      "mcp__devmap__devmap_search",
      "mcp__devmap__devmap_explore",
      "mcp__devmap__devmap_impact",
      "mcp__devmap__devmap_trace",
      "mcp__devmap__devmap_affected_tests",
    ]);
    const rewritten = rewriteToolsBlock(WRAPPED, many);
    const toolLines = rewritten
      .split("\n---\n")[0]
      .split("\n")
      .filter((l) => /^tools:/.test(l) || /^\s+mcp__/.test(l));
    assert.ok(toolLines.length > 1, "15 ids cannot fit on one line");
    for (const line of toolLines.slice(1)) {
      assert.ok(/^\s/.test(line), `continuation line must open with whitespace: ${line}`);
    }
    assert.deepEqual(parseAgentMarkdown(rewritten, "<wrap>").tools, many);
  });

  it("returns the source unchanged when the grant is already widened", () => {
    const widened = rewriteToolsBlock(WRAPPED, expandTools(["Read", "mcp__devmap__devmap_impact"]));
    const again = rewriteToolsBlock(widened, expandTools(["Read", "mcp__devmap__devmap_impact"]));
    assert.equal(again, widened, "a no-op rewrite must not churn the file");
  });

  it("refuses a definition with no tools block to rewrite", () => {
    const noTools = WRAPPED.replace(
      /tools:.*?\n(?=---)/s,
      "",
    );
    assert.throws(() => rewriteToolsBlock(noTools, ["Read"]), AgentDefError);
  });

  it("detects a prefix a host serves that the table does not declare", () => {
    // The whole point of the live probe. Run only against the real table, this
    // comparison has never been shown to detect anything — so give it a table
    // that is deliberately missing the prefix this session actually exposes.
    const incomplete = { devmap: ["mcp__plugin_devmap_devmap__"] };
    const served = [
      "mcp__devmap__devmap_search",
      "mcp__plugin_devmap_devmap__devmap_search",
      "mcp__plugin_gitpulse_gitpulse__devmap_search",
    ];
    const { gaps } = comparePrefixes(observedPrefixes(served, incomplete), incomplete);
    assert.deepEqual(
      gaps.map((g) => g.prefix).sort(),
      ["mcp__devmap__", "mcp__plugin_gitpulse_gitpulse__"],
    );
  });

  it("does not call a declared-but-uninstalled prefix a gap", () => {
    // A grant naming a prefix this machine has no server for is inert. Failing
    // on it would make the check refuse every machine but the fullest one.
    const served = ["mcp__devmap__devmap_search"];
    const { gaps, declaredButAbsent } = comparePrefixes(observedPrefixes(served));
    assert.deepEqual(gaps, [], "a served prefix that IS declared is not a gap");
    assert.ok(
      declaredButAbsent.some((d) => d.prefix === "mcp__plugin_gitpulse_gitpulse__"),
      "the uninstalled prefix is reported separately, not as a failure",
    );
  });

  it("ignores a served family the table does not track at all", () => {
    const served = ["mcp__scholarlm-local__scholarlmGetAppDocs", "mcp__brightdata__scrape"];
    const live = observedPrefixes(served);
    assert.equal(live.size, 0, "an untracked family is not a devmap/gitpulse grant");
    assert.deepEqual(comparePrefixes(live).gaps, []);
  });

  it("every declared family lists at least one prefix", () => {
    for (const [family, prefixes] of Object.entries(MCP_TOOL_PREFIXES)) {
      assert.ok(prefixes.length > 0, `${family} declares no prefix`);
      for (const prefix of prefixes) {
        assert.match(prefix, /^mcp__[a-z0-9_-]+__$/, `${prefix} is not a host tool prefix`);
      }
      assert.equal(new Set(prefixes).size, prefixes.length, `${family} repeats a prefix`);
    }
  });
});
