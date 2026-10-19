package store

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
)

// The statuses a task moves between while it is worked. Anything else is a
// person's decision on the board, not the host's.
const (
	StatusReady      = "ready"
	StatusInProgress = "in_progress"
	StatusReview     = "review"
)

// itemPutFields are the item fields a status move carries forward from the
// stored item. Everything else `items.put` accepts keeps its stored value when
// omitted (locked fields, logs, archived, checklist, links, scope), which is
// what lets a host move a task without reading, size-checking or rewriting the
// parts of it that are not its business — and without being able to widen its
// scope. These are the ones omission would reset, so they are sent.
var itemPutFields = []string{
	"title", "description", "kind", "priority", "severity", "owner", "due_at",
	"labels", "acceptance_criteria", "repository_ids", "primary_repository_id",
	"home_workspace_id", "position",
}

// ErrNoSuchTask is a status move on an id the store holds no item for. It is
// an error, not a "not moved" answer: an operator who typed the wrong id must
// not be told the move was merely unnecessary.
var ErrNoSuchTask = errors.New("store: no such task")

// maxStatusMoveAttempts bounds the read-modify-write when another writer keeps
// editing the item between this client's read and its write.
const maxStatusMoveAttempts = 3

// MoveTaskStatus moves a task's item from one status to another, through the
// same revisioned `items.put` every other edit takes, and reports whether it
// moved. It moves only from `from` (empty means from anywhere, for an operator's
// explicit move): an item that is anywhere else — a person already moved it, or
// the verify ran on a task nobody checked out — is left exactly as it is and
// reported as not moved, which is an answer and not a fault. An id with no item
// is ErrNoSuchTask. A retry after a lost race re-reads, so it never writes over
// an edit it did not see.
func (c *Client) MoveTaskStatus(ctx context.Context, taskID, from, to string) (bool, error) {
	if taskID == "" || to == "" || from == to {
		return false, errors.New("store: a status move needs a task and a status to move to")
	}
	for attempt := 0; attempt < maxStatusMoveAttempts; attempt++ {
		request, err := json.Marshal(map[string]string{"id": taskID})
		if err != nil {
			return false, err
		}
		raw, err := c.Workbench(ctx, "items.get", request)
		if err != nil {
			var refusal *WorkbenchError
			if errors.As(err, &refusal) && refusal.Code == "not_found" {
				return false, fmt.Errorf("%w: %s", ErrNoSuchTask, taskID)
			}
			return false, err
		}
		var got struct {
			Item map[string]json.RawMessage `json:"item"`
		}
		if err := json.Unmarshal(raw, &got); err != nil || got.Item == nil {
			return false, fmt.Errorf("store: items.get for %s returned no item", taskID)
		}
		var status string
		var revision int64
		if json.Unmarshal(got.Item["status"], &status) != nil || json.Unmarshal(got.Item["revision"], &revision) != nil || revision <= 0 {
			return false, fmt.Errorf("store: item %s has no readable status and revision", taskID)
		}
		if status == to || (from != "" && status != from) {
			return false, nil
		}
		put := make(map[string]any, len(itemPutFields)+5)
		for _, field := range itemPutFields {
			if v, ok := got.Item[field]; ok {
				put[field] = v
			}
		}
		sum := sha256.Sum256([]byte(fmt.Sprintf("%s\x00%s\x00%s\x00%d", taskID, from, to, revision)))
		put["id"] = taskID
		put["status"] = to
		put["expected_revision"] = revision
		put["request_id"] = "mv-" + hex.EncodeToString(sum[:])[:40]
		body, err := json.Marshal(put)
		if err != nil {
			return false, err
		}
		if _, err := c.Workbench(ctx, "items.put", body); err != nil {
			var refusal *WorkbenchError
			if errors.As(err, &refusal) && refusal.Code == "revision_conflict" {
				continue
			}
			return false, err
		}
		return true, nil
	}
	return false, fmt.Errorf("store: item %s kept changing; its status was not moved to %s", taskID, to)
}
