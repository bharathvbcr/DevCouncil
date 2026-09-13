# Expanded DevMap comparison

Task 52d63a1a-266d-4a5a-8a09-9b8458a98334.

Completed additional tools: Graphify 0.9.59 (official PyPI package graphifyy), Gortex 0.64.3, CodeGraph 1.6.0, codebase-memory-mcp 0.10.8; retain GitNexus 1.6.9 and ripgrep 15.1.0.

Use official macOS arm64 release binaries with published SHA-256 checks; Graphify gets a separate uv-created virtual environment with its declared core dependencies. Tool files, state, and any temporary daemon stay under /Users/bharath/Code/devtools/DevCouncil/.devcouncil/benchmarks/20260912-expanded. No global agent registration, project dependency manifest change, or hosted model call is part of this run.

The shared corpus is the same 1,186-file DevCouncil commit ee07c183b127e7291c38f993f4d71ca75f1121dc used by the initial report. Each tool gets its own clone. Cold builds and one-file edits get three repetitions; unchanged refreshes and queries get five. Record raw commands, stdout, stderr, timing, process-reported RSS, storage, versions and checksums. Distinguish one-shot CLI timing from persistent daemon query timing and preserve unsupported or failed cases.

Reuse the original three language-spanning symbol queries and five source-inspected direct caller pairs, plus update, restore, and missing-name checks. All results remain local benchmark artifacts.

Exact download URLs, hashes, paths, and protocol are in [plan.json](plan.json).

Completed results and limitations: [REPORT.md](REPORT.md). All 261 competitive timing samples completed; correctness findings are preserved in audit.json.
