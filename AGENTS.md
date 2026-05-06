<!-- Managed by dev map: keep this file in sync with .devcouncil/repo_map.json. -->

# Agent Workspace Guide

Use `.devcouncil/repo_map.json` as the primary file index for this workspace.
Repo map: `.devcouncil/repo_map.json`

Workflow for agents:
1. Open `.devcouncil/repo_map.json` before guessing at file locations.
2. Use the `files` list to resolve module ownership and nearby siblings.
3. Use `subsystems` for subsystem-level navigation (execution, verification, storage, etc.).
4. In `subsystems`, use `entry_points` + `critical_files` for entry points and starting context.
5. Use `role_files` in `subsystems` for subsystem role buckets (entry, runtime, policy, adapters, etc.).
6. Use `neighbors` and `handoff_paths` in `subsystems` to follow cross-subsystem flow.
7. Run `dev map` again after large refactors to refresh the map.

Important surfaces:
1. `src/devcouncil/cli/main.py` for CLI composition.
2. `src/devcouncil/app/orchestrator.py` and `src/devcouncil/app/state_machine.py` for lifecycle control.
3. `src/devcouncil/artifacts/graph.py` and `src/devcouncil/storage/repositories.py` for persistence and evidence.
4. `src/devcouncil/execution/` and `src/devcouncil/executors/` for task execution.
5. `src/devcouncil/verification/` and `src/devcouncil/gating/` for verification and policy gates.
6. `src/devcouncil/storage/` for persistence, SQL models, and repositories.

If the map and source disagree, trust the source and regenerate the map.
