# CLI Command Reference

```bash
dev init                    # Initialize DevCouncil in a repo
dev init --provider vertexai --model YOUR_MODEL_ID # Initialize with one model for every role
dev init --provider doubleword --model YOUR_MODEL_ID # Doubleword drop-in OpenAI-compatible provider
dev init --provider ollama --model qwen2.5-coder:32b # Local Ollama provider (no API key)
dev init --role-model planner_b=YOUR_MODEL_ID # Override one role model during init
dev setup                   # Initialize, run doctor, offer first-run integrations, and print next steps
dev doctor                  # Check dependencies and environment
dev version                 # Display the installed DevCouncil version
dev e2e "goal" --executor codex # Plan, execute, verify, and report in one command
dev e2e "goal" --executor codex --agent # Agent preset: JSON plus .devcouncil/reports/latest.json
dev e2e "goal" --executor codex --force  # Proceed past advisory planning gaps automatically
dev e2e "goal" --executor codex --json --report-file .devcouncil/reports/latest.json # Write machine-readable report
dev go "goal" --executor codex # Short alias for dev e2e
dev map                     # Build the deterministic repository map (no LLM)
dev scaffold-ci             # Write a starter .github/workflows/devcouncil.yml from configured commands
dev scaffold-ci --force     # Overwrite an existing devcouncil.yml workflow
dev plan "goal"             # Run the full planning council debate
dev approve                 # Approve the latest generated plan (AWAITING_USER_DECISIONS -> PLAN_APPROVED)
dev approve --force         # Approve even if blocking gate gaps remain
dev approve --run-id RUN-ID # Approve a specific planning run's decision
dev check                   # LLM audit of current changes (no planning required)
dev check --verify -t "pytest -q" # Deterministic evidence gate on the working tree (no provider keys)
dev check --verify --enforce-coverage # Block when changed lines are not exercised by tests
dev check --json            # Machine-readable check output
dev status                  # Show current project state and cost
dev tasks                   # List planned tasks and statuses
dev show TASK-001           # Show task details and constraints
dev prompt TASK-001         # Generate prompt for an external agent
dev run TASK-001            # Execute task via selected executor
dev run TASK-001 --executor copilot # Built-in executors: codex, gemini, claude, opencode, antigravity, warp, cursor, aider, copilot, goose, amp, qwen, crush
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
dev integrate hooks --apply # Install Codex, Gemini, Claude, Cursor, and OpenCode hooks
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
dev runs show RUN-ID        # Show a run manifest plus a redacted transcript tail
dev lsp inspect             # Inspect optional language-server readiness
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
dev config models --model YOUR_MODEL_ID # Update every configured model role
dev config models --role-model critic_a=YOUR_MODEL_ID # Update one model role by name
dev baseline                # Capture a verification baseline of current changes
dev optimize --agent codex --profile yolo --evals .devcouncil/evals/agent-profile.jsonl # Alias for dev agents optimize
dev reset-demo-state        # Clear demo planning artifacts from the local DevCouncil state
dev integrations status     # Alias for dev integrate status
```
