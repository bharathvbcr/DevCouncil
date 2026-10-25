package main

import (
	"fmt"
	"io"
	"os"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/console"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
)

const reviewHelp = `devcouncil review — the independent reviews llm_review criteria are proved by

An acceptance criterion whose verification_method is llm_review is discharged
by a recorded review of exactly the content under verification: reviewers that
provably did not see the author's context (isolation probe passed), at least
two of them, against a rubric file in this repository, with no blocker left
open. The harness that ran the reviewers records it. Recording is refused in
agent shells: an agent cannot vouch for its own work.

Usage:
  devcouncil review subject [--json] [--project-root DIR]
      Name the working tree's content, as a review must ("tree-sha256:<digest>").
  devcouncil review record --file RECORD.json [--json] [--project-root DIR]
      Record one review (a dc.ReviewRecord, schema 1). Run by the harness or a
      person, never by the agent whose work was reviewed.
  devcouncil review list --task ID [--criterion AC] [--json] [--project-root DIR]
      List a task's reviews, newest first.

A record names its subject_hash; verify uses only a review whose subject is the
content it is verifying, so a change made after a review needs a new one.
`

// maxReviewFile bounds the record read; the store refuses a larger one anyway.
const maxReviewFile = 1 << 20

func runReview(args []string) int {
	if len(args) < 1 {
		console.Error(reviewHelp)
		return 2
	}
	switch args[0] {
	case "-h", "--help":
		console.Error(reviewHelp)
		return 0
	case "subject":
		return runReviewSubject(args[1:])
	case "record":
		return runReviewRecord(args[1:])
	case "list":
		return runReviewList(args[1:])
	default:
		console.Errorf("unknown review subcommand: %s\n", args[0])
		console.Error(reviewHelp)
		return 2
	}
}

func runReviewSubject(args []string) int {
	f, ok := parseTaskFlags(args)
	if !ok {
		return 2
	}
	if len(f.rest) != 0 {
		console.Errorln("usage: devcouncil review subject [--json]")
		return 2
	}
	root, err := resolveRoot(f.root)
	if err != nil {
		console.Errorf("review subject: %v\n", err)
		return 1
	}
	subject, err := verify.WorkingTreeSubject(console.Context(), root)
	if err != nil {
		console.Errorf("review subject: %v\n", err)
		return 1
	}
	if f.json {
		if err := console.JSON(map[string]any{"ok": true, "subject_hash": subject}); err != nil {
			return 1
		}
		return 0
	}
	console.Println(subject)
	return 0
}

func runReviewRecord(args []string) int {
	f, ok := parseTaskFlags(args, "--file")
	if !ok {
		return 2
	}
	path := f.named["--file"]
	if path == "" || len(f.rest) != 0 {
		console.Errorln("usage: devcouncil review record --file RECORD.json")
		return 2
	}
	data, err := readBounded(path, maxReviewFile)
	if err != nil {
		console.Errorf("review record: %v\n", err)
		return 1
	}
	record, err := dc.DecodeReviewRecord(data)
	if err != nil {
		console.Errorf("review record: %v\n", err)
		return 1
	}
	client, root, err := taskClient(f.root, false)
	if err != nil {
		console.Errorf("review record: %v\n", err)
		return 1
	}
	defer client.Close()
	ctx := console.Context()
	id, err := client.ReviewAppend(ctx, record)
	if err != nil {
		console.Errorf("review record: %v\n", err)
		return 1
	}
	// Recorded either way: the record says what was reviewed, and a record of
	// content that has since moved is still a true account of a review. Whether
	// it is the content here now is reported, because only then will verify
	// use it.
	current, subjectErr := verify.WorkingTreeSubject(ctx, root)
	matches := subjectErr == nil && current == record.SubjectHash
	if f.json {
		out := map[string]any{"ok": true, "review_id": id, "subject_hash": record.SubjectHash,
			"matches_working_tree": matches}
		if subjectErr != nil {
			out["working_tree_error"] = subjectErr.Error()
		} else {
			out["working_tree_subject"] = current
		}
		if err := console.JSON(out); err != nil {
			return 1
		}
		return 0
	}
	console.Printf("recorded review %d of %s for %s/%s\n", id, record.SubjectHash, record.TaskID, record.AcceptanceCriterionID)
	switch {
	case subjectErr != nil:
		console.Printf("  the working tree could not be named (%v); verify will match this review only to %s\n", subjectErr, record.SubjectHash)
	case !matches:
		console.Printf("  the working tree is now %s, not the reviewed content: verify will not use this review until they match\n", current)
	}
	return 0
}

func runReviewList(args []string) int {
	f, ok := parseTaskFlags(args, "--task", "--criterion")
	if !ok {
		return 2
	}
	task := f.named["--task"]
	if task == "" || len(f.rest) != 0 {
		console.Errorln("usage: devcouncil review list --task ID [--criterion AC]")
		return 2
	}
	client, _, err := taskClient(f.root, false)
	if err != nil {
		console.Errorf("review list: %v\n", err)
		return 1
	}
	defer client.Close()
	reviews, err := client.Reviews(console.Context(), task, f.named["--criterion"])
	if err != nil {
		console.Errorf("review list: %v\n", err)
		return 1
	}
	if f.json {
		rows := make([]map[string]any, 0, len(reviews))
		for _, r := range reviews {
			row := map[string]any{"review_id": r.ID, "subject_hash": r.SubjectHash, "recorded_at": r.RecordedAt}
			if r.Err != nil {
				row["error"] = r.Err.Error()
			} else {
				row["record"] = r.Record
			}
			rows = append(rows, row)
		}
		if err := console.JSON(map[string]any{"ok": true, "task_id": task, "reviews": rows}); err != nil {
			return 1
		}
		return 0
	}
	if len(reviews) == 0 {
		console.Printf("no reviews recorded for %s\n", task)
		return 0
	}
	for _, r := range reviews {
		if r.Err != nil {
			console.Printf("%d  %s  %s  UNREADABLE: %v\n", r.ID, r.RecordedAt, r.SubjectHash, r.Err)
			continue
		}
		rec := r.Record
		console.Printf("%d  %s  %s  %s  isolation=%s  reviewers=%d/%d  findings=%d\n",
			r.ID, r.RecordedAt, rec.AcceptanceCriterionID, r.SubjectHash, rec.Isolation.Probe,
			rec.CompletedReviewers(), len(rec.Reviewers), len(rec.Findings))
	}
	return 0
}

// readBounded reads at most limit bytes of path, refusing a larger file
// rather than reading a prefix of it.
func readBounded(path string, limit int64) ([]byte, error) {
	file, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer file.Close()
	data, err := io.ReadAll(io.LimitReader(file, limit+1))
	if err != nil {
		return nil, err
	}
	if int64(len(data)) > limit {
		return nil, fmt.Errorf("%s is larger than the %d bytes a review record may be", path, limit)
	}
	return data, nil
}
