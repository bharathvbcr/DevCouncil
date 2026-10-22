package verify

import (
	"encoding/json"
	"os"
	"path/filepath"
	"runtime"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
)

func TestIsTestPathHoldsTheSharedFixture(t *testing.T) {
	_, file, _, _ := runtime.Caller(0)
	raw, err := os.ReadFile(filepath.Join(filepath.Dir(file), "..", "..", "..", "..", "protocols", "test-paths", "v1.json"))
	if err != nil {
		t.Fatal(err)
	}
	var fixture struct {
		Rows []struct {
			Path string `json:"path"`
			Test bool   `json:"test"`
		} `json:"rows"`
	}
	if err := json.Unmarshal(raw, &fixture); err != nil {
		t.Fatal(err)
	}
	if len(fixture.Rows) < 25 {
		t.Fatalf("only %d rows loaded", len(fixture.Rows))
	}
	for _, row := range fixture.Rows {
		if got := isTestPath(row.Path); got != row.Test {
			t.Errorf("isTestPath(%q) = %v, fixture says %v", row.Path, got, row.Test)
		}
	}
}

// An unplanned new test file is advisory whatever its ecosystem's spelling.
func TestUnplannedNewSpecFileIsAdvisoryNotBlocking(t *testing.T) {
	planned := []dc.PlannedFile{{Path: "src/a.ts", AllowedChange: dc.ChangeModify}}
	for _, path := range []string{"src/a.spec.ts", "spec/a_spec.rb", "web/e2e/login.ts", "src/FooTest.java"} {
		gaps := DetectOrphanDiffGaps("T", planned, []string{path}, map[string]struct{}{path: {}})
		if len(gaps) != 1 || gaps[0].Blocking {
			t.Errorf("%s: gaps %+v, want one advisory orphan", path, gaps)
		}
	}
	gaps := DetectOrphanDiffGaps("T", planned, []string{"src/b.ts"}, map[string]struct{}{"src/b.ts": {}})
	if len(gaps) != 1 || !gaps[0].Blocking {
		t.Errorf("an unplanned source file must stay blocking: %+v", gaps)
	}
}
