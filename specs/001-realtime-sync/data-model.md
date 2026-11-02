# Data Model: 001 - Realtime Sync

## 1. Entities & Data Schemas

### SyncEvent
```typescript
interface SyncEvent {
  event_id: string;        // UUID v4
  sequence: number;         // Monotonically increasing 64-bit integer
  timestamp: string;        // ISO 8601 UTC
  source_worktree: string;  // Worktree ID emitting the event
  target_file: string;      // Relative repo path
  diff_payload: string;     // Unified diff or line edit instruction
  checksum: string;         // SHA-256 of resulting buffer
}
```

### SyncSessionState
```typescript
interface SyncSessionState {
  session_id: string;
  connected_worktrees: string[];
  last_committed_sequence: number;
  unacknowledged_events: SyncEvent[];
  status: "idle" | "streaming" | "recovering";
}
```

## 2. Invariants & Storage Constraints
- Sequences must be contiguous; any gap triggers catch-up replay before buffer application.
- Max diff payload size per event: 256 KB.
- Event log retention: 100,000 events or 7 days, whichever comes first.
