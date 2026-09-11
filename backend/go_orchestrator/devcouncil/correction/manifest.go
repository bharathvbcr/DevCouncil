// Package correction ports Python planning/correction_manifest persistence
// shapes into Go. Manifests are written as JSON beside the task and recorded
// in dc-store's correction_manifests table when a store client is available.
package correction

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"time"
)

// Manifest is the repair brief an agent reads after a blocked verify.
type Manifest struct {
	ID          string         `json:"id"`
	TaskID      string         `json:"task_id"`
	RunID       string         `json:"run_id,omitempty"`
	Status      string         `json:"status"`
	Attempt     int            `json:"attempt"`
	RetryBudget int            `json:"retry_budget"`
	CreatedAt   string         `json:"created_at"`
	Gaps        []ManifestGap  `json:"gaps"`
	NextActions []string       `json:"next_actions"`
	Meta        map[string]any `json:"meta,omitempty"`
}

// ManifestGap is the subset of a Gap an agent needs to repair.
type ManifestGap struct {
	ID       string `json:"id"`
	GapType  string `json:"gap_type"`
	Severity string `json:"severity"`
	Blocking bool   `json:"blocking"`
	Action   string `json:"action"`
	File     string `json:"file,omitempty"`
}

// WriteOptions controls where the manifest lands on disk.
type WriteOptions struct {
	Root        string
	TaskID      string
	RunID       string
	Attempt     int
	RetryBudget int
	Gaps        []ManifestGap
	NextActions []string
}

// Write persists a correction manifest under .devcouncil/corrections/ and
// returns the absolute path. Status starts as "open".
func Write(opts WriteOptions) (Manifest, string, error) {
	if opts.Root == "" || opts.TaskID == "" {
		return Manifest{}, "", fmt.Errorf("root and task_id are required")
	}
	budget := opts.RetryBudget
	if budget <= 0 {
		budget = 3
	}
	now := time.Now().UTC().Format(time.RFC3339Nano)
	id := "cm-" + opts.TaskID + "-" + short(now)
	dir := filepath.Join(opts.Root, ".devcouncil", "corrections")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return Manifest{}, "", err
	}
	path := filepath.Join(dir, id+".json")
	m := Manifest{
		ID: id, TaskID: opts.TaskID, RunID: opts.RunID,
		Status: "open", Attempt: opts.Attempt, RetryBudget: budget,
		CreatedAt: now, Gaps: opts.Gaps, NextActions: opts.NextActions,
	}
	if m.Gaps == nil {
		m.Gaps = []ManifestGap{}
	}
	if m.NextActions == nil {
		m.NextActions = []string{}
	}
	raw, err := json.MarshalIndent(m, "", "  ")
	if err != nil {
		return Manifest{}, "", err
	}
	if err := os.WriteFile(path, raw, 0o644); err != nil {
		return Manifest{}, "", err
	}
	return m, path, nil
}

// RecordFunc is injected so this package does not depend on dcstore wire
// details. Callers that have a store pass a function that runs handoff-like
// persistence; nil is fine (disk-only).
type RecordFunc func(ctx context.Context, manifest Manifest, path string) error

// WriteAndRecord writes the manifest and optionally records it.
func WriteAndRecord(ctx context.Context, opts WriteOptions, record RecordFunc) (Manifest, string, error) {
	m, path, err := Write(opts)
	if err != nil {
		return m, path, err
	}
	if record != nil {
		if err := record(ctx, m, path); err != nil {
			return m, path, err
		}
	}
	return m, path, nil
}

func short(s string) string {
	sum := sha256.Sum256([]byte(s))
	return hex.EncodeToString(sum[:])[:10]
}
