# CLI Command Reference

```bash
dev init                    # Initialize DevCouncil in a repo
dev setup                   # Initialize, run doctor, offer first-run integrations, and print next steps
dev doctor                  # Check dependencies and environment
dev version                 # Display the installed DevCouncil version
dev e2e "goal" --executor codex # Plan, execute, verify, and report in one command
dev e2e "goal" --agent      # Agent preset: JSON plus .devcouncil/reports/latest.json
dev e2e "goal" --json --report-file .devcouncil/reports/latest.json # Write machine-readable report
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
dev integrate check         # Verify coding CLI and MCP readiness
dev integrate doctor        # Check optional integration tools
dev lsp inspect             # Inspect optional language-server readiness
dev ast match "symbol"      # Search symbols with structural AST matching
dev dashboard               # Serve the live local status dashboard
dev trace tail --follow     # Tail local DevCouncil trace events
dev artifacts validate      # Validate stored artifact integrity
dev config                  # Inspect or update configuration
```
