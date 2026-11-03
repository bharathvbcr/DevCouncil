"""
antigravity.config package.

Provides unified MCP configuration and tool bridge for DevCouncil, DevMap, and GitPulse.
"""

from .agent_config import (
    BaseMcpServerConfig,
    CapabilitiesConfig,
    LocalAgentConfig,
    McpStdioServer,
    McpStreamableHttpServer,
    PolicyRule,
    TransportType,
    create_unified_antigravity_config,
    validate_fastmcp_identifier,
    DEVCOUNCIL_TOOLS,
    DEVMAP_TOOLS,
    GITPULSE_TOOLS,
)
from .mcp_bridge import (
    call_mcp_tool,
    register_in_process_handler,
    clear_in_process_handlers,
)

__all__ = [
    "BaseMcpServerConfig",
    "CapabilitiesConfig",
    "LocalAgentConfig",
    "McpStdioServer",
    "McpStreamableHttpServer",
    "PolicyRule",
    "TransportType",
    "create_unified_antigravity_config",
    "validate_fastmcp_identifier",
    "DEVCOUNCIL_TOOLS",
    "DEVMAP_TOOLS",
    "GITPULSE_TOOLS",
    "call_mcp_tool",
    "register_in_process_handler",
    "clear_in_process_handlers",
]
