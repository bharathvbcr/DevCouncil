# DevCouncil Implementation Plan

**Project name:** DevCouncil  
**CLI command:** `dev`  
**Architecture:** Gated orchestrator first, full coding-agent CLI later  
**Core thesis:** DevCouncil should not merely generate code. It should make AI-generated work prove that it satisfied the original intent.

---

## 0. Executive Summary

DevCouncil is a command-line orchestration tool for AI-assisted software development. It creates a software-team-style workflow around AI coding agents:

```text
Goal
  -> repo map
  -> requirements
  -> Plan A and Plan B
  -> cross-critique
  -> arbitration
  -> task graph
  -> gated execution
  -> deterministic verification
  -> gap report
  -> repair loop
  -> final evidence report
```

The product should start as a **gated orchestrator** rather than a full native coding agent. That means DevCouncil owns planning, task sequencing, gates, verification, and reporting. Execution can initially be manual or delegated to external tools such as mini-SWE-agent, OpenHands, Aider, Claude Code, Codex CLI, or a human.

Later, once the gating kernel is strong, DevCouncil can grow into a full native coding-agent CLI.

The main differentiator is the persistent artifact graph:

```text
Requirement -> Acceptance Criterion -> Task -> Planned Files -> Changed Files -> Commands -> Evidence -> Gaps
```

This is the product. The LLM council is a way to populate and stress-test the graph, but the graph and gates are the durable source of truth.

---

## 1. Product Positioning

### Bad positioning

```text
DevCouncil is another AI coding agent.
```

That is a weak lane. The market already has strong coding agents, IDE agents, and terminal agents.

### Good positioning

```text
DevCouncil is a gated coding orchestrator that refuses to call work done until each requirement has implementation and verification evidence.
```

Or shorter:

```text
AI coding with staff-engineer-style execution gates.
```

### What DevCouncil owns

DevCouncil should own:

- planning protocol
- independent plans
- cross-model critique
- arbitration into structured artifacts
- task graph
- assumptions log
- requirement coverage
- execution gates
- diff-to-task mapping
- test evidence mapping
- gap detection
- repair task generation
- final evidence report

### What DevCouncil should not own in v1

DevCouncil should not immediately try to own:

- a full IDE
- a full native code-writing agent
- a cloud dev environment
- a browser automation stack
- every tool interface
- enterprise dashboard

Those can come later.

---

## 2. Why DevCouncil Exists

AI coding agents often fail in subtle ways:

- They build the happy path and skip edge cases.
- They implement one requirement while forgetting another.
- They say tests passed but do not prove the important behavior.
- They modify unrelated files.
- They change architecture without saying so.
- They add dependencies without justification.
- They hide assumptions inside chat history.
- They lose the original PRD after several steps.

DevCouncil attacks this with a real engineering workflow:

```text
Requirements are explicit.
Assumptions are tracked.
Tasks are scoped.
Files are constrained.
Tests are mapped to acceptance criteria.
Diffs are mapped to tasks.
Gaps become repair tasks.
```

The LLM debate is not the final authority. It is a source of candidate requirements, risks, and missing checks. Deterministic gates and evidence decide whether work can move forward.

---

## 3. Inspirations and What to Borrow

This project should learn from existing open-source tools without becoming a clone of any single one.

| Source | What to borrow | What to avoid |
|---|---|---|
| GPT Pilot / Pythagora | Team-like workflow: specification writer, architect, tech lead, developer, reviewer, debugger, technical writer. Step-by-step implementation instead of one giant generation. | Do not copy the whole app-building flow. DevCouncil should be narrower and more evidence-driven. |
| MetaGPT | “Software company as multi-agent system” and role-based SOP thinking. | Do not start with a huge agent framework. Start with gates and artifacts. |
| OpenHands | Later executor adapter or SDK substrate. It has tools for bash, file editing, browsing, MCP, and software-agent workflows. | Do not make DevCouncil dependent on OpenHands only. Keep executor adapters modular. |
| llm-council / llm-council-plus | Independent answers, peer review, chairman synthesis pattern. | Replace “chairman final opinion” with evidence-based artifact arbitration. |
| Sage | Watching/cross-checking coding-agent plans and reasoning before implementation. | Sage critiques; DevCouncil should gate. Do not stop at critique cards. |
| mini-SWE-agent | Small, hackable executor loop. Good first external executor adapter. | Do not let bash-only execution bypass DevCouncil’s file/command gates. |
| GitNexus | Codebase knowledge graph and structural awareness. | Use as later indexing inspiration, not v1 dependency. |
| graphify | Always-on graph context, hooks/rules, and multi-agent integration across coding tools. | Do not build a full knowledge graph before the artifact graph works. |

---

## 4. Language Choice

Use **Python** for v1.

### Why Python

- Faster iteration for LLM orchestration.
- Strong schema tooling with Pydantic.
- Easy subprocess orchestration for external executors.
- Natural fit for OpenHands SDK and mini-SWE-agent style integrations.
- Easy local state management with SQLite.
- Easy packaging through `uv`, `pipx`, or `uv tool`.

### Why not Go first

Go is great for a polished single-binary CLI, but DevCouncil’s early complexity is not command-line parsing or runtime performance. The hard parts are:

- LLM routing
- structured outputs
- artifact graph design
- repo mapping
- executor adapters
- prompt protocols
- verification gates

Python is better for proving that core loop quickly.

### Future Go option

Later, DevCouncil could add a Go wrapper or daemon for:

- file watching
- fast install
- single-binary distribution
- background process management

But the intelligence kernel should start in Python.

---

## 5. Tech Stack

Recommended stack:

```text
Python 3.12+
uv
Typer
Rich
Pydantic v2
SQLite
SQLModel or SQLAlchemy
httpx
GitPython or dulwich
pytest
ruff
pyright or mypy
ripgrep subprocess
Tree-sitter later
```

Optional later:

```text
LiteLLM
OpenHands SDK
mini-SWE-agent subprocess adapter
NetworkX
KuzuDB / graph database
MCP server
GitHub Checks API
```

---

## 6. CLI Command Design

The binary should be `dev`.

### Core commands

```bash
dev init
dev doctor

dev plan "Add password reset with expiring single-use tokens"
dev status
dev tasks
dev show TASK-004

dev prompt TASK-004
dev run TASK-004 --executor manual
dev run TASK-004 --executor mini
dev run TASK-004 --executor openhands

dev verify
dev verify TASK-004

dev repair
dev report
dev rollback TASK-004
dev config models
```

### Command responsibilities

| Command | Purpose |
|---|---|
| `dev init` | Create `.devcouncil/` and config. |
| `dev doctor` | Check git, model keys, `rg`, package manager, test commands. |
| `dev plan` | Run repo map, requirements, Plan A, Plan B, critique, rebuttal, arbitration. |
| `dev status` | Show current phase, open gaps, blocked tasks, and cost. |
| `dev tasks` | List task graph and task gate status. |
| `dev show TASK-ID` | Show one task, linked requirements, allowed files, evidence needs. |
| `dev prompt TASK-ID` | Produce constrained prompt for manual/external executor. |
| `dev run TASK-ID` | Execute one task using selected executor. |
| `dev verify` | Run gates, collect evidence, produce gaps. |
| `dev repair` | Convert gaps into repair tasks. |
| `dev report` | Produce final evidence report. |
| `dev rollback TASK-ID` | Revert changes using checkpoint. |
| `dev config models` | Show or edit model role configuration. |

---

## 7. Runtime Project State

When the user runs `dev init`, create:

```text
.devcouncil/
  config.yaml
  state.sqlite
  runs/
    2026-04-27T120000Z-add-password-reset/
      goal.md
      repo_map.json
      requirements.json
      assumptions.json
      blocking_questions.json
      plan_a.json
      plan_b.json
      critiques.json
      rebuttals.json
      decisions.json
      task_graph.json
      gate_results.json
      execution_log.json
      diff_evidence.json
      command_results.json
      test_evidence.json
      gaps.json
      report.md
  cache/
    file_summaries/
    llm_responses/
    repo_maps/
  checkpoints/
    TASK-001-before.patch
    TASK-001-after.patch
  logs/
    model_calls.jsonl
    events.jsonl
```

Use `.devcouncil/` rather than `.dev/` because many repos already use `dev` scripts, folders, or tooling.

---

## 8. Suggested Repository Hierarchy

Use a `src/` layout.

```text
devcouncil/
  pyproject.toml
  README.md
  LICENSE
  uv.lock
  .gitignore
  .pre-commit-config.yaml

  docs/
    architecture.md
    artifact-graph.md
    gating-policy.md
    executor-adapters.md
    model-routing.md
    roadmap.md

  examples/
    password-reset/
      goal.md
      flawed-implementation-notes.md
      expected-report.md
    todo-api/
      goal.md
      expected-report.md

  src/
    devcouncil/
      __init__.py
      __main__.py

      cli/
        __init__.py
        main.py
        commands/
          init.py
          doctor.py
          plan.py
          status.py
          tasks.py
          show.py
          prompt.py
          run.py
          verify.py
          repair.py
          report.py
          rollback.py
          config.py
        renderers/
          tables.py
          cards.py
          markdown.py
          progress.py

      app/
        __init__.py
        orchestrator.py
        state_machine.py
        run_context.py
        errors.py
        events.py

      domain/
        __init__.py
        ids.py
        enums.py
        requirement.py
        assumption.py
        plan.py
        task.py
        critique.py
        rebuttal.py
        decision.py
        evidence.py
        gap.py
        gate.py
        report.py

      artifacts/
        __init__.py
        schemas.py
        graph.py
        coverage.py
        serializer.py
        validators.py
        migrations.py

      council/
        __init__.py
        engine.py
        roles.py
        debate.py
        critique.py
        rebuttal.py
        arbiter.py
        prompts/
          spec_writer.md
          planner_a.md
          planner_b.md
          critic_a.md
          critic_b.md
          rebuttal.md
          arbiter.md
          implementation_reviewer.md

      planning/
        __init__.py
        spec_service.py
        plan_service.py
        question_service.py
        task_decomposer.py
        test_strategy.py

      gating/
        __init__.py
        policy.py
        gate_runner.py
        checks/
          clean_git.py
          requirement_coverage.py
          task_coverage.py
          assumption_check.py
          planned_files_check.py
          orphan_diff_check.py
          test_evidence_check.py
          command_result_check.py
          dependency_change_check.py
          secret_scan_check.py
          migration_check.py

      execution/
        __init__.py
        task_runner.py
        prompt_builder.py
        sandbox.py
        permissions.py
        checkpoints.py
        patch.py
        tool_loop.py

      executors/
        __init__.py
        base.py
        manual.py
        mini_swe.py
        openhands.py
        shell.py
        native/
          __init__.py
          agent.py
          tools.py
          context_builder.py
          loop.py

      verification/
        __init__.py
        verifier.py
        diff_analyzer.py
        test_runner.py
        evidence_collector.py
        gap_detector.py
        implementation_reviewer.py

      repo/
        __init__.py
        git.py
        files.py
        ignore.py
        language.py
        package_managers.py
        commands.py
        dependencies.py
        migrations.py

      indexing/
        __init__.py
        repo_mapper.py
        symbol_index.py
        summaries.py
        graph_index.py
        cache.py

      llm/
        __init__.py
        router.py
        provider.py
        openrouter.py
        litellm_provider.py
        schemas.py
        usage.py
        cache.py
        retry.py

      storage/
        __init__.py
        db.py
        repositories.py
        json_store.py
        sqlite_store.py
        migrations/
          001_init.sql

      reporting/
        __init__.py
        report_builder.py
        markdown_report.py
        json_report.py
        github_check.py

      integrations/
        __init__.py
        github.py
        claude_hooks.py
        codex_hooks.py
        cursor_rules.py
        graphify.py
        gitnexus.py

      telemetry/
        __init__.py
        tokens.py
        cost.py
        traces.py

      utils/
        __init__.py
        paths.py
        hashing.py
        subprocess.py
        json.py
        text.py
        redaction.py

  tests/
    unit/
      test_artifact_schemas.py
      test_coverage_matrix.py
      test_gate_policy.py
      test_gap_detector.py
      test_arbiter.py
      test_repo_mapper.py
    integration/
      test_plan_flow.py
      test_manual_executor_flow.py
      test_verify_flow.py
    fixtures/
      repos/
        tiny_fastapi/
        tiny_nextjs/
      llm_responses/
        planner_a.json
        planner_b.json
        critiques.json
```

---

## 9. Core Domain Objects

### Requirement

```python
from pydantic import BaseModel
from typing import Literal

class AcceptanceCriterion(BaseModel):
    id: str
    description: str
    verification_method: Literal[
        "unit_test",
        "integration_test",
        "manual",
        "static_check",
        "llm_review"
    ]
    required: bool = True

class Requirement(BaseModel):
    id: str
    title: str
    description: str
    priority: Literal["low", "medium", "high", "critical"]
    source: Literal["user", "planner", "critic", "arbiter"]
    acceptance_criteria: list[AcceptanceCriterion]
```

### Assumption

```python
class Assumption(BaseModel):
    id: str
    statement: str
    confidence: Literal["low", "medium", "high"]
    impact: Literal["low", "medium", "high"]
    reversible: bool
    requires_user_confirmation: bool
    linked_requirement_ids: list[str] = []
    status: Literal[
        "open",
        "confirmed",
        "rejected",
        "converted_to_requirement"
    ] = "open"
```

Assumptions are first-class because AI agents often hide product and architecture decisions in chat history.

Example:

```json
{
  "id": "ASM-003",
  "statement": "Use the existing email provider instead of adding a new dependency.",
  "confidence": "medium",
  "impact": "high",
  "reversible": true,
  "requires_user_confirmation": false,
  "linked_requirement_ids": ["REQ-001"]
}
```

If the executor later adds a new email provider dependency, DevCouncil can block the task as an assumption violation.

### Task

```python
class PlannedFile(BaseModel):
    path: str
    reason: str
    allowed_change: Literal["create", "modify", "delete", "read_only"]

class Task(BaseModel):
    id: str
    title: str
    description: str
    requirement_ids: list[str]
    acceptance_criterion_ids: list[str]
    planned_files: list[PlannedFile]
    expected_tests: list[str]
    allowed_commands: list[str]
    forbidden_changes: list[str] = []
    status: Literal[
        "planned",
        "ready",
        "running",
        "blocked",
        "verified",
        "done"
    ] = "planned"
```

### Critique Finding

```python
class CritiqueFinding(BaseModel):
    id: str
    source_agent: str
    target_plan_id: str
    severity: Literal["low", "medium", "high", "critical"]
    finding_type: Literal[
        "missing_requirement",
        "missing_task",
        "missing_test",
        "bad_assumption",
        "architecture_risk",
        "security_risk",
        "performance_risk",
        "dependency_risk",
        "migration_risk",
        "unverifiable_acceptance_criteria"
    ]
    claim: str
    linked_requirement_id: str | None = None
    suggested_requirement: str | None = None
    suggested_task: str | None = None
    falsifiable_check: str
    status: Literal[
        "open",
        "accepted",
        "rejected",
        "converted",
        "needs_user"
    ] = "open"
```

The `falsifiable_check` field is non-negotiable. If a critic cannot say how the claim could be checked, the finding is not actionable.

### Evidence

```python
class CommandResult(BaseModel):
    command: str
    exit_code: int
    stdout_path: str
    stderr_path: str
    summary: str

class DiffEvidence(BaseModel):
    task_id: str
    changed_files: list[str]
    added_files: list[str]
    deleted_files: list[str]
    diff_summary: str

class TestEvidence(BaseModel):
    requirement_id: str
    acceptance_criterion_id: str
    command: str
    status: Literal["passed", "failed", "not_run"]
    evidence_summary: str
```

### Gap

```python
class Gap(BaseModel):
    id: str
    severity: Literal["low", "medium", "high", "critical"]
    gap_type: Literal[
        "requirement_not_planned",
        "task_not_implemented",
        "planned_file_not_changed",
        "orphan_diff",
        "missing_test",
        "test_failed",
        "acceptance_criteria_unproven",
        "assumption_violated",
        "architecture_drift",
        "security_risk",
        "dependency_risk",
        "migration_gap"
    ]
    requirement_id: str | None = None
    task_id: str | None = None
    description: str
    evidence: list[str]
    recommended_fix: str
    blocking: bool
```

---

## 10. The Artifact Graph

Internally, maintain a directed graph:

```text
Requirement
  -> AcceptanceCriterion
  -> Task
  -> PlannedFile
  -> ChangedFile
  -> CommandResult
  -> TestEvidence
  -> Gap
```

Use SQLite for internal storage and JSON for export.

### SQLite tables

```sql
requirements
acceptance_criteria
assumptions
plans
tasks
task_requirements
critique_findings
rebuttals
decisions
changed_files
command_results
evidence
gaps
model_calls
gate_results
```

### Useful coverage queries

Requirements without tasks:

```sql
SELECT r.id
FROM requirements r
LEFT JOIN task_requirements tr ON tr.requirement_id = r.id
WHERE tr.task_id IS NULL;
```

Tasks with no changed files:

```sql
SELECT t.id
FROM tasks t
LEFT JOIN changed_files cf ON cf.task_id = t.id
WHERE cf.path IS NULL;
```

Acceptance criteria without evidence:

```sql
SELECT ac.id
FROM acceptance_criteria ac
LEFT JOIN evidence e ON e.acceptance_criterion_id = ac.id
WHERE e.id IS NULL;
```

---

## 11. Planning Debate Protocol

This is DevCouncil’s core planning process.

### Step 1: Repo map

Before LLM planning, DevCouncil maps the repo.

V1 repo map should be mostly deterministic:

```text
git ls-files
read package.json / pyproject.toml / go.mod / Cargo.toml
framework detection
test/lint/typecheck command detection
ripgrep keyword search
migration directory detection
lockfile detection
candidate file ranking
```

Example output:

```json
{
  "languages": ["typescript"],
  "frameworks": ["nextjs"],
  "package_managers": ["pnpm"],
  "test_commands": ["pnpm test", "pnpm lint", "pnpm typecheck"],
  "important_files": [
    "package.json",
    "prisma/schema.prisma",
    "src/auth/index.ts"
  ],
  "candidate_files": [
    {
      "path": "src/auth/reset-token.ts",
      "reason": "Likely reset token logic"
    }
  ]
}
```

### Step 2: Requirements draft

A spec writer model creates:

```text
requirements.json
assumptions.json
blocking_questions.json
```

No tasks yet. Requirements first.

### Step 3: Independent Plan A and Plan B

Planner A and Planner B should not see each other’s outputs.

Recommended roles:

```text
Planner A: pragmatic tech lead
  Optimize for simplest maintainable implementation.
  Avoid unnecessary dependencies.
  Keep tasks small.

Planner B: production-readiness architect
  Optimize for security, testing, edge cases, failure modes, maintainability.
  Assume the pragmatic plan may miss subtle obligations.
```

### Step 4: Cross-critique

```text
Critic A attacks Plan B.
Critic B attacks Plan A.
```

Critics output only structured findings.

Bad finding:

```text
This seems insecure.
```

Good finding:

```json
{
  "severity": "high",
  "finding_type": "security_risk",
  "claim": "Plan B does not require reset tokens to be stored hashed.",
  "falsifiable_check": "Inspect token creation code and database writes for hashing before persistence.",
  "suggested_requirement": "Raw reset tokens must never be persisted."
}
```

### Step 5: Rebuttal

Each planner gets one rebuttal round.

Rules:

```text
A finding can be rejected only with artifact evidence.
A finding can be accepted and converted into a requirement/task/test.
A finding can become a blocking question.
No hand-wavy rebuttals.
```

### Step 6: Arbitration

The arbiter is not a “chairman who decides by vibes.” It is a graph compiler.

The arbiter outputs:

```json
{
  "accepted_findings": ["FIND-001", "FIND-004"],
  "rejected_findings": [
    {
      "id": "FIND-002",
      "reason": "Already covered by REQ-004 and TASK-006."
    }
  ],
  "converted_findings": [
    {
      "finding_id": "FIND-004",
      "created_requirement_id": "REQ-009",
      "created_task_id": "TASK-012"
    }
  ],
  "blocking_questions": [
    {
      "id": "Q-001",
      "question": "Should successful password reset invalidate all active sessions?"
    }
  ]
}
```

Golden rule:

```text
If one critic raises a high-severity issue and nobody refutes it with artifact evidence, it survives as a requirement, task, assumption, test, or blocking question.
```

---

## 12. Gating State Machine

DevCouncil should represent project progress as a state machine.

```text
NEW
  -> REPO_MAPPED
  -> REQUIREMENTS_DRAFTED
  -> PLANS_GENERATED
  -> CRITIQUES_GENERATED
  -> ARBITRATED
  -> AWAITING_USER_DECISIONS
  -> PLAN_APPROVED
  -> TASK_READY
  -> TASK_EXECUTING
  -> TASK_VERIFYING
  -> TASK_BLOCKED or TASK_VERIFIED
  -> PROJECT_DONE
```

### PLAN_APPROVED gate

Passes only when:

```text
Every requirement has acceptance criteria.
Every acceptance criterion has a verification method.
Every requirement maps to at least one task.
Every task maps to at least one requirement.
Every high-impact assumption is confirmed or converted.
No critical critique finding remains open.
No blocking question remains unanswered.
```

### TASK_READY gate

Passes only when:

```text
Git working tree is clean or checkpointed.
Task has allowed files.
Task has allowed commands.
Task has expected verification evidence.
Task has no unresolved prerequisite tasks.
```

### TASK_VERIFIED gate

Passes only when:

```text
Changed files are allowed or justified.
No orphan diffs exist.
Required commands passed.
Acceptance criteria have evidence.
No dependency/migration/security gap is open.
LLM implementation review has no unrefuted critical/high findings.
```

The LLM does not pass gates. The gate runner passes gates.

---

## 13. Model Router Design

Create one internal model interface.

```python
class ModelRouter:
    async def complete_json(
        self,
        role: str,
        messages: list[dict],
        schema: type[BaseModel],
        run_id: str,
    ) -> BaseModel:
        ...
```

Every model call should record:

```text
role
model
provider
input tokens
output tokens
cost estimate
latency
cache key
schema used
artifact produced
```

### Example `.devcouncil/config.yaml`

```yaml
project:
  name: my-app
  root: "."
  default_branch: main

models:
  provider: openrouter
  roles:
    spec_writer:
      model: openai/gpt-5.2
      temperature: 0.1
    planner_a:
      model: anthropic/claude-sonnet-4.5
      temperature: 0.2
    planner_b:
      model: google/gemini-3-pro
      temperature: 0.2
    critic_a:
      model: openai/gpt-5.2
      temperature: 0.0
    critic_b:
      model: anthropic/claude-opus-4.5
      temperature: 0.0
    arbiter:
      model: openai/gpt-5.2
      temperature: 0.0
    implementation_reviewer:
      model: anthropic/claude-opus-4.5
      temperature: 0.0

provider:
  sort: price
  allow_fallbacks: true
  require_parameters: true
  data_collection: deny

commands:
  test:
    - "pnpm test"
  lint:
    - "pnpm lint"
  typecheck:
    - "pnpm typecheck"

gates:
  require_clean_git_before_task: true
  block_orphan_diffs: true
  block_missing_tests_for_high_requirements: true
  block_dependency_changes_without_approval: true
  block_schema_change_without_migration: true
  block_failed_commands: true

execution:
  default_executor: manual
  max_repair_attempts: 3
  checkpoint_before_each_task: true

indexing:
  use_ripgrep: true
  use_tree_sitter: false
  use_graph_index: false

privacy:
  redact_env_vars: true
  redact_secrets_in_logs: true
  store_prompts_locally: true
```

---

## 14. Executor Strategy

Build executors in layers.

### Executor 1: Manual executor

This is the first MVP executor.

It does not write code. It creates a constrained task prompt.

Example:

```markdown
# Implement TASK-004

## Goal
Implement reset token validation.

## Requirements
- REQ-002: Reset token expires
- REQ-003: Reset token is single-use
- REQ-004: Raw reset token is not stored

## Allowed files
- src/auth/reset-token.ts
- src/auth/reset-token.test.ts

## Forbidden changes
- Do not modify billing code.
- Do not add dependencies.
- Do not change Prisma schema.

## Required evidence
- Test expired token rejected
- Test used token rejected
- Test valid token accepted
- Test raw token is not persisted

## Commands to run
- pnpm test src/auth/reset-token.test.ts
- pnpm lint
- pnpm typecheck
```

Then the user can paste the prompt into Claude Code, Aider, Cursor, OpenHands, Codex, or implement manually.

### Executor 2: mini-SWE-agent adapter

Adapter responsibilities:

```text
write task prompt
start mini-SWE-agent process
capture stdout/stderr
capture trajectory if available
wait for completion
run dev verify
```

### Executor 3: OpenHands adapter

Adapter responsibilities:

```text
start OpenHands task/headless/SDK run
provide constrained task prompt
capture logs/conversation
watch git diff
run verifier
```

### Executor 4: Native executor

Only build this after gates work.

Native loop:

```text
1. Build task context.
2. Ask model for patch plan.
3. Apply patch.
4. Run allowed commands.
5. Collect diff/evidence.
6. Ask model for repair only if deterministic gates fail.
7. Stop when task verified or retry budget is reached.
```

Native tools:

```text
read_file
list_files
search_repo
show_diff
apply_patch
run_allowed_command
git_status
git_checkpoint
rollback_checkpoint
```

Do not give arbitrary shell access in the first native version.

---

## 15. Permission Model

DevCouncil needs a permission system before it becomes a native coding agent.

```yaml
permissions:
  file_write:
    mode: task_allowed_files_only

  shell:
    mode: allowlist
    allowed:
      - "pnpm test*"
      - "pnpm lint*"
      - "pnpm typecheck*"
      - "pytest*"
      - "ruff*"
      - "mypy*"

  network:
    mode: deny_by_default

  dependency_install:
    mode: require_user_approval

  delete_file:
    mode: require_user_approval

  outside_repo:
    mode: deny

  secrets:
    redact_env: true
    scan_before_commit: true
```

Checkpoints:

```text
Before every task:
  git diff > TASK-ID-before.patch
  git status captured

After every task:
  git diff > TASK-ID-after.patch

Rollback:
  git apply -R TASK-ID-after.patch
```

---

## 16. Verification and Gap Detection

The verifier runs after each task and before final completion.

### Deterministic verification order

```text
1. Git cleanliness/checkpoint check
2. Changed-file mapping
3. Planned-file coverage
4. Orphan-diff detection
5. Package/dependency change detection
6. Migration detection
7. Test command execution
8. Lint/typecheck execution
9. Secret scan
10. Acceptance-criteria evidence mapping
11. LLM implementation review
12. Final gate decision
```

### Gap taxonomy

```text
requirement_not_planned
task_not_implemented
planned_file_not_changed
orphan_diff
missing_test
test_failed
acceptance_criteria_unproven
assumption_violated
architecture_drift
security_risk
dependency_risk
migration_gap
```

### Example gap

```json
{
  "id": "GAP-007",
  "gap_type": "missing_test",
  "severity": "high",
  "requirement_id": "REQ-003",
  "task_id": "TASK-004",
  "description": "Single-use token behavior was planned, but no test evidence proves token reuse is rejected.",
  "evidence": [
    "No changed test file mapped to REQ-003",
    "No command output mentions reuse rejection"
  ],
  "recommended_fix": "Add a test that attempts password reset twice with the same token and expects the second attempt to fail.",
  "blocking": true
}
```

---

## 17. Prompt Design

All role prompts must force structured outputs.

Common instruction:

```text
You must output valid JSON matching the schema.
Do not include prose outside JSON.
Every criticism must include a falsifiable_check.
Every task must map to at least one requirement.
Every acceptance criterion must have a verification method.
```

### Planner A prompt

```text
You are the pragmatic tech lead.
Optimize for the simplest maintainable implementation.
Avoid unnecessary dependencies.
Create tasks small enough for one coding-agent execution pass.
```

### Planner B prompt

```text
You are the production-readiness architect.
Optimize for security, tests, edge cases, failure modes, and maintainability.
Assume the first plan will miss subtle requirements.
```

### Critic prompt

```text
You are a hostile staff engineer reviewing another team's implementation plan.
Find missing requirements, bad assumptions, missing tests, security risks, migration risks, and unverifiable claims.
Do not praise.
Do not rewrite the plan.
Only emit structured findings.
Every finding must include a falsifiable_check.
```

### Arbiter prompt

```text
You are an engineering manager compiling a decision ledger.
You do not decide by vibes.
You may reject a finding only if another artifact explicitly covers it.
High-severity unrefuted findings must become requirements, tasks, tests, assumptions, or blocking questions.
```

### Implementation reviewer prompt

```text
You are reviewing actual code changes against the approved artifact graph.
You do not review style unless it affects correctness, maintainability, security, or evidence.
Passing tests are necessary but not sufficient.
Return gaps only.
```

---

## 18. Implementation Phases

### Phase 0: Project skeleton

Build:

```text
pyproject.toml
Typer CLI
Rich output
config loader
.devcouncil directory creation
basic SQLite store
```

Commands:

```bash
dev init
dev doctor
dev status
```

Pass criteria:

```text
Can initialize a repo.
Can detect git root.
Can read/write .devcouncil/config.yaml.
Can create .devcouncil/state.sqlite.
```

---

### Phase 1: Artifact graph

Build Pydantic models and validation.

Main modules:

```text
domain/
artifacts/
storage/
```

Command:

```bash
dev artifacts validate
```

Pass criteria:

```text
Can load/save requirements, tasks, assumptions, findings, evidence, and gaps.
Can validate broken artifacts and explain what is invalid.
```

---

### Phase 2: Repo mapper

Build deterministic repo mapping.

Features:

```text
git ls-files
ignore generated/vendor files
language detection
framework detection
package manager detection
test/lint/typecheck command detection
goal keyword search with ripgrep
candidate file ranking
```

Command:

```bash
dev map "Add password reset"
```

Pass criteria:

```text
Produces repo_map.json.
Finds package manager.
Finds likely test commands.
Finds candidate files.
Does not send whole repo to model.
```

---

### Phase 3: LLM router

Build OpenRouter provider first.

Features:

```text
structured JSON output
retry
schema validation
response healing fallback
usage tracking
cache
role-based model config
```

Pass criteria:

```text
Can request JSON matching a Pydantic schema.
Stores model call metadata.
Caches repeated calls.
```

---

### Phase 4: Requirements generator

Build:

```text
SpecWriter
AssumptionExtractor
BlockingQuestionExtractor
```

Command:

```bash
dev plan "Add password reset" --requirements-only
```

Pass criteria:

```text
requirements.json generated
assumptions.json generated
blocking_questions.json generated
```

---

### Phase 5: Independent planners

Build:

```text
PlannerA
PlannerB
```

Pass criteria:

```text
plan_a.json and plan_b.json are generated independently.
Each task maps to requirements.
Each task has planned files.
Each task has expected tests or an explicit reason why not.
```

---

### Phase 6: Cross-critique and rebuttal

Build:

```text
CriticA attacks PlanB
CriticB attacks PlanA
PlannerA rebuts CriticB
PlannerB rebuts CriticA
```

Pass criteria:

```text
All findings have severity, type, claim, and falsifiable_check.
Rejected findings require evidence.
Accepted findings convert cleanly.
```

---

### Phase 7: Arbiter and plan gates

Build:

```text
Arbiter
DecisionLedger
GatePolicy
PlanApprovalGate
```

Pass criteria:

```text
No open high/critical finding can disappear.
Unrefuted high/critical findings become tasks, requirements, tests, assumptions, or questions.
Final task graph is generated.
```

---

### Phase 8: Manual executor

Build:

```bash
dev prompt TASK-ID
dev run TASK-ID --executor manual
```

Pass criteria:

```text
Produces constrained task prompt.
Creates git checkpoint.
Marks task as waiting_for_external_execution.
```

---

### Phase 9: Verifier

Build:

```text
git diff scanner
changed-file mapper
command runner
test evidence collector
gap detector
report builder
```

Command:

```bash
dev verify TASK-004
```

Pass criteria:

```text
Catches orphan diffs.
Catches missing tests.
Catches failed commands.
Catches dependency changes.
Produces gaps.json and report.md.
```

---

### Phase 10: Repair loop

Build:

```bash
dev repair
```

Pass criteria:

```text
Converts blocking gaps into focused repair tasks.
Repair tasks have allowed files and required evidence.
```

---

### Phase 11: mini-SWE-agent adapter

Build:

```bash
dev run TASK-004 --executor mini
```

Pass criteria:

```text
Can invoke mini-SWE-agent with constrained prompt.
Captures logs.
Runs verifier afterward.
```

---

### Phase 12: OpenHands adapter

Build:

```bash
dev run TASK-004 --executor openhands
```

Pass criteria:

```text
Can launch OpenHands task/headless/SDK flow.
Captures execution result.
Runs verifier afterward.
```

---

### Phase 13: Native executor

Build a minimal code-writing agent.

Pass criteria:

```text
Can modify files via apply_patch only.
Can run allowlisted commands.
Can repair once.
Cannot write outside allowed files.
Cannot run arbitrary shell commands.
```

This is the phase where DevCouncil becomes a true coding-agent CLI.

---

## 19. Native Agent Expansion Plan

Native agent architecture:

```text
NativeAgent
  ├── ContextBuilder
  ├── ToolRegistry
  ├── PermissionPolicy
  ├── PatchEngine
  ├── CommandRunner
  ├── ObservationBuilder
  ├── RepairLoop
  └── VerifierGate
```

Native loop:

```python
while not task_verified and attempts < max_attempts:
    context = context_builder.for_task(task)
    action = model.next_action(context, tools)
    result = tools.execute(action, permissions)
    evidence_collector.record(result)

    if action.type == "finish":
        gate_result = verifier.verify(task)
        if gate_result.passed:
            break
        repair_prompt = repair_builder.from_gaps(gate_result.gaps)
```

Tool permissions:

```text
read_file: allowed
search_repo: allowed
apply_patch: allowed only for task allowed files
run_command: allowlist only
delete_file: approval required
install_dependency: approval required
network: denied by default
```

---

## 20. Real Software Team Workflow Mapping

DevCouncil should feel like a mini engineering org.

### Product phase

Artifacts:

```text
PRD
requirements
acceptance criteria
assumptions
blocking questions
```

Gate:

```text
No vague requirements.
No unverifiable acceptance criteria.
No high-impact unresolved assumptions.
```

### Architecture phase

Artifacts:

```text
Plan A
Plan B
architecture decisions
risk register
test strategy
```

Gate:

```text
Every requirement has tasks.
Every high-risk area has tests.
Every critic finding is resolved or tracked.
```

### Sprint planning phase

Artifacts:

```text
task graph
dependencies
allowed files
expected evidence
```

Gate:

```text
Tasks are small.
Tasks are executable.
Tasks have verification methods.
```

### Implementation phase

Artifacts:

```text
git checkpoint
execution log
changed files
command results
```

Gate:

```text
No unauthorized diff.
No failed tests.
No missing evidence.
```

### QA phase

Artifacts:

```text
gap report
repair tasks
final evidence matrix
```

Gate:

```text
All blocking gaps closed.
```

### Release phase

Artifacts:

```text
final report
summary
known limitations
follow-up tasks
```

Gate:

```text
Project can be marked complete.
```

---

## 21. Final Report Format

`dev report` should produce:

```markdown
# DevCouncil Report: Password Reset Flow

## Verdict
Blocked: 2 high-severity gaps remain.

## Requirements Coverage
| Requirement | Task | Implementation Evidence | Test Evidence | Status |
|---|---|---|---|---|
| REQ-001 Request reset email | TASK-001 | src/auth/reset-request.ts | reset-request.test.ts | Passed |
| REQ-002 Token expires | TASK-004 | src/auth/reset-token.ts | reset-token.test.ts | Passed |
| REQ-003 Token single-use | TASK-004 | src/auth/reset-token.ts | Missing | Blocked |

## Blocking Gaps
### GAP-007: Missing test for token reuse
REQ-003 requires used tokens to be rejected.
The implementation changes token validation code, but no test proves reuse fails.

Recommended repair:
Add a test that uses the same token twice and expects the second attempt to fail.

## Commands
- pnpm test: passed
- pnpm lint: passed
- pnpm typecheck: passed

## Orphan Diffs
None.

## Assumptions
- ASM-001 Use existing email provider: respected
- ASM-002 No new dependency: respected

## Next Repair Prompt
...
```

---

## 22. MVP Demo Scenario

Use password reset as the first demo.

Goal:

```text
Add password reset with expiring single-use tokens.
```

Expected requirements:

```text
Request reset email
Do not leak account existence
Token expires
Token is single-use
Token stored hashed
Password update works
Relevant tests pass
```

Create an intentionally flawed implementation:

```text
UI works
Token expires
But token is reusable
No test for reuse
Raw token stored in DB
```

DevCouncil should catch:

```text
missing single-use evidence
missing token hashing requirement
missing test
security risk
```

Demo story:

```text
The coding agent said done.
DevCouncil blocked release.
DevCouncil found 3 gaps.
DevCouncil generated a repair task.
After repair, DevCouncil marked the graph green.
```

That demo is stronger than another “watch it build a todo app” demo.

---

## 23. Testing Strategy

### Unit tests

```text
artifact schema validation
requirement-task coverage
arbiter conversion rules
gap detection
orphan diff detection
test command parsing
dependency change detection
```

### Integration tests

Use tiny fixture repos:

```text
tiny_fastapi
tiny_nextjs
tiny_django
tiny_go_api
```

Each fixture should have seeded flaws:

```text
missing test
orphan diff
dependency change
schema change without migration
requirement not planned
task not implemented
```

### LLM tests

Mock LLM outputs with golden JSON:

```text
tests/fixtures/llm_responses/
  planner_a_password_reset.json
  planner_b_password_reset.json
  critic_a_findings.json
  critic_b_findings.json
  arbiter_decisions.json
```

CI should not depend on live model calls.

### Evaluation metrics

Track:

```text
requirement recall
gap precision
false blocker rate
token cost
model calls per run
time to verified task
repair attempts per task
```

Key internal benchmark:

```text
Single planner vs DevCouncil gated debate
```

Prove that the gated debate finds more real gaps at acceptable cost.

---

## 24. Security and Privacy

Implement early:

```text
secret redaction before model calls
environment variable redaction
configurable no-upload paths
.gitignore and .devcouncilignore support
command allowlist
dependency install approval
network denial by default for native executor
git checkpoint before task
rollback command
model-call logs stored locally
```

`.devcouncilignore` example:

```text
.env
.env.*
secrets/
credentials/
node_modules/
dist/
build/
coverage/
*.pem
*.key
```

Before sending snippets to models:

```text
redact API keys
redact JWTs
redact private keys
redact database URLs
redact emails optionally
```

---

## 25. DevCouncil vs Sage

Sage is close enough that DevCouncil must differentiate clearly.

```text
Sage:
  Watches coding-agent conversations and emits critiques.

DevCouncil:
  Owns the task graph, gates execution, maps diffs/tests to requirements,
  and refuses to mark work complete until evidence exists.
```

Sage says:

```text
Concern: this plan may miss token reuse.
```

DevCouncil says:

```text
BLOCKED: REQ-003 has no test evidence. Created REPAIR-TASK-002.
```

That is the wedge.

---

## 26. First Code to Write

Write in this order:

```text
1. domain/*.py
2. artifacts/coverage.py
3. gating/policy.py
4. storage/json_store.py
5. cli/commands/init.py
6. cli/commands/status.py
7. repo/repo_mapper.py
8. llm/router.py
9. council/debate.py
10. council/arbiter.py
11. execution/manual.py
12. verification/gap_detector.py
13. reporting/markdown_report.py
```

First real milestone:

```bash
dev init
dev plan "Add password reset with expiring single-use tokens"
dev tasks
dev prompt TASK-004
# user or external agent edits code
dev verify
dev report
```

No native coding yet. Nail this flow first.

---

## 27. Roadmap

### V0: Planning-only prototype

```text
dev init
dev plan
dev tasks
dev report --planning-only
```

Goal: prove Plan A/Plan B/critique/arbitration creates better requirements and tasks than one model.

### V1: Manual gated orchestrator

```text
dev prompt
dev verify
dev repair
dev report
```

Goal: prove requirement-to-diff-to-test evidence works even when implementation is done externally.

### V2: External executor adapters

```text
dev run --executor mini
dev run --executor openhands
```

Goal: DevCouncil controls the loop while existing agents write the code.

### V3: Native minimal executor

```text
dev run --executor native
```

Goal: DevCouncil writes code with patch-only tools under strict gates.

### V4: Full coding-agent CLI

```text
dev build "feature request"
dev continue
dev inspect
dev pr
```

Goal: DevCouncil becomes a complete coding-agent CLI with planning, execution, repair, and reporting.

### V5: Team/CI layer

```text
GitHub Checks
PR evidence reports
policy packs
team dashboards
org model routing
compliance exports
```

Goal: turn the local CLI into a team quality gate.

---

## 28. Blunt Build Advice

Build DevCouncil as a gated orchestrator first.

Do not start with:

```text
"Let's build a better Claude Code."
```

Start with:

```text
"Let's build the tool that makes any coding agent prove it did the job."
```

The winning loop is:

```text
independent plans
+ adversarial critiques
+ evidence-based arbitration
+ deterministic gates
+ persistent artifact graph
```

Once that loop works, adding a native coding agent becomes an expansion, not the whole bet.

---

## 29. Reference Links

- GPT Pilot / Pythagora: https://github.com/Pythagora-io/gpt-pilot
- MetaGPT: https://github.com/FoundationAgents/MetaGPT
- OpenHands SDK: https://docs.openhands.dev/sdk
- Sage: https://github.com/usetig/sage
- mini-SWE-agent: https://github.com/SWE-agent/mini-swe-agent
- Karpathy LLM Council: https://github.com/karpathy/llm-council
- llm-council-plus: https://github.com/jacob-bd/llm-council-plus
- GitNexus: https://github.com/abhigyanpatwari/GitNexus
- graphify: https://github.com/safishamsi/graphify

