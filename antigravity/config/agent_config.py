"""
agent_config.py - Unified MCP Server Configuration for Google Antigravity.

Configures stdio and streamable HTTP transports with FastMCP identifiers
and strict tool allowlists for DevCouncil, DevMap, and GitPulse.
"""

import os
import re
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Dict, List, Optional, Union

# FastMCP Identifier validation invariant (Invariant B3)
FASTMCP_IDENTIFIER_PATTERN = re.compile(r"^[a-zA-Z_][a-zA-Z0-9_]*$")


def validate_fastmcp_identifier(name: str) -> bool:
    """Validates that a tool name satisfies FastMCP / Python identifier rules."""
    return bool(FASTMCP_IDENTIFIER_PATTERN.match(name))


class TransportType(str, Enum):
    STDIO = "stdio"
    HTTP = "http"
    STREAMABLE_HTTP = "streamable_http"


class AgentBehavior(str, Enum):
    AUTONOMOUS = "AUTONOMOUS"
    INTERACTIVE = "INTERACTIVE"
    ASSISTANT = "ASSISTANT"


@dataclass
class BaseMcpServerConfig:
    name: str
    enabled_tools: List[str] = field(default_factory=list)

    def __post_init__(self):
        for tool in self.enabled_tools:
            if not validate_fastmcp_identifier(tool):
                raise ValueError(
                    f"Tool '{tool}' in server '{self.name}' violates FastMCP identifier requirements."
                )


@dataclass
class McpStdioServer(BaseMcpServerConfig):
    command: str = ""
    args: List[str] = field(default_factory=list)
    env: Dict[str, str] = field(default_factory=dict)
    transport: str = "stdio"


@dataclass
class McpStreamableHttpServer(BaseMcpServerConfig):
    url: str = ""
    headers: Dict[str, str] = field(default_factory=dict)
    fallback_command: Optional[str] = None
    fallback_args: List[str] = field(default_factory=list)
    transport: str = "streamable_http"


@dataclass
class PolicyRule:
    rule_type: str  # "allow" or "deny"
    target: Any  # server config, tool name, or capability


@dataclass
class RunCommandConfig:
    enable_sandbox: bool = True
    timeout_seconds: int = 300
    allow_network: bool = False


@dataclass
class CapabilitiesConfig:
    agent_behavior: AgentBehavior = AgentBehavior.AUTONOMOUS
    run_command_config: RunCommandConfig = field(default_factory=RunCommandConfig)


@dataclass
class LocalAgentConfig:
    """Google Antigravity LocalAgentConfig structure."""
    model: str = "gemini-3.8-flash"
    mcp_servers: List[BaseMcpServerConfig] = field(default_factory=list)
    policies: List[PolicyRule] = field(default_factory=list)
    workspaces: List[str] = field(default_factory=list)
    capabilities: CapabilitiesConfig = field(default_factory=CapabilitiesConfig)
    skills_paths: List[str] = field(default_factory=list)

    def get_server(self, name: str) -> Optional[BaseMcpServerConfig]:
        for s in self.mcp_servers:
            if s.name == name:
                return s
        return None

    def list_all_tools(self) -> List[str]:
        tools: List[str] = []
        for s in self.mcp_servers:
            tools.extend(s.enabled_tools)
        return tools


# Canonical Tool Catalogs
DEVCOUNCIL_TOOLS = [
    "devcouncil_checkout_task",
    "devcouncil_renew_lease",
    "devcouncil_release_task",
    "devcouncil_next_task",
    "devcouncil_verify_task",
    "devcouncil_get_gaps",
    "devcouncil_get_diff",
    "devcouncil_policy_check_write",
]

DEVMAP_TOOLS = [
    "devmap_status",
    "devmap_search",
    "devmap_explore",
    "devmap_dependencies",
    "devmap_impact",
    "devmap_trace",
    "devmap_neighbors",
    "devmap_dead_symbols",
    "devmap_clones",
    "devmap_preview",
    "devmap_affected_tests",
    "devmap_blast",
    "devmap_suspects",
    "devmap_ask_evidence",
    "devmap_doc_drift",
]

GITPULSE_TOOLS = [
    "gitpulse_insights",
    "gitpulse_status",
    "gitpulse_active_changes",
    "gitpulse_collision_risk",
    "gitpulse_merge_conflicts",
    "gitpulse_change_context",
    "gitpulse_ledger_events",
    "gitpulse_task_view",
    "gitpulse_add_task",
    "gitpulse_list_tasks",
    "gitpulse_get_task",
    "gitpulse_complete_task",
    "gitpulse_delete_task",
    "gitpulse_worktree_create",
    "gitpulse_worktree_switch",
    "gitpulse_worktree_isolate",
    "gitpulse_worktree_prune",
    "gitpulse_worktree_merge",
]


def create_unified_antigravity_config(
    workspace_root: Optional[str] = None,
    devmap_daemon_port: int = 9095,
    use_http_for_devmap: bool = True,
    model: str = "gemini-3.8-flash",
) -> LocalAgentConfig:
    """
    Constructs a fully configured LocalAgentConfig hosting DevCouncil,
    DevMap, and GitPulse MCP servers with zero-collision tool namespaces.
    """
    ws = workspace_root or os.getcwd()
    mcp_servers: List[BaseMcpServerConfig] = []

    # 1. DevCouncil MCP Server (Stdio Transport)
    devcouncil_server = McpStdioServer(
        name="devcouncil",
        command="devcouncil",
        args=["mcp", "serve"],
        env={"DEVCOUNCIL_WORKSPACE": ws},
        enabled_tools=list(DEVCOUNCIL_TOOLS),
    )
    mcp_servers.append(devcouncil_server)

    # 2. DevMap MCP Server (Streamable HTTP or Stdio Transport)
    if use_http_for_devmap:
        devmap_server = McpStreamableHttpServer(
            name="devmap",
            url=f"http://127.0.0.1:{devmap_daemon_port}/mcp",
            headers={"X-Workspace-Root": ws},
            fallback_command="devmap",
            fallback_args=["serve", "--mcp", "--root", ws],
            enabled_tools=list(DEVMAP_TOOLS),
        )
    else:
        devmap_server = McpStdioServer(
            name="devmap",
            command="devmap",
            args=["serve", "--mcp", "--root", ws],
            enabled_tools=list(DEVMAP_TOOLS),
        )
    mcp_servers.append(devmap_server)

    # 3. GitPulse MCP Server (Stdio Transport)
    gitpulse_server = McpStdioServer(
        name="gitpulse",
        command="gitpulse-mcp",
        args=["--repo", ws],
        env={"GITPULSE_REPOSITORY": ws},
        enabled_tools=list(GITPULSE_TOOLS),
    )
    mcp_servers.append(gitpulse_server)

    # 4. Declarative Safety Policies
    policies = [
        PolicyRule(rule_type="allow", target=devcouncil_server.name),
        PolicyRule(rule_type="allow", target=devmap_server.name),
        PolicyRule(rule_type="allow", target=gitpulse_server.name),
        PolicyRule(rule_type="allow", target="view_file"),
        PolicyRule(rule_type="allow", target="list_directory"),
        PolicyRule(rule_type="allow", target="search_directory"),
        PolicyRule(rule_type="allow", target="find_file"),
        PolicyRule(rule_type="deny", target="*.env*"),
        PolicyRule(rule_type="deny", target="*id_rsa*"),
    ]

    skills_paths = [
        os.path.join(ws, ".agents", "skills"),
        os.path.expanduser("~/.gemini/antigravity/skills"),
    ]

    return LocalAgentConfig(
        model=model,
        mcp_servers=mcp_servers,
        policies=policies,
        workspaces=[ws],
        capabilities=CapabilitiesConfig(
            agent_behavior=AgentBehavior.AUTONOMOUS,
            run_command_config=RunCommandConfig(enable_sandbox=True),
        ),
        skills_paths=skills_paths,
    )
