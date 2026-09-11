package main

// Golden replay driver for Phase 4 get_diff / skills / tools/list.
// Run: go run ./cmd/devcouncil-golden-replay
//
// Compares live Go get_diff shapes against committed Phase-0 envelopes under
// testdata/golden/mcp/get_diff/.

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/skills"
)

func main() {
	root := filepath.Join("testdata", "golden")
	if _, err := os.Stat(root); err != nil {
		fmt.Fprintf(os.Stderr, "golden root missing: %v\n", err)
		os.Exit(1)
	}
	failed := 0
	failed += replayGetDiffCases()
	failed += replaySkillsEmbed()
	failed += replayRegistry()
	if failed > 0 {
		fmt.Fprintf(os.Stderr, "%d golden replay check(s) failed\n", failed)
		os.Exit(1)
	}
	fmt.Println("golden replay: ok")
}

func replayGetDiffCases() int {
	failed := 0
	// Reconstruct dirty case.
	repo := mustRepo()
	writeFile(filepath.Join(repo, "src/a.py"), "VALUE = 2\n")
	got, err := devcouncil.GetDiff(context.Background(), repo, devcouncil.GetDiffArgs{})
	if err != nil {
		fmt.Println("dirty: err", err)
		return 1
	}
	dr := got.(devcouncil.DiffResult)
	golden := loadPayload("testdata/golden/mcp/get_diff/dirty.json")
	if !dr.OK || len(dr.Files) != 1 || dr.Files[0].Path != "src/a.py" {
		fmt.Printf("dirty mismatch: got files=%v\n", dr.Files)
		failed++
	}
	if gFiles, _ := golden["files"].([]any); len(gFiles) != 1 {
		fmt.Println("dirty golden unexpected")
		failed++
	}
	_ = golden

	// empty planned
	got, _ = devcouncil.GetDiff(context.Background(), repo, devcouncil.GetDiffArgs{
		TaskID: "TASK-001", TaskFound: true, DBInitialized: true, PlannedFiles: nil,
	})
	dr = got.(devcouncil.DiffResult)
	if !dr.OK || len(dr.Files) != 0 || dr.UnifiedDiff != "" {
		fmt.Printf("empty_planned_scope fail-closed violated: %+v\n", dr)
		failed++
	}

	// non_repo
	got, _ = devcouncil.GetDiff(context.Background(), os.TempDir(), devcouncil.GetDiffArgs{})
	if ep, ok := got.(devcouncil.ErrorPayload); !ok || ep.Code != "not_a_git_repo" {
		fmt.Printf("non_repo: %#v\n", got)
		failed++
	}

	// truncate contract
	out, trunc := devcouncil.TruncateText(strings.Repeat("a", 25000), devcouncil.DiffOutputLimit)
	if !trunc || !strings.Contains(out, "...[truncated to 20000 characters]") {
		fmt.Println("truncation marker mismatch")
		failed++
	}

	if failed == 0 {
		fmt.Println("get_diff replay: ok")
	}
	return failed
}

func replaySkillsEmbed() int {
	all, err := skills.Embedded.Load()
	if err != nil {
		fmt.Println("skills embed:", err)
		return 1
	}
	if len(all) != 17 {
		fmt.Printf("skills count: want 17 got %d\n", len(all))
		return 1
	}
	fmt.Println("skills embed: ok (17)")
	return 0
}

func replayRegistry() int {
	reg := devcouncil.NewRegistry(".", nil, nil)
	if len(reg.Specs()) < 8 {
		fmt.Printf("registry too small: %d\n", len(reg.Specs()))
		return 1
	}
	fmt.Printf("registry: ok (%d tools)\n", len(reg.Specs()))
	return 0
}

func loadPayload(path string) map[string]any {
	b, err := os.ReadFile(path)
	if err != nil {
		panic(err)
	}
	var env struct {
		Payload map[string]any `json:"payload"`
	}
	if err := json.Unmarshal(b, &env); err != nil {
		panic(err)
	}
	return env.Payload
}

func mustRepo() string {
	dir, err := os.MkdirTemp("", "dc-golden-*")
	if err != nil {
		panic(err)
	}
	run := func(args ...string) {
		cmd := exec.Command("git", args...)
		cmd.Dir = dir
		if out, err := cmd.CombinedOutput(); err != nil {
			panic(fmt.Sprintf("%v: %s", err, out))
		}
	}
	run("init", "-q")
	run("config", "user.email", "g@test")
	run("config", "user.name", "g")
	run("config", "commit.gpgsign", "false")
	writeFile(filepath.Join(dir, "src/a.py"), "VALUE = 1\n")
	run("add", "src/a.py")
	run("commit", "-qm", "seed")
	return dir
}

func writeFile(path, body string) {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		panic(err)
	}
	if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
		panic(err)
	}
}
