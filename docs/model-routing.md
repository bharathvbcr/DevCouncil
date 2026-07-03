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
- `ollama`: local models served by Ollama; needs NO API key. Talks to Ollama's native `/api/chat` endpoint (derived from the base URL) so `num_ctx` and JSON `format` are honored. Override the server with `OLLAMA_BASE_URL` (taken verbatim) or Ollama's native `OLLAMA_HOST` (scheme and `/v1` are auto-normalized). **Set `OLLAMA_NUM_CTX` (recommend `16384`)** — DevCouncil's planning prompts can reach ~15k tokens, and Ollama's small default context (~2048–4096) would silently truncate them; `dev doctor` warns when it is unset or too small. The council roles emit structured JSON, so prefer a capable local model (≥27B, e.g. `qwen2.5-coder:32b` / `gemma2:27b` / `command-r:35b`).

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

- OpenRouter: `nvidia/nemotron-3-ultra-550b-a55b:free` (default), `anthropic/claude-sonnet-4.6`, `openai/gpt-5.5`
- Vertex AI: `google/gemini-2.5-flash`
- Doubleword: `deepseek/deepseek-v4`
- Ollama: `qwen2.5-coder:7b`, `llama3.1` (any locally-pulled model tag; sent verbatim to Ollama)

For local Ollama (no API key required):

```bash
dev setup --provider ollama --model qwen2.5-coder:7b
```

### macOS / Apple Silicon (local models)

On a Mac the local model has to fit in **unified memory** (shared by CPU, GPU,
and the OS), so RAM is the practical ceiling on model size. DevCouncil is
Apple-Silicon-aware here:

- `dev doctor` reports the chip and RAM, pings the local Ollama server, and
  recommends a model that will fit.
- `dev setup --provider ollama` (with no `--model`) auto-selects a size for the
  detected RAM instead of the static 7b default.
- `scripts/install.sh` prints the same guidance after a macOS install.

Recommended `qwen2.5-coder` size by memory (the council roles emit structured
JSON, so larger is better when it fits):

| Unified memory | Recommended model      |
| -------------- | ---------------------- |
| ≥ 48 GB        | `qwen2.5-coder:32b`    |
| 24–47 GB       | `qwen2.5-coder:14b`    |
| < 24 GB        | `qwen2.5-coder:7b`     |

Typical first-run on Apple Silicon:

```bash
brew install ollama && ollama serve      # if not already running
ollama pull qwen2.5-coder:32b            # use the size dev doctor recommends
export OLLAMA_NUM_CTX=16384              # avoid truncating large planning prompts
dev setup --provider ollama             # auto-picks the model for your RAM
dev doctor                              # confirm server + context window
```

Ollama itself handles Metal GPU acceleration on Apple Silicon — no DevCouncil
configuration is required for it.
