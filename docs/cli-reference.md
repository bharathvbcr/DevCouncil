# CLI Command Reference

```bash
dev init                    # Initialize DevCouncil in a repo
dev init --provider vertexai --model YOUR_MODEL_ID # Initialize with one model for every role
dev init --role-model planner_b=YOUR_MODEL_ID # Override one role model during init
dev setup                   # Initialize, run doctor, offer first-run integrations, and print next steps
dev doctor                  # Check dependencies and environment
dev version                 # Display the installed DevCouncil version
dev e2e "goal" --executor codex # Plan, execute, verify, and report in one command
dev e2e "goal" --executor codex --agent # Agent preset: JSON plus .devcouncil/reports/latest.json
dev e2e "goal" --executor codex --json --report-file .devcouncil/reports/latest.json # Write machine-readable report
dev go "goal" --executor codex # Short alias for dev e2e
dev map "goal"              # Map repo context for a goal
dev plan "goal"             # Run the full planning council debate
dev status                  # Show current project state and cost
dev tasks                   # List planned tasks and statuses
dev show TASK-001           # Show task details and constraints
dev prompt TASK-001         # Generate prompt for an external agent
dev run TASK-001            # Execute task via selected executor
dev verify TASK-001         # Verify diff, commands, and evidence
dev repair                  # Generate repair tasks from gaps
dev report                  # Generate final evidence report
dev report --github-pr-comment # Post the report as a GitHub PR comment
dev report --gitlab-pr-comment # Post the report as a GitLab MR comment
dev rollback TASK-001       # Revert changes using task checkpoint
dev mcp-server              # Start DevCouncil MCP server over stdio
dev integrate hooks --apply # Install native Codex, Gemini, and Claude hooks
dev hook --help             # Show lower-level hook commands
dev integrate all --apply   # Configure supported coding CLI integrations
dev integrate warp --apply  # Write Warp/Oz MCP config for DevCouncil
dev integrate cli-agent NAME --command TOOL --apply # Register any prompt-taking CLI executor
dev integrate check         # Verify coding CLI and MCP readiness
dev integrate doctor        # Check optional integration tools
dev agents                  # List built-in and custom CLI agents
dev agents add NAME --command TOOL # Register a prompt-taking CLI agent
dev agents doctor           # Check agent PATH, prompt mode, help command, and profile wiring
dev agents run TASK-001 --agent NAME --profile default # Run a task with a named CLI agent
dev lsp inspect             # Inspect optional language-server readiness
dev ast match "symbol"      # Search symbols with structural AST matching
dev dashboard               # Serve the live local status dashboard
dev trace tail --follow     # Tail local DevCouncil trace events
dev artifacts validate      # Validate stored artifact integrity
dev config                  # Inspect or update configuration
dev config models --model YOUR_MODEL_ID # Update every configured model role
dev config models --role-model critic_a=YOUR_MODEL_ID # Update one model role by name
```
