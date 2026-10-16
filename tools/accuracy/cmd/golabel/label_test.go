package main

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// fixture is the Go accuracy fixture, whose expected edges were written before
// DevMap ran.
const fixture = "../../../../rust/testdata/accuracy/go"

// golden is the fixture's labels as committed; the Rust self-test
// (real_corpus_accuracy.rs) scores DevMap against the same file.
const golden = "../../../../rust/testdata/accuracy/go.labels.json"

// TestFixtureLabelsMatchTheCommittedFile keeps go.labels.json equal to what
// the labeller writes. GOLABEL_UPDATE=1 rewrites it.
func TestFixtureLabelsMatchTheCommittedFile(t *testing.T) {
	labels, err := Label(fixture)
	if err != nil {
		t.Fatal(err)
	}
	got, err := labels.Encode()
	if err != nil {
		t.Fatal(err)
	}
	if os.Getenv("GOLABEL_UPDATE") != "" {
		if err := os.WriteFile(golden, got, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(golden)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(got, want) {
		t.Fatalf("%s is not the labeller's output; regenerate with GOLABEL_UPDATE=1 go test ./... (and re-run the Rust self-test)", golden)
	}
}

// TestFixtureRowsCarryTheAuditVerdicts pins the shapes the audit scored,
// written from the fixture source rather than from the labeller's output.
func TestFixtureRowsCarryTheAuditVerdicts(t *testing.T) {
	labels, err := Label(fixture)
	if err != nil {
		t.Fatal(err)
	}
	type row struct{ construct, caller, truth string }
	want := []row{
		{"var x pkg.T", "app/svc/svc.go::Caller03", "app/store/store.go::Store.PtrGet03"},
		{"multi-value binding", "app/svc/probes.go::multiReturnCaller", "app/store/store.go::Store.ElemGet24"},
		{"method-result binding", "app/svc/probes2.go::methodReturnCaller", "app/store/store.go::Store.ElemGet24"},
		{"chain", "app/svc/svc.go::Caller26", "app/store/store.go::Store.Ctor25"},
		{"range var", "app/svc/svc.go::Caller16", "app/store/store.go::Store.RangeGet16"},
		{"type switch", "app/svc/svc.go::Caller17", "app/store/store.go::Circle.Area17"},
		{"element", "app/svc/svc.go::Service.Caller24", "app/store/store.go::Store.ElemGet24"},
		{"generic type", "app/svc/svc.go::Caller10", "app/store/generic.go::Box.Get10"},
		{"package-qualified", "app/svc/svc.go::Caller10", "app/store/generic.go::NewBox10"},
		{"promoted", "app/svc/svc.go::Caller08", "app/store/store.go::Base.Promoted08"},
		{"promoted", "app/svc/svc.go::Service.Caller09",
			"app/store/store.go::Logger.Log09|app/internal/secret/widen.go::FileSink.Log09|app/main.go::nopLogger.Log09"},
		{"interface dispatch", "app/svc/svc.go::Caller07",
			"app/store/store.go::Reader.Read07|app/store/store.go::FileReader.Read07|app/store/store.go::MemReader.Read07"},
		{"interface dispatch", "app/svc/svc.go::Service.Caller15", "app/svc/svc.go::Repo.Find15|app/svc/svc.go::SQLRepo.Find15"},
		{"bare same-package", "app/svc/probes2.go::Holder.Rebuild", "app/svc/probes2.go::Rebuild"},
		{"var x pkg.T", "app/svc/svc.go::Caller20", "libmod/lib.go::LibType20.LibMethod20"},
	}
	have := map[row]bool{}
	for _, r := range labels.Recall {
		if r.Language != "go" {
			t.Fatalf("row in language %q: %+v", r.Language, r)
		}
		have[row{r.Construct, r.Caller, r.Truth}] = true
	}
	for _, w := range want {
		if !have[w] {
			t.Errorf("missing recall row %+v", w)
		}
	}
	// No call reaches a function outside the corpus or a builtin: Q4's
	// `delete(...)` is the builtin, Q5's `Reset` is bytes.Buffer's.
	for _, r := range labels.Recall {
		if r.Caller == "app/svc/probes2.go::cache.delete" || r.Name == "Reset" {
			t.Errorf("row for a builtin or an external method: %+v", r)
		}
	}
	// A bare call inside a same-named method binds the package function,
	// never the method itself (Q3).
	for _, u := range labels.Oracle[0].Uses {
		if u.Caller == u.Callee {
			t.Errorf("self-use %+v", u)
		}
	}
}

// TestALoadErrorRefusesToLabel: a corpus that does not type-check is not
// labelled, so a partial label set cannot read as the whole corpus.
func TestALoadErrorRefusesToLabel(t *testing.T) {
	dir := t.TempDir()
	write := func(name, body string) {
		if err := os.WriteFile(filepath.Join(dir, name), []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	write("go.mod", "module example.com/broken\n\ngo 1.26\n")
	write("a.go", "package broken\n\nfunc F() int { return undefined() }\n")
	_, err := Label(dir)
	if err == nil || !strings.Contains(err.Error(), "package error") {
		t.Fatalf("want a package-error refusal, got %v", err)
	}
}

// TestNoModuleIsAnError: a root with no go.mod labels nothing, and says so.
func TestNoModuleIsAnError(t *testing.T) {
	if _, err := Label(t.TempDir()); err == nil || !strings.Contains(err.Error(), "no go.mod") {
		t.Fatalf("want a no-go.mod error, got %v", err)
	}
}

// TestDeadRowsAreMergedVerbatim: -dead rows reach the output unchanged.
func TestDeadRowsAreMergedVerbatim(t *testing.T) {
	dead := filepath.Join(t.TempDir(), "dead.json")
	if err := os.WriteFile(dead, []byte(`[{"language":"go","id":"app/svc/probes.go::deadPriv","truly_dead":true}]`), 0o644); err != nil {
		t.Fatal(err)
	}
	var out, errOut bytes.Buffer
	if code := run([]string{"-root", fixture, "-dead", dead}, &out, &errOut); code != 0 {
		t.Fatalf("exit %d: %s", code, errOut.String())
	}
	var labels Labels
	if err := json.Unmarshal(out.Bytes(), &labels); err != nil {
		t.Fatal(err)
	}
	if len(labels.Dead) != 1 || labels.Dead[0] != (DeadLabel{"go", "app/svc/probes.go::deadPriv", true}) {
		t.Fatalf("dead rows: %+v", labels.Dead)
	}
	if code := run(nil, &out, &errOut); code != 2 {
		t.Fatalf("no -root: exit %d, want 2", code)
	}
}
