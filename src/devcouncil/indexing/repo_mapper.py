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
from typing import Dict, List, Optional, Set, Tuple, cast

from pydantic import BaseModel, Field

from devcouncil.indexing.graph.cache import PARSE_CACHE_VERSION
from devcouncil.indexing.graph.schema import CodeGraph
from devcouncil.indexing.lsp import LspInspector

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
    # Full importer counts when ``dependents[path]`` was truncated by ``_DEPENDENTS_MAX``.
    # Absent / empty when no path was truncated. Agents must treat listed dependents as a
    # sample whenever ``dependents_total[path] > len(dependents[path])``.
    dependents_total: Dict[str, int] = Field(default_factory=dict)
    # Freshness fingerprints captured at generation: the git HEAD the map was built from
    # and a hash of the tracked file set. Consumers compare against the current repo to
    # detect a stale map before trusting its structure.
    generated_head: str = ""
    indexed_hash: str = ""
    # sha1 over sorted (path, size, mtime_ns) so plain content edits mark the map stale.
    # Legacy maps without this field stay non-stale (no false alarms).
    content_fingerprint: str = ""
    # True when the last map write used lean/degraded graph fallback. Consumers must
    # treat this as stale until a successful graph-backed refresh clears it.
    graph_degraded: bool = False
    graph_degraded_reason: str = ""
    lsp: Dict[str, object] = Field(default_factory=dict)
    # Optional dependency-vulnerability findings. Populated only when `dev map` is
    # run with SCA explicitly enabled (off by default so the map stays fast and
    # offline-by-default); empty otherwise.
    dependency_risks: List[Dict[str, str]] = Field(default_factory=list)
    # Liveness artifact (computed by default; omit with --no-liveness).
    entry_roots: List[str] = Field(default_factory=list)
    unwired_candidates: List[str] = Field(default_factory=list)
    unreachable_files: List[str] = Field(default_factory=list)
    dead_symbol_candidates: List[str] = Field(default_factory=list)
    # True when production entry roots were empty — unreachable BFS was skipped
    # (fail-soft) so unreachable_files is not meaningful.
    liveness_unreachable_unreliable: bool = False
    liveness_meta: Dict[str, object] = Field(default_factory=dict)
    # Top call-flow processes from the code graph (entry-root BFS), when available.
    processes: List[Dict[str, object]] = Field(default_factory=list)

class RepoMapper:
    def __init__(self, project_root: Path | None = None):
        self.project_root = project_root or Path.cwd()
        self._DEPENDENTS_MAX = type(self)._DEPENDENTS_MAX
        self._LIVENESS_CAP = type(self)._LIVENESS_CAP
        self._js_alias_cache: Optional[List[Tuple[str, List[str]]]] = None
        try:
            from devcouncil.app.config import load_config

            indexing = load_config(self.project_root).indexing
            self._DEPENDENTS_MAX = min(
                int(indexing.repo_map_dependents_cap), self.max_dependents_per_file
            )
            self._LIVENESS_CAP = min(
                int(indexing.repo_map_liveness_cap), self.max_map_size
            )
        except Exception:
            logger.debug("using default repository-map caps", exc_info=True)
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
        self._last_code_graph: CodeGraph | None = None
        # Cache of config-file contents (package.json, pyproject.toml, ...) so framework
        # and test-command detection don't each re-read the same files from disk.
        self._config_file_cache: Dict[str, str] = {}

    def _read_config_file(self, name: str) -> str:
        """Read a repo-root config file once and cache its contents for reuse.

        Unreadable or non-UTF-8 config files must not fail the whole map.
        """
        if name not in self._config_file_cache:
            try:
                self._config_file_cache[name] = (self.project_root / name).read_text(
                    encoding="utf-8", errors="replace"
                )
            except OSError:
                self._config_file_cache[name] = ""
        return self._config_file_cache[name]

    _DEPENDENTS_MAX = 1_024  # serialized dependents per file
    _LIVENESS_CAP = 20_000  # serialized entries per liveness debt list
    max_dependents_per_file = 4_096
    max_map_size = 100_000  # hard safety ceiling for each liveness debt list

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
        "src/devcouncil/campaign": "Multi-agent campaign orchestration and roles",
        "src/devcouncil/knowledge": "Knowledge sources, OKF, and design docs",
        "src/devcouncil/skills": "Skill registry and matching",
        "src/devcouncil/optimization": "Prompt and skill optimization",
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
            "Repo mapping, AST matching, semantic snapshots, and optional live LSP client.",
            [
                "src/devcouncil/indexing/repo_mapper.py",
                "src/devcouncil/indexing/wiring.py",
                "src/devcouncil/indexing/ast_matcher.py",
                "src/devcouncil/indexing/semantic_index.py",
                "src/devcouncil/indexing/lsp.py",
                "src/devcouncil/indexing/lsp_client.py",
            ],
        ),
        "src/devcouncil/integrations": (
            "External system integrations and MCP/Graph adapters.",
            [
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
        "src/devcouncil/campaign": (
            "Multi-agent campaign orchestration, roles, mailbox, and watchers.",
            [
                "src/devcouncil/campaign/orchestrator.py",
                "src/devcouncil/campaign/roles.py",
                "src/devcouncil/campaign/mailbox.py",
                "src/devcouncil/campaign/watcher.py",
            ],
        ),
        "src/devcouncil/knowledge": (
            "Knowledge sources, OKF bundles, design docs, and skill bridging.",
            [
                "src/devcouncil/knowledge/sources.py",
                "src/devcouncil/knowledge/okf.py",
                "src/devcouncil/knowledge/knowledge_select.py",
                "src/devcouncil/knowledge/design.py",
            ],
        ),
        "src/devcouncil/skills": (
            "Skill registry and matching for agent capability selection.",
            [
                "src/devcouncil/skills/registry.py",
            ],
        ),
        "src/devcouncil/optimization": (
            "Prompt/skill optimization loops (GEPA, SkillOpt).",
            [
                "src/devcouncil/optimization/gepa_agent.py",
                "src/devcouncil/optimization/skillopt.py",
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
        "src/devcouncil/campaign": [
            "src/devcouncil/cli",
            "src/devcouncil/execution",
            "src/devcouncil/llm",
            "src/devcouncil/skills",
        ],
        "src/devcouncil/knowledge": [
            "src/devcouncil/indexing",
            "src/devcouncil/skills",
            "src/devcouncil/cli",
        ],
        "src/devcouncil/skills": [
            "src/devcouncil/knowledge",
            "src/devcouncil/campaign",
            "src/devcouncil/execution",
        ],
        "src/devcouncil/optimization": [
            "src/devcouncil/skills",
            "src/devcouncil/llm",
            "src/devcouncil/campaign",
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
        "src/devcouncil/campaign": [
            "campaign/orchestrator.py -> campaign/mailbox.py",
            "campaign/orchestrator.py -> execution/task_runner.py",
            "campaign/roles.py -> skills/registry.py",
        ],
        "src/devcouncil/knowledge": [
            "knowledge/okf.py -> indexing/graph/export.py",
            "knowledge/sources.py -> skills/registry.py",
            "knowledge/knowledge_select.py -> execution/prompt_builder.py",
        ],
        "src/devcouncil/skills": [
            "skills/registry.py -> knowledge/sources.py",
            "skills/registry.py -> campaign/roles.py",
        ],
        "src/devcouncil/optimization": [
            "optimization/skillopt.py -> skills/registry.py",
            "optimization/gepa_agent.py -> llm/router.py",
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
            # Detection + optional live client (lsp_client.py); off by default.
            ("lsp", ["indexing/lsp.py", "indexing/lsp_client.py"]),
            # graph_index.py is a deprecated artifact-graph shim; live graph is indexing/graph/.
            ("graph", ["indexing/graph_index.py", "indexing/graph/"]),
        ],
        "src/devcouncil/integrations": [
            ("vcs", ["integrations/github.py"]),
            ("code_review", ["integrations/code_review_graph.py"]),
            ("comments", ["integrations/pr_comments.py"]),
            ("mcp", ["integrations/mcp/server.py"]),
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
            ("coverage", ["artifacts/coverage.py", "artifacts/validators.py"]),
            ("schema", ["artifacts/graph.py", "artifacts/coverage.py"]),
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
        "src/devcouncil/campaign": [
            ("orchestration", ["campaign/orchestrator.py"]),
            ("roles", ["campaign/roles.py"]),
            ("mailbox", ["campaign/mailbox.py"]),
            ("watch", ["campaign/watcher.py", "campaign/notify.py"]),
        ],
        "src/devcouncil/knowledge": [
            ("sources", ["knowledge/sources.py"]),
            ("okf", ["knowledge/okf.py"]),
            ("design", ["knowledge/design.py", "knowledge/design_conformance.py"]),
            ("select", ["knowledge/knowledge_select.py", "knowledge/skill_bridge.py"]),
        ],
        "src/devcouncil/skills": [
            ("registry", ["skills/registry.py"]),
        ],
        "src/devcouncil/optimization": [
            ("gepa", ["optimization/gepa_agent.py"]),
            ("skillopt", ["optimization/skillopt.py"]),
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
        "coding-cli-integration.md": "Coding CLI tiers, hooks, and stop gate",
        "hero-loop.md": "Certified Claude Code MCP closed loop",
        "code-graph.md": "Repo map and symbol code graph",
        "corpus.md": "Corpus side index and verify gates",
        "model-routing.md": "Provider and model routing setup",
        "quickstart.md": "First-run installation and workflow",
        "workflow.md": "Manual sidecar workflow guide",
        "security.md": "Security and privacy model",
        "project-status.md": "Subsystem maturity snapshot",
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

    def _ancestor_init_files(self, target: str, py_file_set: Set[str]) -> List[str]:
        """Existing ancestor package ``__init__.py`` files between ``target`` and repo root.

        Importing a submodule executes each ancestor package ``__init__``; emit those
        inbound edges so package inits are not falsely flagged unwired/unreachable.
        """
        parts = target.replace("\\", "/").split("/")
        if not parts:
            return []
        if parts[-1] == "__init__.py":
            dir_parts = parts[:-2]  # ancestors above this package
        else:
            dir_parts = parts[:-1]
        out: List[str] = []
        for i in range(len(dir_parts), 0, -1):
            init = "/".join(dir_parts[:i]) + "/__init__.py"
            if init in py_file_set and init != target:
                out.append(init)
        return out

    # Single owner: graph.cache.PARSE_CACHE_VERSION (v4 = + import_details).
    _PARSE_CACHE_VERSION = PARSE_CACHE_VERSION
    _JS_SUFFIXES = frozenset({".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"})

    def _parse_cache_path(self) -> Path:
        from devcouncil.indexing.graph.cache import cache_path

        return cache_path(self.project_root)

    def _load_parse_cache(self) -> Dict[str, Dict[str, object]]:
        """Delegate to graph.cache (single version + merge policy)."""
        from devcouncil.indexing.graph.cache import load_parse_cache

        return cast(Dict[str, Dict[str, object]], load_parse_cache(self.project_root))

    def _save_parse_cache(self, files: Dict[str, Dict[str, object]]) -> None:
        """Delegate to graph.cache."""
        from devcouncil.indexing.graph.cache import save_parse_cache

        save_parse_cache(self.project_root, cast(Dict[str, Dict[str, object]], files))

    @staticmethod
    def _is_js_source_path(path: str) -> bool:
        return Path(path).suffix.lower() in RepoMapper._JS_SUFFIXES

    def _merge_parse_cache(
        self,
        updates: Dict[str, Dict[str, object]],
        managed: Set[str],
    ) -> None:
        """Delegate to graph.cache merge (preserves sibling language/field keys)."""
        from devcouncil.indexing.graph.cache import merge_parse_cache

        merge_parse_cache(
            self.project_root,
            cast(Dict[str, Dict[str, object]], updates),
            managed,
        )

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
        py_file_set = set(py_files)
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
                if not target:
                    continue
                targets = [target, *self._ancestor_init_files(target, py_file_set)]
                for dest in targets:
                    if dest != rel and (rel, dest) not in seen:
                        seen.add((rel, dest))
                        edges.append((rel, dest))
        # Preserve JS/TS cache entries; prune deleted Python files from this pass.
        managed = {k for k in cache if k.endswith(".py")} | set(py_files)
        self._merge_parse_cache(fresh, managed)
        return edges

    # Module specifiers in import/require statements: import ... from "x"; require("x");
    # export ... from "x"; dynamic import("x"). Best-effort; only relative specs resolve.
    _JS_IMPORT_RE = re.compile(
        r"""(?:import|export)\s[^'"]*?from\s*['"](?P<spec>[^'"]+)['"]"""
        r"""|(?:require|import)\s*\(\s*['"](?P<spec2>[^'"]+)['"]\s*\)"""
    )
    # Vite/webpack worker + import.meta.url asset loads (e.g. mupdf-worker.ts).
    _JS_WORKER_URL_RE = re.compile(
        r"""(?:new\s+(?:Worker|SharedWorker)\s*\(\s*)?"""
        r"""new\s+URL\s*\(\s*['"](?P<spec>[^'"]+)['"]\s*,\s*import\.meta\.url"""
    )
    _JS_BARE_IMPORT_RE = re.compile(r"""^\s*import\s*['"](?P<spec>[^'"]+)['"]""")
    _JS_RESOLVE_EXTS = (".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs")
    _GO_IMPORT_BLOCK_RE = re.compile(r"import\s*\((?P<body>[^)]*)\)", re.DOTALL)
    _GO_IMPORT_SINGLE_RE = re.compile(r"""^\s*import\s+(?:[A-Za-z_.]\w*\s+)?['"](?P<spec>[^'"]+)['"]""")
    _GO_IMPORT_SPEC_RE = re.compile(r"""['"](?P<spec>[^'"]+)['"]""")
    _GO_MODULE_RE = re.compile(r"^\s*module\s+(?P<mod>\S+)", re.MULTILINE)

    def _resolve_js_spec(self, importer: str, spec: str, file_set: Set[str]) -> str | None:
        """Resolve a TS/JS import specifier to a repo file.

        Handles relative specs (``./x`` / ``../y``) and tsconfig/jsconfig path
        aliases (``@/x``, ``~/x``, custom). Bare node_modules packages return None.
        """
        if spec.startswith("."):
            return self._resolve_js_relative(importer, spec, file_set)
        return self._resolve_js_alias(spec, file_set)

    def _normalize_js_path(self, target: str) -> str:
        parts: List[str] = []
        for comp in target.replace("\\", "/").split("/"):
            if comp in ("", "."):
                continue
            if comp == "..":
                if parts:
                    parts.pop()
                continue
            parts.append(comp)
        return "/".join(parts)

    def _normalize_js_alias_target(self, target: str) -> str:
        """Like ``_normalize_js_path`` but keeps leading ``../`` segments intact."""
        parts: List[str] = []
        leading_dots = 0
        for comp in target.replace("\\", "/").split("/"):
            if comp in ("", "."):
                continue
            if comp == "..":
                if parts:
                    parts.pop()
                else:
                    leading_dots += 1
                continue
            parts.append(comp)
        prefix = "/".join([".."] * leading_dots)
        rest = "/".join(parts)
        if prefix and rest:
            return f"{prefix}/{rest}"
        return prefix or rest

    def _probe_js_candidates(self, norm: str, file_set: Set[str]) -> str | None:
        """Resolve a normalized path against ``file_set``.

        Handles extensionless specs (``./auth`` → ``auth.ts``) and TypeScript
        ESM suffix rewriting (``./auth.js`` → ``auth.ts`` / ``auth.tsx``).
        """
        if not norm:
            return None
        candidates = [norm]
        candidates += [f"{norm}{ext}" for ext in self._JS_RESOLVE_EXTS]
        candidates += [f"{norm}/index{ext}" for ext in self._JS_RESOLVE_EXTS]
        # TS/Node ESM: import './foo.js' resolves to foo.ts / foo.tsx on disk.
        suffix = Path(norm).suffix.lower()
        if suffix in self._JS_SUFFIXES:
            stem = norm[: -len(suffix)]
            candidates += [f"{stem}{ext}" for ext in self._JS_RESOLVE_EXTS]
            candidates += [f"{stem}/index{ext}" for ext in self._JS_RESOLVE_EXTS]
        for cand in candidates:
            if cand in file_set:
                return cand
        return None

    def _resolve_js_relative(self, importer: str, spec: str, file_set: Set[str]) -> str | None:
        base = Path(importer).parent
        try:
            target = (base / spec).as_posix()
        except Exception:
            return None
        return self._probe_js_candidates(self._normalize_js_path(target), file_set)

    def _load_js_path_aliases(self) -> List[Tuple[str, List[str]]]:
        """Parse tsconfig/jsconfig path aliases across extends + project references.

        Merges ``compilerOptions.paths`` (and ``baseUrl``) from:
        - Root ``tsconfig.json`` / ``jsconfig.json`` / ``tsconfig.base.json``
        - Each config's ``extends`` chain (child overrides parent)
        - ``references[].path`` project configs (and their extends)

        Returns ``(pattern_prefix, [target_prefixes])`` sorted longest-key first.
        Cached per mapper instance. Never raises.
        """
        cached = getattr(self, "_js_alias_cache", None)
        if cached is not None:
            return cast(List[Tuple[str, List[str]]], cached)
        rules: List[Tuple[str, List[str]]] = []
        seen_configs: Set[str] = set()

        def _strip_json_comments(raw: str) -> str:
            raw = re.sub(r"/\*.*?\*/", "", raw, flags=re.DOTALL)
            return re.sub(r"(?m)^\s*//.*?$", "", raw)

        def _read_config(path: Path) -> dict | None:
            try:
                key = path.resolve().as_posix()
            except OSError:
                key = path.as_posix()
            if key in seen_configs:
                return None
            seen_configs.add(key)
            if not path.is_file():
                return None
            try:
                raw = _strip_json_comments(path.read_text(encoding="utf-8", errors="replace"))
                data = json.loads(raw)
            except Exception:
                return None
            return data if isinstance(data, dict) else None

        def _merge_extends(path: Path, data: dict) -> dict:
            """Return compilerOptions with parent ``extends`` merged under child."""
            extends = data.get("extends")
            parent_co: dict = {}
            if isinstance(extends, str) and extends:
                # Node-style: bare @scoped packages ignored; relative/absolute only.
                if extends.startswith("."):
                    parent_path = (path.parent / extends).resolve()
                    if not parent_path.suffix:
                        # extends may omit .json
                        for suffix in (".json", ""):
                            candidate = Path(str(parent_path) + suffix) if suffix else parent_path
                            if candidate.is_file():
                                parent_path = candidate
                                break
                    parent_data = _read_config(parent_path)
                    if parent_data:
                        parent_co = _merge_extends(parent_path, parent_data)
            child_co = data.get("compilerOptions") or {}
            if not isinstance(child_co, dict):
                child_co = {}
            merged = dict(parent_co)
            for k, v in child_co.items():
                if k == "paths" and isinstance(v, dict) and isinstance(merged.get("paths"), dict):
                    paths = dict(merged["paths"])
                    paths.update(v)
                    merged["paths"] = paths
                else:
                    merged[k] = v
            return merged

        def _rules_from_co(co: dict, config_dir: Path) -> None:
            if not isinstance(co, dict):
                return
            base_url = str(co.get("baseUrl") or ".").replace("\\", "/").rstrip("/")
            if base_url in (".", ""):
                base_url = ""
            # Paths are relative to the config file's directory (TS spec), then baseUrl.
            try:
                config_rel = config_dir.relative_to(self.project_root).as_posix()
            except ValueError:
                config_rel = ""
            if config_rel in (".", ""):
                config_prefix = ""
            else:
                config_prefix = config_rel

            paths = co.get("paths") or {}
            if not isinstance(paths, dict):
                return
            for key, targets in paths.items():
                if not isinstance(key, str):
                    continue
                target_list = targets if isinstance(targets, list) else [targets]
                resolved_targets: List[str] = []
                for t in target_list:
                    if not isinstance(t, str):
                        continue
                    t_norm = t.replace("\\", "/")
                    if t_norm.endswith("/*"):
                        t_norm = t_norm[:-2]
                    elif t_norm.endswith("*"):
                        t_norm = t_norm[:-1]
                    # Resolve relative to config dir + baseUrl. Preserve leading
                    # ``../`` (Phase 0: never lstrip/collapse away parent refs).
                    pieces: List[str] = []
                    if config_prefix:
                        pieces.append(config_prefix)
                    if base_url:
                        pieces.append(base_url)
                    if t_norm:
                        pieces.append(t_norm)
                    joined = "/".join(p for p in pieces if p)
                    joined = self._normalize_js_alias_target(joined)
                    while joined.startswith("./"):
                        joined = joined[2:]
                    resolved_targets.append(joined.rstrip("/"))
                if not resolved_targets:
                    continue
                pattern = key
                if pattern.endswith("/*"):
                    pattern = pattern[:-2]
                elif pattern.endswith("*"):
                    pattern = pattern[:-1]
                rules.append((pattern, resolved_targets))

        def _ingest(path: Path) -> None:
            data = _read_config(path)
            if not data:
                return
            co = _merge_extends(path, data)
            _rules_from_co(co, path.parent)
            refs = data.get("references") or []
            if isinstance(refs, list):
                for ref in refs:
                    if not isinstance(ref, dict):
                        continue
                    ref_path = ref.get("path")
                    if not isinstance(ref_path, str) or not ref_path:
                        continue
                    target = (path.parent / ref_path).resolve()
                    if target.is_dir():
                        for name in ("tsconfig.json", "jsconfig.json"):
                            candidate = target / name
                            if candidate.is_file():
                                _ingest(candidate)
                                break
                    elif target.is_file():
                        _ingest(target)
                    else:
                        # path may omit .json
                        for suffix in (".json", "/tsconfig.json"):
                            candidate = Path(str(target) + suffix) if suffix.startswith(".") else Path(str(target) + suffix)
                            if candidate.is_file():
                                _ingest(candidate)
                                break

        try:
            for name in ("tsconfig.json", "jsconfig.json", "tsconfig.base.json"):
                root_cfg = self.project_root / name
                if root_cfg.is_file():
                    _ingest(root_cfg)
            # Nested package tsconfigs (monorepos without root project references).
            # Caps keep large trees bounded; node_modules / dist are skipped.
            _TSCONFIG_NAMES = {
                "tsconfig.json",
                "jsconfig.json",
                "tsconfig.base.json",
                "tsconfig.app.json",
                "tsconfig.node.json",
            }
            file_set = getattr(self, "_last_file_set", None) or set()
            nested_manifests: List[str] = []
            if file_set:
                nested_manifests = sorted(
                    p
                    for p in file_set
                    if p.rsplit("/", 1)[-1] in _TSCONFIG_NAMES
                )[:200]
            # Always also walk the tree: git file lists often omit tsconfig.json,
            # which previously left monorepo ``apps/*/tsconfig.json`` undiscovered.
            try:
                for path in sorted(self.project_root.rglob("tsconfig*.json"))[:200]:
                    if any(
                        part in {"node_modules", "dist", "build", ".git", "target"}
                        for part in path.parts
                    ):
                        continue
                    try:
                        rel = path.relative_to(self.project_root).as_posix()
                    except ValueError:
                        continue
                    if rel not in nested_manifests:
                        nested_manifests.append(rel)
                for path in sorted(self.project_root.rglob("jsconfig.json"))[:50]:
                    if any(
                        part in {"node_modules", "dist", "build", ".git"}
                        for part in path.parts
                    ):
                        continue
                    try:
                        rel = path.relative_to(self.project_root).as_posix()
                    except ValueError:
                        continue
                    if rel not in nested_manifests:
                        nested_manifests.append(rel)
            except Exception:
                logger.debug("nested tsconfig walk failed", exc_info=True)
            nested_manifests = nested_manifests[:200]
            for rel in nested_manifests:
                _ingest(self.project_root / rel)
            # package.json name → source dir (best-effort monorepo/workspace).
            try:
                pkg_text = (self.project_root / "package.json").read_text(
                    encoding="utf-8", errors="replace"
                )
                pkg = json.loads(pkg_text)
                pkg_name = pkg.get("name")
                if isinstance(pkg_name, str) and pkg_name:
                    for candidate in ("src", "lib", "app", "."):
                        probe = "src" if candidate == "src" else candidate
                        if any(
                            f == probe or f.startswith(probe + "/")
                            for f in getattr(self, "_last_file_set", set())
                        ) or (self.project_root / probe).is_dir():
                            rules.append((pkg_name, [probe if probe != "." else ""]))
                            break
            except Exception:
                pass
            # Deduplicate identical (pattern, targets) preserving longest-first sort.
            dedup: Dict[str, List[str]] = {}
            for pattern, targets in rules:
                existing = dedup.get(pattern)
                if existing is None:
                    dedup[pattern] = list(targets)
                else:
                    for t in targets:
                        if t not in existing:
                            existing.append(t)
            rules = [(p, ts) for p, ts in dedup.items()]
            rules.sort(key=lambda r: len(r[0]), reverse=True)
        except Exception:
            logger.debug("JS path-alias load failed", exc_info=True)
            rules = []
        self._js_alias_cache = rules
        return rules

    def _resolve_js_alias(self, spec: str, file_set: Set[str]) -> str | None:
        rules = self._load_js_path_aliases()
        for pattern, targets in rules:
            rest = ""
            if spec == pattern:
                rest = ""
            elif pattern and spec.startswith(pattern + "/"):
                rest = spec[len(pattern) + 1 :]
            else:
                continue
            for target_base in targets:
                if rest:
                    candidate = f"{target_base}/{rest}" if target_base else rest
                else:
                    candidate = target_base
                hit = self._probe_js_candidates(self._normalize_js_path(candidate), file_set)
                if hit:
                    return hit
        return None

    def _extract_js_import_specs(self, source: str) -> List[str]:
        """Raw import/require/export-from/worker-URL specifiers in ``source``.

        Pure function of content — safe to cache by sha256. Resolution against
        the file set runs fresh.
        """
        specs: List[str] = []
        for m in self._JS_IMPORT_RE.finditer(source):
            spec = m.group("spec") or m.group("spec2")
            if spec:
                specs.append(spec)
        for m in self._JS_WORKER_URL_RE.finditer(source):
            spec = m.group("spec")
            if spec:
                specs.append(spec)
        for line in source.splitlines():
            bare = self._JS_BARE_IMPORT_RE.match(line)
            if bare:
                specs.append(bare.group("spec"))
        return specs

    _JS_REEXPORT_RE = re.compile(
        r"""export\s+(?:\*|\{[^}]*\})\s+from\s*['"](?P<spec>[^'"]+)['"]"""
    )

    def _extract_js_reexport_specs(self, source: str) -> List[str]:
        """``export * from`` / ``export {…} from`` targets (barrel re-exports)."""
        return [m.group("spec") for m in self._JS_REEXPORT_RE.finditer(source)]

    def _follow_js_reexports(
        self,
        importer: str,
        target: str,
        file_set: Set[str],
        *,
        depth: int = 0,
        seen: Set[str] | None = None,
    ) -> List[str]:
        """Follow barrel re-exports from ``target`` up to a small hop limit.

        Returns additional files reachable via ``export … from`` so an importer of
        a barrel also depends on the re-exported modules.
        """
        if depth >= 4:
            return []
        visited = seen if seen is not None else set()
        if target in visited:
            return []
        visited.add(target)
        try:
            source = (self.project_root / target).read_text(encoding="utf-8", errors="replace")
        except OSError:
            return []
        extra: List[str] = []
        for spec in self._extract_js_reexport_specs(source):
            resolved = self._resolve_js_spec(target, spec, file_set)
            if not resolved or resolved == importer or resolved == target:
                continue
            extra.append(resolved)
            extra.extend(
                self._follow_js_reexports(
                    importer, resolved, file_set, depth=depth + 1, seen=visited
                )
            )
        return extra

    def _js_import_edges(self, files: List[str], file_set: Set[str]) -> List[Tuple[str, str]]:
        """Resolve TS/JS relative + alias import/require/export-from edges to repo files.

        Specifier extraction is cached in ``.devcouncil/cache/repo_map_parse.json``
        keyed by each file's sha256 (same store as Python ``modules``). Resolution
        against the current file set always runs fresh. Barrel ``export * from`` /
        ``export {x} from`` targets are followed so importers of an index also
        depend on the re-exported modules.
        """
        self._last_file_set = file_set
        # Alias rules may have been cached before the file set was known (root-only
        # tsconfigs). Force a reload so nested apps/*/tsconfig.json are included.
        self._js_alias_cache = None
        js_files = [f for f in files if self._is_js_source_path(f)]
        if not js_files:
            return []
        cache = self._load_parse_cache()
        fresh: Dict[str, Dict[str, object]] = {}
        edges: List[Tuple[str, str]] = []
        seen: Set[Tuple[str, str]] = set()
        for rel in js_files:
            try:
                raw = (self.project_root / rel).read_bytes()
            except OSError:
                continue
            digest = hashlib.sha256(raw).hexdigest()
            entry = cache.get(rel)
            cached_specs = (
                entry.get("specs")
                if isinstance(entry, dict) and entry.get("sha256") == digest
                else None
            )
            if isinstance(cached_specs, list):
                specs = [s for s in cached_specs if isinstance(s, str)]
            else:
                specs = self._extract_js_import_specs(
                    raw.decode("utf-8", errors="replace")
                )
            fresh[rel] = {"sha256": digest, "specs": specs}
            for spec in specs:
                target = self._resolve_js_spec(rel, spec, file_set)
                if not target or target == rel:
                    continue
                to_add = [target]
                to_add.extend(self._follow_js_reexports(rel, target, file_set))
                for dest in to_add:
                    if dest != rel and (rel, dest) not in seen:
                        seen.add((rel, dest))
                        edges.append((rel, dest))
        managed = {k for k in cache if self._is_js_source_path(k)} | set(js_files)
        self._merge_parse_cache(fresh, managed)
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

    def _extract_go_import_specs_fallback(self, source: str) -> List[str]:
        specs: List[str] = []
        for block in self._GO_IMPORT_BLOCK_RE.finditer(source):
            for sm in self._GO_IMPORT_SPEC_RE.finditer(block.group("body")):
                specs.append(sm.group("spec"))
        for line in source.splitlines():
            single = self._GO_IMPORT_SINGLE_RE.match(line)
            if single:
                specs.append(single.group("spec"))
        return specs

    def _go_import_edges(self, files: List[str], file_set: Set[str]) -> List[Tuple[str, str]]:
        """Resolve Go import paths under the module prefix to *all* package .go files.

        File-level (not representative-file) so liveness/dependents treat each
        member of an imported package as wired. Prefer tree-sitter extraction
        when available; fall back to regex.
        """
        module = self._go_module_prefix(file_set)
        if not module:
            return []
        go_files = [f for f in files if f.endswith(".go")]
        if not go_files:
            return []
        pkg_files: Dict[str, List[str]] = defaultdict(list)
        for f in go_files:
            if f.endswith("_test.go"):
                continue
            pkg_files[Path(f).parent.as_posix()].append(f)

        from devcouncil.indexing.ts_imports import extract_go_import_specs

        edges: List[Tuple[str, str]] = []
        seen: Set[Tuple[str, str]] = set()

        # Same-package co-membership: Go compiles all files in a directory as one
        # package, so members wire each other even without import statements.
        for members in pkg_files.values():
            if len(members) < 2:
                continue
            for a in members:
                for b in members:
                    if a != b and (a, b) not in seen:
                        seen.add((a, b))
                        edges.append((a, b))

        for rel in go_files:
            try:
                source = (self.project_root / rel).read_text(encoding="utf-8", errors="replace")
            except OSError:
                continue
            ts_specs = extract_go_import_specs(source)
            specs = ts_specs if ts_specs is not None else self._extract_go_import_specs_fallback(source)
            for spec in specs:
                if spec != module and not spec.startswith(module + "/"):
                    continue  # external/stdlib package
                rel_pkg = spec[len(module):].lstrip("/")
                target_dir = rel_pkg if rel_pkg else "."
                for target in sorted(pkg_files.get(target_dir, ())):
                    if target != rel and (rel, target) not in seen:
                        seen.add((rel, target))
                        edges.append((rel, target))
        return edges

    def _rust_crate_root(self, importer: str, file_set: Set[str]) -> str:
        """Directory containing lib.rs/main.rs for ``importer``, or its parent."""
        parts = Path(importer).parts
        for i in range(len(parts) - 1, -1, -1):
            prefix = "/".join(parts[:i]) if i else ""
            for name in ("lib.rs", "main.rs"):
                candidate = f"{prefix}/{name}" if prefix else name
                if candidate in file_set:
                    return prefix if prefix else "."
        parent = Path(importer).parent.as_posix()
        return parent if parent != "." else "."

    def _probe_rust_module(self, base_dir: str, segments: List[str], file_set: Set[str]) -> List[str]:
        """Resolve module path segments under ``base_dir`` to existing .rs files."""
        if not segments:
            return []
        hits: List[str] = []
        cur = base_dir if base_dir not in (".", "") else ""
        for i, seg in enumerate(segments):
            if seg in {"crate", "super", "self", "Self"}:
                continue
            # Try path-so-far as a module file at each segment (last may be an item).
            prefix = f"{cur}/{seg}" if cur else seg
            candidates = [
                f"{prefix}.rs",
                f"{prefix}/mod.rs",
            ]
            found = None
            for cand in candidates:
                norm = self._normalize_js_path(cand)
                if norm in file_set:
                    found = norm
                    break
            if found:
                hits.append(found)
                cur = prefix
            elif i < len(segments) - 1:
                # Intermediate segment must exist as a directory module.
                cur = prefix
            # else: final segment may be a type/fn name — keep prior hits only
        return hits

    def _rust_import_edges(self, files: List[str], file_set: Set[str]) -> List[Tuple[str, str]]:
        """Rust ``mod`` / ``use`` → module files via optional tree-sitter layer.

        No edges when tree-sitter is unavailable (degrades to pre-Phase-3 output).
        """
        from devcouncil.indexing.ts_imports import extract_rust_import_refs, tree_sitter_available

        if not tree_sitter_available():
            return []
        rust_files = [f for f in files if f.endswith(".rs")]
        if not rust_files:
            return []
        edges: List[Tuple[str, str]] = []
        seen: Set[Tuple[str, str]] = set()
        for rel in rust_files:
            try:
                source = (self.project_root / rel).read_text(encoding="utf-8", errors="replace")
            except OSError:
                continue
            refs = extract_rust_import_refs(source)
            if not refs:
                continue
            importer_dir = Path(rel).parent.as_posix()
            if importer_dir == ".":
                importer_dir = ""
            crate_root = self._rust_crate_root(rel, file_set)
            crate_dir = "" if crate_root in (".", "") else crate_root
            for ref in refs:
                targets: List[str] = []
                if ref.get("kind") == "mod":
                    name = ref.get("name")
                    if isinstance(name, str) and name:
                        targets = self._probe_rust_module(importer_dir, [name], file_set)
                elif ref.get("kind") == "use":
                    segments = ref.get("segments") or []
                    if not isinstance(segments, list) or not segments:
                        continue
                    segs = [s for s in segments if isinstance(s, str)]
                    if not segs:
                        continue
                    if segs[0] == "crate":
                        targets = self._probe_rust_module(crate_dir, segs[1:], file_set)
                    elif segs[0] == "super":
                        parent = Path(rel).parent.parent.as_posix()
                        parent_dir = "" if parent == "." else parent
                        targets = self._probe_rust_module(parent_dir, segs[1:], file_set)
                    elif segs[0] == "self":
                        targets = self._probe_rust_module(importer_dir, segs[1:], file_set)
                    else:
                        # Relative / bare path — try from importer dir then crate root.
                        targets = self._probe_rust_module(importer_dir, segs, file_set)
                        if not targets:
                            targets = self._probe_rust_module(crate_dir, segs, file_set)
                for target in targets:
                    if target != rel and (rel, target) not in seen:
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
        try:
            edges.extend(self._rust_import_edges(files, file_set))
        except Exception:
            logger.debug("Rust import-edge resolution failed", exc_info=True)
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

        graph_roots: List[str] = []
        cg = getattr(self, "_last_code_graph", None)
        if cg is not None:
            roots_attr = getattr(cg, "entry_roots", None)
            if roots_attr is not None:
                graph_roots = [str(r) for r in roots_attr]

        subsystems: List[RepoSubsystem] = []
        for area in sorted(by_area):
            area_files = by_area[area]
            # Skip trivial single-file aux areas (e.g. a lone script) to reduce noise,
            # but keep every real source subsystem.
            if len(area_files) < 2 and area.split("/")[0] in _AUX_AREA_ROOTS:
                continue
            area_file_set = set(area_files)
            area_roots = [r for r in graph_roots if r in area_file_set]
            ranked = self._rank_area_files(area_files, in_degree)

            def _entry_named(path: str) -> bool:
                name = Path(path).stem.lower()
                return any(name == hint for hint in _ENTRY_NAME_HINTS)

            # Prefer real production entry roots + imported hubs over zero-degree
            # orphans when filling critical_files (avoids listing dead.py as critical).
            critical_files: List[str] = []
            for p in area_roots + ranked:
                if p in critical_files:
                    continue
                if p in area_roots or in_degree.get(p, 0) > 0 or _entry_named(p):
                    critical_files.append(p)
                if len(critical_files) >= self._SUBSYSTEM_CRITICAL_MAX:
                    break
            # Only pad with remaining files when the area is tiny / has no hubs yet.
            if len(critical_files) < 2:
                for p in ranked:
                    if p not in critical_files:
                        critical_files.append(p)
                    if len(critical_files) >= min(2, self._SUBSYSTEM_CRITICAL_MAX):
                        break
            for p in ranked:
                if len(critical_files) >= self._SUBSYSTEM_CRITICAL_MAX:
                    break
                if p not in critical_files and (
                    in_degree.get(p, 0) > 0 or _entry_named(p) or p in area_roots
                ):
                    critical_files.append(p)

            entry_points = list(area_roots)
            for p in critical_files:
                if p in entry_points:
                    continue
                if in_degree.get(p, 0) > 0 or _entry_named(p):
                    entry_points.append(p)
                if len(entry_points) >= 3:
                    break
            if not entry_points:
                entry_points = critical_files[:1]

            stems = ", ".join(Path(p).stem for p in critical_files[:3])
            summary = f"{Path(area).name or area}: {stems}" if stems else f"{area} ({len(area_files)} files)"
            # Optional community label from code graph (generic repos).
            community = ""
            if cg is not None:
                try:
                    from devcouncil.indexing.graph.communities import community_label_for_area

                    community = community_label_for_area(cg, area)
                    if community and community not in summary:
                        summary = f"{summary} [{community}]"
                except Exception:
                    community = ""
            subsystems.append(
                RepoSubsystem(
                    area=area,
                    summary=summary,
                    entry_points=entry_points[:3],
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

    def build_dependents(
        self, edges: List[Tuple[str, str]]
    ) -> Tuple[Dict[str, List[str]], Dict[str, int]]:
        """Reverse the import edges into a file -> dependents map (who imports each file),
        capped per file. Returns ``(capped_dependents, totals_when_truncated)``.

        ``totals_when_truncated[path]`` is the full importer count when the listed
        sample was truncated by ``_DEPENDENTS_MAX`` — so agents know the blast radius
        is incomplete rather than silently missing importers.
        """
        reverse: Dict[str, Set[str]] = defaultdict(set)
        for importer, imported in edges:
            reverse[imported].add(importer)
        capped: Dict[str, List[str]] = {}
        totals: Dict[str, int] = {}
        for path, importers in sorted(reverse.items()):
            if not importers:
                continue
            full = sorted(importers)
            capped[path] = full[: self._DEPENDENTS_MAX]
            if len(full) > self._DEPENDENTS_MAX:
                totals[path] = len(full)
        return capped, totals

    def import_edges_for(self, files: List[str] | None = None) -> List[Tuple[str, str]]:
        """Public uncapped forward import edges for the given files (or whole repo).

        Verify gates need live edges — the persisted map won't include brand-new
        files until the next ``dev map``. Never raises.
        """
        try:
            if files is None:
                files = self.get_git_files()
            return self._all_import_edges(files)
        except Exception:
            logger.debug("import_edges_for failed", exc_info=True)
            return []

    def dependents_for(self, files: List[str] | None = None) -> Dict[str, Set[str]]:
        """Uncapped reverse index (imported -> set of importers) for presence checks.

        Unlike :meth:`build_dependents`, this includes zero-importer keys only when
        asked about specific files via the returned dict's ``.get``, and never caps
        the importer lists. Never raises.
        """
        try:
            edges = self.import_edges_for(files)
            reverse: Dict[str, Set[str]] = defaultdict(set)
            for importer, imported in edges:
                reverse[imported].add(importer)
            return dict(reverse)
        except Exception:
            logger.debug("dependents_for failed", exc_info=True)
            return {}

    def _compute_liveness(
        self,
        files: List[str],
        edges: List[Tuple[str, str]],
        *,
        cap: Optional[int] = None,
        lsp_refs: bool = False,
    ) -> Tuple[List[str], List[str], List[str], List[str], List[str], bool]:
        """Compute entry_roots, unwired_candidates, unreachable_files, dead_symbol_candidates.

        Also returns ``symbol_index`` (``path::name`` keys) as the fifth element and
        ``liveness_unreachable_unreliable`` as the sixth.

        File lists delegate to ``graph.liveness.file_liveness`` so map vs ratchet
        cannot drift on empty-root guards / caps.

        ``cap`` defaults to ``_LIVENESS_CAP`` for the stored map debt lists. Pass ``0``
        (or any non-positive) for uncapped lists used by the liveness ratchet baseline.

        Stored ``entry_roots`` are production-only and never capped (only debt lists
        are). Structural exemptions are a skip-list, not BFS seeds. Never raises;
        returns empty lists on failure.

        When ``lsp_refs`` is True, dead-symbol candidates are confirmed via the
        optional live LSP client (external references clear false positives).
        """
        try:
            from devcouncil.indexing.graph.liveness import file_liveness

            if cap is None:
                file_cap: Optional[int] = self._LIVENESS_CAP
            elif cap <= 0:
                file_cap = 0
            else:
                file_cap = cap

            prod_roots, unwired, unreachable, unreliable = file_liveness(
                self.project_root, files, edges, cap=file_cap,
            )

            dead_cap = 0 if file_cap is None or file_cap <= 0 else file_cap
            dead_symbols, symbol_index = self._dead_symbol_candidates(
                files, cap=dead_cap, with_index=True, lsp_refs=lsp_refs,
            )
            return (
                prod_roots,
                unwired,
                unreachable,
                dead_symbols,
                symbol_index,
                unreliable,
            )
        except Exception:
            logger.debug("liveness computation failed", exc_info=True)
            return [], [], [], [], [], True

    def liveness_snapshot(self) -> Dict[str, object]:
        """Uncapped liveness lists for ratchet baseline / verify-side current.

        Always recomputes import edges (never reuses a stale ``self._edges`` cache).
        Includes ``symbol_index`` (``path::name`` keys) so the ratchet can require
        a symbol to have existed at baseline before calling it stranded.
        Never raises; returns empty lists on failure.
        """
        try:
            files = self.get_git_files()
            # Invalidate any prior edge cache — snapshot must reflect current tree.
            self._edges = None
            edges = self._all_import_edges(files)
            use_lsp = False
            try:
                from devcouncil.indexing.lsp_client import lsp_refs_enabled

                use_lsp = lsp_refs_enabled(self.project_root)
            except Exception:
                use_lsp = False
            roots, unwired, unreachable, dead, symbol_index, unreliable = (
                self._compute_liveness(files, edges, cap=0, lsp_refs=use_lsp)
            )
            return {
                "entry_roots": roots,
                "unwired_candidates": unwired,
                "unreachable_files": unreachable,
                "dead_symbol_candidates": dead,
                "symbol_index": symbol_index,
                "liveness_unreachable_unreliable": unreliable,
            }
        except Exception:
            logger.debug("liveness_snapshot failed", exc_info=True)
            return {
                "entry_roots": [],
                "unwired_candidates": [],
                "unreachable_files": [],
                "dead_symbol_candidates": [],
                "symbol_index": [],
                "liveness_unreachable_unreliable": True,
            }

    def _dead_symbol_candidates(
        self,
        files: List[str],
        *,
        cap: int = 200,
        with_index: bool = False,
        lsp_refs: bool = False,
    ):
        """Public top-level symbols never referenced outside their own definition span.

        Format: ``path:line name``. Python + JS/TS only. Best-effort; never raises.
        Same-file uses outside the symbol's span clear it; recursive self-references
        inside the span do not. Test-file references clear a symbol (parity with the
        verify gate); test files themselves are not scanned for definitions.
        ``cap <= 0`` means uncapped.

        When ``with_index`` is True, returns ``(dead_list, symbol_index)`` where
        ``symbol_index`` is sorted ``path::name`` keys for all scanned definitions.
        Otherwise returns just the dead list (legacy callers).

        When ``lsp_refs`` is True, candidates are confirmed via the optional live
        LSP client before inclusion (external references clear false positives).
        """
        empty: List[str] = []
        try:
            from devcouncil.indexing.wiring import (
                decorator_names,
                is_liveness_code_file,
                is_private_symbol,
                is_test_path,
                is_vendored_path,
                is_wiring_decorated,
                iter_js_export_symbols,
                parse_python_all_exports,
                strip_js_comments,
                strip_py_comments,
                strip_string_literals,
            )

            # path, start, end, name
            definitions: List[Tuple[str, int, int, str]] = []
            token_files: Dict[str, Set[str]] = defaultdict(set)
            token_lines: Dict[str, Dict[str, Set[int]]] = defaultdict(lambda: defaultdict(set))
            ident_re = re.compile(r"\b([A-Za-z_][A-Za-z0-9_]*)\b")

            for rel in sorted(files):
                if not is_liveness_code_file(rel):
                    continue
                if is_vendored_path(rel):
                    continue
                try:
                    source = (self.project_root / rel).read_text(encoding="utf-8", errors="replace")
                except OSError:
                    continue
                suffix = Path(rel).suffix.lower()
                if suffix == ".py":
                    cleaned = strip_string_literals(strip_py_comments(source))
                else:
                    cleaned = strip_string_literals(strip_js_comments(source))

                # Index tokens from all code files including tests so a test
                # reference clears a production symbol (verify-gate parity).
                for lineno, line in enumerate(cleaned.splitlines(), 1):
                    for tok in ident_re.findall(line):
                        if len(tok) < 2:
                            continue
                        token_files[tok].add(rel)
                        token_lines[tok][rel].add(lineno)

                if is_test_path(rel):
                    continue

                if suffix == ".py":
                    try:
                        tree = ast.parse(source)
                        protected = parse_python_all_exports(source)
                        for node in tree.body:
                            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                                name = node.name
                                if is_private_symbol(name):
                                    continue
                                if name in protected:
                                    continue
                                if is_wiring_decorated(decorator_names(node)):
                                    continue
                                start = getattr(node, "lineno", 1) or 1
                                end = getattr(node, "end_lineno", start) or start
                                definitions.append((rel, start, end, name))
                    except SyntaxError:
                        pass
                else:
                    for line_no, name in iter_js_export_symbols(source):
                        prev_lines = source.splitlines()[: max(0, line_no - 1)]
                        if prev_lines:
                            prev = prev_lines[-1].strip()
                            if prev.startswith("@"):
                                continue
                        definitions.append((rel, line_no, line_no, name))

            symbol_index = sorted({f"{path}::{name}" for path, _s, _e, name in definitions})
            out: List[str] = []
            for path, start, end, name in definitions:
                refs = token_files.get(name, set())
                if refs - {path}:
                    continue
                same_lines = token_lines.get(name, {}).get(path, set())
                if any(ln < start or ln > end for ln in same_lines):
                    continue
                out.append(f"{path}:{start} {name}")
                if cap > 0 and len(out) >= cap:
                    break
            if lsp_refs and out:
                try:
                    from devcouncil.indexing.lsp_client import filter_dead_symbols_with_lsp

                    out = filter_dead_symbols_with_lsp(self.project_root, out)
                    if cap > 0:
                        out = out[:cap]
                except Exception:
                    logger.debug("LSP dead-symbol confirmation failed", exc_info=True)
            if with_index:
                return out, symbol_index
            return out
        except Exception:
            logger.debug("dead_symbol_candidates failed", exc_info=True)
            if with_index:
                return empty, empty
            return empty

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
        lower_name = name.lower()
        if "__pycache__" in parts or normalized.endswith(".pyc"):
            return True
        if parts.intersection({".git", ".devcouncil", ".pytest_cache", ".ruff_cache", ".mypy_cache", ".venv"}):
            return True
        if normalized.startswith("dist/") or normalized.startswith("build/"):
            return True
        if lower_name.endswith((".tgz", ".whl", ".tar.gz")):
            return True
        if name.startswith(("tmp", "temp", ".tmp")) or name.endswith("~"):
            return True
        return False

    def _git_head(self) -> str:
        from devcouncil.utils.proc import git_output

        return git_output(["rev-parse", "HEAD"], cwd=self.project_root, default="").strip()

    def _files_fingerprint(self, files: List[str]) -> str:
        return hashlib.sha1("\n".join(sorted(files)).encode("utf-8")).hexdigest()

    def _content_fingerprint(self, files: List[str]) -> str:
        from devcouncil.indexing.graph.build import content_fingerprint

        return content_fingerprint(self.project_root, files)

    def map_is_stale(self, repo_map: Dict[str, object]) -> bool:
        """True when the stored map no longer matches the repo's current git HEAD,
        tracked file set, or content fingerprint — i.e. commits, file add/removes, or
        plain edits happened since ``dev map`` last ran.

        Fail-closed: exceptions from ``get_git_files`` / content fingerprinting
        return True (stale) so ``--if-stale`` / verify rebuild rather than trusting
        an unverifiable map.

        Returns False for maps written before fingerprinting (no false alarms).
        Legacy maps without ``content_fingerprint`` skip the content check.
        """
        stored_head = str(repo_map.get("generated_head") or "")
        stored_hash = str(repo_map.get("indexed_hash") or "")
        if not stored_head and not stored_hash:
            return False
        # Lean/degraded maps re-stamp fingerprints but lack a trustworthy graph —
        # fail closed so --if-stale / watch / verify keep retrying until healthy.
        if bool(repo_map.get("graph_degraded")):
            return True
        try:
            files = self.get_git_files()
        except Exception:
            # Fail closed: cannot prove freshness → treat as stale so --if-stale /
            # verify rebuild rather than trusting a possibly outdated map.
            return True
        if self._git_head() != stored_head or self._files_fingerprint(files) != stored_hash:
            return True
        stored_content = str(repo_map.get("content_fingerprint") or "")
        if not stored_content:
            # Legacy maps without content_fingerprint skip the content check.
            return False
        try:
            return self._content_fingerprint(files) != stored_content
        except Exception:
            return True

    def get_git_files(self) -> List[str]:
        try:
            from devcouncil.utils.proc import git_output

            # ``-z`` avoids C-quoting of non-ASCII paths (otherwise ``café.py`` becomes
            # ``"src/caf\303\251.py"`` and fails the ``is_file()`` existence filter).
            output = git_output(
                ["ls-files", "-z", "--cached", "--others", "--exclude-standard"],
                cwd=self.project_root,
            )
            paths = [p.replace("\\", "/") for p in output.split("\0") if p]
            # Skip index entries whose working-tree file was deleted but not staged.
            return [
                path
                for path in paths
                if not self._is_runtime_or_generated_file(path)
                and (self.project_root / path).is_file()
            ]
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
        """Rank candidate files for ``goal`` via ripgrep content search, else path tokens.

        Multi-word goals are OR'd as separate ``-e`` patterns (a single quoted phrase
        almost never appears in source). Search is scoped to roots present in ``files``
        so unscoped ``rg`` cannot hang on huge trees or miss the tree entirely.
        Results are intersected with ``files`` so untracked noise cannot appear.
        """
        file_set = {f.replace("\\", "/") for f in files}
        # Split on whitespace and ``|``; strip call/index punctuation so goals like
        # ``file_liveness(`` or ``(unwired|unreachable)`` still match identifiers.
        raw_parts: List[str] = []
        for part in goal.replace("|", " ").split():
            raw_parts.append(part)
        tokens: List[str] = []
        for raw in raw_parts:
            t = raw.strip().strip("()[]{}<>,:;\"'`")
            while t and t[-1] in "+*?^$\\.":
                t = t[:-1]
            while t and t[0] in "+*?^$\\.":
                t = t[1:]
            if len(t) >= 2:
                tokens.append(t)
        if not tokens and goal.strip():
            tokens = [goal.strip()]
        if not tokens:
            return []

        try:
            # Prefer real source/test roots so alphabetical tops like `.github` /
            # README.md cannot crowd `src`/`tests` out of a small roots budget.
            preferred = (
                "src", "tests", "test", "benchmarks", "scripts", "docs", "bin",
                "packages", "lib", "app", "apps", "services", "cmd", "internal",
            )
            tops_in_files: set[str] = set()
            for f in file_set:
                top = f.split("/", 1)[0]
                if top:
                    tops_in_files.add(top)

            roots: List[str] = []
            for top in preferred:
                if top in tops_in_files and (self.project_root / top).is_dir():
                    roots.append(top)
            for top in sorted(tops_in_files):
                if top in roots:
                    continue
                if top.startswith("."):
                    continue
                if not (self.project_root / top).is_dir():
                    continue
                roots.append(top)
                if len(roots) >= 16:
                    break
            if not roots:
                roots = ["."]

            cmd = [
                "rg",
                "--files-with-matches",
                "--ignore-case",
                # Goal tokens are keywords/identifiers, not regex. Without -F,
                # trailing meta like ``file_liveness(`` or ``foo[`` silently miss.
                "-F",
                "--glob", "!**/.git/**",
                "--glob", "!**/.devcouncil/**",
                "--glob", "!**/.venv/**",
                "--glob", "!**/node_modules/**",
            ]
            for token in tokens:
                cmd.extend(["-e", token])
            cmd.extend(roots)

            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                cwd=self.project_root,
                timeout=10,
            )
            # rg exits 1 when no matches — still a successful run.
            if result.returncode in (0, 1):
                hits: List[Dict[str, str]] = []
                for line in sorted(result.stdout.strip().splitlines()):
                    path = line.strip().replace("\\", "/")
                    if path.startswith("./"):
                        path = path[2:]
                    if not path or path not in file_set:
                        continue
                    hits.append({
                        "path": path,
                        "reason": f"ripgrep match for '{goal}'",
                    })
                    if len(hits) >= 10:
                        break
                if hits:
                    return hits
        except Exception:
            logger.debug("ripgrep search failed; falling back to naive matching", exc_info=True)

        # Naive keyword matching fallback (path tokens only).
        goal_words = set(t.lower() for t in tokens)
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

    def map_repo(
        self,
        goal: str = "",
        *,
        scan_dependencies: bool = False,
        liveness: bool = True,
        lsp_refs: bool = False,
    ) -> RepoMap:
        """Build the repo map.

        ``scan_dependencies`` is opt-in (default off) so the common ``dev map`` path
        stays fast and never shells out to a vulnerability auditor. When enabled, a
        best-effort SCA scan runs locally (only if an auditor is installed) and its
        findings are attached as ``dependency_risks``.

        ``liveness`` (default on) computes entry_roots / unwired / unreachable /
        dead_symbol candidate lists. Pass False (``--no-liveness``) to skip.

        ``lsp_refs`` (default off) confirms dead-symbol candidates via the optional
        live LSP client when a language server is available.

        Builds the symbol-level code knowledge graph first, writes
        ``.devcouncil/graph/code_graph.json``, then derives the summary ``RepoMap``.
        """
        files = self.get_git_files()
        # Decide DevCouncil-vs-generic and the source root BEFORE describing files, so
        # area bucketing and subsystem inference agree within a single run.
        self._use_generic = not any(path.startswith("src/devcouncil/") for path in files)
        self._source_root = self.detect_source_root(files)

        # Single-pass: extract + resolve + liveness/token-scan once; derive map lists
        # and graph dead_code from that pass (no second _token_scan_dead).
        changed = getattr(self, "_graph_changed_paths", None)
        code_graph: CodeGraph | None = getattr(self, "_prebuilt_code_graph", None)
        prebuilt_graph = code_graph is not None
        skip_graph_build = bool(getattr(self, "_skip_code_graph_build", False))
        if code_graph is not None:
            self._last_code_graph = code_graph
            self._edges = [
                (e.source, e.target)
                for e in code_graph.edges
                if e.kind == "imports" and "::" not in e.source and "::" not in e.target
            ]
        elif skip_graph_build:
            self._last_code_graph = None
            self._edges = self._all_import_edges(files)
        else:
            try:
                from devcouncil.indexing.graph.build import build_code_graph, write_code_graph

                code_graph = build_code_graph(
                    self.project_root,
                    files,
                    changed_paths=changed,
                    liveness=liveness,
                    lsp_refs=lsp_refs,
                    mapper=self,
                )
                self._last_code_graph = code_graph
                # File→file import edges only (named-import edges target symbols).
                self._edges = [
                    (e.source, e.target)
                    for e in code_graph.edges
                    if e.kind == "imports" and "::" not in e.source and "::" not in e.target
                ]
            except Exception:
                logger.warning(
                    "code graph build failed; falling back to import edges only "
                    "(dead_symbol_candidates will be omitted — refusing token-only flood)",
                    exc_info=True,
                )
                code_graph = None
                self._last_code_graph = None
                self._edges = self._all_import_edges(files)

        if self._edges is None:
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

        entry_roots_list: List[str] = []
        unwired: List[str] = []
        unreachable: List[str] = []
        dead_syms: List[str] = []
        unreachable_unreliable = False
        liveness_meta: Dict[str, object] = {}
        if liveness and code_graph is not None:
            entry_roots_list = list(code_graph.entry_roots)
            # Caps apply only when serializing repo_map.json (graph stays uncapped).
            from devcouncil.indexing.graph.liveness import apply_liveness_cap

            cap = self._LIVENESS_CAP
            unwired, um = apply_liveness_cap(list(code_graph.unwired_candidates), cap)
            unreachable, rm = apply_liveness_cap(list(code_graph.unreachable_files), cap)
            dead_syms, dm = apply_liveness_cap(list(code_graph.meta.get("legacy_dead_symbol_candidates") or []), cap)
            liveness_meta = {"unwired": um, "unreachable": rm, "dead_symbol": dm}
            unreachable_unreliable = bool(
                code_graph.meta.get("liveness_unreachable_unreliable")
            )
        elif liveness:
            # Graph assemble failed: keep file-level liveness from import edges,
            # but do NOT run the token-only dead-symbol scan (misleading flood).
            logger.warning(
                "code graph unavailable; leaving dead_symbol_candidates empty"
            )
            try:
                from devcouncil.indexing.graph.liveness import file_liveness

                cap = self._LIVENESS_CAP
                entry_roots_list, unwired, unreachable, unreachable_unreliable = (
                    file_liveness(
                        self.project_root,
                        files,
                        self._edges or [],
                        cap=cap,
                    )
                )
            except Exception:
                logger.debug("file_liveness fallback after graph failure failed", exc_info=True)
                entry_roots_list, unwired, unreachable = [], [], []
                unreachable_unreliable = True
            dead_syms = []

        if code_graph is not None and not prebuilt_graph:
            try:
                from devcouncil.indexing.graph.build import write_code_graph

                code_graph.generated_head = self._git_head()
                code_graph.indexed_hash = self._files_fingerprint(files)
                code_graph.content_fingerprint = self._content_fingerprint(files)
                write_code_graph(self.project_root, code_graph)
            except Exception:
                # A missing/stale code_graph.json silently degrades every graph
                # consumer — this must be visible, not a DEBUG-only whisper.
                logger.warning(
                    "failed to write code graph export (.devcouncil/graph/code_graph.json)",
                    exc_info=True,
                )

        processes: List[Dict[str, object]] = []
        if code_graph is not None:
            raw_procs = code_graph.meta.get("processes") or []
            if isinstance(raw_procs, list):
                processes = [p for p in raw_procs[:12] if isinstance(p, dict)]

        dependents, dependents_total = self.build_dependents(self._edges or [])
        subsystems = self._build_subsystem_index(files)

        return RepoMap(
            languages=self.detect_languages(files),
            frameworks=self.detect_frameworks(files),
            package_managers=self.detect_package_managers(files),
            test_commands=self.detect_test_commands(files),
            important_files=important_files,
            candidate_files=candidates,
            files=file_entries,
            subsystems=subsystems,
            dependents=dependents,
            dependents_total=dependents_total,
            generated_head=self._git_head(),
            indexed_hash=self._files_fingerprint(files),
            content_fingerprint=self._content_fingerprint(files),
            lsp=LspInspector(self.project_root).summary(files, client_enabled=lsp_refs),
            dependency_risks=self._scan_dependency_risks() if scan_dependencies else [],
            entry_roots=entry_roots_list,
            unwired_candidates=unwired,
            unreachable_files=unreachable,
            dead_symbol_candidates=dead_syms,
            liveness_unreachable_unreliable=unreachable_unreliable,
            liveness_meta=liveness_meta,
            processes=processes,
        )

RepositoryMapper = RepoMapper
