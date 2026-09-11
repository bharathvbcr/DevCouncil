# Model Routing

> **Architectural boundary:** LLM provider routing (OpenRouter, Vertex AI, Doubleword, Ollama) and role models are owned by upstream agent harnesses such as **Manvi**. See [PHASE7_LONG_TAIL.md](PHASE7_LONG_TAIL.md). DevCouncil is the native Go/Rust verification and code intelligence substrate; it does not require model provider keys.

The Python `ModelRouter`, `src/devcouncil/llm/model_defaults.yaml`, and the `dev setup` / `dev doctor` / `dev cost` commands were deleted with the Phase 7 orchestrator cut (`3286db5`). There is no `ModelRouter` in the Go host.

Configure providers and role models in **Manvi**, not in this repository. `.devcouncil/config.yaml` may still carry a leftover `models:` block from the Python era; the Go host does not read it to call an LLM.

Council role names that used to live here (`planner_a` / `planner_b`, `critic_a` / `critic_b`, `arbiter`, `spec_writer`) were a Python debate pool. Manvi has related subagent roles (`critic`, `planner`); that is a related job, not a bug-for-bug port of A/B debate.
