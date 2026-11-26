package devmap

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// The installing commands write host configuration, and a host entry must
// never be a file the repository supplied. devmap is handed the repository as
// --project-root, so it has no need to run inside it; running there made a file
// named `devmap` at the repository root reachable by name from the working
// directory (the card's default path: the Go host always ran it with the
// repository root as its cwd).
func TestInstallingCommandsDoNotRunInsideTheRepository(t *testing.T) {
	testsupport.RequireShellFakes(t)
	dir := t.TempDir()
	repo := filepath.Join(dir, "repo")
	if err := os.Mkdir(repo, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(repo, "devmap"), []byte("#!/bin/sh\nexit 0\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	cwdLog := filepath.Join(dir, "cwd")
	fake := filepath.Join(dir, "devmap")
	script := "#!/bin/sh\npwd -P >> " + cwdLog + "\necho '{\"check_ok\":true}'\n"
	if err := os.WriteFile(fake, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	c := New(fake, repo)

	if _, err := c.Integrate(context.Background(), "claude", InstallCheck, nil, ""); err != nil {
		t.Fatal(err)
	}
	if _, err := c.InstallSkills(context.Background(), nil, []Skill{{Name: "s", Content: "x"}}, InstallCheck); err != nil {
		t.Fatal(err)
	}

	raw, err := os.ReadFile(cwdLog)
	if err != nil {
		t.Fatal(err)
	}
	resolvedRepo, err := filepath.EvalSymlinks(repo)
	if err != nil {
		t.Fatal(err)
	}
	for _, got := range strings.Fields(string(raw)) {
		if got == resolvedRepo || strings.HasPrefix(got, resolvedRepo+string(filepath.Separator)) {
			t.Fatalf("an installing command ran inside the repository: cwd %s", got)
		}
	}
	if len(strings.Fields(string(raw))) != 2 {
		t.Fatalf("expected two invocations, log: %q", raw)
	}
}
