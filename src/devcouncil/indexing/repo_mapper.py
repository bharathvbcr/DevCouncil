import ast
import hashlib
import json
import logging
import os
import re
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path
from typing import Dict, List, Set, Tuple, cast

from pydantic import BaseModel, Field

from devcouncil.indexing.lsp import LspInspector
from devcouncil.utils.json_persist import read_json

logger = logging.getLogger(__name__)

# File extensions treated as primary source for subsystem inference.
_CODE_EXTENSIONS = {".py", ".ts", ".tsx", ".js", ".jsx", ".go", ".rs", ".java", ".kt", ".rb", ".cs", ".cpp", ".c"}
# Top-level directories grouped as their own area rather than folded into a source root.
_AUX_AREA_ROOTS = {"tests", "test", "docs", "doc", "scripts", "examples", "example", "benchmarks"}
# Filenames that signal an entry point, used to break ties when no import data exists.
_ENTRY_NAME_HINTS = ("__init__", "__main__", "main", "index", "app", "cli", "server", "mod", "lib")


class RepoFileEntry(BaseModel):
    path: str
    area: str
    kind: str
    language: str | None = None
    summary: str


class RepoSubsystem(BaseModel):
    area: str
    summary: str
    entry_points: List[str]
    critical_files: List[str]
    neighbors: List[str] = Field(default_factory=list)
    handoff_paths: List[str] = Field(default_factory=list)
    role_files: Dict[str, List[str]] = Field(default_factory=dict)


class RepoMap(BaseModel):
    languages: List[str]
    frameworks: List[str]
    package_managers: List[str]
    test_commands: List[str]
    important_files: List[str]
    candidate_files: List[Dict[str, str]]
    files: List[RepoFileEntry] = Field(default_factory=list)
    subsystems: List[RepoSubsystem] = Field(default_factory=list)
    # file -> the files that import it (reverse import edges, capped per file). Lets a
    # prompt show the blast radius of changing a file without re-parsing the repo.
    dependents: Dict[str, List[str]] = Field(default_factory=dict)
    # Freshness fingerprints captured at generation: the git HEAD the map was built from
    # and a hash of the tracked file set. Consumers compare against the current repo to
    # detect a stale map before trusting its structure.
    generated_head: str = ""
    indexed_hash: str = ""
    lsp: Dict[str, object] = Field(default_factory=dict)
    # Optional dependency-vulnerability findings. Populated only when `dev map` is
    # run with SCA explicitly enabled (off by default so the map stays fast and
    # offline-by-default); empty otherwise.
    dependency_risks: List[Dict[str, str]] = Field(default_factory=list)

class RepoMapper:
    def __init__(self, project_root: Path):
        self.project_root = project_root
        # Common source-root prefix of the repo's primary code (e.g. "src/pkg"),
        # computed once per map_repo run. Drives generic, non-DevCouncil subsystem
        # inference. None until computed.
        self._source_root: str | None = None
        # True when this is not the DevCouncil source tree, so generic inference is used
        # for area bucketing. Set in map_repo.
        self._use_generic: bool = False
        # Import edges (importer -> imported), computed once per map_repo run and reused
        # by subsystem inference, important-file ranking, and the dependents index.
        self._edges: List[Tuple[str, str]] | None = None
        # Cache of config-file contents (package.json, pyproject.toml, ...) so framework
        # and test-command detection don't each re-read the same files from disk.
        self._config_file_cache: Dict[str, str] = {}

    def _read_config_file(self, name: str) -> str:
        """Read a repo-root config file once and cache its contents for reuse."""
        if name not in self._config_file_cache:
            self._config_file_cache[name] = (self.project_root / name).read_text()
        return self._config_file_cache[name]

    _DEPENDENTS_MAX = 12  # cap dependents listed per file to bound repo_map.json size

    _LANGUAGE_BY_EXTENSION = {
        ".py": "python",
        ".ts": "typescript",
        ".tsx": "typescript",
        ".js": "javascript",
        ".jsx": "javascript",
        ".go": "go",
        ".rs": "rust",
        ".java": "java",
        ".c": "c",
        ".cpp": "cpp",
        ".md": "markdown",
        ".yaml": "yaml",
        ".yml": "yaml",
        ".toml": "toml",
        ".json": "json",
        ".sh": "shell",
        ".ps1": "powershell",
    }

    _AREA_SUMMARIES = {
        "src/devcouncil/cli": "CLI entrypoints and command registration",
        "src/devcouncil/app": "Orchestration runtime and lifecycle state",
        "src/devcouncil/artifacts": "Artifact graph, coverage, and serialization",
        "src/devcouncil/council": "Council prompts and debate scaffolding",
        "src/devcouncil/domain": "Domain entities for requirements, tasks, and evidence",
        "src/devcouncil/execution": "Execution plumbing, prompts, permissions, and task runs",
        "src/devcouncil/executors": "Executor adapters and CLI agent registry",
        "src/devcouncil/gating": "Blocking policies and guardrails",
        "src/devcouncil/indexing": "Repo mapping, AST matching, semantic snapshots, and language-server detection",
        "src/devcouncil/integrations": "External integrations and graph adapters",
        "src/devcouncil/live": "Live review cards, signals, summaries, and transcripts",
        "src/devcouncil/llm": "Model provider routing, defaults, and caching",
        "src/devcouncil/planning": "Planning, critique, repair, and spec services",
        "src/devcouncil/repo": "Repository helpers and filesystem utilities",
        "src/devcouncil/reporting": "JSON and markdown report builders",
        "src/devcouncil/storage": "SQLite persistence and repository layer",
        "src/devcouncil/telemetry": "Trace logging, pricing, and telemetry tracking",
        "src/devcouncil/ui": "Dashboard and lightweight UI helpers",
        "src/devcouncil/utils": "Shared utilities and redaction helpers",
        "src/devcouncil/verification": "Verification gates and implementation review",
        "docs": "Repository documentation",
        "tests": "Automated tests",
        "scripts": "Maintenance and smoke-test scripts",
    }

    _SUBSYSTEM_INDEX: Dict[str, Tuple[str, List[str]]] = {
        "src/devcouncil/council": (
            "Prompt-driven council workflows and debate templates.",
            [
                "src/devcouncil/council/prompts/spec_writer.md",
                "src/devcouncil/council/prompts/planner_a.md",
                "src/devcouncil/council/prompts/implementation_reviewer.md",
            ],
        ),
        "src/devcouncil/domain": (
            "Shared domain entities for tasks, requirements, evidence, and critique.",
            [
                "src/devcouncil/domain/task.py",
                "src/devcouncil/domain/requirement.py",
                "src/devcouncil/domain/gap.py",
                "src/devcouncil/domain/evidence.py",
            ],
        ),
        "src/devcouncil/execution": (
            "Execution planning and task orchestration runtime, including prompt and permission handling.",
            [
                "src/devcouncil/execution/task_runner.py",
                "src/devcouncil/execution/prompt_builder.py",
                "src/devcouncil/execution/permissions.py",
                "src/devcouncil/execution/paths.py",
            ],
        ),
        "src/devcouncil/executors": (
            "Adapter layer that converts tasks into CLI/API side effects.",
            [
                "src/devcouncil/executors/agent_registry.py",
                "src/devcouncil/executors/coding_cli.py",
                "src/devcouncil/executors/mini_swe.py",
                "src/devcouncil/executors/openhands.py",
            ],
        ),
        "src/devcouncil/indexing": (
            "Repo mapping, AST matching, semantic snapshots, and language-server detection (no live LSP client).",
            [
                "src/devcouncil/indexing/repo_mapper.py",
                "src/devcouncil/indexing/ast_matcher.py",
                "src/devcouncil/indexing/semantic_index.py",
                "src/devcouncil/indexing/lsp.py",
            ],
        ),
        "src/devcouncil/integrations": (
            "External system integrations and MCP/Graph adapters.",
            [
                "src/devcouncil/integrations/gitnexus.py",
                "src/devcouncil/integrations/graphify.py",
                "src/devcouncil/integrations/mcp/server.py",
                "src/devcouncil/integrations/github.py",
            ],
        ),
        "src/devcouncil/verification": (
            "Verification gates, evidence checks, and implementation review.",
            [
                "src/devcouncil/verification/verifier.py",
                "src/devcouncil/verification/patch_reviewer.py",
                "src/devcouncil/verification/review_agent.py",
            ],
        ),
        "src/devcouncil/live": (
            "Live review cards, signals, summaries, and repair guidance.",
            [
                "src/devcouncil/live/reviewer.py",
                "src/devcouncil/live/cards.py",
                "src/devcouncil/live/signals.py",
            ],
        ),
        "src/devcouncil/llm": (
            "Model routing, provider registry, and LLM response caching.",
            [
                "src/devcouncil/llm/router.py",
                "src/devcouncil/llm/provider.py",
                "src/devcouncil/llm/cache.py",
            ],
        ),
        "src/devcouncil/planning": (
            "Task planning, spec generation, critique, and arbitration services.",
            [
                "src/devcouncil/planning/plan_service.py",
                "src/devcouncil/planning/spec_service.py",
                "src/devcouncil/planning/critique_service.py",
                "src/devcouncil/planning/repair_service.py",
            ],
        ),
        "src/devcouncil/repo": (
            "Repository helper helpers for workspace interactions.",
            [
                "src/devcouncil/repo/__init__.py",
            ],
        ),
        "src/devcouncil/reporting": (
            "Report generation and check-writing utilities.",
            [
                "src/devcouncil/reporting/report_builder.py",
                "src/devcouncil/reporting/json_report.py",
                "src/devcouncil/reporting/markdown_report.py",
            ],
        ),
        "src/devcouncil/gating": (
            "Policy gates and blocking criteria.",
            [
                "src/devcouncil/gating/gate.py",
                "src/devcouncil/gating/policy.py",
                "src/devcouncil/gating/rules.py",
            ],
        ),
        "src/devcouncil/storage": (
            "Persistence layer for run state, artifacts, and graph-backed history.",
            [
                "src/devcouncil/storage/repositories.py",
                "src/devcouncil/storage/db.py",
                "src/devcouncil/storage/models.py",
            ],
        ),
        "src/devcouncil/cli": (
            "User command surface and command wiring.",
            [
                "src/devcouncil/cli/main.py",
                "src/devcouncil/cli/commands/map.py",
                "src/devcouncil/cli/commands/plan.py",
                "src/devcouncil/cli/commands/run.py",
            ],
        ),
        "src/devcouncil/app": (
            "Orchestrator and state machine controlling project lifecycle.",
            [
                "src/devcouncil/app/orchestrator.py",
                "src/devcouncil/app/state_machine.py",
                "src/devcouncil/app/run_context.py",
            ],
        ),
        "src/devcouncil/artifacts": (
            "Artifact graph primitives and evidence linking.",
            [
                "src/devcouncil/artifacts/graph.py",
                "src/devcouncil/artifacts/exports.py",
                "src/devcouncil/artifacts/types.py",
            ],
        ),
        "src/devcouncil/telemetry": (
            "Telemetry ingestion, tracing, cost, and pricing.",
            [
                "src/devcouncil/telemetry/traces.py",
                "src/devcouncil/telemetry/tracker.py",
                "src/devcouncil/telemetry/cost.py",
            ],
        ),
        "src/devcouncil/ui": (
            "Dashboard rendering and lightweight user interface glue.",
            [
                "src/devcouncil/ui/dashboard.py",
            ],
        ),
        "src/devcouncil/utils": (
            "Shared utility helpers, redaction, and support functions.",
            [
                "src/devcouncil/utils/redaction.py",
            ],
        ),
    }

    _SUBSYSTEM_CRITICAL_MAX = 6

    _SUBSYSTEM_NEIGHBORS: Dict[str, List[str]] = {
        "src/devcouncil/council": [
            "src/devcouncil/planning",
            "src/devcouncil/verification",
        ],
        "src/devcouncil/domain": [
            "src/devcouncil/execution",
            "src/devcouncil/executors",
            "src/devcouncil/storage",
            "src/devcouncil/verification",
            "src/devcouncil/planning",
            "src/devcouncil/gating",
        ],
        "src/devcouncil/execution": [
            "src/devcouncil/executors",
            "src/devcouncil/gating",
            "src/devcouncil/verification",
            "src/devcouncil/storage",
        ],
        "src/devcouncil/executors": [
            "src/devcouncil/execution",
            "src/devcouncil/app",
            "src/devcouncil/storage",
        ],
        "src/devcouncil/verification": [
            "src/devcouncil/storage",
            "src/devcouncil/gating",
            "src/devcouncil/app",
        ],
        "src/devcouncil/gating": [
            "src/devcouncil/execution",
            "src/devcouncil/verification",
            "src/devcouncil/storage",
        ],
        "src/devcouncil/storage": [
            "src/devcouncil/app",
            "src/devcouncil/artifacts",
            "src/devcouncil/verification",
        ],
        "src/devcouncil/cli": [
            "src/devcouncil/app",
            "src/devcouncil/storage",
            "src/devcouncil/indexing",
        ],
        "src/devcouncil/app": [
            "src/devcouncil/cli",
            "src/devcouncil/execution",
            "src/devcouncil/storage",
            "src/devcouncil/verification",
        ],
        "src/devcouncil/artifacts": [
            "src/devcouncil/storage",
            "src/devcouncil/verification",
        ],
        "src/devcouncil/indexing": [
            "src/devcouncil/llm",
            "src/devcouncil/execution",
            "src/devcouncil/cli",
        ],
        "src/devcouncil/integrations": [
            "src/devcouncil/cli",
            "src/devcouncil/live",
            "src/devcouncil/reporting",
            "src/devcouncil/telemetry",
        ],
        "src/devcouncil/live": [
            "src/devcouncil/verification",
            "src/devcouncil/telemetry",
            "src/devcouncil/reporting",
            "src/devcouncil/cli",
        ],
        "src/devcouncil/llm": [
            "src/devcouncil/planning",
            "src/devcouncil/execution",
            "src/devcouncil/verification",
            "src/devcouncil/app",
        ],
        "src/devcouncil/planning": [
            "src/devcouncil/domain",
            "src/devcouncil/llm",
            "src/devcouncil/cli",
            "src/devcouncil/execution",
        ],
        "src/devcouncil/repo": [
            "src/devcouncil/cli",
        ],
        "src/devcouncil/reporting": [
            "src/devcouncil/telemetry",
            "src/devcouncil/integrations",
            "src/devcouncil/cli",
            "src/devcouncil/live",
        ],
        "src/devcouncil/telemetry": [
            "src/devcouncil/cli",
            "src/devcouncil/app",
            "src/devcouncil/execution",
            "src/devcouncil/verification",
            "src/devcouncil/llm",
        ],
        "src/devcouncil/ui": [
            "src/devcouncil/telemetry",
        ],
        "src/devcouncil/utils": [
            "src/devcouncil/execution",
            "src/devcouncil/cli",
            "src/devcouncil/verification",
            "src/devcouncil/executors",
            "src/devcouncil/llm",
        ],
    }

    _SUBSYSTEM_HANDOFFS: Dict[str, List[str]] = {
        "src/devcouncil/council": [
            "planning/arbiter_service.py -> planning/plan_service.py",
            "planning/spec_service.py -> planning/plan_service.py",
        ],
        "src/devcouncil/domain": [
            "domain/task.py -> execution/task_runner.py",
            "domain/evidence.py -> artifacts/graph.py",
            "domain/requirement.py -> verification/verifier.py",
        ],
        "src/devcouncil/execution": [
            "execution/task_runner.py -> executors/*",
            "execution/task_runner.py -> verification/verifier.py",
            "execution/task_runner.py -> storage/repositories.py",
        ],
        "src/devcouncil/executors": [
            "executors/* -> execution/task_runner.py",
            "executors/* -> storage/repositories.py",
        ],
        "src/devcouncil/verification": [
            "verification/verifier.py -> storage/repositories.py",
            "verification/verifier.py -> gating/policy.py",
            "verification/verifier.py -> artifacts/graph.py",
        ],
        "src/devcouncil/gating": [
            "gating/policy.py -> execution/permissions.py",
            "gating/policy.py -> verification/verifier.py",
        ],
        "src/devcouncil/storage": [
            "storage/repositories.py -> app/state_machine.py",
            "storage/repositories.py -> artifacts/graph.py",
        ],
        "src/devcouncil/indexing": [
            "indexing/repo_mapper.py -> cli/commands/map.py",
            "indexing/lsp.py -> execution/task_runner.py",
        ],
        "src/devcouncil/integrations": [
            "integrations/mcp/server.py -> live/reviewer.py",
            "integrations/code_review_graph.py -> live/cards.py",
            "integrations/gitnexus.py -> reporting/report_builder.py",
        ],
        "src/devcouncil/live": [
            "live/summary.py -> live/cards.py",
            "live/reviewer.py -> live/models.py",
            "live/tasks.py -> live/signals.py",
        ],
        "src/devcouncil/llm": [
            "llm/router.py -> telemetry/tracker.py",
            "llm/router.py -> telemetry/traces.py",
            "llm/provider.py -> llm/router.py",
        ],
        "src/devcouncil/planning": [
            "planning/plan_service.py -> execution/task_runner.py",
            "planning/repair_service.py -> verification/implementation_reviewer.py",
            "planning/arbiter_service.py -> verification/verifier.py",
        ],
        "src/devcouncil/repo": [
            "repo/__init__.py -> cli/commands/map.py",
        ],
        "src/devcouncil/reporting": [
            "reporting/report_builder.py -> reporting/markdown_report.py",
            "reporting/report_builder.py -> reporting/json_report.py",
            "reporting/github_check.py -> integrations/pr_comments.py",
        ],
        "src/devcouncil/telemetry": [
            "telemetry/traces.py -> live/summary.py",
            "telemetry/tracker.py -> reporting/markdown_report.py",
        ],
        "src/devcouncil/ui": [
            "ui/dashboard.py -> live/summary.py",
        ],
    }

    _SUBSYSTEM_ROLE_FILES: Dict[str, List[Tuple[str, List[str]]]] = {
        "src/devcouncil/council": [
            ("prompts", ["council/prompts/spec_writer.md", "council/prompts/rebuttal.md", "council/prompts/implementation_reviewer.md"]),
            ("planners", ["council/prompts/planner_a.md", "council/prompts/planner_b.md"]),
            ("critics", ["council/prompts/critic_a.md", "council/prompts/critic_b.md"]),
            ("arbitration", ["council/prompts/arbiter.md"]),
        ],
        "src/devcouncil/domain": [
            ("tasks", ["domain/task.py"]),
            ("requirements", ["domain/requirement.py"]),
            ("evidence", ["domain/evidence.py"]),
            ("gaps", ["domain/gap.py"]),
            ("critiques", ["domain/critique.py"]),
            ("assumptions", ["domain/assumption.py"]),
        ],
        "src/devcouncil/indexing": [
            ("mapping", ["indexing/repo_mapper.py"]),
            ("ast", ["indexing/ast_matcher.py"]),
            ("semantic", ["indexing/semantic_index.py"]),
            # Detection-only LSP helper (no live client); see lsp.py docstring.
            ("lsp", ["indexing/lsp.py"]),
            # GraphIndex is consumed by integrations/gitnexus.py — kept, not dead.
            ("graph", ["indexing/graph_index.py"]),
        ],
        "src/devcouncil/integrations": [
            ("vcs", ["integrations/github.py"]),
            ("graphify", ["integrations/graphify.py"]),
            ("code_review", ["integrations/code_review_graph.py"]),
            ("comments", ["integrations/pr_comments.py"]),
            ("mcp", ["integrations/mcp/server.py"]),
            ("third_party", ["integrations/gitnexus.py"]),
        ],
        "src/devcouncil/cli": [
            ("entrypoints", ["cli/main.py"]),
            ("commands", ["cli/commands/map.py", "cli/commands/plan.py", "cli/commands/run.py", "cli/commands/verify.py"]),
            ("setup", ["cli/commands/init.py", "cli/commands/setup.py", "cli/commands/integrate.py"]),
            ("lifecycle", ["cli/commands/status.py", "cli/commands/show.py", "cli/commands/watch.py"]),
            ("maintenance", ["cli/commands/doctor.py", "cli/commands/version.py", "cli/commands/config.py"]),
        ],
        "src/devcouncil/app": [
            ("orchestration", ["app/orchestrator.py"]),
            ("state", ["app/state_machine.py", "app/run_context.py"]),
            ("events", ["app/events.py"]),
            ("configuration", ["app/config.py", "app/errors.py", "app/project_status.py"]),
        ],
        "src/devcouncil/artifacts": [
            ("graph", ["artifacts/graph.py"]),
            ("coverage", ["artifacts/coverage.py", "artifacts/serializer.py"]),
            ("schema", ["artifacts/schemas.py", "artifacts/migrations.py"]),
            ("validation", ["artifacts/validators.py"]),
        ],
        "src/devcouncil/executors": [
            ("registry", ["executors/agent_registry.py"]),
            ("adapters", ["executors/coding_cli.py", "executors/openhands.py", "executors/mini_swe.py"]),
            ("native", ["executors/native/agent.py"]),
        ],
        "src/devcouncil/execution": [
            ("runtime", ["execution/task_runner.py", "execution/context_builder.py"]),
            ("prompting", ["execution/prompt_builder.py"]),
            ("permissions", ["execution/permissions.py"]),
            ("patching", ["execution/patch.py", "execution/executor.py"]),
            ("paths", ["execution/paths.py"]),
        ],
        "src/devcouncil/gating": [
            ("policy", ["gating/policy.py"]),
            ("checks", ["gating/checks/clean_git.py", "gating/checks/planned_files_check.py"]),
            ("coverage", ["gating/checks/requirement_coverage.py", "gating/checks/secret_scan_check.py"]),
        ],
        "src/devcouncil/live": [
            ("cards", ["live/cards.py"]),
            ("review", ["live/reviewer.py"]),
            ("signals", ["live/signals.py"]),
            ("sessions", ["live/tasks.py", "live/transcripts.py"]),
            ("summaries", ["live/summary.py"]),
            ("models", ["live/models.py"]),
        ],
        "src/devcouncil/llm": [
            ("routing", ["llm/router.py"]),
            ("providers", ["llm/provider.py"]),
            ("cache", ["llm/cache.py"]),
            ("defaults", ["llm/model_defaults.yaml"]),
        ],
        "src/devcouncil/planning": [
            ("plan", ["planning/plan_service.py", "planning/prompt_enhancer_service.py"]),
            ("spec", ["planning/spec_service.py"]),
            ("critique", ["planning/critique_service.py"]),
            ("repair", ["planning/repair_service.py"]),
            ("arbiter", ["planning/arbiter_service.py"]),
        ],
        "src/devcouncil/repo": [
            ("api", ["repo/__init__.py"]),
        ],
        "src/devcouncil/reporting": [
            ("builder", ["reporting/report_builder.py"]),
            ("markdown", ["reporting/markdown_report.py"]),
            ("json", ["reporting/json_report.py"]),
            ("checks", ["reporting/github_check.py"]),
        ],
        "src/devcouncil/verification": [
            ("gates", ["verification/verifier.py", "verification/implementation_reviewer.py"]),
            ("implementation_reviewer", ["verification/implementation_reviewer.py"]),
            ("policy", ["verification/verifier.py"]),
        ],
        "src/devcouncil/telemetry": [
            ("traces", ["telemetry/traces.py"]),
            ("tracker", ["telemetry/tracker.py"]),
            ("cost", ["telemetry/cost.py"]),
            ("pricing", ["telemetry/pricing.py", "telemetry/model_pricing.yaml"]),
        ],
        "src/devcouncil/storage": [
            ("repositories", ["storage/repositories.py"]),
            ("schema", ["storage/models.py"]),
            ("database", ["storage/db.py"]),
        ],
        "src/devcouncil/ui": [
            ("dashboard", ["ui/dashboard.py"]),
        ],
        "src/devcouncil/utils": [
            ("redaction", ["utils/redaction.py"]),
        ],
    }

    _COMMAND_SUMMARIES = {
        "agents": "CLI agent registry and integration commands",
        "artifacts": "Artifact graph inspection commands",
        "ast": "AST matching and symbol discovery commands",
        "baseline": "Capture or inspect a baseline snapshot",
        "config": "Inspect or mutate project configuration",
        "dashboard": "Dashboard launch command",
        "doctor": "Preflight and environment diagnostics",
        "go": "End-to-end task execution alias",
        "hook": "Hook configuration commands",
        "init": "Project initialization and integration bootstrap",
        "integrate": "Coding CLI and MCP integration setup",
        "lsp": "LSP inspection commands",
        "map": "Repository mapping command",
        "mcp_server": "MCP server command",
        "plan": "Planning workflow command",
        "prompt": "Prompt generation for agent handoff",
        "repair": "Repair prompt generation",
        "report": "Task and project reporting commands",
        "reset_demo_state": "Reset demo state and sample data",
        "rollback": "Rollback workflow command",
        "run": "Execute an approved task",
        "setup": "Interactive project setup command",
        "show": "Show current project state",
        "status": "Compact workflow status command",
        "tasks": "Task graph and task listing commands",
        "trace": "Trace inspection commands",
        "verify": "Verification workflow command",
        "version": "Version display command",
        "watch": "Live review and transcript monitoring",
    }

    _DOC_SUMMARIES = {
        "AGENTS.md": "Workspace guide for coding agents",
        "CLAUDE.md": "Workspace guide for Claude-based agents",
        "README.md": "Project overview and usage entrypoint",
        "architecture.md": "Top-level architecture overview",
        "cli-reference.md": "CLI command reference",
        "quickstart.md": "First-run installation and workflow",
        "workflow.md": "Manual sidecar workflow guide",
        "security.md": "Security and privacy model",
        "project-status.md": "Subsystem maturity snapshot",
        "roadmap.md": "Planned work and roadmap",
    }

    def _language_for_file(self, path: str) -> str | None:
        suffix = Path(path).suffix.lower()
        return self._LANGUAGE_BY_EXTENSION.get(suffix)

    def _kind_for_file(self, path: str) -> str:
        normalized = path.replace("\\", "/")
        suffix = Path(normalized).suffix.lower()
        name = Path(normalized).name
        if normalized.startswith("tests/") or name.startswith("test_"):
            return "test"
        if normalized.startswith("docs/") or suffix == ".md":
            return "doc"
        if suffix in {".yaml", ".yml", ".toml", ".json", ".ini"}:
            return "config"
        if suffix in {".sh", ".ps1", ".bat"}:
            return "script"
        if suffix in {".sqlite", ".db"}:
            return "database"
        if suffix in {".py", ".ts", ".tsx", ".js", ".jsx", ".go", ".rs", ".java", ".c", ".cpp"}:
            return "module" if name != "__init__.py" else "package"
        return "file"

    def _summary_for_file(self, path: str) -> str:
        normalized = path.replace("\\", "/")
        name = Path(normalized).name
        parts = normalized.split("/")
        if normalized == "README.md":
            return self._DOC_SUMMARIES["README.md"]
        if normalized.startswith("docs/"):
            stem = Path(name).stem.replace("-", " ")
            return self._DOC_SUMMARIES.get(name, f"Documentation: {stem}")
        if normalized.startswith("tests/"):
            remainder = normalized.removeprefix("tests/")
            if remainder.startswith("unit/"):
                return f"Unit tests for {Path(remainder).stem.replace('test_', '').replace('_', ' ').strip() or 'the package'}"
            return f"Tests for {Path(remainder).stem.replace('test_', '').replace('_', ' ').strip() or 'the package'}"
        if normalized.startswith("src/devcouncil/cli/commands/"):
            stem = Path(name).stem
            return self._COMMAND_SUMMARIES.get(stem, f"CLI command module: {stem}")
        if normalized == "src/devcouncil/cli/main.py":
            return "Typer root command composition"
        if normalized == "src/devcouncil/app/orchestrator.py":
            return "Orchestration coordinator and run lifecycle"
        if normalized == "src/devcouncil/app/state_machine.py":
            return "Allowed project phase transitions"
        if normalized == "src/devcouncil/artifacts/graph.py":
            return "Artifact graph and coverage queries"
        if normalized == "src/devcouncil/indexing/repo_mapper.py":
            return "Repository mapping and file classification"
        if normalized == "src/devcouncil/storage/repositories.py":
            return "Persistence repositories for state and artifacts"
        if normalized == "src/devcouncil/storage/models.py":
            return "SQLModel database schema"
        if normalized == "src/devcouncil/verification/verifier.py":
            return "Verification gates and evidence checks"
        if normalized == "src/devcouncil/planning/plan_service.py":
            return "Plan generation service"
        if normalized == "src/devcouncil/planning/spec_service.py":
            return "Spec generation service"
        if normalized == "src/devcouncil/planning/critique_service.py":
            return "Plan critique service"
        if normalized == "src/devcouncil/planning/repair_service.py":
            return "Repair workflow service"
        if normalized == "src/devcouncil/planning/arbiter_service.py":
            return "Plan arbitration service"
        if normalized == "src/devcouncil/execution/task_runner.py":
            return "Task execution runner"
        if normalized == "src/devcouncil/execution/prompt_builder.py":
            return "Prompt assembly for executors"
        if normalized == "src/devcouncil/execution/permissions.py":
            return "Execution permission policy"
        if normalized == "src/devcouncil/executors/agent_registry.py":
            return "Built-in and configured CLI agent registry"
        if normalized == "src/devcouncil/llm/router.py":
            return "LLM provider routing"
        if normalized == "src/devcouncil/telemetry/traces.py":
            return "Trace logging and event persistence"
        if normalized == "src/devcouncil/live/reviewer.py":
            return "Live review service"
        if normalized == "src/devcouncil/integrations/gitnexus.py":
            return "GitNexus integration shim"
        if normalized == "src/devcouncil/integrations/graphify.py":
            return "Graphify integration shim"
        if normalized.startswith("src/devcouncil/"):
            area = "/".join(parts[:3]) if len(parts) >= 3 else "src/devcouncil"
            return self._AREA_SUMMARIES.get(area, f"{area} subsystem")
        if normalized.startswith("scripts/"):
            return f"Utility script: {name}"
        if name in self._DOC_SUMMARIES:
            return self._DOC_SUMMARIES[name]
        return Path(name).stem.replace("_", " ")

    def _area_for_file(self, path: str) -> str:
        normalized = path.replace("\\", "/")
        if normalized.startswith("src/devcouncil/"):
            parts = normalized.split("/")
            if len(parts) >= 5 and parts[2] == "cli" and parts[3] == "commands":
                return "src/devcouncil/cli/commands"
            if len(parts) >= 4:
                return "/".join(parts[:3])
            return "src/devcouncil"
        if normalized.startswith("tests/"):
            return "tests"
        if normalized.startswith("docs/"):
            return "docs"
        if normalized.startswith("scripts/"):
            return "scripts"
        # Foreign repos: derive the area from the directory tree. Gated on _use_generic
        # so DevCouncil's own map keeps its existing "root" bucketing.
        if self._use_generic:
            return self._generic_area_for_file(normalized, self._source_root or "")
        return "root"

    def _build_subsystem_index(self, files: List[str]) -> List[RepoSubsystem]:
        # The hardcoded index is authoritative for DevCouncil's own tree (preserves
        # its curated summaries/role buckets). For any other repo it matches nothing,
        # so fall back to generic, import-graph-driven inference.
        hardcoded = self._build_hardcoded_subsystems(files)
        if hardcoded:
            return hardcoded
        return self._build_generic_subsystems(files)

    def _build_hardcoded_subsystems(self, files: List[str]) -> List[RepoSubsystem]:
        file_set = set(files)
        # Single O(n) pass: bucket files by their "src/devcouncil/<area>" prefix so the
        # per-subsystem loop below uses O(1) dict lookups instead of rescanning every
        # file for each area (and, previously, again for each neighbor). All subsystem
        # and neighbor keys are 3-component "src/devcouncil/<area>" prefixes, so this
        # bucketing reproduces the prior `path.startswith(f"{area}/")` semantics exactly.
        by_area: Dict[str, List[str]] = {}
        for path in files:
            parts = path.split("/")
            if len(parts) >= 4 and parts[0] == "src" and parts[1] == "devcouncil":
                by_area.setdefault("/".join(parts[:3]), []).append(path)
        for bucket in by_area.values():
            bucket.sort()
        subsystems: List[RepoSubsystem] = []
        for area, (summary, entry_points) in self._SUBSYSTEM_INDEX.items():
            available_entry_points = [path for path in entry_points if path in file_set]
            if not available_entry_points:
                continue
            area_files = by_area.get(area, [])
            ranked_files = [path for path in available_entry_points if path in file_set]
            for path in area_files:
                if path in available_entry_points:
                    continue
                if len(ranked_files) >= self._SUBSYSTEM_CRITICAL_MAX:
                    break
                ranked_files.append(path)
            critical_files = ranked_files[: self._SUBSYSTEM_CRITICAL_MAX]
            neighbors = [n for n in self._SUBSYSTEM_NEIGHBORS.get(area, []) if n in by_area]
            handoff_paths = self._SUBSYSTEM_HANDOFFS.get(area, [])
            role_files = self._build_role_files(area, area_files)
            subsystems.append(
                RepoSubsystem(
                    area=area,
                    summary=summary,
                    entry_points=available_entry_points,
                    critical_files=critical_files,
                    neighbors=neighbors,
                    handoff_paths=handoff_paths,
                    role_files=role_files,
                )
            )
        return subsystems

    def _build_role_files(self, area: str, area_files: List[str]) -> Dict[str, List[str]]:
        role_specs = self._SUBSYSTEM_ROLE_FILES.get(area)
        if not role_specs:
            return {}

        by_role: Dict[str, List[str]] = {}
        used = set()
        for role, tokens in role_specs:
            matches = [path for path in area_files if any(token in path for token in tokens)]
            if not matches:
                continue
            selected = matches[:4]
            by_role[role] = selected
            used.update(selected)

        if not by_role:
            return {}

        leftovers = [path for path in area_files if path not in used][:4]
        if leftovers:
            by_role.setdefault("other", leftovers)

        return by_role

    # ------------------------------------------------------------------
    # Generic (non-DevCouncil) subsystem inference
    # ------------------------------------------------------------------

    def _code_files(self, files: List[str]) -> List[str]:
        return [f for f in files if Path(f).suffix.lower() in _CODE_EXTENSIONS]

    def _primary_code_files(self, files: List[str]) -> List[str]:
        """Code files excluding tests/docs/scripts — the ones that define the repo's
        real structure and so determine the source root."""
        primary: List[str] = []
        for f in self._code_files(files):
            top = f.replace("\\", "/").split("/")[0]
            name = Path(f).name
            if top in _AUX_AREA_ROOTS or name.startswith("test_") or name.endswith("_test.go"):
                continue
            primary.append(f)
        return primary

    def detect_source_root(self, files: List[str]) -> str:
        """Longest common directory prefix shared by the primary source files
        (e.g. ``src/mypkg``). Empty when the code spans unrelated top-level dirs."""
        dirs = [Path(f).parent.as_posix() for f in self._primary_code_files(files)]
        dirs = [d for d in dirs if d not in ("", ".")]
        if not dirs:
            return ""
        split = [d.split("/") for d in dirs]
        common = split[0]
        for parts in split[1:]:
            limit = min(len(common), len(parts))
            i = 0
            while i < limit and common[i] == parts[i]:
                i += 1
            common = common[:i]
            if not common:
                break
        return "/".join(common)

    def _generic_area_for_file(self, path: str, source_root: str) -> str:
        normalized = path.replace("\\", "/")
        parts = normalized.split("/")
        if parts[0] in _AUX_AREA_ROOTS:
            return parts[0]
        if source_root and (normalized == source_root or normalized.startswith(f"{source_root}/")):
            rest = normalized[len(source_root):].lstrip("/").split("/")
            if len(rest) >= 2:
                return f"{source_root}/{rest[0]}"
            return source_root or "root"
        if len(parts) >= 2:
            return parts[0]
        return "root"

    def _module_suffix_index(self, py_files: List[str]) -> Dict[str, str]:
        """Map every dotted suffix of each module's path to its file, so an import
        statement's module string resolves to a repo file. Ambiguous suffixes (shared
        by two files) are dropped to avoid mislinking. Packages (``__init__.py``) are
        also indexed under their package dotted path."""
        index: Dict[str, str] = {}
        ambiguous: Set[str] = set()

        def _register(dotted: str, file: str) -> None:
            comps = [c for c in dotted.split(".") if c]
            for i in range(len(comps)):
                suffix = ".".join(comps[i:])
                if not suffix:
                    continue
                if suffix in index and index[suffix] != file:
                    ambiguous.add(suffix)
                else:
                    index[suffix] = file

        for f in py_files:
            module_path = f[:-3] if f.endswith(".py") else f
            if module_path.endswith("/__init__"):
                # Package import resolves to the __init__ file under the dir's name.
                _register(module_path[: -len("/__init__")].replace("/", "."), f)
            else:
                _register(module_path.replace("/", "."), f)
        for suffix in ambiguous:
            index.pop(suffix, None)
        return index

    def _resolve_module(self, module: str, index: Dict[str, str]) -> str | None:
        comps = [c for c in module.split(".") if c]
        # An absolute import of a stdlib module is never a repo file — don't let a
        # repo file whose stem happens to equal a stdlib name (e.g. a local json.py)
        # create a false edge for `import json`.
        if comps and comps[0] in sys.stdlib_module_names:
            return None
        while comps:
            candidate = ".".join(comps)
            if candidate in index:
                return index[candidate]
            comps = comps[:-1]  # `from pkg.mod import name` -> try pkg.mod, then pkg
        return None

    # Version tag for the on-disk parse cache; bump when the cached payload changes
    # shape so stale caches are discarded wholesale instead of misread.
    _PARSE_CACHE_VERSION = 1

    def _parse_cache_path(self) -> Path:
        # Same .devcouncil/cache/ layout the LLM response cache uses.
        return self.project_root / ".devcouncil" / "cache" / "repo_map_parse.json"

    def _load_parse_cache(self) -> Dict[str, Dict[str, object]]:
        """Load the sha256-keyed import-extraction cache. Best-effort: any missing,
        corrupt, or version-mismatched cache just means a full re-parse."""
        try:
            data = read_json(self._parse_cache_path())
            if data.get("version") == self._PARSE_CACHE_VERSION and isinstance(data.get("files"), dict):
                return cast(Dict[str, Dict[str, object]], data["files"])
        except Exception:
            pass
        return {}

    def _save_parse_cache(self, files: Dict[str, Dict[str, object]]) -> None:
        """Persist the import-extraction cache. Best-effort: a failed write only
        costs the next run a re-parse, so it never raises."""
        try:
            path = self._parse_cache_path()
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(
                json.dumps({"version": self._PARSE_CACHE_VERSION, "files": files}),
                encoding="utf-8",
            )
        except Exception:
            logger.debug("Failed to write repo-map parse cache", exc_info=True)

    def _extract_python_import_modules(self, rel: str, source: str) -> List[str]:
        """Raw module strings referenced by ``rel``'s import statements, with relative
        imports expanded against the file's package path. A pure function of the file's
        path + content — which is what makes the result safe to cache by sha256."""
        tree = ast.parse(source)
        pkg_parts = rel[:-3].replace("/", ".").split(".")  # importer's dotted path
        modules: List[str] = []
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                modules.extend(alias.name for alias in node.names)
            elif isinstance(node, ast.ImportFrom):
                if node.level:
                    base = pkg_parts[: -node.level] if node.level <= len(pkg_parts) else []
                    base_mod = ".".join(base + ([node.module] if node.module else []))
                else:
                    base_mod = node.module or ""
                if base_mod:
                    modules.append(base_mod)
                # `from pkg import sub` / `from . import sub` may import a SUBMODULE
                # file, not just a symbol — resolve each name as a candidate module so
                # those edges aren't silently dropped.
                for alias in node.names:
                    if alias.name and alias.name != "*":
                        modules.append(f"{base_mod}.{alias.name}" if base_mod else alias.name)
        return modules

    def _python_import_edges(self, files: List[str]) -> List[Tuple[str, str]]:
        """Resolve Python import statements into (importer, imported) file edges.

        Extraction (ast.parse + walk) is cached in .devcouncil/cache/repo_map_parse.json
        keyed by each file's sha256, so unchanged files skip re-parsing across runs.
        Resolution against the module index always runs fresh — it depends on the
        CURRENT file set, not on any single file's content."""
        py_files = [f for f in self._code_files(files) if f.endswith(".py")]
        if not py_files:
            return []
        index = self._module_suffix_index(py_files)
        cache = self._load_parse_cache()
        fresh: Dict[str, Dict[str, object]] = {}
        edges: List[Tuple[str, str]] = []
        seen: Set[Tuple[str, str]] = set()  # dedupe so in-degree isn't inflated by repeats
        for rel in py_files:
            try:
                raw = (self.project_root / rel).read_bytes()
            except OSError:
                continue
            digest = hashlib.sha256(raw).hexdigest()
            entry = cache.get(rel)
            cached_modules = entry.get("modules") if isinstance(entry, dict) and entry.get("sha256") == digest else None
            if isinstance(cached_modules, list):
                modules = [m for m in cached_modules if isinstance(m, str)]
            else:
                try:
                    modules = self._extract_python_import_modules(
                        rel, raw.decode("utf-8", errors="replace")
                    )
                except (SyntaxError, ValueError):
                    # Unparseable content contributes no edges; cache the empty result
                    # so the same broken content isn't re-parsed every run.
                    modules = []
            fresh[rel] = {"sha256": digest, "modules": modules}
            for module in modules:
                target = self._resolve_module(module, index)
                if target and target != rel and (rel, target) not in seen:
                    seen.add((rel, target))
                    edges.append((rel, target))
        # Rewriting only on change keeps repeat runs read-only; building `fresh` from
        # scratch also prunes entries for files that no longer exist.
        if fresh != cache:
            self._save_parse_cache(fresh)
        return edges

    # Module specifiers in import/require statements: import ... from "x"; require("x");
    # export ... from "x"; dynamic import("x"). Best-effort; only relative specs resolve.
    _JS_IMPORT_RE = re.compile(
        r"""(?:import|export)\s[^'"]*?from\s*['"](?P<spec>[^'"]+)['"]"""
        r"""|(?:require|import)\s*\(\s*['"](?P<spec2>[^'"]+)['"]\s*\)"""
    )
    _JS_BARE_IMPORT_RE = re.compile(r"""^\s*import\s*['"](?P<spec>[^'"]+)['"]""")
    _JS_RESOLVE_EXTS = (".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs")
    _GO_IMPORT_BLOCK_RE = re.compile(r"import\s*\((?P<body>[^)]*)\)", re.DOTALL)
    _GO_IMPORT_SINGLE_RE = re.compile(r"""^\s*import\s+(?:[A-Za-z_.]\w*\s+)?['"](?P<spec>[^'"]+)['"]""")
    _GO_IMPORT_SPEC_RE = re.compile(r"""['"](?P<spec>[^'"]+)['"]""")
    _GO_MODULE_RE = re.compile(r"^\s*module\s+(?P<mod>\S+)", re.MULTILINE)

    def _resolve_js_spec(self, importer: str, spec: str, file_set: Set[str]) -> str | None:
        """Resolve a relative TS/JS import specifier (``./x`` / ``../y``) to a repo file.
        Bare specifiers (node_modules packages) are intentionally not resolved."""
        if not spec.startswith("."):
            return None
        base = Path(importer).parent
        try:
            target = (base / spec).as_posix()
        except Exception:
            return None
        # Normalize away any ".." segments without touching the filesystem.
        parts: List[str] = []
        for comp in target.split("/"):
            if comp in ("", "."):
                continue
            if comp == "..":
                if parts:
                    parts.pop()
                continue
            parts.append(comp)
        norm = "/".join(parts)
        if not norm:
            return None
        candidates = [norm]
        candidates += [f"{norm}{ext}" for ext in self._JS_RESOLVE_EXTS]
        candidates += [f"{norm}/index{ext}" for ext in self._JS_RESOLVE_EXTS]
        for cand in candidates:
            if cand in file_set:
                return cand
        return None

    def _js_import_edges(self, files: List[str], file_set: Set[str]) -> List[Tuple[str, str]]:
        """Resolve TS/JS relative import/require/export-from edges to repo files."""
        js_files = [f for f in files if Path(f).suffix.lower() in {".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"}]
        edges: List[Tuple[str, str]] = []
        seen: Set[Tuple[str, str]] = set()
        for rel in js_files:
            try:
                source = (self.project_root / rel).read_text(encoding="utf-8", errors="replace")
            except OSError:
                continue
            specs: List[str] = []
            for m in self._JS_IMPORT_RE.finditer(source):
                spec = m.group("spec") or m.group("spec2")
                if spec:
                    specs.append(spec)
            for line in source.splitlines():
                bare = self._JS_BARE_IMPORT_RE.match(line)
                if bare:
                    specs.append(bare.group("spec"))
            for spec in specs:
                target = self._resolve_js_spec(rel, spec, file_set)
                if target and target != rel and (rel, target) not in seen:
                    seen.add((rel, target))
                    edges.append((rel, target))
        return edges

    def _go_module_prefix(self, file_set: Set[str]) -> str | None:
        """The module path declared in go.mod, used to map import paths back to repo dirs."""
        for candidate in (p for p in file_set if p == "go.mod" or p.endswith("/go.mod")):
            try:
                text = (self.project_root / candidate).read_text(encoding="utf-8", errors="replace")
            except OSError:
                continue
            m = self._GO_MODULE_RE.search(text)
            if m:
                return m.group("mod").strip()
        return None

    def _go_import_edges(self, files: List[str], file_set: Set[str]) -> List[Tuple[str, str]]:
        """Resolve Go import paths (under the module prefix) to package directories, then
        to one representative .go file per package (main.go preferred)."""
        module = self._go_module_prefix(file_set)
        if not module:
            return []
        go_files = [f for f in files if f.endswith(".go")]
        if not go_files:
            return []
        # package dir -> .go files in it (excluding _test.go, which aren't imported).
        pkg_files: Dict[str, List[str]] = defaultdict(list)
        for f in go_files:
            if f.endswith("_test.go"):
                continue
            pkg_files[Path(f).parent.as_posix()].append(f)

        def _representative_file(pkg_paths: List[str]) -> str:
            main_go = [p for p in pkg_paths if Path(p).name == "main.go"]
            if main_go:
                return sorted(main_go)[0]
            return min(pkg_paths, key=lambda p: (len(p), p))

        pkg_representative = {
            pkg: _representative_file(paths) for pkg, paths in pkg_files.items()
        }
        edges: List[Tuple[str, str]] = []
        seen: Set[Tuple[str, str]] = set()
        for rel in go_files:
            try:
                source = (self.project_root / rel).read_text(encoding="utf-8", errors="replace")
            except OSError:
                continue
            specs: List[str] = []
            for block in self._GO_IMPORT_BLOCK_RE.finditer(source):
                for sm in self._GO_IMPORT_SPEC_RE.finditer(block.group("body")):
                    specs.append(sm.group("spec"))
            for line in source.splitlines():
                single = self._GO_IMPORT_SINGLE_RE.match(line)
                if single:
                    specs.append(single.group("spec"))
            for spec in specs:
                if spec != module and not spec.startswith(module + "/"):
                    continue  # external/stdlib package
                rel_pkg = spec[len(module):].lstrip("/")
                target_dir = rel_pkg if rel_pkg else "."
                target = pkg_representative.get(target_dir)
                if target and target != rel and (rel, target) not in seen:
                    seen.add((rel, target))
                    edges.append((rel, target))
        return edges

    def _all_import_edges(self, files: List[str]) -> List[Tuple[str, str]]:
        """All cross-file import edges across supported languages, feeding the same
        dependents reverse index. Best-effort; never raises."""
        file_set = set(files)
        edges = list(self._python_import_edges(files))
        try:
            edges.extend(self._js_import_edges(files, file_set))
        except Exception:
            logger.debug("JS/TS import-edge resolution failed", exc_info=True)
        try:
            edges.extend(self._go_import_edges(files, file_set))
        except Exception:
            logger.debug("Go import-edge resolution failed", exc_info=True)
        return edges

    def _rank_area_files(self, area_files: List[str], in_degree: Counter) -> List[str]:
        def sort_key(path: str) -> Tuple[int, int, str]:
            name = Path(path).stem.lower()
            entry_rank = next((i for i, hint in enumerate(_ENTRY_NAME_HINTS) if name == hint), len(_ENTRY_NAME_HINTS))
            # Most-imported first, then entry-named, then alphabetical for stability.
            return (-in_degree.get(path, 0), entry_rank, path)

        return sorted(area_files, key=sort_key)

    def _build_generic_subsystems(self, files: List[str]) -> List[RepoSubsystem]:
        source_root = self._source_root if self._source_root is not None else self.detect_source_root(files)
        code_files = self._code_files(files)
        if not code_files:
            return []

        by_area: Dict[str, List[str]] = defaultdict(list)
        area_of: Dict[str, str] = {}
        for f in code_files:
            area = self._generic_area_for_file(f, source_root)
            by_area[area].append(f)
            area_of[f] = area

        edges = self._edges if self._edges is not None else self._python_import_edges(files)
        in_degree: Counter = Counter(target for _, target in edges)
        area_neighbors: Dict[str, Set[str]] = defaultdict(set)
        area_handoffs: Dict[str, List[str]] = defaultdict(list)
        for importer, imported in edges:
            a, b = area_of.get(importer), area_of.get(imported)
            if a and b and a != b:
                area_neighbors[a].add(b)
                if len(area_handoffs[a]) < 3:
                    area_handoffs[a].append(f"{importer} -> {imported}")

        subsystems: List[RepoSubsystem] = []
        for area in sorted(by_area):
            area_files = by_area[area]
            # Skip trivial single-file aux areas (e.g. a lone script) to reduce noise,
            # but keep every real source subsystem.
            if len(area_files) < 2 and area.split("/")[0] in _AUX_AREA_ROOTS:
                continue
            ranked = self._rank_area_files(area_files, in_degree)
            critical_files = ranked[: self._SUBSYSTEM_CRITICAL_MAX]
            entry_points = [p for p in critical_files if in_degree.get(p, 0) > 0][:3] or critical_files[:1]
            stems = ", ".join(Path(p).stem for p in critical_files[:3])
            summary = f"{Path(area).name or area}: {stems}" if stems else f"{area} ({len(area_files)} files)"
            subsystems.append(
                RepoSubsystem(
                    area=area,
                    summary=summary,
                    entry_points=entry_points,
                    critical_files=critical_files,
                    neighbors=sorted(area_neighbors.get(area, set()))[:6],
                    handoff_paths=area_handoffs.get(area, []),
                    role_files={},
                )
            )
        return subsystems

    def generic_important_files(self, files: List[str]) -> List[str]:
        """The most-depended-on source files across the repo (highest import in-degree),
        used to seed 'important surfaces' on repos without a curated index."""
        edges = self._edges if self._edges is not None else self._python_import_edges(files)
        if not edges:
            return []
        in_degree = Counter(target for _, target in edges)
        ranked = [path for path, _ in in_degree.most_common()]
        return ranked[:8]

    def build_dependents(self, edges: List[Tuple[str, str]]) -> Dict[str, List[str]]:
        """Reverse the import edges into a file -> dependents map (who imports each file),
        capped per file. This is the blast radius an agent needs before changing a file."""
        reverse: Dict[str, Set[str]] = defaultdict(set)
        for importer, imported in edges:
            reverse[imported].add(importer)
        return {
            path: sorted(importers)[: self._DEPENDENTS_MAX]
            for path, importers in sorted(reverse.items())
            if importers
        }

    def describe_file(self, path: str) -> RepoFileEntry:
        return RepoFileEntry(
            path=path,
            area=self._area_for_file(path),
            kind=self._kind_for_file(path),
            language=self._language_for_file(path),
            summary=self._summary_for_file(path),
        )

    def _is_runtime_or_generated_file(self, path: str) -> bool:
        normalized = path.replace("\\", "/")
        parts = set(normalized.split("/"))
        name = Path(normalized).name
        if "__pycache__" in parts or normalized.endswith(".pyc"):
            return True
        if parts.intersection({".git", ".devcouncil", ".pytest_cache", ".ruff_cache", ".mypy_cache", ".venv"}):
            return True
        if normalized.startswith("dist/") or normalized.startswith("build/"):
            return True
        if name.startswith(("tmp", "temp", ".tmp", "debug")) or name.endswith("~"):
            return True
        return False

    def _git_head(self) -> str:
        from devcouncil.utils.proc import git_output

        return git_output(["rev-parse", "HEAD"], cwd=self.project_root, default="").strip()

    def _files_fingerprint(self, files: List[str]) -> str:
        return hashlib.sha1("\n".join(sorted(files)).encode("utf-8")).hexdigest()

    def map_is_stale(self, repo_map: Dict[str, object]) -> bool:
        """True when the stored map no longer matches the repo's current git HEAD or
        tracked file set — i.e. commits or file add/removes happened since ``dev map``
        last ran. Returns False for maps written before fingerprinting (no false alarms)."""
        stored_head = str(repo_map.get("generated_head") or "")
        stored_hash = str(repo_map.get("indexed_hash") or "")
        if not stored_head and not stored_hash:
            return False
        try:
            files = self.get_git_files()
        except Exception:
            return False
        return self._git_head() != stored_head or self._files_fingerprint(files) != stored_hash

    def get_git_files(self) -> List[str]:
        try:
            from devcouncil.utils.proc import git_output

            output = git_output(
                ["ls-files", "--cached", "--others", "--exclude-standard"],
                cwd=self.project_root,
            ).splitlines()
            return [path for path in output if not self._is_runtime_or_generated_file(path)]
        except Exception:
            # Fallback to os.walk if not a git repo or git missing
            from devcouncil.indexing.walk import IGNORED_DIR_NAMES

            files = []
            for root, dirnames, filenames in os.walk(self.project_root):
                dirnames[:] = [name for name in dirnames if name not in IGNORED_DIR_NAMES]
                for f in filenames:
                    rel_path = os.path.relpath(os.path.join(root, f), self.project_root)
                    if not rel_path.startswith(".") and not self._is_runtime_or_generated_file(rel_path):
                        files.append(rel_path)
            return files

    def detect_languages(self, files: List[str]) -> List[str]:
        exts = {os.path.splitext(f)[1] for f in files}
        lang_map = {
            ".py": "python",
            ".ts": "typescript",
            ".tsx": "typescript",
            ".js": "javascript",
            ".jsx": "javascript",
            ".go": "go",
            ".rs": "rust",
            ".java": "java",
            ".c": "c",
            ".cpp": "cpp",
        }
        return sorted(list({lang_map[ext] for ext in exts if ext in lang_map}))

    def detect_frameworks(self, files: List[str]) -> List[str]:
        frameworks = []
        file_set = set(files)
        if "package.json" in file_set:
            content = self._read_config_file("package.json")
            if "next" in content:
                frameworks.append("nextjs")
            if "react" in content:
                frameworks.append("react")
            if "vue" in content:
                frameworks.append("vue")
            if "express" in content:
                frameworks.append("express")
        
        if "requirements.txt" in file_set or "pyproject.toml" in file_set:
            try:
                parts: List[str] = []
                if "requirements.txt" in file_set:
                    parts.append(self._read_config_file("requirements.txt"))
                if "pyproject.toml" in file_set:
                    parts.append(self._read_config_file("pyproject.toml"))
                content = "".join(parts)

                if "fastapi" in content.lower():
                    frameworks.append("fastapi")
                if "flask" in content.lower():
                    frameworks.append("flask")
                if "django" in content.lower():
                    frameworks.append("django")
            except Exception as e:
                logger.debug("Failed to read Python config files: %s", e)
        return frameworks

    def detect_package_managers(self, files: List[str]) -> List[str]:
        managers = []
        file_set = set(files)
        if "package-lock.json" in file_set:
            managers.append("npm")
        elif "package.json" in file_set:
            managers.append("npm")
        if "yarn.lock" in file_set:
            managers.append("yarn")
        if "pnpm-lock.yaml" in file_set:
            managers.append("pnpm")
        if "requirements.txt" in file_set:
            managers.append("pip")
        if "uv.lock" in file_set:
            managers.append("uv")
        if "go.sum" in file_set:
            managers.append("go mod")
        return managers

    def detect_test_commands(self, files: List[str]) -> List[str]:
        """Detect test, lint, and typecheck commands from project config."""
        commands: List[str] = []
        file_set = set(files)

        # Node.js projects: read scripts from package.json
        if "package.json" in file_set:
            try:
                pkg = json.loads(self._read_config_file("package.json"))
                scripts = pkg.get("scripts", {})
                pm = "pnpm" if "pnpm-lock.yaml" in file_set else (
                    "yarn" if "yarn.lock" in file_set else "npm"
                )
                for key in ["test", "lint", "typecheck", "check", "type-check"]:
                    if key in scripts:
                        if pm == "npm" and key != "test":
                            commands.append(f"npm run {key}")
                        else:
                            commands.append(f"{pm} {key}")
            except Exception as e:
                logger.debug("Failed to parse package.json scripts: %s", e)

        # Python projects
        if "pyproject.toml" in file_set or "setup.py" in file_set:
            if any(f.startswith("tests/") or f.startswith("test_") for f in files):
                commands.append("pytest")
            commands.append("ruff check .")
            commands.append("mypy .")

        # Go projects
        if "go.mod" in file_set:
            commands.append("go test ./...")
            commands.append("go vet ./...")

        # Rust projects
        if "Cargo.toml" in file_set:
            commands.append("cargo test")
            commands.append("cargo clippy")

        return commands

    def _ripgrep_search(self, goal: str, files: List[str]) -> List[Dict[str, str]]:
        """Use ripgrep for goal-keyword search if available, else fall back to naive matching."""
        candidates: List[Dict[str, str]] = []
        try:
            # Try ripgrep first for better matching
            result = subprocess.run(
                ["rg", "--files-with-matches", "--ignore-case", "--glob", "!.git", goal],
                capture_output=True, text=True, cwd=self.project_root, timeout=10,
            )
            if result.returncode == 0:
                for line in sorted(result.stdout.strip().splitlines())[:10]:
                    candidates.append({"path": line.strip(), "reason": f"ripgrep match for '{goal}'"})
                return candidates
        except Exception:
            logger.debug("ripgrep search failed; falling back to naive matching", exc_info=True)
            pass  # Fall back to naive matching

        # Naive keyword matching fallback
        goal_words = set(goal.lower().split())
        scored_candidates: list[tuple[int, str]] = []
        for f in files:
            f_lower = f.lower()
            score = sum(1 for word in goal_words if word in f_lower)
            if score > 0:
                scored_candidates.append((score, f))
        candidates = [
            {"path": path, "reason": f"Matches goal keywords (score: {score})"}
            for score, path in sorted(scored_candidates, key=lambda item: (item[0], item[1]), reverse=True)
        ][:10]
        return candidates

    def _scan_dependency_risks(self) -> List[Dict[str, str]]:
        """Best-effort SCA scan; isolated so map_repo stays simple and never raises."""
        try:
            from devcouncil.repo.sca import scan_dependency_risks

            return scan_dependency_risks(self.project_root)
        except Exception:
            logger.debug("Dependency-risk scan failed", exc_info=True)
            return []

    def map_repo(self, goal: str = "", *, scan_dependencies: bool = False) -> RepoMap:
        """Build the repo map.

        ``scan_dependencies`` is opt-in (default off) so the common ``dev map`` path
        stays fast and never shells out to a vulnerability auditor. When enabled, a
        best-effort SCA scan runs locally (only if an auditor is installed) and its
        findings are attached as ``dependency_risks``.
        """
        files = self.get_git_files()
        # Decide DevCouncil-vs-generic and the source root BEFORE describing files, so
        # area bucketing and subsystem inference agree within a single run.
        self._use_generic = not any(path.startswith("src/devcouncil/") for path in files)
        self._source_root = self.detect_source_root(files)
        # Compute the import graph once; reused by subsystem inference, important-file
        # ranking, and the dependents (blast-radius) index. Spans Python, TS/JS, and Go
        # so non-Python repos get dependents/neighbors too.
        self._edges = self._all_import_edges(files)
        file_entries = [self.describe_file(path) for path in sorted(files)]
        file_set = set(files)

        important_candidates = [
            "README.md",
            "AGENTS.md",
            "CLAUDE.md",
            "package.json",
            "pyproject.toml",
            "src/devcouncil/cli/main.py",
            "src/devcouncil/app/orchestrator.py",
            "src/devcouncil/app/state_machine.py",
            "src/devcouncil/artifacts/graph.py",
            "src/devcouncil/indexing/repo_mapper.py",
            "src/devcouncil/storage/repositories.py",
            "src/devcouncil/execution/task_runner.py",
            "src/devcouncil/verification/verifier.py",
        ]
        important_files = [path for path in important_candidates if path in file_set]
        important_files.extend(sorted(path for path in files if path.startswith(".github/workflows/")))
        # On non-DevCouncil repos the curated candidates above mostly miss, so seed
        # important surfaces from the most-depended-on source files.
        if self._use_generic:
            for path in self.generic_important_files(files):
                if path not in important_files:
                    important_files.append(path)

        candidates: List[Dict[str, str]] = []
        if goal:
            candidates = self._ripgrep_search(goal, files)

        return RepoMap(
            languages=self.detect_languages(files),
            frameworks=self.detect_frameworks(files),
            package_managers=self.detect_package_managers(files),
            test_commands=self.detect_test_commands(files),
            important_files=important_files,
            candidate_files=candidates,
            files=file_entries,
            subsystems=self._build_subsystem_index(files),
            dependents=self.build_dependents(self._edges or []),
            generated_head=self._git_head(),
            indexed_hash=self._files_fingerprint(files),
            lsp=LspInspector(self.project_root).summary(files),
            dependency_risks=self._scan_dependency_risks() if scan_dependencies else [],
        )
