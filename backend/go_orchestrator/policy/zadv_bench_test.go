package policy

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
)

// Benchmarks of the write ladder on an ordinary, planned, existing file: the
// path every write evaluation takes. Identical file in the baseline checkout.
func benchRoot(b *testing.B) string {
	b.Helper()
	root, err := filepath.EvalSymlinks(b.TempDir())
	if err != nil {
		b.Fatal(err)
	}
	for _, f := range []string{"src/pkg/deep/a.go", "src/pkg/deep/b.go", "docs/readme.md"} {
		p := filepath.Join(root, f)
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			b.Fatal(err)
		}
		if err := os.WriteFile(p, []byte("x"), 0o644); err != nil {
			b.Fatal(err)
		}
	}
	return root
}

func benchEval(b *testing.B, hard bool, path string) {
	root := benchRoot(b)
	g := FileGate{Root: root, HardRules: hard}
	task := &dc.Task{ID: "T", PlannedFiles: []dc.PlannedFile{{Path: "src/**", AllowedChange: dc.ChangeModify}}}
	b.ReportAllocs()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		if d := g.EvaluateFileChange(path, task, dc.OpWrite, false); d.Blocked() {
			b.Fatalf("unexpected block: %+v", d)
		}
	}
}

func BenchmarkAdvEvaluateFileChangeHardExisting(b *testing.B) {
	benchEval(b, true, "src/pkg/deep/a.go")
}
func BenchmarkAdvEvaluateFileChangeYoloExisting(b *testing.B) {
	benchEval(b, false, "src/pkg/deep/a.go")
}
func BenchmarkAdvEvaluateFileChangeHardNew(b *testing.B) { benchEval(b, true, "src/pkg/deep/new.go") }
