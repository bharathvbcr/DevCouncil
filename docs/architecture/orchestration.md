# Orchestration Flow

The orchestration flow in DevCouncil is designed to be **evidence-driven** and **gated**. It moves through distinct phases where each step must produce artifacts that satisfy specific verification criteria before the next step can begin.

## Workflow Lifecycle

The following sequence diagram shows the typical interaction between the user, the orchestrator, and the various internal engines during a development run.

```mermaid
sequenceDiagram
    autonumber
    participant User
    participant CLI
    participant Orch as Orchestrator
    participant Council
    participant Exec as Execution Engine
    participant Verif as Verification Engine
    participant Graph as Artifact Graph

    Note over User, Graph: Phase: Planning
    User->>CLI: dev plan "goal"
    CLI->>Orch: start_run(goal)
    Orch->>Council: Draft Requirements
    Council->>Graph: Save Requirements
    Orch->>Council: Generate Plans & Critiques
    Council->>Graph: Save Plans/Critiques
    Orch->>Council: Arbitrate Final Plan
    Council->>Graph: Save Task Graph (Tasks, Files)
    
    Note over User, Graph: Phase: Execution
    User->>CLI: dev run TASK-001
    CLI->>Orch: run_task(TASK-001)
    Orch->>Exec: Execute Task
    Exec->>User: (Manual) "Please modify file X"
    Exec->>Graph: Save Evidence (Diffs, Logs)
    
    Note over User, Graph: Phase: Verification
    User->>CLI: dev verify TASK-001
    CLI->>Orch: verify_task(TASK-001)
    Orch->>Verif: Run Gates & Tests
    Verif->>Graph: Save Evidence & Gaps
    alt No Gaps
        Orch->>User: Task Verified
    else Gaps Found
        Orch->>User: Task Blocked (Repair needed)
    end
```

## Core Principles

1.  **Requirement-First**: No code is written until requirements and acceptance criteria are explicitly defined and saved to the Artifact Graph.
2.  **Independent Planning**: The "Council" uses multiple agents to generate competing plans, which are then cross-critiqued to find edge cases and risks.
3.  **Deterministic Gating**: Transitioning from one phase to another (e.g., from Execution to Verification) is managed by a strict State Machine.
4.  **Evidence Persistence**: Every action taken by an agent or executor must produce evidence (diffs, test results, logs) that is linked back to the original requirements.
