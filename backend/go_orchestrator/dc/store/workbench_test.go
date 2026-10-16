package store

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestWorkbenchPersistsThroughHostRestartAndReusesOneChild(t *testing.T) {
	c := client(t)
	t.Cleanup(c.Close)
	call := func(method, input string) json.RawMessage {
		t.Helper()
		result, err := c.Workbench(t.Context(), method, json.RawMessage(input))
		if err != nil {
			t.Fatal(err)
		}
		return result
	}
	call("repositories.put", `{"request_id":"r","id":"r1","expected_revision":0,"name":"Manvi","identity_key":"local:r1"}`)
	first := idleChild(t, c)
	call("workspaces.put", `{"request_id":"w","id":"ws","expected_revision":0,"name":"Devtools","repository_ids":["r1"]}`)
	input := `{"request_id":"t","id":"t1","expected_revision":0,"title":"Preserve \"quoted\" errors\nand 日本語","description":"Detailed brief","repository_ids":["r1"],"primary_repository_id":"r1"}`
	receipt := call("items.put", input)
	if replay := call("items.put", input); !bytes.Equal(receipt, replay) {
		t.Fatal("retry lost original receipt")
	}
	for _, scope := range []string{`{}`, `{"workspace_id":"ws"}`, `{"repository_id":"r1"}`} {
		result := call("items.list", scope)
		var page struct {
			Total int `json:"total"`
			Items []struct {
				ID string `json:"id"`
			} `json:"items"`
		}
		if err := json.Unmarshal(result, &page); err != nil {
			t.Fatal(err)
		}
		if page.Total != 1 || len(page.Items) != 1 || page.Items[0].ID != "t1" {
			t.Fatalf("board: %s", result)
		}
		if bytes.Contains(result, []byte("Detailed brief")) {
			t.Fatal("board loaded full details")
		}
	}
	if idleChild(t, c) != first {
		t.Fatal("sequential task calls replaced the persistent child")
	}
	c.Close()
	c = New(c.Binary, c.DB)
	t.Cleanup(c.Close)
	if result := call("items.get", `{"id":"t1"}`); !bytes.Contains(result, []byte("Detailed brief")) {
		t.Fatalf("restart lost details: %s", result)
	}
}

func TestWorkbenchConflictsStayTypedAndDoNotBreakTheSession(t *testing.T) {
	c := client(t)
	t.Cleanup(c.Close)
	_, err := c.Workbench(t.Context(), "repositories.put", json.RawMessage(`{"request_id":"r","id":"r1","expected_revision":0,"name":"repo","identity_key":"r1"}`))
	if err != nil {
		t.Fatal(err)
	}
	first := idleChild(t, c)
	_, err = c.Workbench(t.Context(), "repositories.put", json.RawMessage(`{"request_id":"stale","id":"r1","expected_revision":0,"name":"overwrite","identity_key":"r1"}`))
	var refusal *WorkbenchError
	if !errors.As(err, &refusal) || refusal.Code != "revision_conflict" {
		t.Fatalf("conflict lost: %v", err)
	}
	if _, err := c.Workbench(t.Context(), "items.list", json.RawMessage(`{"workspace_id":"absent"}`)); !errors.As(err, &refusal) || refusal.Code != "not_found" {
		t.Fatalf("missing scope: %v", err)
	}
	if _, err := c.Workbench(t.Context(), "workspaces.list", nil); err != nil {
		t.Fatal(err)
	}
	if idleChild(t, c) != first {
		t.Fatal("a logical refusal destroyed a healthy child")
	}
}

func TestWorkbenchCannotAffectAnotherProfilesExecutionLease(t *testing.T) {
	leaseClient := client(t)
	seedTask(t, leaseClient, "t1")
	t.Cleanup(leaseClient.Close)
	lease, err := leaseClient.Acquire(t.Context(), AcquireRequest{TaskID: "t1", Owner: "builder", TTL: time.Minute})
	if err != nil {
		t.Fatal(err)
	}
	profile := New(leaseClient.Binary, filepath.Join(t.TempDir(), "profile.sqlite"))
	t.Cleanup(profile.Close)
	if _, err := profile.Workbench(t.Context(), "items.list", nil); err != nil {
		t.Fatal(err)
	}
	valid, err := leaseClient.Valid(t.Context(), "t1", lease.Token)
	if err != nil || !valid {
		t.Fatalf("profile operation changed execution lease: %v %v", valid, err)
	}
}

func TestInvalidWorkbenchInputStartsNoStoreAndCreatesNoDatabase(t *testing.T) {
	c := New("must-not-run", filepath.Join(t.TempDir(), "profile.sqlite"))
	t.Cleanup(c.Close)
	for _, input := range []json.RawMessage{json.RawMessage(`null`), json.RawMessage(`[]`), json.RawMessage(`{`), json.RawMessage(strings.Repeat(" ", maxWorkbenchInput+1)), json.RawMessage{'{', '"', 'x', '"', ':', '"', 0xff, '"', '}'}} {
		_, err := c.Workbench(t.Context(), "items.list", input)
		var refusal *WorkbenchError
		if !errors.As(err, &refusal) || refusal.Code != "invalid_input" {
			t.Fatalf("invalid request crossed boundary: %v", err)
		}
	}
	if _, err := os.Stat(c.DB); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("invalid input created a database: %v", err)
	}
	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	if _, err := c.Workbench(ctx, "items.list", nil); err == nil {
		t.Fatal("cancelled call succeeded")
	}
}

func TestMalformedWorkbenchRepliesCannotBecomeHealthyEmptyBoards(t *testing.T) {
	for _, raw := range []string{
		"{\"ok\":true,\"item\":{\"id\":\"\xff\",\"revision\":1}}",
		`{}`, `null`, `{"ok":true}`, `{"ok":false}`, `{"ok":false,"code":"not_found"}`,
		`{"ok":true,"items":[]}`, `{"ok":true,"item":{}}`,
		`{"ok":true,"items":[],"total":5,"shown":0,"has_more":true,"next_cursor":null}`,
		`{"ok":true,"items":[],"total":0,"shown":1,"has_more":false,"next_cursor":null}`,
		`{"ok":false,"ok":true,"item":{"id":"r","revision":1}}`,
		`{"ok":false,"\u006fk":true,"item":{"id":"r","revision":1}}`,
	} {
		out, err := decodeReply("work", []byte(raw), false, nil, nil)
		if err == nil || out != nil {
			t.Fatalf("malformed work result accepted: %s", raw)
		}
	}
}

func FuzzWorkbenchNeverAcceptsPartialOrFailedChildOutput(f *testing.F) {
	for _, raw := range []string{`{}`, `null`, `{"ok":true,"item":{"id":"r","revision":1}}`, `{"ok":true,"items":[],"total":0,"shown":0,"has_more":false,"next_cursor":null}`, `{"ok":false,"code":"not_found","error":"missing"}`} {
		f.Add(raw, false, false)
	}
	f.Fuzz(func(t *testing.T, raw string, failed, overflow bool) {
		var failure error
		if failed {
			failure = errors.New("child failed")
		}
		out, err := decodeReply("work", []byte(raw), overflow, nil, failure)
		if (out == nil) == (err == nil) {
			t.Fatal("result/error exclusivity violated")
		}
		if (failed || overflow) && out != nil {
			t.Fatal("failed or truncated child accepted")
		}
		if out != nil && (!json.Valid(out.Workbench) || !out.OK) {
			t.Fatal("unvalidated work result accepted")
		}
	})
}

func TestWorkbenchCompactionRunsThroughTheHostClientAndKeepsReceipts(t *testing.T) {
	c := client(t)
	t.Cleanup(c.Close)
	call := func(method, input string) json.RawMessage {
		t.Helper()
		result, err := c.Workbench(t.Context(), method, json.RawMessage(input))
		if err != nil {
			t.Fatalf("%s: %v", method, err)
		}
		return result
	}
	// An empty profile has no event to report a sequence for; the reply is still valid.
	call("store.compact", `{}`)
	call("repositories.put", `{"request_id":"r","id":"r1","expected_revision":0,"name":"Manvi","identity_key":"local:r1"}`)
	input := `{"request_id":"t","id":"t1","expected_revision":0,"title":"Task","repository_ids":["r1"],"primary_repository_id":"r1"}`
	receipt := call("items.put", input)
	for rev := 1; rev <= 6; rev++ {
		call("items.put", fmt.Sprintf(`{"request_id":"t-%d","id":"t1","expected_revision":%d,"title":"Task %d","repository_ids":["r1"],"primary_repository_id":"r1"}`, rev, rev, rev))
	}
	report := call("store.compact", `{"keep_revisions":2}`)
	var wire struct {
		Item struct {
			Revisions struct{ Deleted, Eligible int } `json:"revisions"`
		} `json:"item"`
	}
	if err := json.Unmarshal(report, &wire); err != nil {
		t.Fatal(err)
	}
	if wire.Item.Revisions.Deleted != 5 {
		t.Fatalf("compaction report: %s", report)
	}
	if replay := call("items.put", input); !bytes.Equal(receipt, replay) {
		t.Fatal("a retry inside the window lost its original receipt")
	}
	if _, err := c.Workbench(t.Context(), "store.compact", json.RawMessage(`{"receipt_window_seconds":1}`)); err == nil {
		t.Fatal("a one-second replay window was accepted")
	}
}

func TestWorkbenchUpgradeAndScanGuardAreTypedAndTravelThroughTheHostClient(t *testing.T) {
	c := client(t)
	t.Cleanup(c.Close)
	call := func(method, input string) json.RawMessage {
		t.Helper()
		result, err := c.Workbench(t.Context(), method, json.RawMessage(input))
		if err != nil {
			t.Fatalf("%s: %v", method, err)
		}
		return result
	}
	// A profile this call creates is already current: nothing to upgrade, no backup.
	var upgrade struct {
		Item struct {
			Upgraded bool    `json:"upgraded"`
			Backup   *string `json:"backup"`
		} `json:"item"`
	}
	if err := json.Unmarshal(call("store.upgrade", `{}`), &upgrade); err != nil || upgrade.Item.Upgraded || upgrade.Item.Backup != nil {
		t.Fatalf("a current profile reported an upgrade: %+v %v", upgrade, err)
	}
	call("repositories.put", `{"request_id":"r","id":"r1","expected_revision":0,"name":"Manvi","identity_key":"local:r1"}`)
	var events struct {
		Latest int64 `json:"latest_sequence"`
	}
	if err := json.Unmarshal(call("events.list", `{}`), &events); err != nil || events.Latest <= 0 {
		t.Fatalf("events.list has no high-water mark: %+v %v", events, err)
	}
	task := func(id string, scan int64) string {
		return fmt.Sprintf(`{"request_id":"%s","id":"%s","expected_revision":0,"title":"%s","repository_ids":["r1"],"primary_repository_id":"r1","scan_sequence":%d}`, id, id, id, scan)
	}
	call("items.put", task("first", events.Latest))
	_, err := c.Workbench(t.Context(), "items.put", json.RawMessage(task("second", events.Latest)))
	var refusal *WorkbenchError
	if !errors.As(err, &refusal) || refusal.Code != "board_changed" {
		t.Fatalf("a stale scan was not a typed board_changed refusal: %v", err)
	}
}
