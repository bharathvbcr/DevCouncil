"""Generate the full Claude Code asset surface for a DevCouncil project.

DevCouncil already installs a Claude Code MCP server and three hooks. This module adds
the *static* extensibility surfaces Claude Code reads from a repository so a project gets
complete, installable Claude Code support:

* **Slash commands** — ``.claude/commands/devcouncil/*.md`` (``/devcouncil:status`` ...).
* **Subagents** — ``.claude/agents/devcouncil-*.md``.
* **Output style** — ``.claude/output-styles/devcouncil.md``.
* **Statusline** — a ``settings.json`` ``statusLine`` entry pointing at ``devcouncil
  claude-statusline`` plus the matching CLI command.
* **Permissions** — a ``settings.json`` allow-list for the read-only ``dev``/``devcouncil``
  commands so the slash commands and hooks don't prompt on every run.
* **Plugin bundle** — a self-contained Claude Code plugin + single-repo marketplace under
  ``.devcouncil/claude-plugin/`` bundling the commands, agents, skills, hooks, and MCP
  server so the whole integration installs with one ``/plugin install``.

Every builder is pure (returns text); writers return the list of paths actually changed so
re-running is an idempotent no-op. Keeping the generation here (not in the Typer command)
keeps it unit-testable without a CLI round-trip.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

from devcouncil.knowledge.frontmatter import build_frontmatter_markdown
from devcouncil.integrations.clients.hooks import SESSION_START_MATCHER

# Tools a DevCouncil subagent should be allowed to use: the standard read/edit/run set
# plus the DevCouncil MCP tools it drives the task loop with. Listing the MCP tools keeps
# the agent inside the policy-gated workflow rather than free-handing edits.
_MCP = "mcp__devcouncil"
_SUBAGENT_CORE_TOOLS = ["Read", "Grep", "Glob", "Bash", "Edit", "Write", "TodoWrite"]


@dataclass(frozen=True)
class GeneratedAsset:
    """One generated file: where it goes and what it should contain."""

    path: Path
    content: str

    def write_if_changed(self) -> bool:
        """Write the file only when its content differs; return True if it changed."""
        if self.path.exists() and self.path.read_text(encoding="utf-8") == self.content:
            return False
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.path.write_text(self.content, encoding="utf-8")
        return True


# --- Slash commands -------------------------------------------------------------
# Each command file lives under .claude/commands/devcouncil/ so it surfaces as
# /devcouncil:<name>. Lines beginning with ! run as bash (their output is injected),
# so allowed-tools scopes Bash to exactly the dev subcommand the file shells out to.

@dataclass(frozen=True)
class _SlashCommand:
    name: str
    description: str
    argument_hint: str
    bash: str  # the dev command run via ! ; "" means no shell-out
    body: str
    allowed_tools: str


def _slash_commands() -> list[_SlashCommand]:
    return [
        _SlashCommand(
            name="status",
            description="Show the DevCouncil project status (phase, tasks, blocking gaps).",
            argument_hint="",
            bash="dev status",
            body=(
                "Summarize the DevCouncil status above for the user: the current phase, how many "
                "tasks are planned/running/verified, and any blocking gaps. Recommend the single "
                "best next action (e.g. `/devcouncil:next` or `/devcouncil:repair`)."
            ),
            allowed_tools="Bash(dev status:*)",
        ),
        _SlashCommand(
            name="next",
            description="Find and start the next unblocked DevCouncil task.",
            argument_hint="",
            bash="dev tasks list",
            body=(
                "Identify the highest-priority unblocked task from the list above. Use the DevCouncil "
                "MCP tools to implement it: call `mcp__devcouncil__devcouncil_next_task`, then "
                "`mcp__devcouncil__devcouncil_checkout_task` to acquire a lease, make changes only "
                "through the policy-gated write tools, run tests, then "
                "`mcp__devcouncil__devcouncil_verify_task` and release the lease when verified."
            ),
            allowed_tools="Bash(dev tasks:*)",
        ),
        _SlashCommand(
            name="verify",
            description="Run DevCouncil verification for a task (or the active task).",
            argument_hint="[TASK-ID]",
            bash="dev verify $ARGUMENTS",
            body=(
                "Report the verification result above. If there are blocking gaps, list them and "
                "propose minimal fixes; do not consider the task complete while blocking gaps remain."
            ),
            allowed_tools="Bash(dev verify:*)",
        ),
        _SlashCommand(
            name="repair",
            description="Repair the blocking verification gaps for a task.",
            argument_hint="[TASK-ID]",
            bash="dev repair $ARGUMENTS",
            body=(
                "Work through the repair guidance above. Apply the smallest changes that close the "
                "blocking gaps via the policy-gated DevCouncil write tools, then re-run "
                "`/devcouncil:verify` until the task is clean."
            ),
            allowed_tools="Bash(dev repair:*)",
        ),
        _SlashCommand(
            name="plan",
            description="Plan a goal into DevCouncil requirements and tasks.",
            argument_hint="<goal>",
            bash="",
            body=(
                "Plan the following goal with DevCouncil: $ARGUMENTS\n\n"
                "Run `dev plan \"$ARGUMENTS\"` to generate the requirement/task breakdown, then "
                "summarize the resulting tasks and their scope for the user before any execution."
            ),
            allowed_tools="Bash(dev plan:*)",
        ),
        _SlashCommand(
            name="review",
            description="Review pending live-review critique cards and resolve blockers.",
            argument_hint="[TASK-ID]",
            bash="dev watch cards $ARGUMENTS",
            body=(
                "Review the live critique cards above. For each blocking card, use "
                "`mcp__devcouncil__devcouncil_live_repair_prompt` to get a ready-to-apply repair, "
                "apply it through the policy-gated write tools, and re-verify."
            ),
            allowed_tools="Bash(dev watch:*)",
        ),
        _SlashCommand(
            name="report",
            description="Show the full DevCouncil coverage report.",
            argument_hint="",
            bash="dev report",
            body=(
                "Present the coverage report above concisely: requirement coverage, unmapped "
                "requirements, orphaned tasks, and the blocking gaps that need attention next."
            ),
            allowed_tools="Bash(dev report:*)",
        ),
    ]


def _slash_command_markdown(cmd: _SlashCommand) -> str:
    meta: dict[str, object] = {"description": cmd.description}
    if cmd.argument_hint:
        meta["argument-hint"] = cmd.argument_hint
    if cmd.allowed_tools:
        meta["allowed-tools"] = cmd.allowed_tools
    lines: list[str] = []
    if cmd.bash:
        lines.append(f"!`{cmd.bash}`")
        lines.append("")
    lines.append(cmd.body)
    return build_frontmatter_markdown(meta, "\n".join(lines))


def build_slash_commands(root: Path) -> list[GeneratedAsset]:
    base = root / ".claude" / "commands" / "devcouncil"
    return [
        GeneratedAsset(base / f"{cmd.name}.md", _slash_command_markdown(cmd))
        for cmd in _slash_commands()
    ]


# --- Subagents ------------------------------------------------------------------

@dataclass(frozen=True)
class _Subagent:
    name: str
    description: str
    tools: list[str]
    body: str


def _subagents() -> list[_Subagent]:
    impl_tools = _SUBAGENT_CORE_TOOLS + [
        f"{_MCP}__devcouncil_next_task",
        f"{_MCP}__devcouncil_get_task",
        f"{_MCP}__devcouncil_get_prompt",
        f"{_MCP}__devcouncil_checkout_task",
        f"{_MCP}__devcouncil_read_file",
        f"{_MCP}__devcouncil_get_diff",
        f"{_MCP}__devcouncil_write_file",
        f"{_MCP}__devcouncil_apply_patch",
        f"{_MCP}__devcouncil_run_command",
        f"{_MCP}__devcouncil_verify_task",
        f"{_MCP}__devcouncil_release_task",
        f"{_MCP}__devcouncil_update_task_scope",
    ]
    verify_tools = ["Read", "Grep", "Glob", "Bash"] + [
        f"{_MCP}__devcouncil_get_task",
        f"{_MCP}__devcouncil_get_gaps",
        f"{_MCP}__devcouncil_get_next_actions",
        f"{_MCP}__devcouncil_get_evidence",
        f"{_MCP}__devcouncil_get_task_provenance",
        f"{_MCP}__devcouncil_verify_task",
    ]
    review_tools = ["Read", "Grep", "Glob", "Bash"] + [
        f"{_MCP}__devcouncil_get_diff",
        f"{_MCP}__devcouncil_live_review",
        f"{_MCP}__devcouncil_live_cards",
        f"{_MCP}__devcouncil_live_repair_prompt",
        f"{_MCP}__devcouncil_graph_context",
        f"{_MCP}__devcouncil_policy_check_write",
    ]
    return [
        _Subagent(
            name="devcouncil-implementer",
            description=(
                "Implements a DevCouncil task end-to-end under policy enforcement. Use when the "
                "user wants to pick up the next task or implement a specific TASK-ID through the "
                "DevCouncil lease/verify loop."
            ),
            tools=impl_tools,
            body=(
                "You are the DevCouncil implementer subagent. You make code changes ONLY through "
                "DevCouncil's policy-gated workflow.\n\n"
                "Workflow:\n"
                "1. `devcouncil_next_task` (or use the TASK-ID you were given) and "
                "`devcouncil_checkout_task` to acquire a lease.\n"
                "2. `devcouncil_get_task` + `devcouncil_get_prompt` for scope; `devcouncil_read_file` "
                "and `devcouncil_get_diff` to inspect.\n"
                "3. Change files only via `devcouncil_write_file` / `devcouncil_apply_patch` (the gate "
                "rejects out-of-scope or protected paths) and run tests via `devcouncil_run_command`.\n"
                "4. `devcouncil_verify_task`; fix any blocking gaps and re-verify.\n"
                "5. `devcouncil_release_task` when verified.\n\n"
                "Never touch files outside the task scope. Report the final status and remaining gaps."
            ),
        ),
        _Subagent(
            name="devcouncil-verifier",
            description=(
                "Runs DevCouncil verification and reports blocking gaps and next actions without "
                "modifying code. Use to confirm whether a task actually meets its requirements."
            ),
            tools=verify_tools,
            body=(
                "You are the DevCouncil verifier subagent. You are read-only with respect to source "
                "code: never edit files.\n\n"
                "For the task under review, call `devcouncil_verify_task` (requires a lease) or read "
                "the persisted state with `devcouncil_get_gaps`, `devcouncil_get_next_actions`, "
                "`devcouncil_get_evidence`, and `devcouncil_get_task_provenance`. Report the blocking "
                "gaps, whether the changed code was actually exercised (diff coverage), and the "
                "concrete next actions. Do not declare success while blocking gaps remain."
            ),
        ),
        _Subagent(
            name="devcouncil-reviewer",
            description=(
                "Reviews the working-tree diff against DevCouncil policy and live-review critique "
                "cards. Use for a structural, policy-aware code review before merge."
            ),
            tools=review_tools,
            body=(
                "You are the DevCouncil reviewer subagent. Review the current changes for "
                "correctness, scope, and policy compliance.\n\n"
                "Use `devcouncil_get_diff` for the change set, `devcouncil_graph_context` for "
                "structural impact, `devcouncil_policy_check_write` to confirm changed paths are "
                "in-scope, and `devcouncil_live_review` / `devcouncil_live_cards` for outstanding "
                "critique cards. Summarize findings as blocking vs advisory and reference "
                "file:line. Do not edit code — hand fixes back to the implementer."
            ),
        ),
    ]


def _subagent_markdown(agent: _Subagent) -> str:
    meta = {
        "name": agent.name,
        "description": agent.description,
        "tools": ", ".join(agent.tools),
    }
    return build_frontmatter_markdown(meta, agent.body)


def build_subagents(root: Path) -> list[GeneratedAsset]:
    base = root / ".claude" / "agents"
    return [
        GeneratedAsset(base / f"{agent.name}.md", _subagent_markdown(agent))
        for agent in _subagents()
    ]


# --- Output style ---------------------------------------------------------------

def build_output_style(root: Path) -> list[GeneratedAsset]:
    meta = {
        "name": "DevCouncil",
        "description": "Evidence-first engineering discipline aligned with DevCouncil's verify loop.",
    }
    body = (
        "You are operating inside a DevCouncil-managed repository. Hold to evidence-first "
        "engineering discipline:\n\n"
        "- Prefer the DevCouncil MCP tools and `dev` CLI for status, scope, and verification "
        "rather than guessing project state.\n"
        "- Make the smallest change that satisfies the task's requirements; stay inside the "
        "task scope and never edit protected/secret paths.\n"
        "- Back claims with evidence: run the tests, show the verification result, and cite "
        "`file:line`. Do not call work done while blocking gaps remain.\n"
        "- When unsure of project conventions, consult `.devcouncil/repo_map.json` and the "
        "applicable skills before writing code.\n"
        "- Be concise: report what changed, what was verified, and what is still blocking."
    )
    return [GeneratedAsset(root / ".claude" / "output-styles" / "devcouncil.md", build_frontmatter_markdown(meta, body))]


# --- Plugin bundle + marketplace ------------------------------------------------
# A self-contained Claude Code plugin (and a single-repo marketplace pointing at it) so the
# whole DevCouncil integration installs with one /plugin install. The plugin bundles its own
# copies of the commands/agents/skills and a hooks.json + .mcp.json that resolve paths via
# ${CLAUDE_PROJECT_DIR}, so it works from whatever repo the plugin is enabled in.

PLUGIN_ROOT_REL = Path(".devcouncil") / "claude-plugin"
_PLUGIN_NAME = "devcouncil"
_MARKETPLACE_NAME = "devcouncil-local"


def _plugin_dir(root: Path) -> Path:
    return root / PLUGIN_ROOT_REL / _PLUGIN_NAME


def _plugin_json(version: str) -> str:
    manifest = {
        "name": _PLUGIN_NAME,
        "description": "DevCouncil: evidence-gated planning, execution, and verification for coding agents.",
        "version": version,
        "author": {"name": "DevCouncil"},
        "keywords": ["devcouncil", "verification", "planning", "mcp", "code-review"],
    }
    return json.dumps(manifest, indent=2) + "\n"


def _marketplace_json(version: str) -> str:
    manifest = {
        "name": _MARKETPLACE_NAME,
        "owner": {"name": "DevCouncil"},
        "plugins": [
            {
                "name": _PLUGIN_NAME,
                "source": f"./{_PLUGIN_NAME}",
                "description": "DevCouncil Claude Code integration: commands, subagents, skills, hooks, and MCP.",
                "version": version,
            }
        ],
    }
    return json.dumps(manifest, indent=2) + "\n"


def _plugin_hooks_json(*, write_gate: bool = False) -> str:
    """hooks.json for the plugin, resolving the project root via ${CLAUDE_PROJECT_DIR}.

    Assist-mode by default (no blocking write-gate) so installing the plugin into an
    interactive session never fail-closes it. The blocking PreToolUse/PostToolUse gate is
    included only when ``write_gate`` is True."""
    def cmd(event: str) -> str:
        return f'devcouncil hook {event} --client claude --project-root "${{CLAUDE_PROJECT_DIR}}"'

    hooks: dict[str, list] = {
        "Stop": [{"hooks": [{"type": "command", "command": cmd("agent-response"), "timeout": 10000}]}],
        "SessionStart": [{"matcher": SESSION_START_MATCHER, "hooks": [{"type": "command", "command": cmd("session-start"), "timeout": 10000}]}],
        "UserPromptSubmit": [{"hooks": [{"type": "command", "command": cmd("user-prompt-submit"), "timeout": 10000}]}],
        "SessionEnd": [{"hooks": [{"type": "command", "command": cmd("session-end"), "timeout": 10000}]}],
        "PreCompact": [{"hooks": [{"type": "command", "command": cmd("pre-compact"), "timeout": 10000}]}],
        "PostCompact": [{"hooks": [{"type": "command", "command": cmd("post-compact"), "timeout": 10000}]}],
        "SubagentStop": [{"hooks": [{"type": "command", "command": cmd("subagent-stop"), "timeout": 10000}]}],
        "Notification": [{"hooks": [{"type": "command", "command": cmd("notification"), "timeout": 10000}]}],
    }
    if write_gate:
        tool_matcher = "Bash|Write|Edit|MultiEdit"
        hooks["PreToolUse"] = [{"matcher": tool_matcher, "hooks": [{"type": "command", "command": cmd("pre-tool-use"), "timeout": 10000}]}]
        hooks["PostToolUse"] = [{"matcher": tool_matcher, "hooks": [{"type": "command", "command": cmd("post-tool-use"), "timeout": 10000}]}]
    return json.dumps({"hooks": hooks}, indent=2) + "\n"


def _plugin_mcp_json(root: Path) -> str:
    config = {
        "mcpServers": {
            "devcouncil": {
                "type": "stdio",
                "command": "devcouncil",
                "args": ["mcp-server"],
                "env": {"DEVCOUNCIL_PROJECT_ROOT": "${CLAUDE_PROJECT_DIR}"},
            }
        }
    }
    return json.dumps(config, indent=2) + "\n"


def _plugin_readme() -> str:
    return (
        "# DevCouncil Claude Code plugin\n\n"
        "This plugin bundles DevCouncil's full Claude Code integration: slash commands, "
        "subagents, engineering skills, lifecycle hooks, and the DevCouncil MCP server.\n\n"
        "## Install\n\n"
        "```\n"
        "/plugin marketplace add <path-to-repo>/.devcouncil/claude-plugin\n"
        f"/plugin install {_PLUGIN_NAME}@{_MARKETPLACE_NAME}\n"
        "```\n\n"
        "Requires the `devcouncil` CLI on PATH (`pipx install devcouncil`) and a "
        "DevCouncil-initialized repo (`dev init`).\n"
    )


def build_plugin_bundle(
    root: Path, *, version: str, skill_assets: list[GeneratedAsset] | None = None, write_gate: bool = False
) -> list[GeneratedAsset]:
    """Build the plugin tree: manifest, marketplace, bundled commands/agents, hooks, MCP."""
    plugin = _plugin_dir(root)
    market_root = root / PLUGIN_ROOT_REL
    assets: list[GeneratedAsset] = [
        GeneratedAsset(market_root / ".claude-plugin" / "marketplace.json", _marketplace_json(version)),
        GeneratedAsset(plugin / ".claude-plugin" / "plugin.json", _plugin_json(version)),
        GeneratedAsset(plugin / "hooks" / "hooks.json", _plugin_hooks_json(write_gate=write_gate)),
        GeneratedAsset(plugin / ".mcp.json", _plugin_mcp_json(root)),
        GeneratedAsset(plugin / "README.md", _plugin_readme()),
    ]
    # Bundle command + agent copies into the plugin tree (plugin layout puts them at the
    # plugin root, not under .claude/).
    for cmd in _slash_commands():
        assets.append(GeneratedAsset(plugin / "commands" / "devcouncil" / f"{cmd.name}.md", _slash_command_markdown(cmd)))
    for agent in _subagents():
        assets.append(GeneratedAsset(plugin / "agents" / f"{agent.name}.md", _subagent_markdown(agent)))
    # Bundle the selected skills (passed in so selection logic stays in the skills layer).
    for skill_asset in skill_assets or []:
        rel = skill_asset.path
        # skill_asset.path is .claude/skills/<name>/SKILL.md — re-root under the plugin.
        try:
            tail = rel.relative_to(root / ".claude" / "skills")
        except ValueError:
            tail = Path(rel.name)
        assets.append(GeneratedAsset(plugin / "skills" / tail, skill_asset.content))
    return assets
