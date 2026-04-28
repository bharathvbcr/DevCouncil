# DevCouncil Architecture

DevCouncil is a gated orchestrator for AI-assisted software development. It ensures that AI-generated work proves it satisfies the original intent.

## Core Components
- **CLI**: A Typer-based command-line interface.
- **Orchestrator & State Machine**: Manages the transitions between planning, execution, and verification phases.
- **Artifact Graph**: A directed graph representing the relationship between requirements, tasks, files, evidence, and gaps.
- **Planning Council**: Multi-agent LLM debate for planning and critique.
- **Executors**: Adapters to run tasks via manual, mini-SWE-agent, OpenHands, or native execution.
- **Verifier & Gating**: Checks for Git cleanliness, authorized file modifications, and test evidence.
