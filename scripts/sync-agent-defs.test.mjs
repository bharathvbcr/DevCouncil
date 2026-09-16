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
  PLUGIN_AGENTS_DIR,
  drift,
  parseAgentMarkdown,
  plan,
  renderCodexToml,
  tomlMultiline,
  write,
} from "./sync-agent-defs.mjs";

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
      assert.deepEqual(
        outputs.map((o) => o.relative).sort(),
        [
          path.join(CODEX_AGENTS_DIR, "devcouncil-reviewer.toml"),
          path.join(PLUGIN_AGENTS_DIR, "devcouncil-reviewer.md"),
        ].sort(),
      );

      assert.equal(drift(root).length, 2, "nothing is generated yet, so both targets are drift");
      assert.equal(write(root).length, 2);
      assert.deepEqual(drift(root), []);
      assert.equal(write(root).length, 0, "a second run rewrites nothing");

      const plugin = readFileSync(
        path.join(root, PLUGIN_AGENTS_DIR, "devcouncil-reviewer.md"),
        "utf8",
      );
      assert.equal(plugin, WRAPPED, "the plugin copy is the source, byte for byte");
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
