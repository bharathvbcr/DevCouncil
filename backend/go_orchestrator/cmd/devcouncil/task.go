package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/console"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/gatescfg"
)

const taskHelp = `devcouncil task — create, move and show tasks

A task is a workbench item; these commands wrap ` + "`dcstore work`" + ` and refuse what the
store refuses (an invalid scope is refused with code invalid_scope).

Usage:
  devcouncil task create --id ID --title TITLE [--description TEXT] [--status S] [--priority 1-3]
                         [--scope-json JSON | --scope-file PATH] [--project-root DIR]
  devcouncil task status ID --to inbox|backlog|ready|in_progress|review|done [--project-root DIR]
  devcouncil task show ID [--json] [--project-root DIR]

Checking a task out sets it in_progress; a verify that passes sets it to review.
Closing it to done is a person's decision: ` + "`devcouncil task status ID --to done`" + `.

--scope-json is the item's scope object, for example:
  {"planned_files":["src/a.go",{"path":"docs/x.md","allowed_change":"create"}],
   "expected_tests":["go test ./..."],"requirement_ids":["REQ-1"]}
`

// taskStatuses is the workbench vocabulary; the store validates it too.
var taskStatuses = map[string]bool{
	"inbox": true, "backlog": true, "ready": true, "in_progress": true, "review": true, "done": true,
}

func runTask(args []string) int {
	if len(args) < 1 {
		console.Error(taskHelp)
		return 2
	}
	switch args[0] {
	case "-h", "--help":
		console.Error(taskHelp)
		return 0
	case "create":
		return runTaskCreate(args[1:])
	case "status":
		return runTaskStatus(args[1:])
	case "show":
		return runTaskShow(args[1:])
	default:
		console.Errorf("unknown task subcommand: %s\n", args[0])
		console.Error(taskHelp)
		return 2
	}
}

// taskFlags is what the three subcommands share: where the repository is.
type taskFlags struct {
	root  string
	json  bool
	named map[string]string
	rest  []string
}

// parseTaskFlags reads `--name value` pairs from the allowed set, plus the
// shared --project-root and --json, and returns the positional arguments.
func parseTaskFlags(args []string, valueFlags ...string) (taskFlags, bool) {
	out := taskFlags{named: map[string]string{}}
	allowed := map[string]bool{}
	for _, name := range valueFlags {
		allowed[name] = true
	}
	for i := 0; i < len(args); i++ {
		arg := args[i]
		switch {
		case arg == "--json":
			out.json = true
		case arg == "--project-root":
			i++
			root, ok := projectRootFlag(args, i)
			if !ok {
				return out, false
			}
			out.root = root
		case allowed[arg]:
			i++
			if i >= len(args) {
				console.Errorf("%s needs a value\n", arg)
				return out, false
			}
			if _, dup := out.named[arg]; dup {
				console.Errorf("%s given twice\n", arg)
				return out, false
			}
			out.named[arg] = args[i]
		case strings.HasPrefix(arg, "-"):
			console.Errorf("unknown flag: %s\n", arg)
			return out, false
		default:
			out.rest = append(out.rest, arg)
		}
	}
	return out, true
}

// taskClient opens the store the task commands act on. create may bring the
// store into existence; the others require it.
func taskClient(root string, create bool) (*store.Client, string, error) {
	root, err := resolveRoot(root)
	if err != nil {
		return nil, "", err
	}
	loc, err := gatescfg.ResolveWorkbenchDB(root)
	if err != nil {
		return nil, "", err
	}
	if _, err := os.Stat(loc.Path); err != nil {
		if !create || !os.IsNotExist(err) {
			return nil, "", fmt.Errorf("no task store at %s: %w", loc.Path, err)
		}
		if err := os.MkdirAll(filepath.Dir(loc.Path), 0o755); err != nil {
			return nil, "", err
		}
	}
	client := dcstoreClient(root, loc.Path)
	if client == nil {
		return nil, "", errors.New("no dcstore can run against the task store")
	}
	return client, root, nil
}

func runTaskCreate(args []string) int {
	f, ok := parseTaskFlags(args, "--id", "--title", "--description", "--status", "--priority", "--scope-json", "--scope-file")
	if !ok {
		return 2
	}
	if len(f.rest) > 0 {
		console.Errorf("unexpected argument: %s\n", f.rest[0])
		return 2
	}
	id, title := f.named["--id"], f.named["--title"]
	if id == "" || title == "" {
		console.Errorln("task create needs --id and --title")
		return 2
	}
	status := f.named["--status"]
	if status == "" {
		status = "ready"
	}
	if !taskStatuses[status] {
		console.Errorf("unknown status %q (want inbox, backlog, ready, in_progress, review or done)\n", status)
		return 2
	}
	put := map[string]any{
		"id": id, "title": title, "status": status, "expected_revision": 0,
		// Keyed by the id, so repeating a create that already happened replays
		// its answer instead of colliding with it.
		"request_id": "task-create-" + id,
	}
	if d := f.named["--description"]; d != "" {
		put["description"] = d
	}
	if p := f.named["--priority"]; p != "" {
		n, err := strconv.Atoi(p)
		if err != nil || n < 1 || n > 3 {
			console.Errorf("--priority %q is not 1, 2 or 3\n", p)
			return 2
		}
		put["priority"] = n
	}
	scope, err := scopeFromFlags(f.named)
	if err != nil {
		console.Errorf("task create: %v\n", err)
		return 2
	}
	if scope != nil {
		put["scope"] = scope
	}
	client, _, err := taskClient(f.root, true)
	if err != nil {
		console.Errorf("task create: %v\n", err)
		return 1
	}
	defer client.Close()
	ctx := console.Context()
	repo, err := client.LocalRepository(ctx)
	if err != nil {
		console.Errorf("task create: %v\n", err)
		return 1
	}
	put["repository_ids"] = []string{repo}
	put["primary_repository_id"] = repo
	body, err := json.Marshal(put)
	if err != nil {
		console.Errorf("task create: %v\n", err)
		return 1
	}
	reply, err := client.Workbench(ctx, "items.put", body)
	return printTaskReply("task create", reply, err, f.json)
}

// scopeFromFlags reads --scope-json or --scope-file (not both) as one JSON
// object. It is not validated here: the store owns the scope grammar, and a
// second copy of it would drift.
func scopeFromFlags(named map[string]string) (json.RawMessage, error) {
	inline, file := named["--scope-json"], named["--scope-file"]
	if inline != "" && file != "" {
		return nil, errors.New("pass --scope-json or --scope-file, not both")
	}
	raw := []byte(inline)
	if file != "" {
		info, err := os.Stat(file)
		if err != nil {
			return nil, err
		}
		if info.Size() > 256<<10 {
			return nil, fmt.Errorf("%s is larger than the 256 KiB a scope may be", file)
		}
		if raw, err = os.ReadFile(file); err != nil {
			return nil, err
		}
	}
	if len(strings.TrimSpace(string(raw))) == 0 {
		return nil, nil
	}
	if !json.Valid(raw) {
		return nil, errors.New("the scope is not valid JSON")
	}
	return json.RawMessage(raw), nil
}

func runTaskStatus(args []string) int {
	f, ok := parseTaskFlags(args, "--to")
	if !ok {
		return 2
	}
	to := f.named["--to"]
	if len(f.rest) != 1 || to == "" {
		console.Errorln("usage: devcouncil task status ID --to STATUS")
		return 2
	}
	if !taskStatuses[to] {
		console.Errorf("unknown status %q (want inbox, backlog, ready, in_progress, review or done)\n", to)
		return 2
	}
	client, _, err := taskClient(f.root, false)
	if err != nil {
		console.Errorf("task status: %v\n", err)
		return 1
	}
	defer client.Close()
	moved, err := client.MoveTaskStatus(console.Context(), f.rest[0], "", to)
	if errors.Is(err, store.ErrNoSuchTask) {
		if f.json {
			_ = console.JSON(map[string]any{"ok": false, "code": "no_such_task", "task_id": f.rest[0]})
		} else {
			console.Errorf("task status: no task %s in this store\n", f.rest[0])
		}
		return 1
	}
	if err != nil {
		console.Errorf("task status: %v\n", err)
		return 1
	}
	// With no from status, the only move MoveTaskStatus declines is to the
	// status the task already has.
	if f.json {
		_ = console.JSON(map[string]any{"ok": true, "task_id": f.rest[0], "status": to, "moved": moved})
	} else if moved {
		console.Printf("%s is now %s\n", f.rest[0], to)
	} else {
		console.Printf("%s is already %s\n", f.rest[0], to)
	}
	return 0
}

func runTaskShow(args []string) int {
	f, ok := parseTaskFlags(args)
	if !ok {
		return 2
	}
	if len(f.rest) != 1 {
		console.Errorln("usage: devcouncil task show ID [--json]")
		return 2
	}
	client, _, err := taskClient(f.root, false)
	if err != nil {
		console.Errorf("task show: %v\n", err)
		return 1
	}
	defer client.Close()
	ctx := console.Context()
	request, _ := json.Marshal(map[string]string{"id": f.rest[0]})
	reply, err := client.Workbench(ctx, "items.get", request)
	if err != nil {
		console.Errorf("task show: %v\n", err)
		return 1
	}
	view, err := client.Task(ctx, f.rest[0])
	if err != nil {
		console.Errorf("task show: %v\n", err)
		return 1
	}
	lease, err := client.Active(ctx, f.rest[0])
	if err != nil {
		console.Errorf("task show: %v\n", err)
		return 1
	}
	if view == nil {
		console.Errorf("task show: %s is not a task of this repository (its primary repository is not this one, or it is deleted)\n", f.rest[0])
		return 1
	}
	if f.json {
		if err := console.JSON(map[string]any{
			"ok": true, "item": itemOf(reply), "blocking_gap_count": view.BlockingGapCount, "lease": lease,
		}); err != nil {
			return 1
		}
		return 0
	}
	console.Printf("%s  %s\n", view.ID, view.Title)
	console.Printf("  status:        %s\n", view.Status)
	console.Printf("  blocking gaps: %d\n", view.BlockingGapCount)
	if lease != nil {
		console.Printf("  leased by:     %s until %s\n", lease.Owner, lease.ExpiresAt)
	}
	console.Printf("  planned files: %d  expected tests: %d  requirements: %d\n",
		len(view.PlannedFiles), len(view.ExpectedTests), len(view.RequirementIDs))
	return 0
}

// itemOf is the item inside a workbench reply.
func itemOf(reply json.RawMessage) json.RawMessage {
	var wire struct {
		Item json.RawMessage `json:"item"`
	}
	if json.Unmarshal(reply, &wire) != nil {
		return nil
	}
	return wire.Item
}

// printTaskReply reports a workbench write, refusals included.
func printTaskReply(what string, reply json.RawMessage, err error, jsonOut bool) int {
	if err != nil {
		var refusal *store.WorkbenchError
		if errors.As(err, &refusal) {
			if jsonOut {
				_ = console.JSON(map[string]any{"ok": false, "code": refusal.Code, "error": refusal.Message})
			} else {
				console.Errorf("%s: refused (%s): %s\n", what, refusal.Code, refusal.Message)
			}
			return 1
		}
		console.Errorf("%s: %v\n", what, err)
		return 1
	}
	item := itemOf(reply)
	if jsonOut {
		_ = console.JSON(map[string]any{"ok": true, "item": item})
		return 0
	}
	var brief struct {
		ID, Status, Title string
	}
	_ = json.Unmarshal(item, &brief)
	console.Printf("%s %s (%s): %s\n", what, brief.ID, brief.Status, brief.Title)
	return 0
}
