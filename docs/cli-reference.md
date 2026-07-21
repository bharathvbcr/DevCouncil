# CLI Command Reference

```bash
dev init                    # Initialize DevCouncil in a repo
dev init --provider vertexai --model YOUR_MODEL_ID # Initialize with one model for every role
dev init --provider doubleword --model YOUR_MODEL_ID # Doubleword drop-in OpenAI-compatible provider
dev init --provider ollama --model qwen2.5-coder:32b # Local Ollama provider (no API key)
dev init --role-model planner_b=YOUR_MODEL_ID # Override one role model during init
dev setup                   # Initialize, run doctor, offer first-run integrations, and print next steps
dev doctor                  # Check dependencies and environment (includes subsystem maturity table)
dev version                 # Display the installed DevCouncil version
dev e2e "goal" --executor codex # Plan, execute, verify, and report in one command
dev e2e "goal" --executor codex --agent # Agent preset: JSON plus .devcouncil/reports/latest.json
dev e2e "goal" --executor codex --force  # Proceed past advisory planning gaps automatically
dev e2e "goal" --executor codex --json --report-file .devcouncil/reports/latest.json # Write machine-readable report
dev go "goal" --executor codex # Short alias for dev e2e
dev map                     # Build the deterministic repository map + code graph (no LLM)
dev map --if-stale          # Skip rebuild when the on-disk map fingerprint is still fresh
dev map --no-liveness       # Skip entry_roots / unwired / unreachable / dead_symbol lists
dev map --lsp-refs          # Confirm dead-symbol candidates via live LSP references
dev map --wiki / --no-wiki  # Refresh codebase-wiki skeletons after map (default on)
dev map --scan-deps         # Opt-in SCA auditors → dependency_risks (off by default)
dev map --watch             # Incrementally refresh the map on code edits
dev graph ingest            # Unified analyze: codeintel sync → graph export → repo map write
dev graph ingest PATH...    # Path-scoped ingest (full reconcile when paths omitted)
dev graph query NAME        # 360° symbol view: definition, callers, callees, importers
dev graph trace A B         # Shortest path between two graph nodes
dev graph dead              # Dead-code report with confidence tiers (extracted|inferred|ambiguous); uncapped
dev graph dead --min-confidence inferred  # Filter to inferred+extracted only
dev graph check             # God nodes (top-connected) and circular-import detection
dev graph process [ENTRY]   # BFS call-flows from entry roots
dev graph impact PATH...    # Blast radius for paths (or --diff for working-tree changes)
dev graph search QUERY      # FTS5 symbol/path search over the committed generation
dev graph search QUERY --semantic  # Opt-in local embeddings when indexing.embeddings.enabled
dev graph cypher 'MATCH … RETURN …'  # Supported Cypher subset over native SQLite graph store
dev graph explain --category command-injection  # PDG taint findings (opt-in PDG layer)
dev graph pdg-query --mode controls --target SYMBOL  # PDG control dependence
dev graph pdg-query --mode flows --target SYMBOL --variable x  # PDG data flows
dev graph html              # Write interactive .devcouncil/graph/graph.html (not written by default on dev map)
dev graph view              # Serve/open the graph HTML via a local HTTP server
dev graph demo              # Sample self-contained interactive HTML (no map required); see docs/code-graph.md
dev graph export -o out.graphml  # Export GraphML (or --format okf / okf-links)
dev scaffold-ci             # Write a starter .github/workflows/devcouncil.yml from configured commands
dev scaffold-ci --force     # Overwrite an existing devcouncil.yml workflow
dev scaffold-ci --evidence  # Also write .github/workflows/devcouncil-evidence.yml (verify → evidence artifacts)
dev boot "goal"             # One-command setup + integrate --apply + go (see quickstart)
dev boot "goal" --skip-integrations --scaffold-ci-evidence --executor codex # Opt out of integration apply; optional CI scaffold; pass executor to go
dev plan "goal"             # Run the full planning council debate
dev approve                 # Approve the latest generated plan (AWAITING_USER_DECISIONS -> PLAN_APPROVED)
dev approve --force         # Approve even if blocking gate gaps remain
dev approve --run-id RUN-ID # Approve a specific planning run's decision
dev check                   # LLM audit of current changes (no planning required)
dev check --verify -t "pytest -q" # Deterministic evidence gate on the working tree (no provider keys)
dev check --verify --enforce-coverage # Block when changed lines are not exercised by tests
dev check --json            # Machine-readable check output
dev status                  # Show current project state and cost
dev tasks                   # List planned tasks, statuses, and active lease owners (Lease column)
dev tasks cancel TASK-001   # Cancel a task that is not done or cancelled
dev tasks edit TASK-001 --title "New title" # Edit task metadata (title, priority, scope fields)
dev tasks reprioritize TASK-001 --priority high # Change task priority (high | medium | low)
dev gaps                    # List all verification gaps (blocking and advisory)
dev gaps --blocking-only --fail-on-blocking # Exit non-zero when blocking gaps remain
dev gaps --json             # Machine-readable gap list
dev requirements            # List requirements with derived status and linked task counts
dev requirements --json     # Machine-readable requirements summary
dev export                  # Write requirements, tasks, and gaps to .devcouncil/export/state.json
dev export --json           # Print export payload to stdout
dev export -o ./snapshot.json # Write export to a custom path
dev show TASK-001           # Show task details and constraints
dev prompt TASK-001         # Generate prompt for an external agent
dev run TASK-001            # Execute task via selected executor
dev run TASK-001 --executor copilot # Built-in executors: codex, claude, opencode, antigravity, warp, cursor, aider, copilot, goose, amp, qwen, crush (gemini deprecated — compat only)
dev verify TASK-001         # Verify diff, commands, and evidence
dev verify TASK-001 --sandbox local|docker|nix # Run verification in a sandbox
dev shell TASK-001 --command "pytest tests/" # Run one guarded shell command
dev watch fs --task TASK-001 --once           # Attribute filesystem changes once
dev semantic snapshot TASK-001 --stage before # Capture semantic snapshot
dev semantic diff TASK-001                    # Compare semantic before/after snapshots
dev evidence suggest TASK-001 --apply         # Append high-confidence expected tests
dev handoff TASK-001 --from codex --to aider  # Write agent handoff manifest
dev repair                  # Generate repair tasks from gaps
dev report                  # Generate final evidence report
dev report --github-pr-comment # Post the report as a GitHub PR comment
dev report --gitlab-pr-comment # Post the report as a GitLab MR comment
dev okf export -o ./bundle   # Export the artifact graph as an Open Knowledge Format bundle
dev okf validate ./bundle    # Validate an OKF bundle (typed docs, resolved links)
dev okf ingest ./bundle      # Ingest an OKF bundle as planning/coding context
dev okf html ./bundle -o ./site # Render an OKF bundle as a self-contained static HTML site
dev design lint              # Lint the project design.md (refs, contrast, ordering)
dev design export -f css     # Export design tokens (css | tailwind | w3c)
dev design show              # Summarize design tokens and sections
dev design check [files...]  # Fail on hardcoded color/spacing/typography literals that bypass design.md tokens (CI-friendly, exits non-zero)
dev rollback TASK-001       # Revert changes using task checkpoint
dev mcp-server              # Start DevCouncil MCP server over stdio
dev integrate hooks --apply # Install Codex, Claude, Cursor, Grok, and OpenCode hooks (Gemini excluded from --tool all; deprecated explicit --tool gemini)
dev integrate aider --apply   # Enable built-in Aider headless executor
dev hook --help             # Show lower-level hook commands
dev integrate all --apply   # Configure supported coding CLI integrations
dev integrate cursor --apply # Write project Cursor MCP config for DevCouncil
dev integrate opencode --apply # Write project OpenCode MCP config for DevCouncil
dev integrate antigravity --apply # Write project Antigravity MCP config for DevCouncil
dev integrate warp --apply  # Write Warp/Oz MCP config for DevCouncil
dev integrate cli-agent NAME --command TOOL --apply # Register any prompt-taking CLI executor
dev integrate recommend
dev integrate status
dev integrate status --json
dev integrate matrix
dev integrate check
dev integrate check --strict
dev integrate check --json
dev integrate check --report-file .devcouncil/integration-report.json
dev integrate check -o .devcouncil/integration-report.json
dev run TASK --stream       # Stream coding CLI output live during execution
dev integrate all --apply --strict  # Apply integrations then run strict check
dev go GOAL                 # Auto-picks first coding CLI on PATH when default_executor is manual
dev integrate doctor        # Check optional integration tools
dev agents                  # List built-in and custom CLI agents
dev agents add NAME --command TOOL # Register a prompt-taking CLI agent
dev agents doctor           # Check agent PATH, prompt mode, help command, and profile wiring
dev agents run TASK-001 --agent NAME --profile default # Run a task with a named CLI agent
dev agents optimize --agent codex --profile yolo --evals .devcouncil/evals/agent-profile.jsonl --dry-run # GEPA prompt-profile optimization
dev skills                  # List bundled engineering skills and which apply to this repo
dev skills show NAME        # Print the full body of one skill
dev skills scaffold         # Write applicable skills to .claude/skills/<name>/SKILL.md
dev cost show               # Report estimated model-call cost grouped by task and run
dev cost show --json        # Machine-readable cost report
dev runs list               # List recorded coding-agent runs, newest first
dev runs list --json        # Machine-readable run summaries (includes orphaned flag)
dev runs list --status running --limit 10 # Filter by status; default limit 20
dev runs show RUN-ID        # Show a run manifest plus a redacted transcript tail
dev runs show RUN-ID --json # Full manifest, orphaned flag, and redacted transcript tail
dev runs timeline REF       # Full reversible trace for a run id or task id (events, checkpoints, diff stat)
dev runs timeline REF --json --limit 40 # JSON timeline; default limit 40 events
dev runs diff REF           # Workspace changes the run produced (from git checkpoints)
dev runs diff REF --stat    # Diff stat only
dev runs revert REF         # Reverse workspace effects (prompts for confirmation)
dev runs revert REF --yes   # Skip confirmation (-y)
dev runs supervise REF      # Supervisor verdict: keep | revert | repair (default --llm)
dev runs supervise REF --no-llm # Deterministic heuristics only (no run_supervisor model role)
dev runs supervise REF --apply  # CLI-only: revert immediately when verdict is revert
dev runs supervise REF --json   # Machine-readable verdict payload
dev lsp inspect             # Inspect optional language-server readiness
dev lsp inspect --json        # Compact {mode, servers_detected, note} JSON for automation
dev ast match "symbol"      # Search symbols with structural AST matching
dev dashboard --open        # Serve the live local status dashboard and open a browser
dev trace tail --follow     # Tail local DevCouncil trace events
dev logs tail -n 100        # Tail the durable run log (.devcouncil/logs/devcouncil.log)
dev logs tail -f --grep ERROR # Follow the log, showing only matching lines
dev logs tail --run RUN-ID  # Read one executor run's isolated run.log
dev logs runs               # List per-run logs, newest first
dev <command> -v | -vv | -q # Raise/lower console log verbosity (file always DEBUG)
dev artifacts validate      # Validate stored artifact integrity
dev config                  # Inspect or update configuration
dev config show             # Display key DevCouncil settings (executor, rigor, gates)
dev config set semantic_layer.enabled true  # Enable semantic LLM cache/routing/compression (uv sync --group semantic)
dev config set semantic_layer.cache.enabled true # Toggle FAISS semantic cache (default on when layer enabled)
dev config set semantic_layer.router.enabled true # Opt-in complexity routing for local Ollama tiers
dev config set semantic_layer.compressor.enabled true # Toggle long-context compression before LLM calls
dev config set execution.command_timeout 600 # Set a common dotted config key
dev config set execution.stop_gate.mode assist # Stop-hook claim+verify gate (off|assist|block); see coding-cli-integration.md
dev corpus build            # Build advisory doc/PDF/image corpus graph (config.yaml paths)
dev corpus query "topic"    # Search corpus concepts
dev corpus status           # Corpus freshness vs doc fingerprints
dev config models --model YOUR_MODEL_ID # Update every configured model role
dev config models --role-model critic_a=YOUR_MODEL_ID # Update one model role by name
dev wiki update             # Generate/refresh the agent-facing codebase wiki (OKF bundle)
dev wiki status             # Report wiki freshness vs the current repo map
dev campaign run            # Parallel multi-agent campaign over the planned task graph
dev campaign roster         # Show campaign role hierarchy
dev campaign inbox          # Inspect the on-disk campaign mailbox
dev baseline                # Capture a verification baseline of current changes
dev optimize --agent codex --profile yolo --evals .devcouncil/evals/agent-profile.jsonl # Alias for dev agents optimize
dev reset-demo-state        # Clear demo planning artifacts from the local DevCouncil state
dev integrations status     # Alias for dev integrate status
```
