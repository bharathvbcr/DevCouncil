package verify

import (
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

func TestFilePtrOmitsEmpty(t *testing.T) {
	if filePtr("") != nil {
		t.Fatal("empty file must be JSON null")
	}
	p := filePtr("src/a.go")
	if p == nil || *p != "src/a.go" {
		t.Fatal("non-empty file must be kept")
	}
}

func TestMCPResultOmitsCorrectionPathAndBlockedVerifyWritesNoCorrectionManifest(t *testing.T) {
	t.Setenv(AllowNoRigorEnv, "1")

	mcpType := reflect.TypeFor[MCPResult]()
	for i := 0; i < mcpType.NumField(); i++ {
		field := mcpType.Field(i)
		if field.Name == "CorrectionPath" || strings.HasPrefix(field.Tag.Get("json"), "correction_path") {
			t.Fatalf("MCPResult must not carry %s (%q)", field.Name, field.Tag.Get("json"))
		}
	}

	git := testsupport.Tool(t, "git")
	root := t.TempDir()
	runGit := func(args ...string) {
		cmd := exec.Command(git, args...)
		cmd.Dir = root
		cmd.Env = append(os.Environ(),
			"GIT_AUTHOR_NAME=t", "GIT_AUTHOR_EMAIL=t@e.invalid",
			"GIT_COMMITTER_NAME=t", "GIT_COMMITTER_EMAIL=t@e.invalid",
		)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v %s", args, err, out)
		}
	}
	runGit("init", "-q", root)
	if err := os.MkdirAll(filepath.Join(root, "src"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "src", "a.go"), []byte("package a\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	runGit("add", ".")
	runGit("commit", "-qm", "base")
	if err := os.WriteFile(filepath.Join(root, "extra.go"), []byte("package main\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Join(root, ".devcouncil"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, ".devcouncil", "config.yaml"), []byte("gates:\n  mode: enforce\n"), 0o600); err != nil {
		t.Fatal(err)
	}

	dbPath := filepath.Join(root, ".devcouncil", "state.sqlite")
	client := store.New(testsupport.DCStore(t), dbPath)
	t.Cleanup(client.Close)
	if _, err := client.ActiveLeases(context.Background()); err != nil {
		t.Fatal(err)
	}
	scope, err := json.Marshal(map[string]any{
		"planned_files":  []map[string]string{{"path": "src/a.go", "allowed_change": "modify"}},
		"expected_tests": []string{"true"},
	})
	if err != nil {
		t.Fatal(err)
	}
	testsupport.SeedTask(t, client.DB, testsupport.TaskSeed{ID: "T1", Title: "t", Scope: string(scope)})

	result, _, err := VerifyTask(context.Background(), root, client, "T1", GateState{Mode: "enforce"}, "", "")
	if err != nil {
		t.Fatalf("VerifyTask: %v", err)
	}
	if result.Passed || result.Status != "blocked" {
		t.Fatalf("expected blocked verify, got passed=%v status=%q", result.Passed, result.Status)
	}

	raw, err := json.Marshal(result)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(raw), "correction") {
		t.Fatalf("MCPResult JSON must not mention correction: %s", raw)
	}
	if _, err := os.Stat(filepath.Join(root, ".devcouncil", "corrections")); !os.IsNotExist(err) {
		t.Fatalf(".devcouncil/corrections must not exist, stat err=%v", err)
	}
}
