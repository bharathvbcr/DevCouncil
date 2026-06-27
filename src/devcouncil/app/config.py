"""Typed configuration loader with Pydantic validation.

Replaces scattered yaml.safe_load() calls with a single validated config service.
"""

from __future__ import annotations

import os
import shutil
import subprocess
from pathlib import Path
from typing import Dict, List

from pydantic import BaseModel, Field, field_validator


class ModelRoleConfig(BaseModel):
    model: str
    temperature: float = 0.0
    # Optional per-role provider override. When unset, the role uses
    # ``models.provider``. Lets a single run route some roles to one provider
    # (e.g. planning on OpenRouter) and others to another (e.g. live review on
    # local Ollama). Validated/normalized against the supported provider list.
    provider: str | None = None

    @field_validator("provider")
    @classmethod
    def _normalize_provider(cls, value: str | None) -> str | None:
        if value is None:
            return None
        # Lazy import avoids a circular import (llm.provider imports app.config).
        from devcouncil.llm.provider import validate_model_provider

        return validate_model_provider(value)


class ModelsConfig(BaseModel):
    provider: str = "openrouter"
    roles: Dict[str, ModelRoleConfig] = Field(default_factory=dict)


class ProjectConfig(BaseModel):
    name: str = "devcouncil-project"
    root: str = "."
    default_branch: str = "main"


class CommandsConfig(BaseModel):
    test: List[str] = Field(default_factory=list)
    lint: List[str] = Field(default_factory=list)
    typecheck: List[str] = Field(default_factory=list)


class VerificationSandboxConfig(BaseModel):
    docker_image: str = "python:3.12-slim"
    docker_setup_commands: List[str] = Field(default_factory=list)
    nix_flake_attr: str | None = None


class DiffCoverageConfig(BaseModel):
    """Diff↔coverage gating: prove the *changed* lines were exercised by tests.

    ``measure`` runs the diff-coverage analysis and records it as evidence (and a
    non-blocking signal) whenever the target repo's coverage tooling is present.
    ``enforce`` promotes an unexercised diff to a *blocking* gap. Enforcement is
    off by default so the signal is visible before it ever gates — a passing test
    that does not touch the new code is surfaced first, then teams opt in to
    blocking. ``min_ratio`` of 0.0 means "require at least one changed executable
    line to be exercised"; a higher value demands that fraction of changed lines.
    """

    measure: bool = True
    enforce: bool = False
    min_ratio: float = 0.0


class VerificationConfig(BaseModel):
    sandbox: VerificationSandboxConfig = Field(default_factory=VerificationSandboxConfig)
    diff_coverage: DiffCoverageConfig = Field(default_factory=DiffCoverageConfig)


class GatesConfig(BaseModel):
    require_clean_git_before_task: bool = True
    block_orphan_diffs: bool = True
    block_missing_tests_for_high_requirements: bool = True
    block_dependency_changes_without_approval: bool = True
    block_schema_change_without_migration: bool = True
    block_failed_commands: bool = True


class ExecutionConfig(BaseModel):
    default_executor: str = "manual"
    max_repair_attempts: int = 3
    checkpoint_before_each_task: bool = True
    command_timeout: int = 300
    stream_cli_output: bool = False
    # Default lifetime of an MCP task lease. A crashed/disconnected agent's lease
    # auto-expires after this, so the task frees up without a human running force.
    lease_ttl_seconds: int = 1800
    # When true, the post-task coding-CLI hook runs deterministic verification of the
    # active task (and records gaps) instead of only printing a reminder. Off by default
    # so hooks stay fast/cheap unless a team opts in.
    verify_on_post_task: bool = False
    cursor_resume_mode: str = "off"
    coding_cli_probe_order: List[str] = Field(default_factory=list)


class PrivacyConfig(BaseModel):
    redact_env_vars: bool = True
    redact_secrets_in_logs: bool = True
    store_prompts_locally: bool = True


class AgentFlowIntegrationConfig(BaseModel):
    enabled: bool = False
    trace_path: str = ".devcouncil/logs/traces.jsonl"
    mode: str = "jsonl"


class CodeReviewGraphIntegrationConfig(BaseModel):
    enabled: bool = False
    command: str = "code-review-graph"
    optional: bool = True


class LiveReviewIntegrationConfig(BaseModel):
    enabled: bool = True
    cards_path: str = ".devcouncil/live/cards"
    signals_path: str = ".devcouncil/live/signals"
    default_client: str = "claude"


class WarpIntegrationConfig(BaseModel):
    enabled: bool = False
    command: str = "oz"
    run_mode: str = "local"
    mcp_config_path: str = ".devcouncil/integrations/warp-mcp.json"
    profile: str | None = None
    model: str | None = None
    environment: str | None = None
    share: List[str] = Field(default_factory=list)


class OpenCodeIntegrationConfig(BaseModel):
    enabled: bool = False
    config_path: str = "opencode.json"


class AntigravityIntegrationConfig(BaseModel):
    enabled: bool = False
    mcp_config_path: str = ".agents/mcp_config.json"


class CursorIntegrationConfig(BaseModel):
    enabled: bool = False
    config_path: str = ".cursor/mcp.json"
    hooks_path: str = ".cursor/hooks.json"


class AiderIntegrationConfig(BaseModel):
    enabled: bool = False


class CliAgentProfileConfig(BaseModel):
    description: str = ""
    timeout_seconds: int | None = None
    prompt_preamble: str = ""
    require_explicit_confirmation: bool = False
    # Per-profile CLI containment overrides. Empty/None reproduce today's behavior
    # exactly so a profile that only sets a prompt preamble is a no-op on the
    # subprocess invocation. ``extra_args`` are appended verbatim to the resolved
    # command, ``permission_mode`` is translated into the right per-CLI flag where
    # known (and overly-permissive flags are dropped for stricter modes), and
    # ``model`` overrides the model flag for CLIs that accept one.
    extra_args: List[str] = Field(default_factory=list)
    permission_mode: str | None = None
    model: str | None = None


class CustomCliAgentConfig(BaseModel):
    command: str
    args: List[str] = Field(default_factory=list)
    input_mode: str = "stdin"
    prompt_arg: str | None = None
    timeout_seconds: int | None = None
    env: Dict[str, str] = Field(default_factory=dict)
    display_name: str | None = None
    kind: str = "custom"
    supports_mcp: bool = False
    supports_diff_review: bool = False
    default_profile: str = "default"
    help_command: List[str] = Field(default_factory=list)


class CliAgentsIntegrationConfig(BaseModel):
    enabled: bool = True
    profiles: Dict[str, CliAgentProfileConfig] = Field(default_factory=dict)
    agents: Dict[str, CustomCliAgentConfig] = Field(default_factory=dict)


class McpIntegrationConfig(BaseModel):
    write_task_scope_to_config: bool = False


class IntegrationsConfig(BaseModel):
    mcp: McpIntegrationConfig = Field(default_factory=McpIntegrationConfig)
    agent_flow: AgentFlowIntegrationConfig = Field(default_factory=AgentFlowIntegrationConfig)
    code_review_graph: CodeReviewGraphIntegrationConfig = Field(default_factory=CodeReviewGraphIntegrationConfig)
    live_review: LiveReviewIntegrationConfig = Field(default_factory=LiveReviewIntegrationConfig)
    cursor: CursorIntegrationConfig = Field(default_factory=CursorIntegrationConfig)
    aider: AiderIntegrationConfig = Field(default_factory=AiderIntegrationConfig)
    antigravity: AntigravityIntegrationConfig = Field(default_factory=AntigravityIntegrationConfig)
    warp: WarpIntegrationConfig = Field(default_factory=WarpIntegrationConfig)
    opencode: OpenCodeIntegrationConfig = Field(default_factory=OpenCodeIntegrationConfig)
    cli_agents: CliAgentsIntegrationConfig = Field(default_factory=CliAgentsIntegrationConfig)


class KnowledgeConfig(BaseModel):
    """Ingested knowledge (Open Knowledge Format bundles + a project design.md) that gets
    injected into planning/council/task prompts.

    Sources live under ``<directory>/{okf,design}``. A design system is always selected
    (``design_always``) because a coding agent should honor it on every UI task; OKF
    knowledge is selected by goal keywords / document tags. The ``*_max_chars`` budgets
    bound how much rides inline so a large knowledge base can't crowd out file context.
    """

    enabled: bool = True
    directory: str = ".devcouncil/knowledge"
    design_always: bool = True
    okf_max_chars: int = 3000
    design_max_chars: int = 4000


class ProviderConfig(BaseModel):
    sort: str = "price"
    allow_fallbacks: bool = True
    require_parameters: bool = True
    data_collection: str = "deny"


class DevCouncilConfig(BaseModel):
    """Top-level validated configuration."""

    project: ProjectConfig = Field(default_factory=ProjectConfig)
    models: ModelsConfig = Field(default_factory=ModelsConfig)
    provider: ProviderConfig = Field(default_factory=ProviderConfig)
    commands: CommandsConfig = Field(default_factory=CommandsConfig)
    gates: GatesConfig = Field(default_factory=GatesConfig)
    execution: ExecutionConfig = Field(default_factory=ExecutionConfig)
    verification: VerificationConfig = Field(default_factory=VerificationConfig)
    privacy: PrivacyConfig = Field(default_factory=PrivacyConfig)
    integrations: IntegrationsConfig = Field(default_factory=IntegrationsConfig)
    knowledge: KnowledgeConfig = Field(default_factory=KnowledgeConfig)


def load_config(project_root: Path = Path(".")) -> DevCouncilConfig:
    """Load and validate .devcouncil/config.yaml.
    
    Returns DevCouncilConfig with defaults for any missing fields.
    Raises FileNotFoundError if config doesn't exist.
    """
    import yaml  # type: ignore[import-untyped]

    config_path = project_root / ".devcouncil" / "config.yaml"
    if not config_path.exists():
        raise FileNotFoundError(f"Config not found at {config_path}. Run 'dev init' first.")

    with open(config_path, encoding="utf-8") as f:
        try:
            raw = yaml.safe_load(f) or {}
        except yaml.YAMLError as exc:
            raise ValueError(
                f"Invalid YAML in {config_path}: {exc}. Fix the syntax or re-run 'dev init'."
            ) from exc

    return DevCouncilConfig.model_validate(raw)


def provider_api_key_env_var(provider: str = "openrouter") -> str:
    normalized = provider.strip().lower().replace("-", "").replace("_", "")
    env_map = {
        "openrouter": "OPENROUTER_API_KEY",
        "vertexai": "VERTEXAI_ACCESS_TOKEN",
        "doubleword": "DOUBLEWORD_API_KEY",
        "ollama": "OLLAMA_API_KEY",
        "openai": "OPENAI_API_KEY",
        "anthropic": "ANTHROPIC_API_KEY",
    }
    return env_map.get(normalized, f"{normalized.upper()}_API_KEY")


def _normalized_provider_name(provider: str) -> str:
    return provider.strip().lower().replace("-", "").replace("_", "")


def load_local_secrets(project_root: Path = Path(".")) -> Dict[str, str]:
    secrets_path = project_root / ".devcouncil" / "secrets.env"
    if not secrets_path.exists():
        return {}

    secrets: Dict[str, str] = {}
    for line in secrets_path.read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#") or "=" not in stripped:
            continue
        key, value = stripped.split("=", 1)
        secrets[key.strip()] = value.strip().strip('"').strip("'")
    return secrets


def get_gcloud_access_token() -> str | None:
    executable = shutil.which("gcloud")
    if not executable:
        return None
    try:
        token = subprocess.check_output(
            [executable, "auth", "print-access-token"],
            stderr=subprocess.STDOUT,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=10,
        ).strip()
    except Exception:
        return None
    return token or None


def get_api_key(provider: str = "openrouter", project_root: Path = Path(".")) -> str:
    """Retrieve the API key for the configured provider from environment.
    
    Raises ValueError if not found.
    """
    env_var = provider_api_key_env_var(provider)
    key = os.environ.get(env_var) or load_local_secrets(project_root).get(env_var)
    if not key and _normalized_provider_name(provider) == "vertexai":
        key = get_gcloud_access_token()
    if not key and _normalized_provider_name(provider) == "ollama":
        # Ollama is a local server and needs no API key; an explicitly-set
        # OLLAMA_API_KEY still flows through above if present.
        return ""
    if not key:
        extra = (
            " You can also authenticate with 'gcloud auth login' for vertexai."
            if _normalized_provider_name(provider) == "vertexai"
            else ""
        )
        raise ValueError(
            f"API key not found. Set {env_var} in your environment or run 'dev setup'. "
            f"Provider: {provider}.{extra}"
        )
    return key
