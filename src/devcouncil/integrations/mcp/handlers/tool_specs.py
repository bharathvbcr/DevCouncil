"""MCP tool schema definitions."""

from __future__ import annotations

from mcp.types import Tool


def all_tools() -> list[Tool]:
    return [
        Tool(
            name="devcouncil_status",
            description="Get the current status of the DevCouncil project, including phase, tasks, and gaps.",
            inputSchema={
                "type": "object",
                "properties": {}
            }
        ),
        Tool(
            name="devcouncil_integration_status",
            description="Get read-only coding CLI integration status, capability rows, detected clients, and recommended executor.",
            inputSchema={"type": "object", "properties": {}},
        ),
        Tool(
            name="devcouncil_report",
            description="Get the full coverage report and a list of all requirements and blocking gaps.",
            inputSchema={
                "type": "object",
                "properties": {}
            }
        ),
        Tool(
            name="devcouncil_get_task",
            description="Get details, constraints, and requirements for a specific implementation task.",
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "The ID of the task, e.g. TASK-001"
                    }
                },
                "required": ["task_id"]
            }
        ),
        Tool(
            name="devcouncil_get_gaps",
            description=(
                "Read the persisted verification gaps for a task WITHOUT re-running "
                "verification. Cheap and idempotent — use it to resume after a "
                "reconnect or to inspect outstanding work before deciding to repair."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "blocking_only": {"type": "boolean", "default": False},
                },
                "required": ["task_id"],
            },
        ),
        Tool(
            name="devcouncil_get_next_actions",
            description=(
                "Get the typed, machine-routable next-actions contract for a task from "
                "its persisted gaps, WITHOUT re-verifying. Returns blocking next_actions "
                "plus advisory_actions and the tools allowed next."
            ),
            inputSchema={
                "type": "object",
                "properties": {"task_id": {"type": "string"}},
                "required": ["task_id"],
            },
        ),
        Tool(
            name="devcouncil_get_task_provenance",
            description=(
                "Inspect the recorded audit trail for a task: gated file changes "
                "(write_file/apply_patch and hook events), verification runs, diff-coverage "
                "evidence (was the changed code actually exercised), and the latest "
                "correction manifest. Read-only — lets a developer or agent trust what "
                "actually happened on disk."
            ),
            inputSchema={
                "type": "object",
                "properties": {"task_id": {"type": "string"}},
                "required": ["task_id"],
            },
        ),
        Tool(
            name="devcouncil_live_review",
            description="Get live coding-agent review status, pending signals, critique-card counts, and blockers.",
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "Optional task scope for live-review blocker calculation.",
                    }
                },
            },
        ),
        Tool(
            name="devcouncil_live_cards",
            description="List live-review critique cards with optional task, status, verdict, and client filters.",
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "Optional task scope for critique cards.",
                    },
                    "status": {
                        "type": "string",
                        "enum": ["open", "resolved", "ignored"],
                        "description": "Optional card status filter.",
                    },
                    "verdict": {
                        "type": "string",
                        "enum": ["approved", "concerns", "critical"],
                        "description": "Optional card verdict filter.",
                    },
                    "client": {
                        "type": "string",
                        "description": "Optional coding-agent client filter.",
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 200,
                        "default": 20,
                    },
                },
            },
        ),
        Tool(
            name="devcouncil_live_repair_prompt",
            description="Generate a ready-to-paste repair prompt for a live-review critique card.",
            inputSchema={
                "type": "object",
                "properties": {
                    "card_id": {
                        "type": "string",
                        "description": "The critique card ID, e.g. CARD-abc123.",
                    }
                },
                "required": ["card_id"],
            },
        ),
        Tool(
            name="devcouncil_live_repair_all",
            description="Generate one repair prompt for all blocking live-review critique cards in scope.",
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "Optional task scope for blocking live-review cards.",
                    }
                },
            },
        ),
        Tool(
            name="devcouncil_list_tasks",
            description="List DevCouncil tasks with status and requirement mappings. Supports a status filter and limit/offset paging so large projects don't blow the agent's context.",
            inputSchema={
                "type": "object",
                "properties": {
                    "status": {"type": "string", "description": "Optional status filter (e.g. planned, running, blocked, verified, done)."},
                    "limit": {"type": "integer", "description": "Max tasks to return (default 100, max 500)."},
                    "offset": {"type": "integer", "description": "Number of tasks to skip (default 0)."},
                },
            },
        ),
        Tool(
            name="devcouncil_get_prompt",
            description="Get the raw implementation prompt for a DevCouncil task.",
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string", "description": "The ID of the task, e.g. TASK-001"},
                },
                "required": ["task_id"],
            },
        ),
        Tool(
            name="devcouncil_tail_trace",
            description="Return recent DevCouncil trace events as JSON.",
            inputSchema={
                "type": "object",
                "properties": {
                    "limit": {"type": "integer", "minimum": 1, "maximum": 200, "default": 20},
                },
            },
        ),
        Tool(
            name="devcouncil_policy_check_write",
            description="Check whether a file write is allowed for a task or the active running task.",
            inputSchema={
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Repository-relative or absolute path to check."},
                    "task_id": {"type": "string", "description": "Optional task ID. Defaults to the running task."},
                },
                "required": ["path"],
            },
        ),
        Tool(
            name="devcouncil_graph_context",
            description="Get optional code-review-graph structural context for changed or planned files.",
            inputSchema={
                "type": "object",
                "properties": {
                    "files": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Repository-relative files to contextualize.",
                    }
                },
            },
        ),
        Tool(
            name="devcouncil_lsp_status",
            description="Return detected language servers and starter LSP initialize payloads.",
            inputSchema={"type": "object", "properties": {}},
        ),
        Tool(
            name="devcouncil_ast_match",
            description="Search code symbols structurally using optional tree-sitter support and deterministic fallbacks.",
            inputSchema={
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "language": {"type": "string"},
                    "kind": {"type": "string"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 500, "default": 100},
                },
            },
        ),
        Tool(
            name="devcouncil_cli",
            description="Run a safe DevCouncil CLI command for status, tasks, report, map, prompt, show, trace, lsp, or ast.",
            inputSchema={
                "type": "object",
                "properties": {
                    "args": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Arguments after the dev command, for example ['status','--json'].",
                    }
                },
                "required": ["args"],
            },
        ),
        Tool(
            name="devcouncil_prepare_execution",
            description="Return a task prompt plus planned files and allowed commands for external execution tooling.",
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string", "description": "The ID of the task, e.g. TASK-001"},
                },
                "required": ["task_id"],
            },
        ),
        Tool(
            name="devcouncil_checkout_task",
            description="Acquire a task lease and return scope for MCP write tools.",
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "client_id": {"type": "string"},
                    "agent": {"type": "string"},
                    "force": {"type": "boolean", "default": False},
                },
                "required": ["task_id", "client_id"],
            },
        ),
        Tool(
            name="devcouncil_release_task",
            description="Release a task lease using its token.",
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "lease_token": {"type": "string"},
                },
                "required": ["task_id", "lease_token"],
            },
        ),
        Tool(
            name="devcouncil_renew_lease",
            description=(
                "Extend a held task lease's TTL so a long-running agent does not lose it "
                "to expiry. Returns the new expires_at."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "lease_token": {"type": "string"},
                    "ttl_seconds": {"type": "integer"},
                },
                "required": ["task_id", "lease_token"],
            },
        ),
        Tool(
            name="devcouncil_list_leases",
            description=(
                "List task leases for fleet supervision — task_id, owner, agent, "
                "expires_at, and whether each is expired. Defaults to active leases."
            ),
            inputSchema={
                "type": "object",
                "properties": {"active_only": {"type": "boolean", "default": True}},
            },
        ),
        Tool(
            name="devcouncil_update_task_scope",
            description="Append unique expected tests or allowed commands for a leased task.",
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "lease_token": {"type": "string"},
                    "expected_tests": {"type": "array", "items": {"type": "string"}},
                    "allowed_commands": {"type": "array", "items": {"type": "string"}},
                },
                "required": ["task_id", "lease_token"],
            },
        ),
        Tool(
            name="devcouncil_append_evidence",
            description="Append command evidence for a leased task.",
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "lease_token": {"type": "string"},
                    "command": {"type": "string"},
                    "exit_code": {"type": "integer"},
                    "summary": {"type": "string"},
                },
                "required": ["task_id", "lease_token", "command", "exit_code", "summary"],
            },
        ),
        Tool(
            name="devcouncil_record_command",
            description="Record a shell command event for a leased task.",
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "lease_token": {"type": "string"},
                    "command": {"type": "string"},
                    "status": {"type": "string", "enum": ["started", "finished", "failed", "blocked"]},
                    "exit_code": {"type": "integer"},
                    "reason": {"type": "string"},
                },
                "required": ["task_id", "lease_token", "command", "status"],
            },
        ),
        Tool(
            name="devcouncil_write_file",
            description=(
                "Write a file for a leased task through DevCouncil's policy gate. The write "
                "is checked against the task's scope BEFORE it lands (out-of-scope or "
                "protected paths are rejected), applied atomically, and recorded as a "
                "FileChangeEvent. Returns applied_files and rejected_files."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "lease_token": {"type": "string"},
                    "path": {"type": "string"},
                    "content": {"type": "string"},
                },
                "required": ["task_id", "lease_token", "path", "content"],
            },
        ),
        Tool(
            name="devcouncil_apply_patch",
            description=(
                "Apply a unified diff for a leased task through DevCouncil's policy gate. "
                "EVERY target file is policy-checked first; if any is out of scope the whole "
                "patch is rejected (never partially applied). Applied atomically via git and "
                "each file recorded as a FileChangeEvent. Returns applied_files/rejected_files."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "lease_token": {"type": "string"},
                    "unified_diff": {"type": "string"},
                },
                "required": ["task_id", "lease_token", "unified_diff"],
            },
        ),
        Tool(
            name="devcouncil_verify_task",
            description="Run verification for a leased task (local sandbox).",
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "lease_token": {"type": "string"},
                    "sandbox": {"type": "string", "enum": ["local"], "default": "local", "description": "Only 'local' is supported in this build."},
                },
                "required": ["task_id", "lease_token"],
            },
        ),
        Tool(
            name="devcouncil_handoff_agent",
            description="Hand off a task between coding CLI agents.",
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "lease_token": {"type": "string"},
                    "from_agent": {"type": "string"},
                    "to_agent": {"type": "string"},
                    "instruction": {"type": "string"},
                },
                "required": ["task_id", "lease_token", "from_agent", "to_agent"],
            },
        ),
        Tool(
            name="devcouncil_read_file",
            description=(
                "Read a repository file (read-only, no lease required) so an MCP-only "
                "agent can inspect content before constructing a diff or overwriting it. "
                "Containment-checked against the project root and refuses secret/credential "
                "paths. Supports offset/limit or line_range windowing. Returns content "
                "(truncated), sha256, and line_count."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Repository-relative or absolute path inside the project."},
                    "offset": {"type": "integer", "minimum": 0, "description": "0-based line offset to start from."},
                    "limit": {"type": "integer", "minimum": 1, "description": "Max number of lines to return."},
                    "line_range": {
                        "type": "string",
                        "description": "Inclusive 1-based line range like '10-40' (overrides offset/limit).",
                    },
                },
                "required": ["path"],
            },
        ),
        Tool(
            name="devcouncil_get_diff",
            description=(
                "Return the working-tree diff for the project (requires a git repo). When "
                "task_id is given the diff is scoped to that task's planned/changed files. "
                "Set staged=true to include the staged (git diff --cached) changes. Returns "
                "per-file status with additions/deletions and the truncated unified diff."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string", "description": "Optional task to scope the diff to its files."},
                    "paths": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Optional explicit repo-relative paths to scope the diff to.",
                    },
                    "staged": {"type": "boolean", "default": False, "description": "Include staged changes."},
                },
            },
        ),
        Tool(
            name="devcouncil_get_evidence",
            description=(
                "Read persisted CommandResult evidence for a task and inline the truncated "
                "stdout/stderr from the stored log files (best-effort; tolerates missing "
                "files). Pairs with verification to close the diagnose leg of the loop."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "command": {"type": "string", "description": "Optional substring filter on the recorded command."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 100, "default": 20},
                },
                "required": ["task_id"],
            },
        ),
        Tool(
            name="devcouncil_run_command",
            description=(
                "Run a command for a leased task through DevCouncil's allowlist gate. The "
                "command must pass the task's allowed_commands policy (same gate as the "
                "hooks); otherwise it is refused and nothing runs. Executed with a clean "
                "subprocess env and a timeout, recorded as a ShellCommandEvent. Returns "
                "exit_code and truncated stdout/stderr."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "lease_token": {"type": "string"},
                    "command": {"type": "string"},
                },
                "required": ["task_id", "lease_token", "command"],
            },
        ),
        Tool(
            name="devcouncil_list_agent_runs",
            description=(
                "List recorded coding-agent runs (from .devcouncil/runs/*/agent-run.json), "
                "newest first. Each entry includes run_id, task, agent, profile, status, "
                "started time, and an orphaned flag for runs still marked running whose "
                "manifest has gone stale (executor likely crashed). Read-only."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "status": {"type": "string", "description": "Optional status filter (e.g. running, finished, failed, timeout)."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 500, "default": 20},
                },
            },
        ),
        Tool(
            name="devcouncil_get_run",
            description=(
                "Get the full manifest for a single coding-agent run plus a redacted "
                "transcript tail when a transcript/log file exists in the run directory. "
                "Includes the resolved CLI invocation and an orphaned flag. Read-only."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "run_id": {"type": "string", "description": "The run id to inspect."},
                },
                "required": ["run_id"],
            },
        ),
        Tool(
            name="devcouncil_next_task",
            description=(
                "Return the highest-priority task that is unblocked (its depends_on are "
                "satisfied) and has no active lease, so an autonomous agent can bootstrap "
                "deterministically instead of racing list_tasks. Includes a blocking-gap "
                "summary and a ready_to_checkout flag."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "client_id": {"type": "string", "description": "Optional client id (informational)."},
                    "status": {"type": "string", "description": "Optional status filter (default planned/ready)."},
                },
            },
        ),
        Tool(
            name="devcouncil_select_knowledge",
            description=(
                "Select the ingested project knowledge (OKF documents and the design "
                "system) that applies to a goal and return it as a ready-to-inject "
                "markdown preamble, so a coding agent can ask 'what project knowledge "
                "applies to <goal>?'. Always-on design knowledge is included; OKF "
                "documents are matched on goal keywords. Returns the matched sources "
                "and the rendered preamble."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "goal": {"type": "string", "description": "The task or goal to find applicable knowledge for."},
                },
                "required": ["goal"],
            },
        ),
        Tool(
            name="devcouncil_wiki_page",
            description=(
                "Read the generated codebase wiki (OKF bundle under "
                ".devcouncil/knowledge/okf/wiki/). With no arguments, returns the page "
                "index. Pass 'page' (a bundle-relative path like "
                "'subsystems/src-devcouncil-council.md') for one page, or 'query' to "
                "find pages whose title/tags/description match. Read-only; refresh "
                "with `dev wiki update`."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "page": {"type": "string", "description": "Bundle-relative page path (e.g. subsystems/<slug>.md)."},
                    "query": {"type": "string", "description": "Keyword(s) to match against page titles, tags, and descriptions."},
                },
            },
        ),
        Tool(
            name="devcouncil_run_timeline",
            description=(
                "Get a run's full reversible trace (Shepherd-style): manifest, trace "
                "events, git checkpoints (before/after/attempts), diff stat, and whether "
                "the run is reversible. Accepts a run id or task id. Read-only — use "
                "`dev runs revert <ref>` to reverse a run's workspace effects."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "reference": {"type": "string", "description": "A run id or task id."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 500, "default": 40},
                },
                "required": ["reference"],
            },
        ),
        Tool(
            name="devcouncil_run_supervise",
            description=(
                "Ask the supervisor meta-agent for a keep/revert/repair verdict on a "
                "recorded run, from its manifest, trace events, and diff. Uses the "
                "run_supervisor model role when configured, degrading to deterministic "
                "heuristics. Never modifies the workspace; the verdict is logged to the "
                "trace and reverting stays an explicit separate step."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "reference": {"type": "string", "description": "A run id or task id."},
                },
                "required": ["reference"],
            },
        ),
    ]
