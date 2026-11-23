package dc

import (
	"encoding/json"
	"testing"
)

func TestNormalizePlanPathIsTheOneStripRule(t *testing.T) {
	for in, want := range map[string]string{
		"src/a.go":     "src/a.go",
		"./src/a.go":   "src/a.go",
		"././src/a.go": "src/a.go",
		".\\src\\a.go": "src/a.go",
		"src\\a.go":    "src/a.go",
		"./secret/x":   "secret/x",
		".env":         ".env",
		"../x":         "../x",
		"":             "",
	} {
		if got := NormalizePlanPath(in); got != want {
			t.Errorf("NormalizePlanPath(%q) = %q, want %q", in, got, want)
		}
	}
	if NormalizePlanPaths(nil) != nil {
		t.Error("nil must stay nil")
	}
}

// Entries are spelled once, at decode, so no consumer has to re-spell them.
func TestPlannedFileDecodeNormalisesPaths(t *testing.T) {
	var files []PlannedFile
	body := `["./src/b.go", {"path":"./src/a.go","allowed_change":"read_only"}, {"path":"docs\\x.md"}]`
	if err := json.Unmarshal([]byte(body), &files); err != nil {
		t.Fatal(err)
	}
	want := []PlannedFile{
		{Path: "src/b.go", AllowedChange: ChangeModify},
		{Path: "src/a.go", AllowedChange: ChangeReadOnly},
		{Path: "docs/x.md", AllowedChange: ChangeModify},
	}
	for i := range want {
		if files[i] != want[i] {
			t.Errorf("entry %d = %+v, want %+v", i, files[i], want[i])
		}
	}
}
