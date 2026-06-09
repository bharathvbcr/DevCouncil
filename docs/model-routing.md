# Model Routing

DevCouncil implements `ModelRouter` and `Provider` architectures.
Provider-specific role defaults are loaded from `src/devcouncil/llm/model_defaults.yaml` and can be replaced at initialization time.
It also supports Vertex AI through Google's OpenAI-compatible Chat Completions endpoint.

A user defines roles for `spec_writer`, `planner_a`, `planner_b`, `critic_a`, `critic_b`, `arbiter`, and `native_agent` in `.devcouncil/config.yaml`.
The `ModelRouter` wraps `JSON` enforcement using standard JSON parsing or fallback prompts.

Use `dev init --model YOUR_MODEL_ID`, `dev setup --model YOUR_MODEL_ID`, or `dev config models --model YOUR_MODEL_ID` to set all role models without hand-editing YAML. Add `--role-model ROLE=MODEL` for role-specific overrides.

## Providers

Supported `models.provider` values:

- `openrouter`: uses `OPENROUTER_API_KEY`.
- `vertexai`: uses `VERTEXAI_ACCESS_TOKEN` or `gcloud auth print-access-token`, `VERTEXAI_PROJECT` or `GOOGLE_CLOUD_PROJECT`, and optional `VERTEXAI_LOCATION` defaulting to `global`.
- `doubleword`: uses `DOUBLEWORD_API_KEY` and the OpenAI-compatible chat API at `https://api.doubleword.ai/v1`.

Vertex AI uses Google Cloud Auth access tokens, not long-lived OpenRouter-style API keys. For local use, generate a token with:

```bash
gcloud auth print-access-token
```

Then configure DevCouncil:

```bash
export VERTEXAI_PROJECT=your-gcp-project
export VERTEXAI_LOCATION=global
dev setup --provider vertexai --api-key "$(gcloud auth print-access-token)"
```

Or store the project and location in local DevCouncil secrets:

```bash
dev setup --provider vertexai --vertex-project your-gcp-project --vertex-location global --api-key "$(gcloud auth print-access-token)"
```

Google Cloud access tokens are short-lived. If `VERTEXAI_ACCESS_TOKEN` is not set in the environment or local secrets, DevCouncil falls back to `gcloud auth print-access-token`.

Use provider-specific model names in role config, for example:

- OpenRouter: `anthropic/claude-sonnet-4.6`, `openai/gpt-5.5`, `google/gemini-2.5-pro`
- Vertex AI: `google/gemini-2.5-flash`
- Doubleword: `deepseek/deepseek-v4`
