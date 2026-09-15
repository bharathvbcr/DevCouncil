package main

import "github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/version"

// Version is the product version printed by `devcouncil --version`.
//
// An alias, not a second literal: the constant is owned by
// `devcouncil/version` so that the MCP server can report the same value. Two
// literals that must agree are the shape that let `serverInfo` sit at 0.1.0
// through three releases.
const Version = version.Version
