"""Typed configuration loader with Pydantic validation.

Replaces scattered yaml.safe_load() calls with a single validated config service.
"""

from __future__ import annotations

import logging
import os
import shutil
import subprocess
import time
from pathlib import Path
from typing import Dict, List, Optional, Tuple

from pydantic import BaseModel, Field, field_validator

logger = logging.getLogger(__name__)


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


class AcceptanceCheckConfig(BaseModel):
    """Tuning for DevCouncil's per-criterion compiled acceptance checks.

    These default to single-shot behavior (``samples=1``, ``repair_attempts=1``)
    so a strong cloud model is unaffected. They exist to make a WEAK/LOCAL model
    (e.g. an Ollama reviewer) trustworthy: such a model frequently emits a check
    that does not run (wrong import, broken one-liner) — recorded as ``incomplete``
    — or a single mis-asserting check that false-``blocked`` correct code.

    - ``repair_attempts``: when a compiled check FAILS TO RUN (malformed/unrunnable,
      proves nothing), feed the error back and regenerate the COMMAND up to this many
      times. Safe by construction — a check that never ran cannot weaken the gate.
    - ``samples``: generate this many INDEPENDENT checks per criterion and decide by
      majority vote (proven iff a strict majority pass; unanimous-fail blocks; a split
      stays unproven with a non-blocking advisory). Local sampling is cost-free, so
      raising this (e.g. 3) outvotes a single mis-generated check without ever
      auto-passing a real defect. ``1`` reproduces today's single-check behavior.
    """

    samples: int = 1
    repair_attempts: int = 1
    # Compile one criterion per model call instead of batching them all into a single
    # prompt. A weak/local model batching N criteria into one JSON routinely omits or
    # mis-attributes some (a false "incomplete"); a focused single-criterion prompt is far
    # more reliable. Costs N× the calls — cheap on a local monitor, so opt-in. Off keeps
    # the single batched call.
    per_criterion: bool = False


class ReviewerCheckConfig(BaseModel):
    """Self-consistency voting for the LLM live reviewer.

    A weak/local reviewer can emit a lone, mis-calibrated "Critical Issues" verdict that
    falsely BLOCKS (the live card becomes a blocking gap). Sampling ``samples`` independent
    reviews and majority-voting the verdict outvotes a single bad judgment: the gate only
    escalates to the blocking verdict when a strict majority agree; a split de-escalates to
    the non-blocking "Concerns". ``samples=1`` reproduces today's single-review behavior, so
    a strong cloud model is unaffected. Local sampling is cost-free — raise it (e.g. 3) there.
    """

    samples: int = 1


class VerificationConfig(BaseModel):
    sandbox: VerificationSandboxConfig = Field(default_factory=VerificationSandboxConfig)
    diff_coverage: DiffCoverageConfig = Field(default_factory=DiffCoverageConfig)
    acceptance_checks: AcceptanceCheckConfig = Field(default_factory=AcceptanceCheckConfig)
    reviewer_checks: ReviewerCheckConfig = Field(default_factory=ReviewerCheckConfig)


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
    # Opt-in scope gate for executors WITHOUT a pre-write hook (CLI subprocesses that write
    # directly to disk). When true, DevCouncil re-checks every file a coding-CLI subprocess
    # changed against the task's authorization right after it exits and REVERTS any the task
    # did not allow — so unplanned drift never reaches the verify gate or a commit, instead
    # of only being flagged post-verify by orphan_diff. Off by default (it reverts the
    # agent's writes; teams opt in once their plans declare planned_files reliably).
    enforce_file_scope_pre_verify: bool = False


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


# Memoized parsed configs keyed by resolved config path. Each entry stores the
# file's stat signature (mtime_ns, size, inode) so a rewritten config.yaml — common in
# tests that mutate config mid-process, including delete+recreate — is re-read instead
# of served stale.
_CONFIG_CACHE: Dict[Path, Tuple[Tuple[int, int, int], DevCouncilConfig]] = {}


def load_config(project_root: Path = Path(".")) -> DevCouncilConfig:
    """Load and validate .devcouncil/config.yaml.

    Returns DevCouncilConfig with defaults for any missing fields.
    Raises FileNotFoundError if config doesn't exist.

    Parsed results are memoized per resolved config path and invalidated when the
    file's mtime/size changes, so repeated callers avoid redundant disk reads while
    still picking up a rewritten config.
    """
    import yaml  # type: ignore[import-untyped]

    config_path = project_root / ".devcouncil" / "config.yaml"
    try:
        stat = config_path.stat()
    except FileNotFoundError:
        raise FileNotFoundError(f"Config not found at {config_path}. Run 'dev init' first.")

    cache_key = config_path.resolve()
    # Include the inode so a delete+recreate under the same path (new inode) is treated
    # as changed even if the rewritten file lands with the same mtime and size.
    signature = (stat.st_mtime_ns, stat.st_size, stat.st_ino)
    cached = _CONFIG_CACHE.get(cache_key)
    if cached is not None and cached[0] == signature:
        # The cached instance is shared across callers; treat it as read-only. Nothing
        # in the codebase mutates a loaded DevCouncilConfig (settings are rewritten on
        # disk via the raw-config helpers, then re-read), so we avoid a per-call copy.
        return cached[1]

    logger.debug("Loading config from %s", config_path)
    with open(config_path, encoding="utf-8") as f:
        try:
            raw = yaml.safe_load(f) or {}
        except yaml.YAMLError as exc:
            logger.error("Invalid YAML in %s: %s", config_path, exc)
            raise ValueError(
                f"Invalid YAML in {config_path}: {exc}. Fix the syntax or re-run 'dev init'."
            ) from exc

    config = DevCouncilConfig.model_validate(raw)
    _CONFIG_CACHE[cache_key] = (signature, config)
    logger.debug("Config loaded: provider=%s", config.models.provider)
    return config


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


# Memoized parsed secrets keyed by resolved secrets path, with the same
# stat-signature (mtime_ns, size) invalidation as load_config.
_SECRETS_CACHE: Dict[Path, Tuple[Tuple[int, int, int], Dict[str, str]]] = {}


def load_local_secrets(project_root: Path = Path(".")) -> Dict[str, str]:
    secrets_path = project_root / ".devcouncil" / "secrets.env"
    try:
        stat = secrets_path.stat()
    except FileNotFoundError:
        return {}

    cache_key = secrets_path.resolve()
    # Include the inode so a delete+recreate under the same path (new inode) is treated
    # as changed even if the rewritten file lands with the same mtime and size.
    signature = (stat.st_mtime_ns, stat.st_size, stat.st_ino)
    cached = _SECRETS_CACHE.get(cache_key)
    if cached is not None and cached[0] == signature:
        # Copy so callers can't mutate the cached mapping.
        return dict(cached[1])

    secrets: Dict[str, str] = {}
    for line in secrets_path.read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#") or "=" not in stripped:
            continue
        key, value = stripped.split("=", 1)
        secrets[key.strip()] = value.strip().strip('"').strip("'")
    _SECRETS_CACHE[cache_key] = (signature, dict(secrets))
    return secrets


def get_gcloud_access_token() -> str | None:
    """Fetch a fresh gcloud access token by shelling out to ``gcloud``.

    This always spawns the subprocess (no caching) so callers that need a guaranteed
    fresh token — e.g. the Vertex provider refreshing after a 401/403 — get one. For
    the hot path (per-provider-construction key lookups) use
    :func:`get_cached_gcloud_access_token`, which memoizes the result with a TTL.
    """
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


# Cached gcloud access token + monotonic expiry. gcloud tokens last ~60 min, so the
# hot path (a fresh provider per role/run calling get_api_key) reuses a fetched token
# for a conservative window instead of spawning ``gcloud auth print-access-token`` on
# every lookup. A single gcloud identity is assumed, so no cache key is needed. A
# failed fetch (None) is never cached.
_GCLOUD_TOKEN_TTL_SECONDS = 50 * 60
_gcloud_token_cache: Optional[Tuple[str, float]] = None


def get_cached_gcloud_access_token() -> str | None:
    """Return a gcloud access token, reusing a recent one within the TTL window.

    Falls back to :func:`get_gcloud_access_token` on a cache miss/expiry. ``gcloud``
    absence is checked first so an environment without gcloud short-circuits to None
    without ever consulting the cache (preserving the uncached error/None behavior).
    """
    global _gcloud_token_cache

    if shutil.which("gcloud") is None:
        return None

    cached = _gcloud_token_cache
    if cached is not None and time.monotonic() < cached[1]:
        return cached[0]

    token = get_gcloud_access_token()
    if token:
        _gcloud_token_cache = (token, time.monotonic() + _GCLOUD_TOKEN_TTL_SECONDS)
    return token


def get_api_key(provider: str = "openrouter", project_root: Path = Path(".")) -> str:
    """Retrieve the API key for the configured provider from environment.
    
    Raises ValueError if not found.
    """
    env_var = provider_api_key_env_var(provider)
    key = os.environ.get(env_var) or load_local_secrets(project_root).get(env_var)
    if not key and _normalized_provider_name(provider) == "vertexai":
        key = get_cached_gcloud_access_token()
    if not key and _normalized_provider_name(provider) == "ollama":
        # Ollama is a local server and needs no API key; an explicitly-set
        # OLLAMA_API_KEY still flows through above if present.
        return ""
    if not key:
        logger.warning("Provider credentials not configured for %s", provider)
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
