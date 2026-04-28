# Model Routing

DevCouncil implements `ModelRouter` and `Provider` architectures.
It currently defaults to `OpenRouter` to access `claude-3-5-sonnet`, `gemini-1.5-pro`, and `gpt-4o` dynamically.

A user defines roles for `spec_writer`, `planner_a`, `planner_b`, `critic_a`, `critic_b`, `arbiter`, and `native_agent` in `.devcouncil/config.yaml`.
The `ModelRouter` wraps `JSON` enforcement using standard JSON parsing or fallback prompts.
