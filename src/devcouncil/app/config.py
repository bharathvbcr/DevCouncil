"""Typed configuration loader with Pydantic validation.

Replaces scattered yaml.safe_load() calls with a single validated config service.
"""

from __future__ import annotations

import os
import shutil
import subprocess
from pathlib import Path
from typing import Dict, List

from pydantic import BaseModel, Field


class ModelRoleConfig(BaseModel):
    model: str
    temperature: float = 0.0


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


class IntegrationsConfig(BaseModel):
    agent_flow: AgentFlowIntegrationConfig = Field(default_factory=AgentFlowIntegrationConfig)
    code_review_graph: CodeReviewGraphIntegrationConfig = Field(default_factory=CodeReviewGraphIntegrationConfig)
    live_review: LiveReviewIntegrationConfig = Field(default_factory=LiveReviewIntegrationConfig)
    cursor: CursorIntegrationConfig = Field(default_factory=CursorIntegrationConfig)
    aider: AiderIntegrationConfig = Field(default_factory=AiderIntegrationConfig)
    antigravity: AntigravityIntegrationConfig = Field(default_factory=AntigravityIntegrationConfig)
    warp: WarpIntegrationConfig = Field(default_factory=WarpIntegrationConfig)
    opencode: OpenCodeIntegrationConfig = Field(default_factory=OpenCodeIntegrationConfig)
    cli_agents: CliAgentsIntegrationConfig = Field(default_factory=CliAgentsIntegrationConfig)


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
    privacy: PrivacyConfig = Field(default_factory=PrivacyConfig)
    integrations: IntegrationsConfig = Field(default_factory=IntegrationsConfig)


def load_config(project_root: Path = Path(".")) -> DevCouncilConfig:
    """Load and validate .devcouncil/config.yaml.
    
    Returns DevCouncilConfig with defaults for any missing fields.
    Raises FileNotFoundError if config doesn't exist.
    """
    import yaml  # type: ignore[import-untyped]

    config_path = project_root / ".devcouncil" / "config.yaml"
    if not config_path.exists():
        raise FileNotFoundError(f"Config not found at {config_path}. Run 'dev init' first.")

    with open(config_path) as f:
        raw = yaml.safe_load(f) or {}

    return DevCouncilConfig.model_validate(raw)


def provider_api_key_env_var(provider: str = "openrouter") -> str:
    normalized = provider.strip().lower().replace("-", "").replace("_", "")
    env_map = {
        "openrouter": "OPENROUTER_API_KEY",
        "vertexai": "VERTEXAI_ACCESS_TOKEN",
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
