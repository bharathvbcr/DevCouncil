# Artifact Graph

The Artifact Graph is DevCouncil's core data structure. It connects the "why" (requirements) to the "what" (tasks) and the "proof" (evidence/gaps).

```mermaid
graph TD;
    Requirement-->AcceptanceCriterion;
    Requirement-->Task;
    Task-->PlannedFile;
    Task-->ChangedFile;
    Task-->TestEvidence;
    Task-->CommandResult;
    Requirement-->Gap;
    Task-->Gap;
```

This graph enables structural querying of test coverage, unmodified files, orphan diffs, and missing evidence.
