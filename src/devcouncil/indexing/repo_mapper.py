import json
import logging
import os
import subprocess
from pathlib import Path
from typing import Dict, List, Tuple

from pydantic import BaseModel, Field

from devcouncil.indexing.lsp import LspInspector

logger = logging.getLogger(__name__)


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
    lsp: Dict[str, object] = Field(default_factory=dict)

class RepoMapper:
    def __init__(self, project_root: Path):
        self.project_root = project_root

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
        "src/devcouncil/indexing": "Repo mapping, AST/LSP indexing, and symbol discovery",
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
            "Source indexing, AST, LSP, and graph-assisted symbol mapping.",
            [
                "src/devcouncil/indexing/repo_mapper.py",
                "src/devcouncil/indexing/ast_matcher.py",
                "src/devcouncil/indexing/lsp.py",
                "src/devcouncil/indexing/symbol_index.py",
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
            ("lsp", ["indexing/lsp.py"]),
            ("graph", ["indexing/graph_index.py"]),
            ("symbols", ["indexing/symbol_index.py"]),
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
        return "root"

    def _build_subsystem_index(self, files: List[str]) -> List[RepoSubsystem]:
        file_set = set(files)
        subsystems: List[RepoSubsystem] = []
        for area, (summary, entry_points) in self._SUBSYSTEM_INDEX.items():
            available_entry_points = [path for path in entry_points if path in file_set]
            if not available_entry_points:
                continue
            area_files = sorted(path for path in files if path.startswith(f"{area}/"))
            ranked_files = [path for path in available_entry_points if path in file_set]
            for path in area_files:
                if path in available_entry_points:
                    continue
                if len(ranked_files) >= self._SUBSYSTEM_CRITICAL_MAX:
                    break
                ranked_files.append(path)
            critical_files = ranked_files[: self._SUBSYSTEM_CRITICAL_MAX]
            neighbors = [n for n in self._SUBSYSTEM_NEIGHBORS.get(area, []) if any(f.startswith(f"{n}/") for f in files)]
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
        if "__pycache__" in parts or normalized.endswith(".pyc"):
            return True
        if parts.intersection({".git", ".devcouncil", ".pytest_cache", ".ruff_cache", ".mypy_cache", ".venv"}):
            return True
        if normalized.startswith("dist/") or normalized.startswith("build/"):
            return True
        return False

    def get_git_files(self) -> List[str]:
        try:
            output = subprocess.check_output(
                ["git", "ls-files", "--cached", "--others", "--exclude-standard"],
                cwd=self.project_root,
                stderr=subprocess.DEVNULL
            ).decode().splitlines()
            return [path for path in output if not self._is_runtime_or_generated_file(path)]
        except Exception:
            # Fallback to os.walk if not a git repo or git missing
            files = []
            for root, _, filenames in os.walk(self.project_root):
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
            content = (self.project_root / "package.json").read_text()
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
                content = ""
                if "requirements.txt" in file_set:
                    content += (self.project_root / "requirements.txt").read_text()
                if "pyproject.toml" in file_set:
                    content += (self.project_root / "pyproject.toml").read_text()
                
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
                pkg = json.loads((self.project_root / "package.json").read_text())
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
            pass  # Fall back to naive matching

        # Naive keyword matching fallback
        goal_words = set(goal.lower().split())
        for f in files:
            f_lower = f.lower()
            score = sum(1 for word in goal_words if word in f_lower)
            if score > 0:
                candidates.append((score, f))
        candidates = [
            {"path": path, "reason": f"Matches goal keywords (score: {score})"}
            for score, path in sorted(candidates, key=lambda item: (item[0], item[1]), reverse=True)
        ][:10]
        return candidates

    def map_repo(self, goal: str = "") -> RepoMap:
        files = self.get_git_files()
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
            lsp=LspInspector(self.project_root).summary(files),
        )
