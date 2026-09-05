"""MCP tool schema definitions."""

from __future__ import annotations

from mcp.types import Tool, ToolAnnotations

# (read_only, destructive, idempotent, open_world) for every advertised tool.
#
# All four are stated for every tool rather than leaning on the spec's
# "``destructiveHint`` is meaningful only when ``readOnlyHint`` is false"
# conditional. Clients differ in how carefully they implement that conditional,
# and the field's own default is ``true`` — so an unset hint on a read-only
# query reads as "may perform destructive updates", which is how a status call
# ends up behind the same confirmation prompt as ``apply_patch``.
#
# ``open_world`` is true where the effect leaves this repository: a tool that
# spawns an arbitrary process, attaches a debugger to one, or calls a network
# model. Everything else is closed over the project's own files and database.
#
# Rows are behaviour claims about the handler, not restatements of the
# description. Where the two could disagree the handler was read: ``next_task``
# discards ``client_id`` and takes no lease (``task_gate_ops.py:444``),
# ``graph_cypher`` rejects every mutating clause (``indexing/graph/cypher.py:46``),
# and ``devcouncil_cli`` is not read-only because its allowlist
# (``cli_gate.py:11-18``) includes ``write``, ``apply-patch`` and ``rollback``.
_RO = (True, False, True, False)  # a pure query over local state

TOOL_BEHAVIOUR: dict[str, tuple[bool, bool, bool, bool]] = {
    # --- code intelligence ---
    "devcouncil_code_explore": _RO,
    "devcouncil_code_search": _RO,
    "devcouncil_code_path": _RO,
    "devcouncil_code_impact": _RO,
    "devcouncil_code_dead": _RO,  # "never deletes code"
    "devcouncil_code_affected_tests": _RO,
    "devcouncil_code_sync": (False, False, True, False),  # rebuilds the index in place
    "devcouncil_code_status": _RO,
    # --- debugger ---
    "devcouncil_debug_discover": (False, False, True, True),  # persists consent; scans the host
    "devcouncil_debug_start": (False, True, False, True),  # launches or attaches to a process
    "devcouncil_debug_breakpoints": (False, False, True, False),  # replace-all, so repeatable
    "devcouncil_debug_control": (False, False, False, False),  # stepping advances the debuggee
    "devcouncil_debug_inspect": _RO,
    "devcouncil_debug_evaluate": (False, True, False, True),  # "side-effectful" by contract
    "devcouncil_debug_trace": (False, False, False, True),  # runs a script under a tracer
    "devcouncil_debug_stop": (False, True, True, True),  # terminates the debuggee by default
    # --- project state, read side ---
    "devcouncil_status": _RO,
    "devcouncil_integration_status": _RO,
    "devcouncil_report": _RO,
    "devcouncil_get_task": _RO,
    "devcouncil_get_gaps": _RO,
    "devcouncil_get_next_actions": _RO,
    "devcouncil_get_task_provenance": _RO,
    "devcouncil_live_review": _RO,
    "devcouncil_live_cards": _RO,
    "devcouncil_live_repair_prompt": _RO,
    "devcouncil_live_repair_all": _RO,
    "devcouncil_list_tasks": _RO,
    "devcouncil_get_prompt": _RO,
    "devcouncil_tail_trace": _RO,
    "devcouncil_policy_check_write": _RO,  # answers a question, writes nothing
    "devcouncil_prepare_execution": _RO,  # `show` + `prompt`, both reads
    "devcouncil_list_leases": _RO,
    "devcouncil_read_file": _RO,
    "devcouncil_get_diff": _RO,
    "devcouncil_get_evidence": _RO,
    "devcouncil_list_agent_runs": _RO,
    "devcouncil_get_run": _RO,
    "devcouncil_next_task": _RO,
    "devcouncil_select_knowledge": _RO,
    "devcouncil_wiki_page": _RO,
    "devcouncil_run_timeline": _RO,
    # --- map / graph, read side ---
    "devcouncil_graph_context": _RO,
    "devcouncil_repo_map": _RO,
    "devcouncil_impact": _RO,
    "devcouncil_liveness": _RO,
    "devcouncil_graph_runs": _RO,
    "devcouncil_graph_cypher": _RO,
    "devcouncil_pdg_query": _RO,
    "devcouncil_explain": _RO,
    "devcouncil_graph_query": _RO,
    "devcouncil_graph_trace": _RO,
    "devcouncil_graph_impact": _RO,
    "devcouncil_route_map": _RO,
    "devcouncil_shape_check": _RO,
    "devcouncil_api_impact": _RO,
    "devcouncil_lsp_status": _RO,
    "devcouncil_ast_match": _RO,
    # --- map / graph, write side ---
    "devcouncil_graph_ingest": (False, False, True, False),  # sync + export + map write
    "devcouncil_graph_doctor": (False, False, True, False),  # fix=true repairs the store
    # --- leases and task state ---
    "devcouncil_checkout_task": (False, False, False, False),  # a second call is a second claim
    "devcouncil_release_task": (False, False, True, False),
    "devcouncil_renew_lease": (False, False, False, False),  # each call extends again
    "devcouncil_update_task_scope": (False, False, True, False),  # appends *unique* entries
    "devcouncil_append_evidence": (False, False, False, False),  # appends on every call
    "devcouncil_record_command": (False, False, False, False),
    "devcouncil_handoff_agent": (False, False, False, False),
    # --- the tools that change the working tree ---
    "devcouncil_write_file": (False, True, True, False),  # overwrites; same content, same result
    "devcouncil_apply_patch": (False, True, False, False),  # re-applying a diff is not a no-op
    "devcouncil_run_command": (False, True, False, True),  # runs an allowlisted shell command
    "devcouncil_cli": (False, True, False, True),  # allowlist reaches write / apply-patch / rollback
    "devcouncil_verify_task": (False, False, True, True),  # runs the task's own test commands
    # --- meta-agent ---
    "devcouncil_run_supervise": (False, False, False, True),  # asks a network model for a verdict
}


def all_tools() -> list[Tool]:
    """Every advertised tool, with schema and behaviour filled in from one owner.

    The closing happens here rather than in 73 literals so there is one owner
    for the rule and no schema can be added that quietly opts out of it. It was
    the absence of this that let ``create_planned_files`` — a field that widens
    a task's write scope — be read by a handler while the advertised contract
    said it did not exist: an undeclared field validated silently, so nothing
    ever reported the gap.

    Behaviour annotations are attached the same way and for the same reason: a
    tool added without a ``TOOL_BEHAVIOUR`` row ships unannotated, and an
    unannotated tool is one the spec tells clients to assume may be
    destructive. The gap is silent in the payload, so a test asserts the table
    covers every advertised name instead.
    """
    return [_annotated(_closed(tool)) for tool in _tools()]


def _closed(tool: Tool) -> Tool:
    """Return *tool* with ``additionalProperties: false`` on its root schema.

    Only the root object is closed. Nested ``additionalProperties`` (the
    per-source breakpoint map, DAP ``configuration``/``arguments`` passthroughs)
    describe payloads this server forwards rather than reads, and stay open.
    A schema that already states its own answer is left alone.
    """
    schema = tool.input_schema
    if "additionalProperties" in schema:
        return tool
    return tool.model_copy(update={"input_schema": {**schema, "additionalProperties": False}})


def _annotated(tool: Tool) -> Tool:
    """Return *tool* with its ``TOOL_BEHAVIOUR`` row as ``annotations``.

    A tool the table does not name is left unannotated rather than given a
    guessed row: a wrong ``readOnlyHint`` is worse than an absent one, because
    a client acts on the wrong answer instead of falling back to its own
    conservative default. The missing row is caught by the coverage test.
    A tool that already carries its own annotations keeps them.
    """
    if tool.annotations is not None:
        return tool
    behaviour = TOOL_BEHAVIOUR.get(tool.name)
    if behaviour is None:
        return tool
    read_only, destructive, idempotent, open_world = behaviour
    return tool.model_copy(
        update={
            # Field names, as everywhere this package builds an SDK model:
            # ``mcp.types`` accepts the wire alias too, but only the field name
            # type-checks. Serialization is ``by_alias``, so the wire still says
            # ``readOnlyHint``.
            "annotations": ToolAnnotations(
                read_only_hint=read_only,
                destructive_hint=destructive,
                idempotent_hint=idempotent,
                open_world_hint=open_world,
            )
        }
    )


def _tools() -> list[Tool]:
    from devcouncil.integrations.mcp.handlers.codeintel import tools as codeintel_tools
    from devcouncil.integrations.mcp.handlers.debug import tools as debug_tools

    return [
        *codeintel_tools(),
        *debug_tools(),
        Tool(
            name="devcouncil_status",
            description=(
                "Get a compact project status summary (phase, requirement/task/gap counts). "
                "Default responses stay within a ~32 KB agent context budget; use "
                "devcouncil_get_task / get_gaps / get_next_actions for detail by ID."
            ),
            input_schema={
                "type": "object",
                "properties": {}
            }
        ),
        Tool(
            name="devcouncil_integration_status",
            description="Get read-only coding CLI integration status, capability rows, detected clients, and recommended executor.",
            input_schema={"type": "object", "properties": {}},
        ),
        Tool(
            name="devcouncil_report",
            description="Get the full coverage report and a list of all requirements and blocking gaps.",
            input_schema={
                "type": "object",
                "properties": {}
            }
        ),
        Tool(
            name="devcouncil_get_task",
            description="Get details, constraints, and requirements for a specific implementation task.",
            input_schema={
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
                "Read persisted verification gaps for a task WITHOUT re-running "
                "verification. Returns a compact projection (IDs, severity, type, "
                "file/line) within a ~32 KB context budget; use get_task for full "
                "detail. Cheap and idempotent for resume/repair decisions."
            ),
            input_schema={
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
                "Get the typed next-actions contract for a task from persisted gaps, "
                "WITHOUT re-verifying. Compact by default (gap_id/category/action/file; "
                "~32 KB context budget). Returns blocking next_actions, advisory_actions, "
                "and allowed_next_tools."
            ),
            input_schema={
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
            input_schema={
                "type": "object",
                "properties": {"task_id": {"type": "string"}},
                "required": ["task_id"],
            },
        ),
        Tool(
            name="devcouncil_live_review",
            description="Get live coding-agent review status, pending signals, critique-card counts, and blockers.",
            input_schema={
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
            input_schema={
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
            input_schema={
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
            input_schema={
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
            description=(
                "List DevCouncil tasks as compact rows (id/title/status/priority/"
                "requirements/lease) within a ~32 KB context budget. Supports status "
                "filter and limit/offset paging; use get_task for full detail."
            ),
            input_schema={
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
            input_schema={
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
            input_schema={
                "type": "object",
                "properties": {
                    "limit": {"type": "integer", "minimum": 1, "maximum": 200, "default": 20},
                },
            },
        ),
        Tool(
            name="devcouncil_policy_check_write",
            description="Check whether a file write is allowed for a task or the active running task.",
            input_schema={
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
            input_schema={
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
            name="devcouncil_repo_map",
            description=(
                "Query the repo map summary (languages, subsystems) or one subsystem's "
                "detail (entry_points, critical_files, neighbors, role_files). Optional "
                "path resolves a file to its subsystem area. Includes stale freshness flag."
            ),
            input_schema={
                "type": "object",
                "properties": {
                    "subsystem": {
                        "type": "string",
                        "description": "Subsystem area prefix to return in detail.",
                    },
                    "path": {
                        "type": "string",
                        "description": "Repository-relative file path; resolves to its subsystem area.",
                    },
                },
            },
        ),
        Tool(
            name="devcouncil_impact",
            description=(
                "Blast-radius impact for given paths: import dependents, neighbor "
                "subsystem areas, and cross-boundary area pairs from the repo map. "
                "`cross_boundary_checked` is false when the map never established "
                "which subsystems are neighbors, in which case an empty "
                "`cross_boundary_pairs` means the check could not run, not that "
                "every touched area is adjacent. "
                "Set precise=true to resolve dependents via live LSP references when "
                "a language server is available (falls back to import-level dependents)."
            ),
            input_schema={
                "type": "object",
                "properties": {
                    "paths": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Repository-relative file paths to analyze.",
                    },
                    "precise": {
                        "type": "boolean",
                        "description": (
                            "When true, use live LSP textDocument/references for "
                            "dependents instead of import-level map edges."
                        ),
                        "default": False,
                    },
                },
                "required": ["paths"],
            },
        ),
        Tool(
            name="devcouncil_liveness",
            description=(
                "Liveness debt from the repo map: unwired candidates, unreachable files, "
                "dead-symbol candidates, and entry roots. Filterable by subsystem area or path prefix. "
                "When a code graph exists, also returns structured confidence-tagged dead_code "
                "(default min_confidence=inferred; pass ambiguous to include all tiers). "
                "Prefer extracted confidence + greps; treat inferred as unconfirmed. "
                "If entry_roots are empty or unreachable_unreliable is true, ignore unreachable_files "
                "and mass inferred dead."
            ),
            input_schema={
                "type": "object",
                "properties": {
                    "area": {
                        "type": "string",
                        "description": "Optional subsystem area to filter results.",
                    },
                    "path_prefix": {
                        "type": "string",
                        "description": "Optional path prefix to filter results.",
                    },
                    "min_confidence": {
                        "type": "string",
                        "enum": ["extracted", "inferred", "ambiguous"],
                        "description": (
                            "Minimum dead_code confidence tier to include "
                            "(default: inferred). Prefer extracted for deletion decisions."
                        ),
                    },
                },
            },
        ),
        Tool(
            name="devcouncil_graph_ingest",
            description="Unified native ingest: codeintel sync, graph export, repo map write.",
            input_schema={
                "type": "object",
                "properties": {
                    "paths": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Optional repository-relative paths; full reconcile when omitted.",
                    },
                },
            },
        ),
        Tool(
            name="devcouncil_graph_doctor",
            description=(
                "Diagnose the map engine: coded checks with exact fix commands, the build "
                "running now, the last build. fix=true applies every fix the doctor can "
                "apply inside the repository and re-checks."
            ),
            input_schema={
                "type": "object",
                "properties": {"fix": {"type": "boolean", "default": False}},
            },
        ),
        Tool(
            name="devcouncil_graph_runs",
            description=(
                "Records of recent kernel runs (build / manifest / repair): argv, exit code, "
                "duration, the kernel's notes, and the diagnosis code of a failure."
            ),
            input_schema={
                "type": "object",
                "properties": {
                    "limit": {"type": "integer", "minimum": 1, "maximum": 200, "default": 10},
                    "failedOnly": {"type": "boolean", "default": False},
                },
            },
        ),
        Tool(
            name="devcouncil_graph_cypher",
            description="Run a supported Cypher subset over the native code graph store.",
            input_schema={
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "MATCH … RETURN … query."},
                },
                "required": ["query"],
            },
        ),
        Tool(
            name="devcouncil_pdg_query",
            description=(
                "Query opt-in PDG control or data dependence for a symbol qualname or file path. "
                "Requires `dev map --pdg` or `dev map pdg build` first."
            ),
            input_schema={
                "type": "object",
                "properties": {
                    "mode": {
                        "type": "string",
                        "enum": ["controls", "flows"],
                        "description": "controls = CDG edges; flows = reaching-def edges.",
                    },
                    "target": {
                        "type": "string",
                        "description": "Symbol qualname or repository-relative file path.",
                    },
                    "variable": {
                        "type": "string",
                        "description": "Optional variable filter when mode=flows.",
                    },
                },
                "required": ["mode", "target"],
            },
        ),
        Tool(
            name="devcouncil_explain",
            description=(
                "Report heuristic PDG taint findings (source→sink) from the opt-in PDG layer. "
                "Filter by file path and/or taint category."
            ),
            input_schema={
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Optional repository-relative file path filter.",
                    },
                    "category": {
                        "type": "string",
                        "description": "Optional taint category filter (e.g. command-injection).",
                    },
                },
            },
        ),
        Tool(
            name="devcouncil_graph_query",
            description=(
                "360° symbol/file query over the code knowledge graph: definition, callers, "
                "callees, and importers. Requires `dev map` (writes .devcouncil/graph/code_graph.json)."
            ),
            input_schema={
                "type": "object",
                "properties": {
                    "name_or_path": {
                        "type": "string",
                        "description": "Symbol name, qualified id, or file path.",
                    },
                },
                "required": ["name_or_path"],
            },
        ),
        Tool(
            name="devcouncil_graph_trace",
            description=(
                "Shortest path between two nodes in the code knowledge graph "
                "(imports/calls/contains)."
            ),
            input_schema={
                "type": "object",
                "properties": {
                    "from": {
                        "type": "string",
                        "description": "Start node (name or path).",
                    },
                    "to": {
                        "type": "string",
                        "description": "End node (name or path).",
                    },
                },
                "required": ["from", "to"],
            },
        ),
        Tool(
            name="devcouncil_graph_impact",
            description=(
                "Symbol-level blast radius from the code knowledge graph: map paths "
                "(or working-tree diff) to enclosing symbols, then inbound callers/"
                "importers at depth 1/2/3 with confidence tiers. Distinct from "
                "devcouncil_impact (file-level repo-map dependents)."
            ),
            input_schema={
                "type": "object",
                "properties": {
                    "paths": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Repository-relative file paths to analyze.",
                    },
                    "diff": {
                        "type": "boolean",
                        "description": (
                            "When true, seed from working-tree changed files "
                            "(optionally filtered by paths)."
                        ),
                        "default": False,
                    },
                },
            },
        ),
        Tool(
            name="devcouncil_route_map",
            description=(
                "Map HTTP routes (ROUTE nodes) to handlers, registration owners, "
                "and client fetch/axios/requests/httpx consumers from the code graph."
            ),
            input_schema={"type": "object", "properties": {}},
        ),
        Tool(
            name="devcouncil_shape_check",
            description=(
                "Compare handler return dict keys vs keys accessed by API consumers "
                "after fetch calls; flags shape mismatches."
            ),
            input_schema={
                "type": "object",
                "properties": {
                    "route": {
                        "type": "string",
                        "description": "Optional route path or id filter.",
                    },
                },
            },
        ),
        Tool(
            name="devcouncil_api_impact",
            description=(
                "API route blast radius: consumers, middleware (registers edges), "
                "response-shape mismatches, and risk tier (high/medium/low/none)."
            ),
            input_schema={
                "type": "object",
                "properties": {
                    "route_or_path": {
                        "type": "string",
                        "description": "Route path, graph id, or normalized segment.",
                    },
                },
                "required": ["route_or_path"],
            },
        ),
        Tool(
            name="devcouncil_lsp_status",
            description=(
                "Return detected language servers and mode (detection-only or client). "
                "Client mode reflects indexing.lsp_refs / live references capability."
            ),
            input_schema={"type": "object", "properties": {}},
        ),
        Tool(
            name="devcouncil_ast_match",
            description="Search code symbols structurally using optional tree-sitter support and deterministic fallbacks.",
            input_schema={
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
            input_schema={
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
            input_schema={
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
            input_schema={
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
            input_schema={
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
            input_schema={
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
            input_schema={
                "type": "object",
                "properties": {"active_only": {"type": "boolean", "default": True}},
            },
        ),
        Tool(
            name="devcouncil_update_task_scope",
            description=(
                "Append unique expected tests, allowed commands, or planned files "
                "for a leased task. Use planned_files to authorize editing an "
                "intended caller when wiring a new module, and create_planned_files "
                "to authorize creating a new one."
            ),
            input_schema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "lease_token": {"type": "string"},
                    "expected_tests": {"type": "array", "items": {"type": "string"}},
                    "allowed_commands": {"type": "array", "items": {"type": "string"}},
                    "planned_files": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": (
                            "Paths to append as modify-op planned files "
                            "(secret/restricted paths rejected)."
                        ),
                    },
                    "create_planned_files": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": (
                            "Paths to append as create-op planned files, authorizing "
                            "the task to create them (secret/restricted paths rejected)."
                        ),
                    },
                },
                "required": ["task_id", "lease_token"],
            },
        ),
        Tool(
            name="devcouncil_append_evidence",
            description="Append command evidence for a leased task.",
            input_schema={
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
            input_schema={
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
                "Write a file through DevCouncil. In gates.mode=enforce a valid lease and "
                "task scope are required; advisory/off relax coordination and scope gates. "
                "Hard safety is always checked before the atomic write."
            ),
            input_schema={
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "Optional unless gates.mode=enforce.",
                    },
                    "lease_token": {"type": "string"},
                    "path": {"type": "string"},
                    "content": {"type": "string"},
                },
                "required": ["path", "content"],
            },
        ),
        Tool(
            name="devcouncil_apply_patch",
            description=(
                "Apply a unified diff through DevCouncil. In gates.mode=enforce a valid "
                "lease and task scope are required; advisory/off relax those gates. Every "
                "target still passes hard-safety checks before atomic application."
            ),
            input_schema={
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "Optional unless gates.mode=enforce.",
                    },
                    "lease_token": {"type": "string"},
                    "unified_diff": {"type": "string"},
                },
                "required": ["unified_diff"],
            },
        ),
        Tool(
            name="devcouncil_verify_task",
            description=(
                "Process task completion under gates.mode: enforce blocks, advisory records "
                "non-safety findings, and off skips quality verification."
            ),
            input_schema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "lease_token": {"type": "string"},
                    "sandbox": {"type": "string", "enum": ["local"], "default": "local", "description": "Only 'local' is supported in this build."},
                },
                "required": ["task_id"],
            },
        ),
        Tool(
            name="devcouncil_handoff_agent",
            description="Hand off a task between coding CLI agents.",
            input_schema={
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
                "paths. When task_id is set, the path must intersect that task's planned "
                "files (fail-closed; never broadens scope). Supports offset/limit or "
                "line_range windowing. Returns content (truncated), sha256, and line_count."
            ),
            input_schema={
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Repository-relative or absolute path inside the project."},
                    "task_id": {
                        "type": "string",
                        "description": (
                            "Optional task scope. When set, only planned-file paths for "
                            "that task may be read."
                        ),
                    },
                    # Both ceilings mirror the handler's own clamp
                    # (`int_argument(..., maximum=10_000_000)`), so the advertised
                    # range is the range that is actually enforced. `limit` had no
                    # ceiling anywhere: the handler only floors it at 1.
                    "offset": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": 10_000_000,
                        "description": "0-based line offset to start from.",
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 10_000_000,
                        "description": "Max number of lines to return.",
                    },
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
            input_schema={
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
            input_schema={
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
                "Run a command through DevCouncil. gates.mode=enforce requires a valid "
                "lease and task allowlist; advisory/off retain dangerous-Git safety while "
                "relaxing those gates. Uses a clean environment and bounded timeout."
            ),
            input_schema={
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "Optional unless gates.mode=enforce.",
                    },
                    "lease_token": {"type": "string"},
                    "command": {"type": "string"},
                },
                "required": ["command"],
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
            input_schema={
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
            input_schema={
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
            input_schema={
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
            input_schema={
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
            input_schema={
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
            input_schema={
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
            input_schema={
                "type": "object",
                "properties": {
                    "reference": {"type": "string", "description": "A run id or task id."},
                },
                "required": ["reference"],
            },
        ),
    ]
