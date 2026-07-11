# Certified Agent Paths

DevCouncil certifies one end-to-end closed loop. Other coding CLI integrations are
**best-effort adapters** until they pass the same golden fixture suite and lease-gated
write path documented here.

## Certified path (Stable)

| Agent | OS | Transport | Status |
| --- | --- | --- | --- |
| **Claude Code** | macOS, Linux | MCP (`devcouncil_*` tools) + optional hooks | **Certified / Stable** |
| **Claude Code** | macOS, Linux | Slash commands (`/devcouncil:*`) shelling to MCP | **Certified / Stable** |
| **Claude Code** | macOS, Linux | Subagent `devcouncil-implementer` | **Certified / Stable** |

The certified loop:

```
checkout_task → write_file/apply_patch → verify_task → next_actions → self-repair → release_task
```

Golden coverage lives in `tests/unit/test_mcp_closed_loop.py` and
`tests/unit/test_hero_loop_golden.py` (checkout → write → verify → repair → rollback → release).

### Lease contract (long runs)

| Failure | Code | Recovery |
| --- | --- | --- |
| TTL not yet expired | — | `devcouncil_renew_lease` before `expires_at` |
| TTL expired | `lease_expired` | `devcouncil_checkout_task` again (renew only works before TTL expiry) |
| Wrong token / no lease | `invalid_lease` | Checkout with correct `client_id` |
| Another agent holds task | `lease_held_by_other` | `devcouncil_next_task` or wait |

## Best-effort adapters (Preview)

These executors integrate through the coding CLI layer and hooks. They reuse the same
**verifier** and **next-actions** contract but are not certified for the full MCP closed
loop unless noted.

| Agent | OS | Transport | Status |
| --- | --- | --- | --- |
| Codex CLI | macOS, Linux | Hooks + `dev go` | Preview |
| Gemini CLI | macOS, Linux | Hooks + `dev go` | Preview |
| Cursor Agent | macOS, Linux, Windows | `.cursor/hooks.json` + MCP (partial) | Preview |
| Grok Build | macOS, Linux | `.grok/hooks/devcouncil.json` + MCP | Preview |
| OpenCode | macOS, Linux | Bundled plugin | Preview |
| Warp / Aider / Copilot / others | varies | `dev e2e --executor <name>` | Preview |

Use `dev integrate check` to confirm wiring. For production agent loops, prefer the
certified Claude Code MCP path above.

## Multi-agent campaigns

Large goals with dependency DAGs should use **`dev campaign`** (Director → Coordinator →
Worker pool + Reviewer QC), not the retired feudal-theme naming. Campaign mode:

- dispatches tasks in parallel waves respecting `depends_on`;
- serializes Reviewer verification by default (safe against git races);
- optional per-task leases when running with a real executor;
- writes progress to `.devcouncil/campaign/dashboard.md`.

See `dev campaign roster` for the role hierarchy.
