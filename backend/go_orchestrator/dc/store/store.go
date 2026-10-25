// Package store is the Go side of the harness's state boundary.
//
// The execution plane is Go; the state store is Rust, because SQLite is. The
// two are joined by a process rather than by cgo: linking them in-process would
// forfeit CGO_ENABLED=0, simple cross-compilation, and the single static binary
// that the Go side was chosen for in the first place. So this package runs the
// `dcstore` binary and reads JSON objects from its stdout.
//
// The process is kept; the fork per call is not. A store call used to be a
// whole process — ~3.9ms measured from here on an idle machine, of which
// ~2.1ms was fork and exec — on the path every write-gate check goes through. The client now holds a small
// pool of long-lived store processes and sends each of them many requests, and
// falls back to one process per call for a binary that predates that. See
// serve.go, which also records why the boundary itself stays a process.
//
// The boundary is deliberately narrow. Everything that decides *whether* work
// may proceed — mutual exclusion, expiry, ownership — lives on the Rust side
// where the schema's partial unique index enforces it. This package transports
// answers; it does not compute them, and it must never be tempted to cache one.
// A cached lease is a lease that has already expired somewhere else.
package store

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os/exec"
	"strconv"
	"strings"
	"sync/atomic"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

// Client runs the store binary.
type Client struct {
	// Binary is the path to `dcstore`.
	Binary string
	// DB is the SQLite file both this harness and DevCouncil read.
	DB string
	// Timeout bounds a single invocation. A store call is local and fast; a
	// hung one means something is wrong with the filesystem or the lock, and
	// blocking a turn forever is the wrong answer to that.
	Timeout time.Duration

	// pool holds the persistent store processes this client talks to. It is
	// lazy: a client that is never called starts nothing. See serve.go.
	pool servePool

	// oneShot records that this binary does not understand `serve`, so the
	// discovery is made once rather than on every call. It is only ever set
	// from false to true, and only after a refusal that provably ran no
	// command.
	oneShot atomic.Bool
}

// New builds a client with a default timeout.
func New(binary, db string) *Client {
	return &Client{Binary: binary, DB: db, Timeout: 10 * time.Second}
}

// Lease mirrors the store's lease record.
type Lease struct {
	ID        string `json:"id"`
	TaskID    string `json:"task_id"`
	Owner     string `json:"owner"`
	Agent     string `json:"agent,omitempty"`
	ClientID  string `json:"client_id,omitempty"`
	RunID     string `json:"run_id,omitempty"`
	Branch    string `json:"branch,omitempty"`
	Token     string `json:"token"`
	Status    string `json:"status"`
	CreatedAt string `json:"created_at"`
	ExpiresAt string `json:"expires_at,omitempty"`
}

// UnreadableLease is an active lease whose stored expiry is not a timestamp.
// It fails closed for its own task: nothing is offered or acquired over it
// without a forced acquire, which retires it.
type UnreadableLease struct {
	TaskID    string  `json:"task_id"`
	LeaseID   string  `json:"lease_id"`
	ExpiresAt *string `json:"expires_at"`
}

// response is the envelope every command returns.
type response struct {
	OK              bool     `json:"ok"`
	Store           string   `json:"store"`
	SchemaVersion   int      `json:"schema_version"`
	ExclusionIndex  string   `json:"exclusion_index"`
	Code            string   `json:"code"`
	Error           string   `json:"error"`
	Holder          string   `json:"holder"`
	Lease           *Lease   `json:"lease"`
	Leases          []Lease  `json:"leases"`
	Released        bool     `json:"released"`
	SuggestedAction string   `json:"suggested_action"`
	SuggestedTool   string   `json:"suggested_tool"`
	Task            *Task    `json:"task"`
	Tasks           []string `json:"tasks"`
	// Ready is `ready`'s detail, one entry per id in Tasks and in the same order.
	Ready  []ReadyTask `json:"ready"`
	Mode   string      `json:"mode"`
	RepoID string      `json:"repository_id"`
	// Degraded and UnreadableLeases: leases whose expiry the store could not
	// read. They are excluded from Tasks and Leases and named here.
	Degraded         bool              `json:"degraded"`
	UnreadableLeases []UnreadableLease `json:"unreadable_leases"`
	Written          bool              `json:"written"`
	// CurrentAppended is the appended-scope value the store holds, sent back
	// when a compare-and-swap found a different one. It is raw so the retry
	// compares the store's own bytes rather than a re-serialisation of them.
	CurrentAppended json.RawMessage `json:"current_appended"`
	// Workbench retains the bounded profile result after its separate envelope
	// contract has been checked. Execution-lease commands never populate it.
	Workbench   json.RawMessage `json:"-"`
	EvidenceID  int64           `json:"-"`
	ComponentID string          `json:"-"`
	Version     string          `json:"version"`
	// GapID is gap-upsert's answer. It is a separate key from EvidenceID
	// rather than sharing `id`: one envelope decodes every reply on this
	// boundary, and a string and an integer cannot occupy one field.
	GapID    string        `json:"gap_id"`
	Evidence []EvidenceRow `json:"evidence"`
	// Requirements, Missing and TaskFound are the `requirements` reply.
	// TaskFound is a pointer so a reply without the key is refused rather
	// than read as an unknown task.
	Requirements []requirementRow `json:"requirements"`
	Missing      []string         `json:"missing"`
	TaskFound    *bool            `json:"task_found"`
	Gaps         []GapRow         `json:"gaps"`
	History      []GapHistoryRow  `json:"history"`
	Truncated    bool             `json:"truncated"`
	Shown        int              `json:"shown"`
	// ReviewID is review-append's answer and Reviews the `reviews` reply.
	// ReviewID is its own key for the reason GapID is: `id` is the evidence
	// row's integer.
	ReviewID int64       `json:"review_id"`
	Reviews  []reviewRow `json:"reviews"`
}

func (r *response) UnmarshalJSON(data []byte) error {
	type alias response
	var raw struct {
		alias
		RawID json.RawMessage `json:"id"`
	}
	if err := json.Unmarshal(data, &raw); err != nil {
		return err
	}
	*r = response(raw.alias)
	if len(raw.RawID) > 0 {
		var intID int64
		if err := json.Unmarshal(raw.RawID, &intID); err == nil {
			r.EvidenceID = intID
		} else {
			var strID string
			if err := json.Unmarshal(raw.RawID, &strID); err == nil {
				r.ComponentID = strID
			}
		}
	}
	return nil
}

// Conflict reports that another agent holds the task. It is a distinct type
// because contention is an ordinary outcome an orchestrator routes on — take
// the next task — not a failure it should retry or surface as an outage.
type Conflict struct {
	TaskID string
	Holder string
}

func (c *Conflict) Error() string {
	return fmt.Sprintf("task %s is held by %s", c.TaskID, c.Holder)
}

// Code returns the lease code an agent branches on.
func (c *Conflict) Code() dc.LeaseCode { return dc.LeaseHeldByOther }

// TaskUnavailable reports that the task cannot be leased: it is not a live item
// of this repository, it is archived, or its status is not one that can be
// worked. Like Conflict it is an ordinary outcome a caller routes on.
type TaskUnavailable struct {
	TaskID string
	// Code is the store's: no_such_task, task_archived or task_not_ready.
	Code   string
	Reason string
}

func (e *TaskUnavailable) Error() string { return "store: " + e.Reason }

// AcquireRequest asks for a lease.
type AcquireRequest struct {
	TaskID   string
	Owner    string
	Agent    string
	ClientID string
	RunID    string
	Branch   string
	// TTL bounds how long the lease survives without renewal. Zero is refused:
	// a lease that cannot expire is a task that stays locked when its holder
	// dies, and the harness cannot tell the difference between a builder that
	// is thinking and one whose process is gone.
	TTL time.Duration
	// Force steals an active lease. Human intervention only — the grant seam
	// is how an agent asks for something like this, and it cannot grant it.
	Force bool
}

// Acquire takes a lease, or returns *Conflict when another agent holds it.
func (c *Client) Acquire(ctx context.Context, req AcquireRequest) (*Lease, error) {
	if req.TTL <= 0 {
		return nil, errors.New("store: a lease needs a TTL; an unexpiring lease strands the task when its holder dies")
	}
	args := []string{"acquire",
		"--task", req.TaskID,
		"--owner", req.Owner,
		"--ttl-seconds", strconv.Itoa(int(req.TTL.Seconds())),
	}
	for flag, value := range map[string]string{
		"--agent": req.Agent, "--client-id": req.ClientID,
		"--run-id": req.RunID, "--branch": req.Branch,
	} {
		if value != "" {
			args = append(args, flag, value)
		}
	}
	if req.Force {
		args = append(args, "--force", "true")
	}

	out, err := c.run(ctx, args...)
	if err != nil {
		return nil, err
	}
	if !out.OK {
		if out.Code == string(dc.LeaseHeldByOther) {
			return nil, &Conflict{TaskID: req.TaskID, Holder: out.Holder}
		}
		switch out.Code {
		case "no_such_task", "task_archived", "task_not_ready":
			return nil, &TaskUnavailable{TaskID: req.TaskID, Code: out.Code, Reason: out.Error}
		}
		return nil, fmt.Errorf("store: acquire failed: %s", out.Error)
	}
	return out.Lease, nil
}

// Diagnose classifies a token, returning the code and the recovery an agent
// should take. This is the check every mutating tool call passes through.
func (c *Client) Diagnose(ctx context.Context, taskID, token string) (dc.LeaseCode, string, string, error) {
	out, err := c.run(ctx, "diagnose", "--task", taskID, "--token", token)
	if err != nil {
		return "", "", "", err
	}
	code := dc.LeaseCode(out.Code)
	if code == "" {
		return "", "", "", errors.New("store: diagnose returned no code")
	}
	return code, out.SuggestedAction, out.SuggestedTool, nil
}

// Valid is the predicate the write gate uses.
func (c *Client) Valid(ctx context.Context, taskID, token string) (bool, error) {
	code, _, _, err := c.Diagnose(ctx, taskID, token)
	if err != nil {
		// A store that cannot be reached must not read as a valid lease. Fail
		// closed: the whole point of the check is to stop a write.
		return false, err
	}
	return code == dc.LeaseValid, nil
}

// Release ends a lease. It reports whether one was actually released, so a
// caller can tell "released" from "there was nothing to release".
func (c *Client) Release(ctx context.Context, taskID, token string) (bool, error) {
	out, err := c.run(ctx, "release", "--task", taskID, "--token", token)
	if err != nil {
		return false, err
	}
	return out.Released, nil
}

// Renew extends a lease before it expires. A nil lease with no error means the
// lease had already expired — renewal only works before the TTL passes, and the
// recovery is to check out again.
func (c *Client) Renew(ctx context.Context, taskID, token string, ttl time.Duration) (*Lease, error) {
	out, err := c.run(ctx, "renew", "--task", taskID, "--token", token,
		"--ttl-seconds", strconv.Itoa(int(ttl.Seconds())))
	if err != nil {
		return nil, err
	}
	if !out.OK {
		return nil, nil
	}
	return out.Lease, nil
}

// Active returns the live lease on a task, or nil.
func (c *Client) Active(ctx context.Context, taskID string) (*Lease, error) {
	out, err := c.run(ctx, "active", "--task", taskID)
	if err != nil {
		return nil, err
	}
	return out.Lease, nil
}

// ActiveLeases lists every live lease — what `dev tasks` shows in its Lease
// column, read from the same file.
func (c *Client) ActiveLeases(ctx context.Context) ([]Lease, error) {
	out, err := c.run(ctx, "list")
	if err != nil {
		return nil, err
	}
	return out.Leases, nil
}

// Task is a task's scope, as the gate needs it.
type Task struct {
	ID         string `json:"id"`
	Title      string `json:"title"`
	Status     string `json:"status"`
	Difficulty string `json:"difficulty,omitempty"`
	// PlannedFiles is the union the store returns: the plan, plus whatever an
	// executor appended to it. Domain splits the two apart.
	PlannedFiles     []dc.PlannedFile `json:"planned_files"`
	AllowedCommands  []string         `json:"allowed_commands"`
	ExpectedTests    []string         `json:"expected_tests"`
	ForbiddenChanges []string         `json:"forbidden_changes"`
	// AgentAppendedRaw is the appended-scope column exactly as stored. It is
	// kept in its wire form because the store's compare-and-swap compares text:
	// a re-serialisation would differ in whitespace or in the bare-string form
	// a hand-written plan may use, and every widening would report as a
	// conflict with itself.
	AgentAppendedRaw json.RawMessage `json:"agent_appended_planned_files"`
	// AgentAppended is AgentAppendedRaw decoded, filled in by Client.Task.
	AgentAppended []dc.PlannedFile `json:"-"`
	// RequirementIDs and AcceptanceCriterionIDs are why the task exists and
	// what it must prove. The store creates both columns because they are in
	// DevCouncil's schema, and until recently selected neither, so every task
	// crossing this boundary arrived with no requirements at all -- which a
	// coverage gate cannot tell apart from a task that satisfies none.
	RequirementIDs         []string `json:"requirement_ids"`
	AcceptanceCriterionIDs []string `json:"acceptance_criterion_ids"`
	// BlockingGapCount is derived from the task's gaps on every read; "blocked"
	// is not a status. An explicit checkout of a blocked task still works, and
	// this is how its payload says why it is blocked.
	BlockingGapCount int `json:"blocking_gap_count"`
}

// Domain converts to the type the policy gate evaluates against.
//
// The union the store returns is split back into the plan and the runtime
// widening, because the gate has to be able to say which authorised a write.
// The subtraction is by path: an entry that appears in both is attributed to
// the widening, which is the conservative direction — it under-reports scope as
// planned, never over-reports it.
func (t *Task) Domain() *dc.Task {
	if t == nil {
		return nil
	}
	planned := t.PlannedFiles
	if len(t.AgentAppended) > 0 {
		appended := make(map[string]bool, len(t.AgentAppended))
		for _, pf := range t.AgentAppended {
			appended[pf.Path] = true
		}
		planned = make([]dc.PlannedFile, 0, len(t.PlannedFiles))
		for _, pf := range t.PlannedFiles {
			if !appended[pf.Path] {
				planned = append(planned, pf)
			}
		}
	}
	return &dc.Task{
		ID:                        t.ID,
		Title:                     t.Title,
		Status:                    t.Status,
		Difficulty:                t.Difficulty,
		PlannedFiles:              planned,
		AgentAppendedPlannedFiles: t.AgentAppended,
		AllowedCommands:           t.AllowedCommands,
		ForbiddenChanges:          t.ForbiddenChanges,
		RequirementIDs:            t.RequirementIDs,
		AcceptanceCriterionIDs:    t.AcceptanceCriterionIDs,
	}
}

// Task reads one task's scope. A nil task with no error means the id is unknown
// — a normal answer a caller branches on, not a fault.
func (c *Client) Task(ctx context.Context, taskID string) (*Task, error) {
	out, err := c.run(ctx, "task", "--task", taskID)
	if err != nil {
		return nil, err
	}
	if out.Task == nil {
		return nil, nil
	}
	// Decoded here rather than lazily at the gate. An unreadable scope column
	// must not reach a write decision as an empty one: "the plan authorises
	// nothing extra" and "nobody could read what the plan authorises" are
	// different answers, and only one of them is safe to act on.
	if len(trimJSONSpace(out.Task.AgentAppendedRaw)) > 0 {
		if err := json.Unmarshal(out.Task.AgentAppendedRaw, &out.Task.AgentAppended); err != nil {
			return nil, fmt.Errorf("store: task %s has an unreadable agent-appended scope: %w", taskID, err)
		}
	}
	return out.Task, nil
}

// Scope-widening ceilings. They mirror the store's own, and exist on this side
// as well because this is where the list is assembled: a caller that would
// exceed them should be told before a 64 KiB argv is built, not after.
const (
	maxAppendedScopeEntries = 256
	maxAppendedScopeBytes   = 64 << 10
	// A stale swap means someone else widened this task between the read and
	// the write. Re-merging against what the store handed back is the right
	// response, and bounding the attempts is what stops a permanently-contended
	// task from spinning.
	//
	// Eight rather than a smaller number because each lost race is another
	// contender that has *succeeded*: under n-way contention a writer can be
	// displaced n-1 times before its own turn comes, and a ceiling below that
	// converts ordinary queueing into a refusal. Contention this high should be
	// impossible — one lease per task, one holder, sequential calls — so the
	// headroom costs nothing in the case that actually happens.
	maxScopeAppendAttempts = 8
)

// AppendPlannedFiles widens a task's own plan with paths its lease holder
// argued for, and returns the entries actually added.
//
// This is the durable half of the override seam. A grant makes one blocked
// write accountable and then expires; on a task longer than the grant TTL the
// agent meets the same block again and re-argues it, which is a worse use of
// everyone's attention than recording the conclusion. Writing the path into the
// task's appended scope records it where the gate reads scope from — and the
// gate marks every write it authorises as widened, so the record does not
// become indistinguishable from the plan.
//
// Already-scoped paths are dropped rather than appended again: the store has no
// set semantics, and a retried write would otherwise grow the column on every
// attempt. Nothing to add is not an error — it is the answer when the path was
// already in scope.
func (c *Client) AppendPlannedFiles(ctx context.Context, taskID, token string, add []dc.PlannedFile) ([]dc.PlannedFile, error) {
	if strings.TrimSpace(taskID) == "" || strings.TrimSpace(token) == "" {
		return nil, errors.New("store: widening scope needs the task and the lease token that authorises it")
	}

	wanted := make([]dc.PlannedFile, 0, len(add))
	for _, pf := range add {
		path := strings.TrimSpace(pf.Path)
		if path == "" {
			return nil, errors.New("store: a scope entry with no path widens nothing")
		}
		change := pf.AllowedChange
		if change == "" {
			change = dc.ChangeModify
		}
		// Appended scope may only widen. A read-only entry is a restriction,
		// and a restriction an executor writes about itself is not a
		// restriction — it is a way to make the plan look stricter than the
		// permissions actually in force.
		if change == dc.ChangeReadOnly {
			return nil, fmt.Errorf("store: %q cannot be appended read-only; appended scope only ever widens", path)
		}
		if !change.Valid() {
			return nil, fmt.Errorf("store: %q has allowed_change %q, which is not one of create, modify, delete", path, string(change))
		}
		wanted = append(wanted, dc.PlannedFile{Path: path, AllowedChange: change})
	}
	if len(wanted) == 0 {
		return nil, nil
	}

	// The task is read once. The plan does not move while a lease is held, and
	// the appended half is carried forward from whatever the store hands back
	// on a lost race — so a retry costs one process, not two, and the window
	// between reading a value and swapping on it is as small as this boundary
	// allows. Re-reading instead measurably lost races it did not need to.
	task, err := c.Task(ctx, taskID)
	if err != nil {
		return nil, err
	}
	if task == nil {
		return nil, fmt.Errorf("store: no task %q; scope cannot be widened onto a task that does not exist", taskID)
	}
	// PlannedFiles is the union, so this is the plan and the existing widening
	// in one pass. Only the appended half can move under us.
	base := make(map[string]bool, len(task.PlannedFiles))
	for _, pf := range task.PlannedFiles {
		base[pf.Path] = true
	}
	appended := task.AgentAppended
	expected := rawArray(task.AgentAppendedRaw)

	stale := 0
	for attempt := 0; attempt < maxScopeAppendAttempts; attempt++ {
		have := make(map[string]bool, len(base)+len(appended))
		for path := range base {
			have[path] = true
		}
		for _, pf := range appended {
			have[pf.Path] = true
		}

		merged := append([]dc.PlannedFile(nil), appended...)
		var added []dc.PlannedFile
		for _, pf := range wanted {
			if have[pf.Path] {
				continue
			}
			have[pf.Path] = true
			merged = append(merged, pf)
			added = append(added, pf)
		}
		if len(added) == 0 {
			return nil, nil
		}
		if len(merged) > maxAppendedScopeEntries {
			return nil, fmt.Errorf("store: task %s would hold %d appended scope entries, past the ceiling of %d; "+
				"a plan that has grown this far past itself needs a human, not another append",
				taskID, len(merged), maxAppendedScopeEntries)
		}

		body, err := json.Marshal(merged)
		if err != nil {
			return nil, fmt.Errorf("store: encoding appended scope for %s: %w", taskID, err)
		}
		if len(body) > maxAppendedScopeBytes {
			return nil, fmt.Errorf("store: appended scope for %s is %d bytes, past the ceiling of %d",
				taskID, len(body), maxAppendedScopeBytes)
		}

		out, err := c.run(ctx, "scope-append",
			"--task", taskID, "--token", token,
			"--expected", expected, "--appended", string(body))
		if err != nil {
			return nil, err
		}
		if out.Written {
			return added, nil
		}
		if out.Code != scopeStaleCode {
			return nil, fmt.Errorf("store: task %s refused the scope widening: %s (code %q)",
				taskID, out.Error, out.Code)
		}

		// Lost the race. The store hands back what it holds now, so the merge
		// is recomputed against that rather than against a second read.
		stale++
		expected = rawArray(out.CurrentAppended)
		appended = nil
		if err := json.Unmarshal([]byte(expected), &appended); err != nil {
			return nil, fmt.Errorf("store: task %s reported an unreadable appended scope after a lost race: %w", taskID, err)
		}
	}
	return nil, fmt.Errorf("store: task %s had its scope changed under this one %d times running; "+
		"giving up rather than retrying forever", taskID, stale)
}

// jsonSpace is JSON's own whitespace and the only whitespace the store trims
// (json::trim_ws on the Rust side). bytes.TrimSpace strips more — a form feed,
// a non-breaking space — so a value one side called padded and the other called
// content could be stored, then never match its own compare-and-swap.
const jsonSpace = " \t\n\r"

func trimJSONSpace(b []byte) []byte { return bytes.Trim(b, jsonSpace) }

// rawArray is a stored JSON array as text, defaulting to the empty array.
//
// The store compares this text, so it is never re-encoded on the way through:
// a value that round-tripped through a decoder and back would differ from what
// is on disk and the swap would refuse a value it should have matched.
func rawArray(raw json.RawMessage) string {
	if trimmed := trimJSONSpace(raw); len(trimmed) > 0 {
		return string(trimmed)
	}
	return "[]"
}

// scopeStaleCode is the store's answer when the appended scope moved between
// the read and the swap.
const scopeStaleCode = "scope_stale"

// ReadyMode is the gate mode as `ready` orders work by it. Under Enforce a task
// with a blocking gap is not offered; under Advisory and Off it is listed after
// every unblocked one, flagged not ready to check out.
type ReadyMode string

const (
	ReadyEnforce  ReadyMode = "enforce"
	ReadyAdvisory ReadyMode = "advisory"
	ReadyOff      ReadyMode = "off"
)

// ReadyTask is one task `ready` offers, with what makes it more or less ready.
type ReadyTask struct {
	ID               string `json:"id"`
	ReadyToCheckout  bool   `json:"ready_to_checkout"`
	BlockingGapCount int    `json:"blocking_gap_count"`
}

// ReadyReport is ReadyTasks plus the leases the store could not judge, so an
// operator sees why a task is missing rather than the store failing as a whole.
func (c *Client) ReadyReport(ctx context.Context, mode ReadyMode) ([]ReadyTask, []UnreadableLease, error) {
	out, err := c.run(ctx, "ready", "--mode", string(mode))
	if err != nil {
		return nil, nil, err
	}
	ready, err := readyFrom(out)
	if err != nil {
		return nil, nil, err
	}
	return ready, out.UnreadableLeases, nil
}

// ReadyTasks lists the workbench items that are workable and unheld, in the
// order the mode dictates. The conditions are answered in one query on the store
// side, so a caller cannot be handed a task that was claimed between them. The
// mode is the caller's and is required: a caller that does not say which gate is
// in force does not get to guess the permissive one.
func (c *Client) ReadyTasks(ctx context.Context, mode ReadyMode) ([]ReadyTask, error) {
	ready, _, err := c.ReadyReport(ctx, mode)
	return ready, err
}

// readyFrom checks that the detail and the id list describe the same tasks
// before either is believed.
func readyFrom(out *response) ([]ReadyTask, error) {
	if len(out.Ready) != len(out.Tasks) {
		return nil, fmt.Errorf("store: ready listed %d ids but %d details", len(out.Tasks), len(out.Ready))
	}
	for i, t := range out.Ready {
		if t.ID != out.Tasks[i] {
			return nil, fmt.Errorf("store: ready detail %d is %q but the id list says %q", i, t.ID, out.Tasks[i])
		}
		if t.ReadyToCheckout != (t.BlockingGapCount == 0) {
			return nil, fmt.Errorf("store: ready task %s says ready_to_checkout=%v with %d blocking gaps", t.ID, t.ReadyToCheckout, t.BlockingGapCount)
		}
	}
	return out.Ready, nil
}

// LocalRepository makes sure the repository row every task names as its
// primary exists, and returns its id.
func (c *Client) LocalRepository(ctx context.Context) (string, error) {
	out, err := c.run(ctx, "local-repository")
	if err != nil {
		return "", err
	}
	if out.RepoID == "" {
		return "", errors.New("store: local-repository returned no repository id")
	}
	return out.RepoID, nil
}

// Available reports whether the store is reachable and usable.
//
// It requires an answer, not merely the absence of an error: a missing binary,
// an unreadable database, or a malformed reply all mean "unknown", and unknown
// must never be reported as an empty-but-healthy store. This is the same rule
// the Rust port learned the hard way — an unbuilt store that reported itself
// available returned a confident zero.
//
// A *missing* database is one of those unknowns, and it used not to be: the
// store opened the path with SQLite's default flags, so health created the file
// it was asked about and answered "healthy, zero leases" from a database nobody
// else was using. Two harnesses configured with two spellings of one path
// therefore shared no exclusion at all while both reported fine. health is now
// the one command that will not bring a store into existence, so this reports
// unavailable rather than manufacturing the store it was asked to check.
func (c *Client) Available(ctx context.Context) error {
	out, err := c.run(ctx, "health")
	if err != nil {
		return fmt.Errorf("store unavailable: %w", err)
	}
	// A positive identity, not the absence of an error. Any program that
	// prints `{"ok":true}` satisfies "no error"; only the real store answers
	// with its own name and a schema revision this build understands.
	if out.Store != storeIdentity {
		return fmt.Errorf("store unavailable: %s identified itself as %q, not %q",
			c.Binary, out.Store, storeIdentity)
	}
	if out.SchemaVersion != SchemaVersion {
		return fmt.Errorf("store unavailable: %s speaks schema %d, this harness speaks %d — "+
			"reading a lease through the wrong column layout would produce answers that look valid",
			c.Binary, out.SchemaVersion, SchemaVersion)
	}
	// The third assertion, and the one a schema version cannot make. A version
	// number says which columns the store expects; it says nothing about
	// whether the database it just opened carries the partial unique index that
	// makes two racing acquires resolve to one winner. That index is created
	// with IF NOT EXISTS, which matches on name alone, so a same-named index
	// with any other definition silently turns mutual exclusion back into the
	// check-then-insert it is supposed to backstop — and a 24-way race against
	// such a database elected two winners while health answered ok. The store
	// now reads its own schema back and says so here.
	if out.ExclusionIndex != exclusionIndexVerified {
		return fmt.Errorf("store unavailable: %s did not report its lease exclusion index as %q (got %q) — "+
			"without the partial unique index on task_id, two builders racing for one task both win",
			c.Binary, exclusionIndexVerified, out.ExclusionIndex)
	}
	return nil
}

// exclusionIndexVerified is what health answers once it has read the opened
// database's own schema and found the exclusion index intact. A store that
// cannot make the assertion — an older binary, or one that never looked —
// reports something else, and something else is not good enough for a check
// whose failure mode is two agents in one working tree.
const exclusionIndexVerified = "verified"

// SchemaVersion is the lease-schema revision this harness understands. It must
// match dc_store::SCHEMA_VERSION; Available refuses to proceed when it does not.
// 11 is the store that records and lists reviews (ReviewAppend, Reviews).
const SchemaVersion = 11

// storeIdentity is the name the real store answers to.
const storeIdentity = "dc-store"

// maxOutput bounds what a single store invocation may print. The replies are
// small and bounded by construction — one lease, one task, a list of ids — so
// anything approaching this is a store that has gone wrong, and reading it into
// memory unbounded would turn that into an out-of-memory kill of the harness.
const maxOutput = 8 << 20

// errOversize reports that the store produced more output than any legitimate
// reply.
var errOversize = errors.New("output exceeded the maximum a store reply may be")

// EvidenceRow is one persisted evidence record.
type EvidenceRow struct {
	ID                    int64           `json:"id"`
	Type                  string          `json:"type"`
	TaskID                string          `json:"task_id,omitempty"`
	RequirementID         string          `json:"requirement_id,omitempty"`
	AcceptanceCriterionID string          `json:"acceptance_criterion_id,omitempty"`
	DataJSON              json.RawMessage `json:"data_json"`
	// DataError is set instead of DataJSON when the stored text was not
	// provably one JSON value; the store never splices unproven text.
	DataError string `json:"data_error,omitempty"`
}

// GapRow is one persisted verification gap.
type GapRow struct {
	ID             string          `json:"id"`
	Severity       string          `json:"severity"`
	GapType        string          `json:"gap_type"`
	TaskID         string          `json:"task_id,omitempty"`
	Description    string          `json:"description"`
	RecommendedFix string          `json:"recommended_fix"`
	Blocking       bool            `json:"blocking"`
	EvidenceJSON   json.RawMessage `json:"evidence_json"`
	// EvidenceError is set when the stored evidence was not provably a JSON
	// array. The gap is still reported (a malformed row must not hide a
	// blocking gap) and EvidenceJSON then stands in as an empty array.
	EvidenceError string `json:"evidence_error,omitempty"`
	// The linkage that makes an acceptance-criterion gap actionable: which
	// requirement and criterion it is about, and how that criterion was meant
	// to be proven. Nil is "not about a criterion" and travels as SQL NULL and
	// JSON null; it is never written as an empty string.
	RequirementID              *string `json:"requirement_id"`
	AcceptanceCriterionID      *string `json:"acceptance_criterion_id"`
	ExpectedVerificationMethod *string `json:"expected_verification_method"`
	// Where the gap is and how to reproduce it. Nil is "not file-scoped" or
	// "no command", carried the same way as the links above.
	File             *string `json:"file"`
	Line             *int    `json:"line"`
	SuggestedCommand *string `json:"suggested_command"`
	// Where a failed verification command's captured output was written.
	StdoutPath *string `json:"stdout_path"`
	StderrPath *string `json:"stderr_path"`
}

// GapHistoryRow is what became of one gap across a task's verification runs.
//
// The `gaps` table is current-state: it is cleared and rewritten every run, so
// a gap that was raised, reported fixed, and raised again is indistinguishable
// there from one nobody has touched. This is the part that survives, and
// Resurfaces is the reason it exists.
type GapHistoryRow struct {
	TaskID       string `json:"task_id"`
	GapID        string `json:"gap_id"`
	GapType      string `json:"gap_type"`
	FirstSeenRun int    `json:"first_seen_run"`
	LastSeenRun  int    `json:"last_seen_run"`
	Occurrences  int    `json:"occurrences"`
	// Resurfaces counts how many times this gap was absent for at least one
	// run and came back. Nonzero means something reported it fixed and a later
	// run disagreed — a statement about the reports, not about the code, since
	// a run that could not measure the gap also makes it disappear.
	Resurfaces int `json:"resurfaces"`
}

// VerificationRun is one persisted verification run.
type VerificationRun struct {
	ID              string `json:"id"`
	TaskID          string `json:"task_id"`
	Sandbox         string `json:"sandbox"`
	EnvironmentJSON string `json:"environment_json"`
	CommandsJSON    string `json:"commands_json"`
	Status          string `json:"status"`
	StartedAt       string `json:"started_at"`
	FinishedAt      string `json:"finished_at,omitempty"`
}

// AgentHandoff is one persisted agent handoff.
type AgentHandoff struct {
	ID           string `json:"id"`
	TaskID       string `json:"task_id"`
	FromAgent    string `json:"from_agent"`
	ToAgent      string `json:"to_agent"`
	RunID        string `json:"run_id"`
	ManifestPath string `json:"manifest_path"`
	Status       string `json:"status"`
	CreatedAt    string `json:"created_at"`
}

// EvidenceAppend persists one evidence row via dcstore evidence-append.
func (c *Client) EvidenceAppend(ctx context.Context, kind string, taskID, requirementID, acceptanceCriterionID string, data json.RawMessage) (int64, error) {
	args := []string{"evidence-append", "--type", kind, "--data", string(data)}
	if taskID != "" {
		args = append(args, "--task", taskID)
	}
	if requirementID != "" {
		args = append(args, "--requirement-id", requirementID)
	}
	if acceptanceCriterionID != "" {
		args = append(args, "--acceptance-criterion-id", acceptanceCriterionID)
	}
	out, err := c.run(ctx, args...)
	if err != nil {
		return 0, err
	}
	if !out.OK {
		return 0, fmt.Errorf("evidence-append refused: %s", out.Error)
	}
	return out.EvidenceID, nil
}

// requirementRow is one `requirements` row as the store sends it. The criteria
// arrive as a JSON string the store does not validate; Requirements decodes
// them through dc.Requirement, which refuses an unknown verification method.
type requirementRow struct {
	ID                     string `json:"id"`
	Title                  string `json:"title"`
	Description            string `json:"description"`
	Priority               string `json:"priority"`
	Source                 string `json:"source"`
	AcceptanceCriteriaJSON string `json:"acceptance_criteria_json"`
}

// LinkedRequirements is what a task's requirement links resolve to.
type LinkedRequirements struct {
	Requirements []dc.Requirement
	// Missing is every linked id no requirement row defines.
	Missing []string
}

// Requirements reads the requirements a task links to, decoded and validated.
//
// Every failure is an error rather than a shorter list: a requirement that
// could not be read, a criterion whose method no gate discharges, a truncated
// reply and an unknown task all leave the caller unable to say what the task
// has to prove, and a verifier must not read that as "nothing to prove".
func (c *Client) Requirements(ctx context.Context, taskID string) (LinkedRequirements, error) {
	out, err := c.run(ctx, "requirements", "--task", taskID)
	if err != nil {
		return LinkedRequirements{}, err
	}
	if out.TaskFound == nil {
		return LinkedRequirements{}, fmt.Errorf("store: requirements reply for %s carries no task_found", taskID)
	}
	if !*out.TaskFound {
		return LinkedRequirements{}, fmt.Errorf("store: task %s not found", taskID)
	}
	if out.Truncated {
		return LinkedRequirements{}, fmt.Errorf("store: task %s links more requirements than one reply carries", taskID)
	}
	linked := LinkedRequirements{
		Requirements: make([]dc.Requirement, 0, len(out.Requirements)),
		Missing:      out.Missing,
	}
	for _, row := range out.Requirements {
		criteria := json.RawMessage(row.AcceptanceCriteriaJSON)
		if !json.Valid(criteria) {
			return LinkedRequirements{}, fmt.Errorf("store: requirement %s has unreadable acceptance criteria", row.ID)
		}
		doc, err := json.Marshal(map[string]any{
			"id": row.ID, "title": row.Title, "description": row.Description,
			"priority": row.Priority, "source": row.Source, "acceptance_criteria": criteria,
		})
		if err != nil {
			return LinkedRequirements{}, fmt.Errorf("store: requirement %s: %w", row.ID, err)
		}
		var req dc.Requirement
		if err := json.Unmarshal(doc, &req); err != nil {
			return LinkedRequirements{}, fmt.Errorf("store: requirement %s: %w", row.ID, err)
		}
		linked.Requirements = append(linked.Requirements, req)
	}
	return linked, nil
}

// reviewRow is one `reviews` row as the store sends it. The record arrives
// embedded as the JSON object it was stored as, or as null beside a
// record_error when the stored bytes no longer parse.
type reviewRow struct {
	ID                    int64           `json:"id"`
	TaskID                string          `json:"task_id"`
	AcceptanceCriterionID string          `json:"acceptance_criterion_id"`
	SubjectHash           string          `json:"subject_hash"`
	Record                json.RawMessage `json:"record"`
	RecordError           string          `json:"record_error"`
	RecordedAt            string          `json:"recorded_at"`
}

// StoredReview is one recorded review as read back, with the record decoded
// and validated. A row whose record could not be decoded carries the reason in
// Err and no Record: it is reported, never skipped, so the newest review that
// is unreadable is not silently replaced by an older readable one.
type StoredReview struct {
	ID          int64
	SubjectHash string
	RecordedAt  string
	Record      dc.ReviewRecord
	Err         error
}

// ReviewAppend records one review. The record is validated here before it is
// sent (dc.ReviewRecord.Validate), so a malformed record is refused by the
// writer rather than stored for every reader to refuse.
//
// This is the harness's write. Nothing that serves the agent being verified —
// its shell (the gate refuses the command, policy.RuleCommandHarnessOnly) or
// the host's MCP tools — reaches it.
func (c *Client) ReviewAppend(ctx context.Context, record dc.ReviewRecord) (int64, error) {
	if err := record.Validate(); err != nil {
		return 0, err
	}
	data, err := json.Marshal(record)
	if err != nil {
		return 0, fmt.Errorf("review record: %w", err)
	}
	out, err := c.run(ctx, "review-append",
		"--task", record.TaskID,
		"--acceptance-criterion-id", record.AcceptanceCriterionID,
		"--subject-hash", record.SubjectHash,
		"--record", string(data))
	if err != nil {
		return 0, err
	}
	if !out.OK {
		return 0, fmt.Errorf("review-append refused: %s", out.Error)
	}
	if out.ReviewID <= 0 {
		return 0, fmt.Errorf("store: review-append reply carries no review_id")
	}
	return out.ReviewID, nil
}

// Reviews reads a task's reviews, newest first, for one criterion or (with
// criterionID empty) all of them.
//
// A truncated reply is an error rather than a shorter list: the newest rows
// come first, so a cut list would still hold the one verify reads, but a
// caller listing reviews for a person must not show part of them as all.
func (c *Client) Reviews(ctx context.Context, taskID, criterionID string) ([]StoredReview, error) {
	args := []string{"reviews", "--task", taskID}
	if criterionID != "" {
		args = append(args, "--acceptance-criterion-id", criterionID)
	}
	out, err := c.run(ctx, args...)
	if err != nil {
		return nil, err
	}
	if !out.OK {
		return nil, fmt.Errorf("reviews refused: %s", out.Error)
	}
	if out.Reviews == nil {
		return nil, fmt.Errorf("store: reviews reply for %s carries no reviews", taskID)
	}
	if out.Truncated {
		return nil, fmt.Errorf("store: task %s has more reviews than one reply carries", taskID)
	}
	reviews := make([]StoredReview, 0, len(out.Reviews))
	for _, row := range out.Reviews {
		stored := StoredReview{ID: row.ID, SubjectHash: row.SubjectHash, RecordedAt: row.RecordedAt}
		switch {
		case row.RecordError != "" || len(row.Record) == 0 || string(row.Record) == "null":
			stored.Err = fmt.Errorf("review %d: the stored record does not parse: %s", row.ID, row.RecordError)
		default:
			record, err := dc.DecodeReviewRecord(row.Record)
			switch {
			case err != nil:
				stored.Err = fmt.Errorf("review %d: %w", row.ID, err)
			case record.TaskID != row.TaskID || record.AcceptanceCriterionID != row.AcceptanceCriterionID ||
				record.SubjectHash != row.SubjectHash:
				// The row's keys are what the store indexes and the record's
				// are what the harness wrote; a disagreement is a record filed
				// under someone else's name.
				stored.Err = fmt.Errorf("review %d: the record names %s/%s/%s but is stored under %s/%s/%s",
					row.ID, record.TaskID, record.AcceptanceCriterionID, record.SubjectHash,
					row.TaskID, row.AcceptanceCriterionID, row.SubjectHash)
			default:
				stored.Record = record
			}
		}
		reviews = append(reviews, stored)
	}
	return reviews, nil
}

// EvidenceList returns evidence rows (newest first), optionally for one task.
func (c *Client) EvidenceList(ctx context.Context, taskID string) ([]EvidenceRow, bool, error) {
	args := []string{"evidence-list"}
	if taskID != "" {
		args = append(args, "--task", taskID)
	}
	out, err := c.run(ctx, args...)
	if err != nil {
		return nil, false, err
	}
	if !out.OK {
		return nil, false, fmt.Errorf("evidence-list refused: %s", out.Error)
	}
	return out.Evidence, out.Truncated, nil
}

// Gaps returns gap rows, optionally for one task.
func (c *Client) Gaps(ctx context.Context, taskID string) ([]GapRow, bool, error) {
	args := []string{"gaps"}
	if taskID != "" {
		args = append(args, "--task", taskID)
	}
	out, err := c.run(ctx, args...)
	if err != nil {
		return nil, false, err
	}
	if !out.OK {
		return nil, false, fmt.Errorf("gaps refused: %s", out.Error)
	}
	return out.Gaps, out.Truncated, nil
}

// GapHistory returns the recurrence record for a task's gaps, or for every
// task when taskID is empty.
func (c *Client) GapHistory(ctx context.Context, taskID string) ([]GapHistoryRow, bool, error) {
	args := []string{"gap-history"}
	if taskID != "" {
		args = append(args, "--task", taskID)
	}
	out, err := c.run(ctx, args...)
	if err != nil {
		return nil, false, err
	}
	if !out.OK {
		return nil, false, fmt.Errorf("gap-history refused: %s", out.Error)
	}
	return out.History, out.Truncated, nil
}

// MaxGapsPerReplace is the most gaps one replacement may carry. It mirrors
// dc_store::records::MAX_GAPS_PER_REPLACE, which enforces it; the client checks
// first so an over-limit set fails before it is encoded and sent.
const MaxGapsPerReplace = 500

// GapsReplace replaces a task's gaps in one store transaction: the whole new
// set is stored or the previous set is left exactly as it was.
//
// It used to clear the task and then upsert each gap in turn, so a failure or
// a kill part-way left the task with no gaps or a partial set, which
// devcouncil_get_gaps then served as authoritative.
func (c *Client) GapsReplace(ctx context.Context, taskID string, gaps []GapRow) error {
	return c.RecordVerification(ctx, nil, taskID, gaps)
}

// RecordVerification commits a verification run (when run is non-nil) and the
// task's replacement gap set as one transaction: a run is never stored with a
// status and no gaps, and a failed commit leaves the previous gaps in place.
// A set over MaxGapsPerReplace is refused.
//
// This needs a dcstore that knows `gaps-replace`; an older one answers
// "unknown command", which is returned as an error rather than answered by
// falling back to the non-transactional path this replaced.
func (c *Client) RecordVerification(ctx context.Context, run *VerificationRun, taskID string, gaps []GapRow) error {
	if taskID == "" {
		return fmt.Errorf("gaps-replace requires task_id")
	}
	if len(gaps) > MaxGapsPerReplace {
		return fmt.Errorf("gaps-replace refused: %d gaps exceeds the limit of %d", len(gaps), MaxGapsPerReplace)
	}
	if run != nil && run.TaskID != taskID {
		return fmt.Errorf("gaps-replace refused: run %s belongs to task %q, not %q", run.ID, run.TaskID, taskID)
	}
	out, err := c.run(ctx, "gaps-replace", "--task", taskID, "--wire", encodeVerificationWire(run, gaps))
	if err != nil {
		return err
	}
	if !out.OK {
		return fmt.Errorf("gaps-replace refused: %s", out.Error)
	}
	return nil
}

// encodeVerificationWire writes the framing dc_store::commit_wire documents:
// every field is "<byte length>:<bytes>" and an absent optional is "-".
func encodeVerificationWire(run *VerificationRun, gaps []GapRow) string {
	var w strings.Builder
	field := func(s string) {
		// argv cannot carry NUL and the store reads UTF-8: both are replaced
		// rather than refused, because a command's output tail is arbitrary.
		s = strings.ToValidUTF8(strings.ReplaceAll(s, "\x00", "\uFFFD"), "\uFFFD")
		w.WriteString(strconv.Itoa(len(s)))
		w.WriteByte(':')
		w.WriteString(s)
	}
	optional := func(s *string) {
		if s == nil || *s == "" {
			w.WriteByte('-')
			return
		}
		field(*s)
	}
	field("DCVC1")
	if run == nil {
		field("N")
	} else {
		field("R")
		for _, v := range []string{run.ID, run.TaskID, run.Sandbox, orDefault(run.EnvironmentJSON, "{}"), orDefault(run.CommandsJSON, "[]"), run.Status, run.StartedAt} {
			field(v)
		}
		finished := run.FinishedAt
		optional(&finished)
	}
	field(strconv.Itoa(len(gaps)))
	for _, g := range gaps {
		field(g.ID)
		field(g.Severity)
		field(g.GapType)
		field(g.Description)
		field(orDefault(strings.TrimSpace(string(g.EvidenceJSON)), "[]"))
		field(g.RecommendedFix)
		if g.Blocking {
			field("1")
		} else {
			field("0")
		}
		// Absent fields are omitted rather than sent empty: an empty string is
		// stored as such, not NULL, and an empty id would read back as a gap
		// about a criterion that has no name.
		optional(g.RequirementID)
		optional(g.File)
		if g.Line != nil {
			field(strconv.Itoa(*g.Line))
		} else {
			w.WriteByte('-')
		}
		optional(g.SuggestedCommand)
		optional(g.AcceptanceCriterionID)
		optional(g.ExpectedVerificationMethod)
		optional(g.StdoutPath)
		optional(g.StderrPath)
	}
	return w.String()
}

func orDefault(s, fallback string) string {
	if s == "" {
		return fallback
	}
	return s
}

// RunRecord upserts a verification run.
func (c *Client) RunRecord(ctx context.Context, run VerificationRun) error {
	args := []string{
		"run-record",
		"--id", run.ID,
		"--task", run.TaskID,
		"--sandbox", run.Sandbox,
		"--status", run.Status,
		"--started-at", run.StartedAt,
	}
	if run.EnvironmentJSON != "" {
		args = append(args, "--environment", run.EnvironmentJSON)
	}
	if run.CommandsJSON != "" {
		args = append(args, "--commands", run.CommandsJSON)
	}
	if run.FinishedAt != "" {
		args = append(args, "--finished-at", run.FinishedAt)
	}
	out, err := c.run(ctx, args...)
	if err != nil {
		return err
	}
	if !out.OK {
		return fmt.Errorf("run-record refused: %s", out.Error)
	}
	return nil
}

// Handoff upserts an agent handoff.
func (c *Client) Handoff(ctx context.Context, h AgentHandoff) error {
	args := []string{
		"handoff",
		"--id", h.ID,
		"--task", h.TaskID,
		"--from-agent", h.FromAgent,
		"--to-agent", h.ToAgent,
		"--run-id", h.RunID,
		"--manifest-path", h.ManifestPath,
		"--status", h.Status,
		"--created-at", h.CreatedAt,
	}
	out, err := c.run(ctx, args...)
	if err != nil {
		return err
	}
	if !out.OK {
		return fmt.Errorf("handoff refused: %s", out.Error)
	}
	return nil
}

// timeout is the bound one call runs under.
func (c *Client) timeout() time.Duration {
	if c.Timeout <= 0 {
		return 10 * time.Second
	}
	return c.Timeout
}

// run performs one store command.
//
// It prefers a pooled persistent process (serve.go) and falls back to one
// process per call. The fallback is not a silent one: it is entered only when
// the binary refuses `serve` from argv, which it does before reading any
// request, so the command provably did not run and making it again is safe.
// Every other served failure is reported rather than retried, because a call
// that may have been applied must not be applied twice — a second `acquire`
// would look like contention with itself.
func (c *Client) run(ctx context.Context, args ...string) (*response, error) {
	if c.Binary == "" {
		return nil, errors.New("store: no dcstore binary configured")
	}
	if c.DB == "" {
		return nil, errors.New("store: no database path configured")
	}
	// Refused rather than indexed. Every caller in this package passes a
	// command, so an empty argument vector is a caller bug — and reaching the
	// store with one would either panic on args[0] or, worse, run whatever the
	// store makes of no command at all. Naming the command once here is also
	// what keeps the two transports from each re-deriving it.
	if len(args) == 0 {
		return nil, errors.New("store: a command is required")
	}
	command := args[0]

	ctx, cancel := context.WithTimeout(ctx, c.timeout())
	defer cancel()

	if !c.oneShot.Load() {
		result, err := c.runServed(ctx, command, args)
		switch {
		case errors.Is(err, errServeUnsupported):
			c.oneShot.Store(true)
		case err != nil:
			return nil, err
		default:
			// A served failure carries errStoreFatal in the place the one-shot
			// path carries a non-zero exit, so decodeReply reaches the same
			// verdict about the same bytes either way.
			var fatal error
			if result.fatal {
				fatal = errStoreFatal
			}
			return decodeReply(command, result.body, result.overflow, result.stderr, fatal)
		}
	}
	return c.runOneShot(ctx, command, args)
}

// runOneShot performs one store command as its own process.
func (c *Client) runOneShot(ctx context.Context, command string, args []string) (*response, error) {
	full := append([]string{"--db", c.DB}, args...)
	cmd := exec.CommandContext(ctx, c.Binary, full...)
	stdout := &proc.CappedBuffer{Limit: maxOutput}
	stderr := &proc.CappedBuffer{Limit: 64 << 10}
	cmd.Stdout = stdout
	cmd.Stderr = stderr

	// Killing the process is not enough to unblock Wait. Go copies a
	// non-*os.File stdout through a pipe, and Wait waits for that copy to
	// finish — so a store that spawns a child holding the write end leaves the
	// harness blocked for as long as the *grandchild* lives, regardless of this
	// timeout. WaitDelay bounds that second wait: after the context fires and
	// the process is killed, the pipes are closed and Wait returns.
	cmd.WaitDelay = 2 * time.Second
	proc.ConfigureGroup(cmd)

	// Run on its own goroutine so the bound covers the fork, not just the
	// process. Everything above this line — the context, WaitDelay, the process
	// group — starts counting after Start has returned, so a Start that does
	// not return is outside all of them, and this client's advertised 10-second
	// bound would not hold. This is the lease path: every Diagnose on the write
	// gate and every Acquire comes through here, and a wedged fork would stall
	// the turn indefinitely rather than fail it.
	//
	// On the deadline path the goroutine is abandoned — it is blocked in the
	// kernel and there is nothing to cancel — so nothing below reads stdout or
	// stderr on that path. A buffer still being written by an abandoned writer
	// is a data race, and a partial reply decoded out of one would be worse
	// than the timeout it replaced.
	runErr, timedOut := proc.RunBounded(ctx, cmd.Run)
	if timedOut {
		return nil, fmt.Errorf("store: %s did not return within %s (the process could not be started or reaped): %w",
			command, c.timeout(), ctx.Err())
	}
	return decodeReply(command, stdout.Bytes(), stdout.Overflowed(), stderr.Bytes(), runErr)
}

// decodeReply turns one invocation's raw streams into either a reply or a
// reason there is none. It is the whole of what this package decides about a
// child's answer, and it is a function rather than a tail of run() because that
// decision is the boundary's actual contract: everything above it is process
// mechanics, and everything a caller acts on comes out of here.
//
// Separating it also makes the contract testable at the rate it needs to be
// tested at. The producer is a binary built from another language's toolchain,
// so the bytes below are the least trustworthy input this package has, and
// reaching them through a fork costs a process per case — which is the
// difference between a fuzz target that explores this decision and one that
// samples it. See FuzzStoreReplyIsNeverAZeroValueReadAsSuccess.
//
// The contract, which that target asserts rather than restates:
//
//   - Exactly one of the two returns is ever set. A nil reply with a nil error
//     would be read by every caller here as an empty-but-healthy store.
//   - A child that failed, or one whose output was cut off at the bound, never
//     produces a reply, whatever its bytes happened to say.
//   - A reply only ever comes back for a command whose ok:false is a real
//     answer; for the rest, ok:false is a failure the store did not classify.
func decodeReply(command string, stdout []byte, overflowed bool, stderr []byte, runErr error) (*response, error) {
	if overflowed {
		return nil, fmt.Errorf("store: %s %w (%d bytes)", command, errOversize, maxOutput)
	}

	// The contract is that every outcome — including a refusal — arrives as
	// JSON on stdout, so parse before judging the exit code. An exit code with
	// no parseable payload is the genuinely broken case.
	var out response
	if decodeErr := json.Unmarshal(bytes.TrimSpace(stdout), &out); decodeErr != nil {
		if runErr != nil {
			return nil, fmt.Errorf("store: %s failed: %w (stderr: %s)",
				command, runErr, bytes.TrimSpace(stderr))
		}
		return nil, fmt.Errorf("store: %s returned unparseable output: %w (output: %q)",
			command, decodeErr, stdout)
	}
	// `null` is valid JSON and unmarshals into a struct without error, leaving
	// every field at its zero value — which is the one document that would
	// otherwise arrive here indistinguishable from a real reply that said
	// nothing. It is refused by name rather than left to the ok:false rule
	// below, because that rule lets four commands through on ok:false and each
	// of them would then be acting on a decode that produced no data at all.
	if bytes.Equal(bytes.TrimSpace(stdout), []byte("null")) {
		return nil, fmt.Errorf("store: %s returned a JSON null, which decodes to an empty reply "+
			"that is indistinguishable from a store with nothing to say", command)
	}
	if runErr != nil && out.Error != "" {
		return nil, fmt.Errorf("store: %s: %s", command, out.Error)
	}
	if runErr != nil {
		return nil, fmt.Errorf("store: %s failed: %w", command, runErr)
	}
	if command == "work" {
		validated, err := decodeWorkbenchReply(stdout)
		if err != nil {
			return nil, err
		}
		out.Workbench = validated
		return &out, nil
	}

	// A reply that says ok:false without naming a code is a failure the store
	// did not classify. Only the commands below use ok:false as a meaningful
	// answer a caller branches on; for the rest, letting it through would hand
	// back a zero value — no task, no leases — that reads exactly like a
	// healthy empty store.
	if !out.OK && !signalsOutcomeWithOK[command] {
		reason := out.Error
		if reason == "" {
			reason = "no reason given"
		}
		return nil, fmt.Errorf("store: %s reported failure: %s", command, reason)
	}
	return &out, nil
}

// signalsOutcomeWithOK lists the commands for which ok:false is a real answer
// rather than a fault: a task held by someone else, a token that is not valid,
// a lease that has already expired.
var signalsOutcomeWithOK = map[string]bool{
	"acquire":  true,
	"diagnose": true,
	"renew":    true,
	// A widening loses a race with another widening on the same task, or is
	// asked for by something that no longer holds the lease. Both are answers
	// the caller acts on — re-read and merge, or stop — and turning them into
	// an error here would have made the retry loop in AppendPlannedFiles
	// unreachable: contention would have surfaced as a store fault, and the
	// merge that would have resolved it would never have been attempted.
	"scope-append": true,
}
