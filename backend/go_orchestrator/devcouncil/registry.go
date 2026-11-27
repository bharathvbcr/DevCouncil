package devcouncil

import (
	"context"
	"encoding/json"
	"fmt"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
	"sort"
	"strings"
	"sync"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/gatescfg"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/stopgate"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/gate"
)

// ToolBehaviour is (readOnly, destructive, idempotent, openWorld).
type ToolBehaviour = dc.SharedToolBehaviour

// ToolSpec is one advertised MCP tool.
type ToolSpec = dc.SharedToolSpec

// Registry is the Phase-4 MCP tool surface. It does not import Manvi packages.
type Registry struct {
	Root  string
	Store *store.Client
	Gate  *gate.Gate
	Lease *LeaseService

	// OpenGate, when set, rebuilds the write gate whenever the repository's
	// config.yaml changes (gatescfg.ConfigKey, the key gates.mode is already
	// re-read on), so a corrected config reaches a running host without a
	// restart. Gate then holds the last build and is replaced on each one.
	// A rebuild that fails leaves no gate: policy_check_write answers
	// not_initialized with the reason until the file is fixed, never the
	// previous gate (which may be looser than what the file now says) and never
	// "off".
	OpenGate func() (*gate.Gate, error)

	// OpenStore, when set, lazily opens the repository's task store when Store
	// is still nil (for example when .devcouncil/state.sqlite is initialized
	// after a global MCP server started).
	OpenStore func() *store.Client

	// ResolveRepo, when set, resolves a validated absolute repo_path/root that
	// differs from r.Root to a *Registry configured for that repository. When
	// nil, calls naming a different repository fail closed rather than silently
	// answering against r.Root.
	ResolveRepo func(root string) (*Registry, error)

	storeMu sync.Mutex

	gateMu    sync.Mutex
	gateBuilt bool
	gateKey   gatescfg.FileKey
	gateErr   error
}

// NewRegistry builds the MCP registry. Store/Gate may be nil when the project
// is not initialized; tools then fail closed with not_initialized.
func NewRegistry(root string, storeClient *store.Client, g *gate.Gate) *Registry {
	lease := &LeaseService{Store: storeClient, Root: root}
	return &Registry{
		Root:  root,
		Store: storeClient,
		Gate:  g,
		Lease: lease,
	}
}

// Specs returns every advertised tool. tools/list is generated from this so
// schema drift between list and call is impossible.
func (r *Registry) Specs() []ToolSpec { return toolSpecs() }

// ServedToolNames names every tool this host advertises.
//
// Anything inside this package that needs to name the served surface reads it
// from here instead of spelling a second list. A hand-written copy is precisely
// how the checkout path came to hand every agent six tool names this host does
// not have (GAP-P7-NEXT-TOOLS-DRIFT). verify.AllowedNextToolsForVerify still
// keeps a copy because package devcouncil imports verify and reading back would
// cycle — that copy is safe only because allowed_next_tools_test.go holds it to
// this list in both directions. Callers that can reach this function have no
// such excuse.
func ServedToolNames() []string {
	return dc.SharedToolNames()
}

// toolSpecs returns the canonical advertised shared devcouncil_* tools from dc.
func toolSpecs() []ToolSpec {
	return dc.SharedToolSpecs()
}

// Call dispatches one tool by name.
//
// Arguments the tool's own input schema does not declare are refused first,
// with invalid_argument, before any lease, store or git is consulted. Nothing
// else enforces a schema at runtime, and an argument dropped without a word is
// a configuration the caller believes it made: `sandbox: "docker"` on
// verify_task came back as a report the caller would read as the isolated run
// it asked for, while every command ran on the host (TASK-P7-2).
func (r *Registry) Call(ctx context.Context, name string, args map[string]any) (any, error) {
	return r.CallWithResolver(ctx, name, args, nil)
}

// CallWithResolver dispatches one tool by name, using fallbackResolver when
// args names a different repository via repo_path/root and r.ResolveRepo is nil.
func (r *Registry) CallWithResolver(
	ctx context.Context,
	name string,
	args map[string]any,
	fallbackResolver func(root string) (*Registry, error),
) (any, error) {
	if refusal, ok := undeclaredArgument(name, args); ok {
		return refusal, nil
	}
	target, refusal := r.resolveTarget(name, args, fallbackResolver)
	if refusal != nil {
		return *refusal, nil
	}
	target.ensureStore()
	switch name {
	case "devcouncil_get_diff":
		return target.callGetDiff(ctx, args)
	case "devcouncil_checkout_task":
		return target.callCheckout(ctx, args), nil
	case "devcouncil_renew_lease":
		return target.callRenew(ctx, args), nil
	case "devcouncil_release_task":
		return target.callRelease(ctx, args), nil
	case "devcouncil_next_task":
		return target.callNextTask(ctx), nil
	case "devcouncil_verify_task":
		return target.callVerify(ctx, args), nil
	case "devcouncil_get_gaps":
		return target.callGetGaps(ctx, args), nil
	case "devcouncil_policy_check_write":
		return target.callPolicyCheck(ctx, args), nil
	default:
		return nil, fmt.Errorf("unknown tool: %s", name)
	}
}

func (r *Registry) ensureStore() {
	if r.OpenStore == nil {
		return
	}
	r.storeMu.Lock()
	defer r.storeMu.Unlock()
	if r.Store != nil {
		return
	}
	if st := r.OpenStore(); st != nil {
		r.Store = st
		if r.Lease == nil {
			r.Lease = &LeaseService{Store: st, Root: r.Root}
		} else {
			r.Lease.Store = st
		}
	}
}

func (r *Registry) currentStore() *store.Client {
	r.storeMu.Lock()
	defer r.storeMu.Unlock()
	return r.Store
}

func (r *Registry) resolveTarget(
	tool string,
	args map[string]any,
	fallbackResolver func(root string) (*Registry, error),
) (*Registry, *ErrorPayload) {
	rawRepoPath, hasRepoPath := args["repo_path"].(string)
	rawRoot, hasRoot := args["root"].(string)
	if !hasRepoPath && !hasRoot {
		return r, nil
	}
	var cleanRepoPath, cleanRoot string
	if hasRepoPath {
		var err error
		cleanRepoPath, err = validateRepoRootArg(tool, "repo_path", rawRepoPath)
		if err != nil {
			return nil, &ErrorPayload{OK: false, Code: "invalid_argument", Error: err.Error()}
		}
	}
	if hasRoot {
		var err error
		cleanRoot, err = validateRepoRootArg(tool, "root", rawRoot)
		if err != nil {
			return nil, &ErrorPayload{OK: false, Code: "invalid_argument", Error: err.Error()}
		}
	}
	if hasRepoPath && hasRoot && !sameRepoDir(cleanRepoPath, cleanRoot) {
		return nil, &ErrorPayload{
			OK:    false,
			Code:  "invalid_argument",
			Error: fmt.Sprintf("%s: repo_path (%q) and root (%q) disagree; pass one repository root", tool, cleanRepoPath, cleanRoot),
		}
	}
	requested := cleanRepoPath
	if requested == "" {
		requested = cleanRoot
	}
	if r.Root != "" && sameRepoDir(requested, r.Root) {
		return r, nil
	}
	resolver := r.ResolveRepo
	if resolver == nil {
		resolver = fallbackResolver
	}
	if resolver == nil {
		return nil, &ErrorPayload{
			OK:    false,
			Code:  "not_initialized",
			Error: fmt.Sprintf("%s: this server is scoped to %q and cannot switch to %q", tool, r.Root, requested),
		}
	}
	other, err := resolver(requested)
	if err != nil {
		return nil, &ErrorPayload{
			OK:    false,
			Code:  "invalid_argument",
			Error: fmt.Sprintf("%s: cannot resolve repository %q: %v", tool, requested, err),
		}
	}
	if other == nil {
		return nil, &ErrorPayload{
			OK:    false,
			Code:  "not_initialized",
			Error: fmt.Sprintf("%s: repository %q could not be initialized", tool, requested),
		}
	}
	return other, nil
}

func (r *Registry) callGetDiff(ctx context.Context, args map[string]any) (any, error) {
	taskID, _ := args["task_id"].(string)
	staged, _ := args["staged"].(bool)
	var paths []string
	if raw, ok := args["paths"].([]any); ok {
		for _, v := range raw {
			if s, ok := v.(string); ok {
				paths = append(paths, s)
			}
		}
	}
	diffArgs := GetDiffArgs{TaskID: taskID, Paths: paths, Staged: staged, DBInitialized: r.Store != nil}
	if taskID != "" {
		if r.Store == nil {
			diffArgs.DBInitialized = false
		} else {
			entries, found, err := r.Lease.PlannedEntries(ctx, taskID)
			if err != nil {
				return ErrorPayload{OK: false, Code: "store_error", Error: err.Error()}, nil
			}
			diffArgs.TaskFound = found
			diffArgs.PlannedEntries = entries
			diffArgs.DBInitialized = true
		}
	}
	return GetDiff(ctx, r.Root, diffArgs)
}

func (r *Registry) callCheckout(ctx context.Context, args map[string]any) any {
	taskID, _ := args["task_id"].(string)
	clientID, _ := args["client_id"].(string)
	agent, _ := args["agent"].(string)
	force, _ := args["force"].(bool)
	if taskID == "" || clientID == "" {
		return ErrorPayload{OK: false, Code: "missing_argument", Error: "task_id and client_id are required"}
	}
	return r.Lease.Checkout(ctx, taskID, clientID, agent, force)
}

func (r *Registry) callRenew(ctx context.Context, args map[string]any) any {
	taskID, _ := args["task_id"].(string)
	token, _ := args["lease_token"].(string)
	if taskID == "" || token == "" {
		return ErrorPayload{OK: false, Code: "missing_argument", Error: "task_id and lease_token are required"}
	}
	return r.Lease.Renew(ctx, taskID, token)
}

func (r *Registry) callRelease(ctx context.Context, args map[string]any) any {
	taskID, _ := args["task_id"].(string)
	token, _ := args["lease_token"].(string)
	if taskID == "" || token == "" {
		return ErrorPayload{OK: false, Code: "missing_argument", Error: "task_id and lease_token are required"}
	}
	return r.Lease.Release(ctx, taskID, token)
}

// GateState is the gate mode as of this call. Every caller that needs the mode
// asks here, so a `gate set` on disk reaches a host that is already running.
func (r *Registry) GateState() verify.GateState {
	if r.Lease != nil {
		return r.Lease.GateState()
	}
	return (&LeaseService{Root: r.Root}).GateState()
}

func (r *Registry) callNextTask(ctx context.Context) any {
	if r.Store == nil {
		return map[string]any{"ok": false, "error": "DevCouncil not initialized in this directory.", "code": "not_initialized"}
	}
	gate := r.GateState()
	// The store orders the work for the gate mode in force: under enforce a task
	// with a blocking gap is not offered at all; under advisory and off it comes
	// after every unblocked task, flagged. Taking the first id is only right
	// because of that ordering, and it is the store's, not this function's.
	ready, err := r.Store.ReadyTasks(ctx, store.ReadyMode(gate.Mode))
	if err != nil {
		return map[string]any{"ok": false, "error": err.Error(), "code": "store_error"}
	}
	if len(ready) == 0 {
		return withGateConfigError(map[string]any{"ok": true, "task": nil}, gate)
	}
	next := ready[0]
	task, err := r.Store.Task(ctx, next.ID)
	if err != nil {
		return map[string]any{"ok": false, "error": err.Error(), "code": "store_error"}
	}
	return withGateConfigError(map[string]any{
		"ok": true, "task_id": next.ID, "task": task,
		"ready_to_checkout":  next.ReadyToCheckout,
		"blocking_gap_count": next.BlockingGapCount,
	}, gate)
}

func (r *Registry) callVerify(ctx context.Context, args map[string]any) any {
	taskID, _ := args["task_id"].(string)
	token, _ := args["lease_token"].(string)
	if taskID == "" || token == "" {
		return ErrorPayload{OK: false, Code: "missing_argument", Error: "task_id and lease_token are required"}
	}
	if refusal := r.Lease.RequireLease(ctx, taskID, token); refusal != nil {
		return refusal
	}
	if r.Store == nil {
		return ErrorPayload{OK: false, Code: "not_initialized", Error: "DevCouncil state is unavailable in this directory."}
	}
	gate := r.GateState()
	out := stopgate.Run(ctx, stopgate.Input{
		Root: r.Root, Store: r.Store, TaskID: taskID, GateMode: gate.Mode, Sandbox: verify.SandboxLocal,
		GateOrigin: gate.Origin, GateConfigPath: gate.ConfigPath, GateConfigError: gate.Error,
	})
	if out.Decision.Skipped {
		return ErrorPayload{OK: false, Code: "verify_error", Error: out.Decision.Reason}
	}
	return out.MCP
}

func (r *Registry) callGetGaps(ctx context.Context, args map[string]any) any {
	taskID, _ := args["task_id"].(string)
	if taskID == "" {
		return ErrorPayload{OK: false, Code: "missing_argument", Error: "task_id is required"}
	}
	if r.Store == nil {
		return ErrorPayload{OK: false, Code: "not_initialized", Error: "DevCouncil state is unavailable in this directory."}
	}
	rows, truncated, err := r.Store.Gaps(ctx, taskID)
	if err != nil {
		return ErrorPayload{OK: false, Code: "store_error", Error: err.Error()}
	}
	// An id nothing was planned under, and that holds no gaps, is not "no
	// gaps": the other task tools say not_found, and an empty list read as a
	// clean bill of health for a task that does not exist.
	if len(rows) == 0 {
		task, err := r.Store.Task(ctx, taskID)
		if err != nil {
			return ErrorPayload{OK: false, Code: "store_error", Error: err.Error()}
		}
		if task == nil {
			return ErrorPayload{OK: false, Code: "not_found", Error: fmt.Sprintf("Task %s not found.", taskID)}
		}
	}
	// Recurrence rides along with the gap rather than sitting behind a tool of
	// its own. "You have reported this fixed before" is only useful at the
	// moment something is deciding what to do about the gap, and a separate
	// tool is one an agent has to know to call — which the ones that most need
	// this signal are the least likely to do.
	//
	// A history read that fails is not fatal here: the gaps are the answer to
	// this call and withholding them because an annotation could not be
	// gathered would be the worse failure. The absence is reported instead, so
	// a caller can tell "no gap has ever recurred" from "recurrence was not
	// available" — the two are opposite facts and must not share a rendering.
	history := map[string]store.GapHistoryRow{}
	historyAvailable := true
	historyTruncated := false
	if rows, histTruncated, err := r.Store.GapHistory(ctx, taskID); err == nil {
		historyTruncated = histTruncated
		for _, h := range rows {
			history[h.GapID] = h
		}
	} else {
		historyAvailable = false
	}

	blocking := 0
	gaps := make([]map[string]any, 0, len(rows))
	for _, row := range rows {
		if row.Blocking {
			blocking++
		}
		gap := map[string]any{
			"id":              row.ID,
			"severity":        row.Severity,
			"gap_type":        row.GapType,
			"task_id":         row.TaskID,
			"description":     row.Description,
			"recommended_fix": row.RecommendedFix,
			"blocking":        row.Blocking,
			"evidence_json":   row.EvidenceJSON,
			// Which requirement and criterion an acceptance gap is about, and
			// the method it expected; null for a gap that is not about one.
			"requirement_id":               row.RequirementID,
			"acceptance_criterion_id":      row.AcceptanceCriterionID,
			"expected_verification_method": row.ExpectedVerificationMethod,
			// Where the gap is and what reproduces it; null when it has none.
			"file":              row.File,
			"line":              row.Line,
			"suggested_command": row.SuggestedCommand,
			// Where a failed command's captured output is, so the gap can be
			// read without re-running the command.
			"stdout_path": row.StdoutPath,
			"stderr_path": row.StderrPath,
		}
		// Present and null when the evidence was readable, so a consumer can
		// tell "no problem" from a server that does not report the field.
		gap["evidence_error"] = nil
		if row.EvidenceError != "" {
			gap["evidence_error"] = row.EvidenceError
		}
		if h, ok := history[row.ID]; ok {
			gap["occurrences"] = h.Occurrences
			gap["resurfaces"] = h.Resurfaces
			gap["first_seen_run"] = h.FirstSeenRun
		}
		gaps = append(gaps, gap)
	}
	return map[string]any{
		"ok":       true,
		"task_id":  taskID,
		"gaps":     gaps,
		"blocking": blocking,
		// False means the recurrence annotation is missing from every gap
		// above, not that nothing has ever recurred.
		"recurrence_available": historyAvailable,
		"recurrence_truncated": historyTruncated,
		"truncated":            truncated,
	}
}

// CurrentGate is the write gate for the config as it is on disk now: the one
// Gate holds, or, when OpenGate is set, one rebuilt if config.yaml changed since
// the last build. err is why there is no gate.
func (r *Registry) CurrentGate() (*gate.Gate, error) {
	if r.OpenGate == nil {
		return r.Gate, nil
	}
	key := gatescfg.ConfigKey(r.Root)
	r.gateMu.Lock()
	defer r.gateMu.Unlock()
	if !r.gateBuilt || key != r.gateKey {
		r.Gate, r.gateErr = r.OpenGate()
		if r.gateErr != nil {
			r.Gate = nil
		}
		r.gateKey, r.gateBuilt = key, true
	}
	return r.Gate, r.gateErr
}

// callPolicyCheck judges a write against the named task's scope. It used to
// read only path: task_id was dropped without a word, so the ladder stopped at
// task.absent and a caller asking "is this in my task's scope?" got an answer
// about no task at all, which it would read as the one it asked for.
//
// Call has already refused any argument the schema does not declare, of the
// wrong type, or outside its enum, so operation is one of create, modify or
// delete here. dc.OpWrite is deliberately not offered: a caller that can say
// which of create and modify it means gets the stricter answer.
func (r *Registry) callPolicyCheck(ctx context.Context, args map[string]any) any {
	path, _ := args["path"].(string)
	if path == "" {
		return ErrorPayload{OK: false, Code: "missing_argument", Error: "path is required"}
	}
	op := dc.OpModify
	if raw, given := args["operation"].(string); given {
		op = dc.Operation(raw)
	}
	taskID, taskGiven := args["task_id"].(string)
	if taskGiven && taskID == "" {
		return ErrorPayload{OK: false, Code: "invalid_argument",
			Error: "devcouncil_policy_check_write: task_id is empty; omit it to ask without a task"}
	}
	g, gateErr := r.CurrentGate()
	if g == nil {
		if gateErr != nil {
			return ErrorPayload{OK: false, Code: "not_initialized",
				Error: "the write gate could not be built from this directory's config, so this path was not judged: " + gateErr.Error()}
		}
		// Not a verdict. This used to answer `ok: true, allowed: true` with a
		// note, so a host whose gate failed to build (an unreadable
		// config.yaml, a refused flag) reported every path writable — a check
		// that could not run answering as one that ran and passed.
		return ErrorPayload{OK: false, Code: "not_initialized",
			Error: "no write gate is configured for this directory, so this path was not judged; " +
				"see the MCP server's stderr for why the gate could not be built"}
	}
	// Without a task, EvaluateWrite still answers secret/restricted rules. With
	// one, every failure to load it is its own answer: falling through to a nil
	// task would report task.absent for a store that could not be read.
	var task *dc.Task
	if taskGiven {
		if r.Store == nil {
			return ErrorPayload{OK: false, Code: "not_initialized",
				Error: "DevCouncil state is unavailable in this directory, so task " + taskID + " could not be loaded."}
		}
		stored, err := r.Store.Task(ctx, taskID)
		if err != nil {
			return ErrorPayload{OK: false, Code: "store_error", Error: err.Error()}
		}
		if stored == nil {
			return map[string]any{"ok": false, "error": fmt.Sprintf("Task %s not found.", taskID), "code": "not_found", "task_id": taskID}
		}
		task = stored.Domain()
	}
	d, err := g.EvaluateWrite(path, task, op)
	if err != nil {
		return map[string]any{"ok": false, "error": err.Error(), "code": "policy_error"}
	}
	return map[string]any{
		"ok":      true,
		"path":    path,
		"allowed": !d.Blocked(),
		// True when the write is allowed here but verification will raise the
		// path as an unplanned file: it is lent by a planned file's
		// neighbourhood, or its scope refusal was demoted by the posture.
		"would_block_verification": policy.WouldBlockVerificationIn(r.Root, d, task) && !(op == dc.OpCreate && verify.IsAdvisoryNewTestPath(path)),
		"decision":                 d,
	}
}

// argumentHints says, for an argument callers send expecting it to work, why
// it is refused rather than merely that it is.
var argumentHints = map[string]string{
	"sandbox": "verification runs on the host and the only sandbox is " + verify.SandboxLocal,
}

// undeclaredArgument refuses an argument the named tool's input schema does
// not declare, when that schema sets additionalProperties:false. The schema is
// read from toolSpecs, the same text tools/list advertises, so the check and
// the advertisement cannot disagree. Names are checked in sorted order so the
// refusal is deterministic. An unknown tool is left to Call's own refusal.
func undeclaredArgument(tool string, args map[string]any) (ErrorPayload, bool) {
	if len(args) == 0 {
		return ErrorPayload{}, false
	}
	for _, spec := range toolSpecs() {
		if spec.Name != tool {
			continue
		}
		var schema struct {
			Properties           map[string]json.RawMessage `json:"properties"`
			AdditionalProperties *bool                      `json:"additionalProperties"`
		}
		if err := json.Unmarshal(spec.InputSchema, &schema); err != nil {
			// A schema this package wrote and cannot read is a bug here, and
			// refusing every call names it rather than accepting anything.
			return ErrorPayload{OK: false, Code: "invalid_argument",
				Error: tool + " has an unreadable input schema: " + err.Error()}, true
		}
		closed := schema.AdditionalProperties != nil && !*schema.AdditionalProperties
		names := make([]string, 0, len(args))
		for name := range args {
			names = append(names, name)
		}
		sort.Strings(names)
		for _, name := range names {
			if _, declared := schema.Properties[name]; declared || !closed {
				continue
			}
			declaredNames := make([]string, 0, len(schema.Properties))
			for p := range schema.Properties {
				declaredNames = append(declaredNames, p)
			}
			sort.Strings(declaredNames)
			msg := tool + " takes only " + strings.Join(declaredNames, ", ") + "; refusing argument " + name
			if hint, ok := argumentHints[name]; ok {
				msg += ": " + hint
			}
			return ErrorPayload{OK: false, Code: "invalid_argument", Error: msg}, true
		}
		for _, name := range names {
			property, declared := schema.Properties[name]
			if !declared {
				continue
			}
			if refusal, refused := mistypedArgument(tool, name, property, args[name]); refused {
				return refusal, true
			}
		}
		return ErrorPayload{}, false
	}
	return ErrorPayload{}, false
}

// mistypedArgument refuses a declared argument whose value is not the type, or
// not one of the values, its schema property names. Handlers read arguments
// with `args[k].(T)`, which drops a mistyped value as silently as an undeclared
// one: `force: "true"` on checkout ran an unforced checkout, `staged: "yes"`
// diffed the working tree, a numeric task_id asked about no task at all. A
// property shape this checker does not understand refuses the call rather than
// passing whatever arrived.
func mistypedArgument(tool, name string, property json.RawMessage, value any) (ErrorPayload, bool) {
	var p struct {
		Type  string `json:"type"`
		Enum  []any  `json:"enum"`
		Items *struct {
			Type string `json:"type"`
		} `json:"items"`
	}
	if err := json.Unmarshal(property, &p); err != nil {
		return ErrorPayload{OK: false, Code: "invalid_argument",
			Error: tool + " has an unreadable schema for " + name + ": " + err.Error()}, true
	}
	refuse := func(want string) (ErrorPayload, bool) {
		return ErrorPayload{OK: false, Code: "invalid_argument",
			Error: fmt.Sprintf("%s: %s must be %s, got %s", tool, name, want, jsonTypeName(value))}, true
	}
	switch p.Type {
	case "string", "boolean":
		if jsonTypeName(value) != p.Type {
			return refuse("a " + p.Type)
		}
	case "array":
		items, ok := value.([]any)
		if !ok || p.Items == nil {
			return refuse("an array")
		}
		for _, item := range items {
			if jsonTypeName(item) != p.Items.Type {
				return refuse("an array of " + p.Items.Type + "s")
			}
		}
	default:
		return ErrorPayload{OK: false, Code: "invalid_argument",
			Error: fmt.Sprintf("%s: the schema for %s declares type %q, which this host cannot check", tool, name, p.Type)}, true
	}
	if len(p.Enum) == 0 {
		return ErrorPayload{}, false
	}
	if p.Type == "array" {
		// Comparing two slices with == panics, and no schema here needs it.
		return ErrorPayload{OK: false, Code: "invalid_argument",
			Error: fmt.Sprintf("%s: the schema for %s puts an enum on an array, which this host cannot check", tool, name)}, true
	}
	allowed := make([]string, 0, len(p.Enum))
	for _, v := range p.Enum {
		if v == value {
			return ErrorPayload{}, false
		}
		allowed = append(allowed, fmt.Sprint(v))
	}
	return ErrorPayload{OK: false, Code: "invalid_argument",
		Error: fmt.Sprintf("%s: %s must be one of %s, not %q", tool, name, strings.Join(allowed, ", "), fmt.Sprint(value))}, true
}

// jsonTypeName is the JSON Schema type of a value decoded by encoding/json.
func jsonTypeName(v any) string {
	switch v.(type) {
	case nil:
		return "null"
	case string:
		return "string"
	case bool:
		return "boolean"
	case float64, json.Number:
		return "number"
	case []any:
		return "array"
	case map[string]any:
		return "object"
	default:
		return fmt.Sprintf("%T", v)
	}
}
