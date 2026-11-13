package main

import (
	"encoding/json"
	"strings"
	"testing"
)

func TestCanonicalTaskStatusReadsInboxAsBacklog(t *testing.T) {
	for _, tc := range []struct {
		in    string
		want  string
		valid bool
	}{
		{"inbox", "backlog", true},
		{"backlog", "backlog", true},
		{"ready", "ready", true},
		{"in_progress", "in_progress", true},
		{"review", "review", true},
		{"done", "done", true},
		// The alias is the exact word, as in the store.
		{"Inbox", "Inbox", false},
		{" inbox", " inbox", false},
		{"triage", "triage", false},
		{"", "", false},
	} {
		got, valid := canonicalTaskStatus(tc.in)
		if got != tc.want || valid != tc.valid {
			t.Errorf("canonicalTaskStatus(%q) = (%q, %v), want (%q, %v)", tc.in, got, valid, tc.want, tc.valid)
		}
	}
}

func TestTaskHelpAdvertisesFiveStatusesAndNotInbox(t *testing.T) {
	if strings.Contains(strings.ToLower(taskHelp), "inbox") {
		t.Errorf("task help still mentions inbox:\n%s", taskHelp)
	}
	if !strings.Contains(taskHelp, "--to backlog|ready|in_progress|review|done") {
		t.Errorf("task help does not list the five statuses:\n%s", taskHelp)
	}
	stderr, restore := swapStderr(t)
	code := dispatch([]string{"--help"})
	restore()
	if code != 0 {
		t.Fatalf("--help exit %d", code)
	}
	if strings.Contains(strings.ToLower(stderr.String()), "inbox") {
		t.Errorf("usage still mentions inbox:\n%s", stderr.String())
	}
	if !strings.Contains(stderr.String(), "--to backlog|ready|in_progress|review|done") {
		t.Errorf("usage does not list the five statuses:\n%s", stderr.String())
	}
}

func TestTaskStatusRefusalNamesTheFiveStatuses(t *testing.T) {
	root := t.TempDir()
	for _, args := range [][]string{
		{"task", "status", "T-1", "--to", "triage", "--project-root", root},
		{"task", "create", "--id", "T-1", "--title", "t", "--status", "triage", "--project-root", root},
		{"spec", "export-tasks", "feature", "--target-status", "triage", "--project-root", root},
	} {
		stderr, restore := swapStderr(t)
		code := dispatch(args)
		restore()
		if code != 2 {
			t.Errorf("%v exit %d, want 2", args, code)
		}
		out := stderr.String()
		if !strings.Contains(out, "backlog, ready, in_progress, review or done") || strings.Contains(strings.ToLower(out), "inbox") {
			t.Errorf("%v refusal = %q", args, out)
		}
	}
}

func TestInboxIsAcceptedAsBacklogOnTheCommandLine(t *testing.T) {
	root := t.TempDir()
	status := func(id string) string {
		stdout, restoreOut := swapStdout(t)
		stderr, restoreErr := swapStderr(t)
		code := dispatch([]string{"task", "show", id, "--json", "--project-root", root})
		restoreOut()
		restoreErr()
		if code != 0 {
			t.Fatalf("task show %s exit %d stderr=%s", id, code, stderr.String())
		}
		var payload struct {
			Item struct {
				Status string `json:"status"`
			} `json:"item"`
		}
		if err := json.Unmarshal(stdout.Bytes(), &payload); err != nil {
			t.Fatalf("task show output: %v (%s)", err, stdout.String())
		}
		return payload.Item.Status
	}
	stderr, restore := swapStderr(t)
	code := dispatch([]string{"task", "create", "--id", "T-1", "--title", "first", "--status", "inbox", "--project-root", root})
	restore()
	if code != 0 {
		t.Fatalf("task create --status inbox exit %d stderr=%s", code, stderr.String())
	}
	if got := status("T-1"); got != "backlog" {
		t.Fatalf("created with --status inbox, stored as %q, want backlog", got)
	}

	stderr, restore = swapStderr(t)
	code = dispatch([]string{"task", "status", "T-1", "--to", "ready", "--project-root", root})
	restore()
	if code != 0 || status("T-1") != "ready" {
		t.Fatalf("task status --to ready exit %d stderr=%s", code, stderr.String())
	}
	stderr, restore = swapStderr(t)
	code = dispatch([]string{"task", "status", "T-1", "--to", "inbox", "--json", "--project-root", root})
	restore()
	if code != 0 {
		t.Fatalf("task status --to inbox exit %d stderr=%s", code, stderr.String())
	}
	if got := status("T-1"); got != "backlog" {
		t.Fatalf("moved --to inbox, stored as %q, want backlog", got)
	}
}
