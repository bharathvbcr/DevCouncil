package devmap

import (
	"context"
	"path/filepath"
	"testing"
)

// A relative Root is handed to devmap as --project-root, and devmap now starts in
// the temp directory, so the name no longer means the repository.
func TestAttackInstallWithARelativeRootPassesAnAbsoluteProjectRoot(t *testing.T) {
	f := newAttackFixture(t)
	t.Chdir(f.dir)
	if _, err := New(f.fake, "repo").Integrate(context.Background(), "claude", InstallCheck, nil, ""); err != nil {
		t.Fatal(err)
	}
	args := f.args(t)
	for i, a := range args {
		if a == "--project-root" && !filepath.IsAbs(args[i+1]) {
			t.Fatalf("--project-root %q is relative and was run from the temp directory", args[i+1])
		}
	}
}

func TestAttackRemoveServersWithARelativeOnlyPathPassesAnAbsolutePath(t *testing.T) {
	f := newAttackFixture(t)
	t.Chdir(f.repo)
	if _, err := New(f.fake, f.repo).RemoveServers(context.Background(), "cursor", InstallCheck, nil, []string{".cursor/mcp.json"}); err != nil {
		t.Fatal(err)
	}
	args := f.args(t)
	for i, a := range args {
		if a == "--only-path" && !filepath.IsAbs(args[i+1]) {
			t.Fatalf("--only-path %q is relative and was run from the temp directory", args[i+1])
		}
	}
}
