package dc

import (
	"bytes"
	"encoding/json"
	"fmt"
	"strings"
	"unicode"
	"unicode/utf8"
)

// ReviewRecord is one independent review of a task's change against one
// acceptance criterion whose verification method is llm_review: the evidence
// that discharges such a criterion.
//
// The shape is the Elephant-Goldfish Model's review contract made checkable.
// Reviewers run in fresh contexts that never saw the author's deliberation
// (the isolation probe proves it), at least two of them independently (one
// reviewer's miss is not the review's miss), against a rubric whose exact
// bytes are named (so "reviewed" means "reviewed against this"), and every
// finding carries a severity and what became of it.
//
// A record is written by the harness that ran the reviewers. It is never the
// word of the agent whose work was reviewed: the host refuses the recording
// command in agent shells and serves no MCP tool for it. What a record claims
// is checked twice, at write (DecodeReviewRecord refuses a malformed one) and
// again when verify reads it, because the store keeps whatever bytes it was
// given and a reader that trusted the writer would trust every writer.
type ReviewRecord struct {
	// Schema is ReviewRecordSchema. A record of another version is refused
	// rather than read through this layout.
	Schema                int    `json:"schema"`
	TaskID                string `json:"task_id"`
	AcceptanceCriterionID string `json:"acceptance_criterion_id"`
	// SubjectHash names the exact content reviewed, in the form
	// verify.WorkingTreeSubject computes ("git-tree:<object id>").
	SubjectHash string `json:"subject_hash"`
	// Harness names what ran the reviewers, for the reader of the record.
	Harness   string          `json:"harness"`
	Rubric    ReviewRubric    `json:"rubric"`
	Isolation ReviewIsolation `json:"isolation"`
	Reviewers []ReviewerRun   `json:"reviewers"`
	Findings  []ReviewFinding `json:"findings"`
}

// ReviewRubric is the standard the reviewers judged against: a file in the
// repository, named by path and by the SHA-256 of its bytes.
type ReviewRubric struct {
	Path   string `json:"path"`
	SHA256 string `json:"sha256"`
}

// ReviewIsolation is the result of the probe that shows the reviewers did not
// see the author's context.
type ReviewIsolation struct {
	Probe  IsolationProbe `json:"probe"`
	Detail string         `json:"detail"`
}

// IsolationProbe is what the isolation probe found.
type IsolationProbe string

const (
	// IsolationPassed: a reviewer was shown to see only its task, persona,
	// rubric and target — no parent history, no project instruction files.
	IsolationPassed IsolationProbe = "passed"
	IsolationFailed IsolationProbe = "failed"
	// IsolationNotRun is recorded honestly rather than omitted: a review
	// whose isolation nobody checked is a different fact from one that was
	// checked and failed, and neither discharges anything.
	IsolationNotRun IsolationProbe = "not_run"
)

// ReviewerRun is one reviewer the harness started.
type ReviewerRun struct {
	// ID is unique within the record: two runs of one persona are two ids.
	ID      string         `json:"id"`
	Persona string         `json:"persona"`
	Model   string         `json:"model"`
	Status  ReviewerStatus `json:"status"`
	Error   string         `json:"error,omitempty"`
}

// ReviewerStatus is how one reviewer run ended.
type ReviewerStatus string

const (
	ReviewerCompleted ReviewerStatus = "completed"
	ReviewerFailed    ReviewerStatus = "failed"
	ReviewerCancelled ReviewerStatus = "cancelled"
)

// ReviewFinding is one issue a reviewer raised, after deduplication.
type ReviewFinding struct {
	ID         string            `json:"id"`
	Severity   FindingSeverity   `json:"severity"`
	Summary    string            `json:"summary"`
	Location   string            `json:"location,omitempty"`
	Resolution FindingResolution `json:"resolution"`
	// Note is required when Resolution is contested: a finding set aside
	// without a reason is one nobody can check the setting aside of.
	Note string `json:"note,omitempty"`
}

// FindingSeverity is the review's triage tier.
type FindingSeverity string

const (
	SeverityBlocker FindingSeverity = "blocker"
	SeverityMajor   FindingSeverity = "major"
	SeverityNit     FindingSeverity = "nit"
)

// FindingResolution is what became of a finding before the record was made.
type FindingResolution string

const (
	ResolutionOpen FindingResolution = "open"
	// ResolutionFixed: the reviewed content already contains the fix. A fix
	// made after the review changes the content, and so the subject hash, and
	// needs a new review.
	ResolutionFixed FindingResolution = "fixed"
	// ResolutionContested: the finding was judged wrong, with a Note saying
	// why. Verify does not block on it but reports it, so a person sees every
	// finding set aside.
	ResolutionContested FindingResolution = "contested"
)

// ReviewRecordSchema is the record layout this code reads and writes.
const ReviewRecordSchema = 1

// MinIndependentReviewers is how many reviewers must complete for a review to
// discharge a criterion. Two is the Elephant-Goldfish Model's A/B sampling:
// one reviewer's blind spot is not the review's.
const MinIndependentReviewers = 2

// Bounds on a record, so one cannot grow without limit and every field fits
// a report line. Far past an honest review.
const (
	maxReviewers     = 64
	maxFindings      = 1000
	maxReviewText    = 4096
	maxReviewIDBytes = 256
	maxRecordBytes   = 1 << 20
)

// DecodeReviewRecord decodes and validates one record. Unknown fields are
// refused: a field this code does not read is a claim it cannot check, and a
// record that relied on one would read as checked.
func DecodeReviewRecord(data []byte) (ReviewRecord, error) {
	if len(data) > maxRecordBytes {
		return ReviewRecord{}, fmt.Errorf("review record is over %d bytes", maxRecordBytes)
	}
	dec := json.NewDecoder(bytes.NewReader(data))
	dec.DisallowUnknownFields()
	var r ReviewRecord
	if err := dec.Decode(&r); err != nil {
		return ReviewRecord{}, fmt.Errorf("review record: %w", err)
	}
	if dec.More() {
		return ReviewRecord{}, fmt.Errorf("review record: trailing data after the object")
	}
	if err := r.Validate(); err != nil {
		return ReviewRecord{}, err
	}
	return r, nil
}

// Validate checks the record's shape: every enum is a known member, every
// identifier is present and bounded, reviewer ids are unique, and a contested
// finding says why. It does not judge whether the review discharges anything;
// that is verify's question, asked against the content it is looking at.
func (r ReviewRecord) Validate() error {
	if r.Schema != ReviewRecordSchema {
		return fmt.Errorf("review record schema %d is not %d", r.Schema, ReviewRecordSchema)
	}
	for _, field := range []struct{ name, value string }{
		{"task_id", r.TaskID},
		{"acceptance_criterion_id", r.AcceptanceCriterionID},
		{"subject_hash", r.SubjectHash},
		{"harness", r.Harness},
		{"rubric.path", r.Rubric.Path},
	} {
		if err := checkReviewID(field.name, field.value); err != nil {
			return err
		}
	}
	if !ValidSubjectHash(r.SubjectHash) {
		return fmt.Errorf("review record subject_hash %q may hold only a-z, 0-9, ':' and '-'", r.SubjectHash)
	}
	if !validSHA256(r.Rubric.SHA256) {
		return fmt.Errorf("review record rubric.sha256 %q is not 64 lowercase hex digits", r.Rubric.SHA256)
	}
	switch r.Isolation.Probe {
	case IsolationPassed, IsolationFailed, IsolationNotRun:
	default:
		return fmt.Errorf("review record isolation.probe %q is not passed, failed or not_run", r.Isolation.Probe)
	}
	if err := checkReviewText("isolation.detail", r.Isolation.Detail); err != nil {
		return err
	}
	if len(r.Reviewers) == 0 || len(r.Reviewers) > maxReviewers {
		return fmt.Errorf("review record lists %d reviewers (want 1-%d)", len(r.Reviewers), maxReviewers)
	}
	seen := make(map[string]bool, len(r.Reviewers))
	for i, run := range r.Reviewers {
		if err := checkReviewID(fmt.Sprintf("reviewers[%d].id", i), run.ID); err != nil {
			return err
		}
		if seen[run.ID] {
			return fmt.Errorf("review record lists reviewer %q twice: independent runs need distinct ids", run.ID)
		}
		seen[run.ID] = true
		if err := checkReviewID(fmt.Sprintf("reviewers[%d].persona", i), run.Persona); err != nil {
			return err
		}
		if err := checkReviewText(fmt.Sprintf("reviewers[%d].model", i), run.Model); err != nil {
			return err
		}
		if err := checkReviewText(fmt.Sprintf("reviewers[%d].error", i), run.Error); err != nil {
			return err
		}
		switch run.Status {
		case ReviewerCompleted, ReviewerFailed, ReviewerCancelled:
		default:
			return fmt.Errorf("review record reviewer %q has status %q (want completed, failed or cancelled)", run.ID, run.Status)
		}
	}
	if len(r.Findings) > maxFindings {
		return fmt.Errorf("review record lists %d findings (at most %d)", len(r.Findings), maxFindings)
	}
	findingIDs := make(map[string]bool, len(r.Findings))
	for i, f := range r.Findings {
		if err := checkReviewID(fmt.Sprintf("findings[%d].id", i), f.ID); err != nil {
			return err
		}
		if findingIDs[f.ID] {
			return fmt.Errorf("review record lists finding %q twice", f.ID)
		}
		findingIDs[f.ID] = true
		switch f.Severity {
		case SeverityBlocker, SeverityMajor, SeverityNit:
		default:
			return fmt.Errorf("review record finding %q has severity %q (want blocker, major or nit)", f.ID, f.Severity)
		}
		switch f.Resolution {
		case ResolutionOpen, ResolutionFixed:
		case ResolutionContested:
			if strings.TrimSpace(f.Note) == "" {
				return fmt.Errorf("review record finding %q is contested with no note saying why", f.ID)
			}
		default:
			return fmt.Errorf("review record finding %q has resolution %q (want open, fixed or contested)", f.ID, f.Resolution)
		}
		if strings.TrimSpace(f.Summary) == "" {
			return fmt.Errorf("review record finding %q has no summary", f.ID)
		}
		for name, text := range map[string]string{"summary": f.Summary, "location": f.Location, "note": f.Note} {
			if err := checkReviewText(fmt.Sprintf("findings[%d].%s", i, name), text); err != nil {
				return err
			}
		}
	}
	return nil
}

// CompletedReviewers counts the reviewer runs that finished.
func (r ReviewRecord) CompletedReviewers() int {
	n := 0
	for _, run := range r.Reviewers {
		if run.Status == ReviewerCompleted {
			n++
		}
	}
	return n
}

// checkReviewID is the store's rule for a key, so a record Go accepts is one
// dc-store accepts: present, bounded, and no control character at all — a
// newline in an identifier is a second line in every report that prints it.
func checkReviewID(field, value string) error {
	if strings.TrimSpace(value) == "" || len(value) > maxReviewIDBytes {
		return fmt.Errorf("review record %s must be 1-%d bytes and not blank", field, maxReviewIDBytes)
	}
	if strings.ContainsFunc(value, unicode.IsControl) {
		return fmt.Errorf("review record %s carries a control character", field)
	}
	return checkReviewText(field, value)
}

// ValidSubjectHash reports whether s has the alphabet dc-store stores a
// subject hash in: lowercase letters, digits, ':' and '-'.
func ValidSubjectHash(s string) bool {
	if s == "" || len(s) > maxReviewIDBytes {
		return false
	}
	for _, c := range s {
		if !(c >= 'a' && c <= 'z' || c >= '0' && c <= '9' || c == ':' || c == '-') {
			return false
		}
	}
	return true
}

func checkReviewText(field, value string) error {
	if len(value) > maxReviewText {
		return fmt.Errorf("review record %s is over %d bytes", field, maxReviewText)
	}
	if !utf8.ValidString(value) {
		return fmt.Errorf("review record %s is not valid UTF-8", field)
	}
	for _, c := range value {
		if c < 0x20 && c != '\n' && c != '\t' {
			return fmt.Errorf("review record %s carries a control character", field)
		}
	}
	return nil
}

func validSHA256(s string) bool {
	if len(s) != 64 {
		return false
	}
	for _, c := range s {
		if !(c >= '0' && c <= '9' || c >= 'a' && c <= 'f') {
			return false
		}
	}
	return true
}
