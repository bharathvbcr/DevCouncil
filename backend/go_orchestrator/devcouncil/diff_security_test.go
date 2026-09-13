package devcouncil_test

import (
	"context"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
)

func TestUntrackedDiffPreservesNativeFilenames(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("backslash is a Windows separator")
	}
	root := newRepo(t)
	for _, name := range []string{`src\a.py`, " padded name.txt "} {
		write(t, root, name, "native filename content\n")
	}
	got, err := devcouncil.GetDiff(context.Background(), root, devcouncil.GetDiffArgs{})
	if err != nil {
		t.Fatal(err)
	}
	diff := mustDiff(t, got)
	if !diff.OK || strings.Count(diff.UnifiedDiff, "+native filename content") != 2 || strings.Contains(diff.UnifiedDiff, "+VALUE = 1") {
		t.Fatalf("native filenames were reinterpreted: %+v", diff)
	}
}

func TestUntrackedDiffRefusesOutsideAndSecretReads(t *testing.T) {
	for _, kind := range []string{"external symlink", "backslash traversal", "secret name"} {
		t.Run(kind, func(t *testing.T) {
			if kind == "backslash traversal" && runtime.GOOS == "windows" {
				t.Skip("backslash is a Windows separator")
			}
			root := newRepo(t)
			outside := t.TempDir()
			write(t, outside, "outside.txt", "synthetic external sentinel\n")
			switch kind {
			case "external symlink":
				if err := os.Symlink(filepath.Join(outside, "outside.txt"), filepath.Join(root, "link.txt")); err != nil {
					t.Fatal(err)
				}
			case "backslash traversal":
				rel, err := filepath.Rel(root, filepath.Join(outside, "outside.txt"))
				if err != nil {
					t.Fatal(err)
				}
				write(t, root, strings.ReplaceAll(rel, "/", `\`), "ordinary native filename\n")
			case "secret name":
				write(t, root, "secrets/synthetic.txt", "synthetic external sentinel\n")
			}
			got, err := devcouncil.GetDiff(context.Background(), root, devcouncil.GetDiffArgs{})
			if err != nil {
				t.Fatal(err)
			}
			diff := mustDiff(t, got)
			if diff.OK || diff.Error == "" || strings.Contains(diff.UnifiedDiff, "synthetic external sentinel") {
				t.Fatalf("unsafe untracked read was not refused: %+v", diff)
			}
		})
	}
}

func TestUntrackedDiffBoundsInputBeforeReading(t *testing.T) {
	root := newRepo(t)
	f, err := os.Create(filepath.Join(root, "oversize.txt"))
	if err != nil {
		t.Fatal(err)
	}
	if err := f.Truncate(8*1024*1024 + 1); err != nil {
		t.Fatal(err)
	}
	if err := f.Close(); err != nil {
		t.Fatal(err)
	}
	got, err := devcouncil.GetDiff(context.Background(), root, devcouncil.GetDiffArgs{})
	if err != nil {
		t.Fatal(err)
	}
	diff := mustDiff(t, got)
	if diff.OK || !strings.Contains(diff.Error, "limit") {
		t.Fatalf("oversize untracked file was not explicitly refused: %+v", diff)
	}
}
