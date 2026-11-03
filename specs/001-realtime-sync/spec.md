# Feature: 001 - Realtime Sync

## 1. Problem Statement & Motivation
State synchronization across distributed edge nodes and agent worktrees requires low-latency, causal broadcast with deterministic conflict resolution. Without a unified spec-driven sync protocol, disparate clients experience desynchronization and unresolvable merge conflicts during multi-agent editing.

## 2. User Stories & Acceptance Scenarios

### US1: Realtime Delta Synchronization (Priority: P1 - MVP)
As an agent or human operator,
I want incremental state changes to synchronize within 50ms across all active worktrees,
So that concurrent work is immediately visible and conflicts are avoided at the edit boundary.

**Acceptance Scenarios**:
- **Scenario 1: Clean Delta Broadcast**
  - **Given** two active worktrees connected to the event broker
  - **When** worktree A commits an edit to a tracked file
  - **Then** worktree B receives the delta notification within 50ms and updates its buffer without data loss.
- **Scenario 2: Concurrent Mutation Resolution**
  - **Given** worktrees A and B holding concurrent uncommitted edits to non-overlapping lines
  - **When** both buffers flush changes to the synchronizer
  - **Then** the operational transform merges both line sets deterministically without conflict.

### US2: Offline Reconnection & Catch-Up (Priority: P2)
As a temporarily disconnected client,
I want to replay missed delta frames from the ledger sequence upon reconnecting,
So that local state achieves eventual consistency without full resynchronization.

**Acceptance Scenarios**:
- **Scenario 1: Fast Catch-Up Replay**
  - **Given** a client disconnected for 30 seconds
  - **When** the connection is re-established with last sequence number 1042
  - **Then** the broker transmits delta frames 1043 through current without dropping messages.

## 3. Success Metrics & Testability Criteria
- **SC-001**: Delta propagation latency remains under 50ms for payloads up to 64KB across 10 active peers.
- **SC-002**: 100% of non-overlapping line edits merge without operator intervention.
- **SC-003**: Reconnection replay recovers up to 10,000 sequenced events in under 500ms.

## 4. Edge Cases & Exclusions
- **Exclusion**: Binary blobs larger than 10MB are not handled by realtime sync; they route via content-addressed object store.
- **Edge Case**: Split-brain network partition resolves in favor of the largest quorum of nodes with cryptographic timestamps.
