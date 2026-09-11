# Phase 4 — Go `devcouncil` binary

Binary: `backend/go_orchestrator/cmd/devcouncil`  
Install: `go build -o ~/.local/bin/devcouncil ./cmd/devcouncil` (from `backend/go_orchestrator`)

| Command | Purpose |
|---|---|
| `devcouncil mcp` | Hand-rolled JSON-RPC MCP stdio server |
| `devcouncil mcp-server` | Alias for hosts still using the Python argv |
| `devcouncil integrate <host> [--apply\|--check\|--dry-run]` | Host MCP/rules/hooks; spawns `devmap integrate` / `skills install` |
| `devcouncil skills scaffold [--skill NAME]` | Embed.FS domain skills (17) with Python registry bounds |

Manvi must not be imported from this module tree. Shared get_diff/lease live in
`devcouncil/`; Manvi keeps `manvi/devcouncil` for harness-coupled tools.

Golden replay: `go test ./devcouncil/...` and `go run ./cmd/devcouncil-golden-replay`.
