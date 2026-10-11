package main

// Golden replay driver for get_diff, the embedded skills library and the MCP
// tool surface.
//
// Run from backend/go_orchestrator: go run ./cmd/devcouncil-golden-replay
// CI runs it (.github/workflows/analysis-plane.yml). A deliberate change to the
// skills library or the tool list is recorded with -update, which rewrites
// testdata/golden/replay_surface.json; the diff of that file is the review.
//
// Compares live Go get_diff results against the committed Phase-0 envelopes
// under testdata/golden/mcp/get_diff/.

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"slices"
	"strings"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/skills"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

// gitBudget bounds each git invocation that seeds the replay repository.
const gitBudget = 30 * time.Second

const surfaceFile = "replay_surface.json"

func main() {
	golden := flag.String("golden", filepath.Join("testdata", "golden"), "golden corpus directory")
	update := flag.Bool("update", false, "rewrite "+surfaceFile+" from the live skills library and tool list")
	flag.Parse()
	os.Exit(run(*golden, *update, os.Stdout, os.Stderr))
}

// surface is the part of the golden corpus that is a list of names rather
// than an envelope: what the host embeds and what it serves.
type surface struct {
	Skills []string `json:"skills"`
	Tools  []string `json:"tools"`
}

func run(golden string, update bool, out, errw io.Writer) int {
	if _, err := os.Stat(golden); err != nil {
		fmt.Fprintf(errw, "golden root missing: %v\n", err)
		return 1
	}
	repo, err := seedRepo()
	if repo != "" {
		defer func() { _ = os.RemoveAll(repo) }()
	}
	if err != nil {
		fmt.Fprintf(errw, "seeding the replay repository: %v\n", err)
		return 1
	}
	live, err := liveSurface(repo)
	if err != nil {
		fmt.Fprintf(errw, "reading the live surface: %v\n", err)
		return 1
	}
	if update {
		if err := writeSurface(golden, live); err != nil {
			fmt.Fprintf(errw, "%v\n", err)
			return 1
		}
		fmt.Fprintf(out, "golden replay: %s updated (%d skills, %d tools)\n", surfaceFile, len(live.Skills), len(live.Tools))
		return 0
	}
	failed := 0
	failed += replayGetDiffCases(golden, repo, out, errw)
	failed += replaySurface(golden, live, out, errw)
	if failed > 0 {
		fmt.Fprintf(errw, "%d golden replay check(s) failed\n", failed)
		return 1
	}
	fmt.Fprintln(out, "golden replay: ok")
	return 0
}

func liveSurface(repo string) (surface, error) {
	all, err := skills.Embedded.Load()
	if err != nil {
		return surface{}, fmt.Errorf("skills embed: %w", err)
	}
	var live surface
	for _, s := range all {
		live.Skills = append(live.Skills, s.Name)
	}
	for _, spec := range devcouncil.NewRegistry(repo, nil, nil).Specs() {
		live.Tools = append(live.Tools, spec.Name)
	}
	slices.Sort(live.Skills)
	slices.Sort(live.Tools)
	return live, nil
}

func writeSurface(golden string, live surface) error {
	encoded, err := json.MarshalIndent(live, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(filepath.Join(golden, surfaceFile), append(encoded, '\n'), 0o644)
}

// replaySurface compares the live skills and tool names with the committed
// ones. A count hardcoded here is what let this driver rot when the library
// grew from 17 to 18; the names are data, and a difference names what moved.
func replaySurface(golden string, live surface, out, errw io.Writer) int {
	raw, err := os.ReadFile(filepath.Join(golden, surfaceFile))
	if err != nil {
		fmt.Fprintf(errw, "%s: %v (record it with -update)\n", surfaceFile, err)
		return 1
	}
	var want surface
	if err := json.Unmarshal(raw, &want); err != nil {
		fmt.Fprintf(errw, "%s: %v\n", surfaceFile, err)
		return 1
	}
	failed := 0
	for _, check := range []struct {
		what      string
		got, want []string
	}{{"skills", live.Skills, want.Skills}, {"tools", live.Tools, want.Tools}} {
		if len(check.want) == 0 {
			fmt.Fprintf(errw, "%s: golden lists no %s; an empty golden proves nothing\n", surfaceFile, check.what)
			failed++
			continue
		}
		if added, removed := difference(check.got, check.want), difference(check.want, check.got); len(added)+len(removed) > 0 {
			fmt.Fprintf(errw, "%s drifted: live-only %v, golden-only %v (deliberate? rerun with -update)\n", check.what, added, removed)
			failed++
			continue
		}
		fmt.Fprintf(out, "%s: ok (%d)\n", check.what, len(check.got))
	}
	return failed
}

// difference is the entries of a that b lacks.
func difference(a, b []string) []string {
	var only []string
	for _, name := range a {
		if !slices.Contains(b, name) {
			only = append(only, name)
		}
	}
	return only
}

// payloadOf is the golden envelope's normalized response body.
func payloadOf(golden, rel string) (map[string]any, error) {
	raw, err := os.ReadFile(filepath.Join(golden, rel))
	if err != nil {
		return nil, err
	}
	var env struct {
		Payload map[string]any `json:"payload"`
	}
	if err := json.Unmarshal(raw, &env); err != nil {
		return nil, fmt.Errorf("%s: %w", rel, err)
	}
	return env.Payload, nil
}

// sameAsGolden reports whether a live result serializes to the golden payload.
func sameAsGolden(name, golden, rel string, got any, errw io.Writer) bool {
	want, err := payloadOf(golden, rel)
	if err != nil {
		fmt.Fprintf(errw, "%s: %v\n", name, err)
		return false
	}
	encoded, err := json.Marshal(got)
	if err != nil {
		fmt.Fprintf(errw, "%s: %v\n", name, err)
		return false
	}
	var live map[string]any
	if err := json.Unmarshal(encoded, &live); err != nil {
		fmt.Fprintf(errw, "%s: %v\n", name, err)
		return false
	}
	if !reflect.DeepEqual(live, want) {
		fmt.Fprintf(errw, "%s differs from golden %s:\n  live   %s\n  golden %s\n", name, rel, encoded, mustMarshal(want))
		return false
	}
	return true
}

func mustMarshal(v any) string {
	b, err := json.Marshal(v)
	if err != nil {
		return err.Error()
	}
	return string(b)
}

func replayGetDiffCases(golden, repo string, out, errw io.Writer) int {
	failed := 0
	ctx := context.Background()

	writeFile(filepath.Join(repo, "src/a.py"), "VALUE = 2\n")
	got, err := devcouncil.GetDiff(ctx, repo, devcouncil.GetDiffArgs{})
	if err != nil || !sameAsGolden("dirty", golden, "mcp/get_diff/dirty.json", got, errw) {
		if err != nil {
			fmt.Fprintf(errw, "dirty: %v\n", err)
		}
		failed++
	}

	got, err = devcouncil.GetDiff(ctx, repo, devcouncil.GetDiffArgs{
		TaskID: "TASK-001", TaskFound: true, DBInitialized: true, PlannedFiles: nil,
	})
	if err != nil || !sameAsGolden("empty_planned_scope", golden, "mcp/get_diff/empty_planned_scope.json", got, errw) {
		if err != nil {
			fmt.Fprintf(errw, "empty_planned_scope: %v\n", err)
		}
		failed++
	}

	notARepo, err := os.MkdirTemp("", "dc-golden-nonrepo-*")
	if err != nil {
		fmt.Fprintf(errw, "non_repo: %v\n", err)
		return failed + 1
	}
	defer func() { _ = os.RemoveAll(notARepo) }()
	got, _ = devcouncil.GetDiff(ctx, notARepo, devcouncil.GetDiffArgs{})
	if !sameAsGolden("non_repo", golden, "mcp/get_diff/non_repo.json", got, errw) {
		failed++
	}

	trimmed, truncated := devcouncil.TruncateText(strings.Repeat("a", 25000), devcouncil.DiffOutputLimit)
	if !truncated || !strings.Contains(trimmed, "...[truncated to 20000 characters]") {
		fmt.Fprintln(errw, "truncation marker mismatch")
		failed++
	}

	if failed == 0 {
		fmt.Fprintln(out, "get_diff replay: ok")
	}
	return failed
}

// seedRepo creates a temporary repository holding one committed file.
//
// It writes (init, config, commit), so it cannot go through proc.GitCommand,
// which builds guarded reads against the repository under analysis. The
// repository to keep out here is the checkout this tool runs from, not the
// fresh temporary one, so git is resolved once with proc.LookPathOutside
// against the working directory: a git the checkout carries on PATH is
// refused rather than run.
// gitOutputLimit bounds what a seeding git command may say.
const gitOutputLimit = 64 << 10

func seedRepo() (string, error) {
	git, err := proc.LookPathOutside("git", "")
	if err != nil {
		return "", fmt.Errorf("resolving git: %w", err)
	}
	dir, err := os.MkdirTemp("", "dc-golden-*")
	if err != nil {
		return "", err
	}
	run := func(args ...string) error {
		ctx, cancel := context.WithTimeout(context.Background(), gitBudget)
		defer cancel()
		cmd := exec.CommandContext(ctx, git, args...)
		// The deadline must reach whatever git starts, not only git.
		proc.ConfigureGroup(cmd)
		cmd.Dir = dir
		// Capped output and a bound that covers Start, like every other child:
		// git's chatter is only ever read for the failure message.
		out := &proc.CappedBuffer{Limit: gitOutputLimit}
		cmd.Stdout, cmd.Stderr = out, out
		cmd.WaitDelay = time.Second
		runErr, timedOut := proc.RunBounded(ctx, cmd.Run)
		if timedOut {
			return fmt.Errorf("git %v: did not return within %s", args, gitBudget)
		}
		if runErr != nil {
			return fmt.Errorf("git %v: %w: %s", args, runErr, out.Bytes())
		}
		return nil
	}
	writeFile(filepath.Join(dir, "src/a.py"), "VALUE = 1\n")
	for _, args := range [][]string{
		{"init", "-q"},
		{"config", "user.email", "g@test"},
		{"config", "user.name", "g"},
		{"config", "commit.gpgsign", "false"},
		{"add", "src/a.py"},
		{"commit", "-qm", "seed"},
	} {
		if err := run(args...); err != nil {
			return dir, err
		}
	}
	return dir, nil
}

func writeFile(path, body string) {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		panic(err)
	}
	if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
		panic(err)
	}
}
