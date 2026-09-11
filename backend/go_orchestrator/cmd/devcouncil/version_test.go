package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func TestVersionCommandPrintsTheProductVersion(t *testing.T) {
	stdout, restore := swapStdout(t)
	code := dispatch([]string{"--version"})
	restore()
	if code != 0 {
		t.Fatalf("exit %d, want 0", code)
	}
	got := strings.TrimSpace(stdout.String())
	want := "devcouncil " + Version
	if got != want {
		t.Fatalf("stdout = %q, want %q", got, want)
	}
}

func TestVersionAgreesWithNpmAndRustWorkspace(t *testing.T) {
	root := repoRoot(t)
	npm := npmVersion(t, filepath.Join(root, "package.json"))
	if npm != Version {
		t.Fatalf("package.json version %q, Go Version %q", npm, Version)
	}
	lock := npmVersion(t, filepath.Join(root, "package-lock.json"))
	if lock != Version {
		t.Fatalf("package-lock.json version %q, Go Version %q", lock, Version)
	}
	rust := rustWorkspaceVersion(t, filepath.Join(root, "rust", "Cargo.toml"))
	if rust != Version {
		t.Fatalf("rust/Cargo.toml workspace.package.version %q, Go Version %q", rust, Version)
	}
}

func repoRoot(t *testing.T) string {
	t.Helper()
	_, file, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("runtime.Caller failed")
	}
	root := filepath.Clean(filepath.Join(filepath.Dir(file), "..", "..", "..", ".."))
	if _, err := os.Stat(filepath.Join(root, "package.json")); err != nil {
		t.Fatalf("repo root does not contain package.json: %s (%v)", root, err)
	}
	return root
}

func npmVersion(t *testing.T, path string) string {
	t.Helper()
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var payload struct {
		Version string `json:"version"`
	}
	if err := json.Unmarshal(raw, &payload); err != nil {
		t.Fatal(err)
	}
	if payload.Version == "" {
		t.Fatalf("%s has an empty version", path)
	}
	return payload.Version
}

func rustWorkspaceVersion(t *testing.T, path string) string {
	t.Helper()
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	section := ""
	for _, line := range strings.Split(string(raw), "\n") {
		trimmed := strings.TrimSpace(line)
		if strings.HasPrefix(trimmed, "[") && strings.HasSuffix(trimmed, "]") {
			section = trimmed
			continue
		}
		if section != "[workspace.package]" {
			continue
		}
		if !strings.HasPrefix(trimmed, "version") {
			continue
		}
		_, value, ok := strings.Cut(trimmed, "=")
		if !ok {
			continue
		}
		return strings.Trim(strings.TrimSpace(value), `"`)
	}
	t.Fatalf("%s has no [workspace.package] version", path)
	return ""
}
