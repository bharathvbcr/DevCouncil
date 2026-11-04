---
description: DevCouncil navigation, and the task loop when you want it
trigger: always_on
---

# DevCouncil

Navigate with DevMap before reading or grepping: `devmap_explore`, `devmap_search`, `devmap_ask_evidence` (a behaviour whose name you do not know: files, source and tests in one answer), `devmap_impact`, `devmap_trace`, `devmap_neighbors`, `devmap_dead_symbols`, `devmap_affected_tests` — or the matching `devmap` CLI commands. Run impact analysis before editing a symbol, and read `truncated` / `total` on every envelope before treating a list as complete. When DevMap cannot answer, record a gap in `.devcouncil/codeintel/sessions/gaps.jsonl` rather than switching to another index.

`.devcouncil/repo_map.json` is the file-level index (subsystems, entry_points, critical_files).

## Product stack

DevCouncil is **components and modules** (`devmap`, `dcstore`, `dcverify`, `dcgrep`, and this host). **Manvi** wraps them into a coding-agent harness. Host apps such as **GitPulse** use Manvi for policy, workbench and agent hosting, and DevCouncil components for code intelligence and related analysis. Take only the modules an app needs; update them independently.

## Tasks and gates are opt-in

DevCouncil host hooks are retired, so **interactive Shell and Write need no task lease**. Do not claim "Shell is gated", and do not check out a task in order to run a command. Use the `devcouncil_*` MCP tools when you actually want task state, scope or verification — and prefer them over guessing it. A host that uses MCP policy must honor its results. The `--write-gate` flag has been removed rather than kept as a refusal, and `integrations.<host>.write_gate` is no longer read: both named a pre-tool-use write gate that only the retired hooks ever installed. A copy left in an existing `config.yaml` is inert.

Engineering skills live under `.agents/skills` (`devcouncil skills scaffold`).

## Agents

The session agent `devcouncil` delegates file-changing work to `devcouncil-implementer`, diff review to `devcouncil-reviewer`, and gap reporting to `devcouncil-verifier`. Delegate review and verification so they do not share the implementer's context.
