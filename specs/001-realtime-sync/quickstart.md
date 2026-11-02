# Quickstart & Verification Guide: 001 - Realtime Sync

## 1. Quick Verification
To validate this specification against project constitution and schema rules:
```bash
dev spec validate 001-realtime-sync --json
```

## 2. Dry-Run Task Export
To inspect the tasks generated from `tasks.md` without modifying state store:
```bash
dev spec export-tasks 001-realtime-sync --dry-run
```

## 3. Automated Test Verification
Run test suites covering the specification and task export engine:
```bash
cd backend/go_orchestrator && go test ./devcouncil/spec/...
```
