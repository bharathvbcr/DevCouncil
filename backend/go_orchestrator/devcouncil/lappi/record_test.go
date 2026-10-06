package lappi

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"sync"
	"testing"
	"time"
)

// heldoutRoot is a store root under a literal `heldout` segment, as
// caller_record.rs check_store_path requires.
func heldoutRoot(t *testing.T) string {
	t.Helper()
	return filepath.Join(t.TempDir(), "heldout", "caller-records")
}

var fixedNow = time.Date(2026, 10, 6, 15, 20, 0, 0, time.UTC)

func newTestWriter(t *testing.T, root string) *Writer {
	t.Helper()
	w, err := NewWriter(root, &bytes.Buffer{})
	if err != nil {
		t.Fatal(err)
	}
	w.now = func() time.Time { return fixedNow }
	return w
}

func todayFile(root string) string {
	return filepath.Join(root, App, "2026-10-06.jsonl")
}

// The key sets caller_record.rs validate_line and check_lappi require
// (caller_record.rs:168-172, :261, :278). A record with one more or one fewer
// key is refused there, so it is pinned here.
var (
	decisionKeys  = []string{"record", "record_version", "kind", "record_id", "app", "app_version", "decision_point", "created_at", "admission", "facts", "app_choice", "lappi", "redaction"}
	lappiKeys     = []string{"asked", "task", "reading", "kind", "backend", "slots", "latency_ms"}
	redactionKeys = []string{"policy", "fields_redacted"}
	factKeys      = []string{"language", "hunks", "lines_added", "lines_deleted"}
	choiceKeys    = []string{"status", "blocking_gaps"}
	recordIDRe    = regexp.MustCompile(`^[0-9a-f]{32}$`)
	createdAtRe   = regexp.MustCompile(`^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$`)
)

func keysOf(m map[string]json.RawMessage) []string {
	out := make([]string, 0, len(m))
	for k := range m {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}

func sameKeys(t *testing.T, where string, m map[string]json.RawMessage, want []string) {
	t.Helper()
	w := append([]string(nil), want...)
	sort.Strings(w)
	if got := keysOf(m); strings.Join(got, ",") != strings.Join(w, ",") {
		t.Fatalf("%s keys %v, want %v", where, got, w)
	}
}

// checkDecisionLine re-derives caller_record.rs's rules for a decision line.
// It is a mirror, not the checker; TestRecordsPassTheRustChecker runs the
// checker itself when it is built.
func checkDecisionLine(t *testing.T, line []byte) map[string]json.RawMessage {
	t.Helper()
	if len(line) > MaxRecordBytes {
		t.Fatalf("line is %d bytes", len(line))
	}
	var top map[string]json.RawMessage
	if err := json.Unmarshal(line, &top); err != nil {
		t.Fatalf("not a JSON object: %v: %s", err, line)
	}
	sameKeys(t, "$", top, decisionKeys)
	str := func(k string) string {
		var s string
		if err := json.Unmarshal(top[k], &s); err != nil {
			t.Fatalf("$.%s is not a string: %s", k, top[k])
		}
		return s
	}
	if str("record") != "lappi.caller_record" || string(top["record_version"]) != "1" || str("kind") != "decision" ||
		str("app") != "devcouncil" || str("decision_point") != "code.defect_class" || str("admission") != "not_admitted" {
		t.Fatalf("envelope: %s", line)
	}
	if !recordIDRe.MatchString(str("record_id")) || !createdAtRe.MatchString(str("created_at")) || str("app_version") == "" {
		t.Fatalf("id/time/version: %s", line)
	}
	for k, want := range map[string][]string{"facts": factKeys, "app_choice": choiceKeys, "redaction": redactionKeys, "lappi": lappiKeys} {
		var obj map[string]json.RawMessage
		if err := json.Unmarshal(top[k], &obj); err != nil {
			t.Fatalf("$.%s: %v", k, err)
		}
		sameKeys(t, "$."+k, obj, want)
	}
	var red struct {
		Policy string `json:"policy"`
	}
	if err := json.Unmarshal(top["redaction"], &red); err != nil || red.Policy != RedactionPolicy {
		t.Fatalf("redaction: %s", top["redaction"])
	}
	var l struct {
		Asked     bool            `json:"asked"`
		Task      *string         `json:"task"`
		Reading   string          `json:"reading"`
		Kind      *string         `json:"kind"`
		Backend   *string         `json:"backend"`
		Slots     json.RawMessage `json:"slots"`
		LatencyMs *int64          `json:"latency_ms"`
	}
	if err := json.Unmarshal(top["lappi"], &l); err != nil {
		t.Fatal(err)
	}
	slots := string(l.Slots) != "null"
	if l.Asked != (l.Reading != "not_asked") || (l.Asked && l.Task == nil) {
		t.Fatalf("asked/task inconsistent: %s", top["lappi"])
	}
	switch l.Reading {
	case "model_answered", "model_abstained":
		if !slots || l.Backend == nil || l.Kind != nil {
			t.Fatalf("answer shape: %s", top["lappi"])
		}
	case "request_refused", "backend_failed", "unavailable":
		if l.Kind == nil || slots {
			t.Fatalf("failure shape: %s", top["lappi"])
		}
	case "not_asked":
		if l.Kind != nil || slots || l.Backend != nil || l.Task != nil || l.LatencyMs != nil {
			t.Fatalf("not-asked shape: %s", top["lappi"])
		}
	default:
		t.Fatalf("reading %q", l.Reading)
	}
	return top
}

func readLines(t *testing.T, path string) [][]byte {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if len(data) > 0 && data[len(data)-1] != '\n' {
		t.Fatalf("%s does not end in a newline", path)
	}
	return bytes.Split(bytes.TrimSuffix(data, []byte("\n")), []byte("\n"))
}

func TestTheWriterAppendsWholeLinesWithPrivateModes(t *testing.T) {
	root := heldoutRoot(t)
	w := newTestWriter(t, root)
	for _, line := range []string{`{"a":1}`, `{"b":2}`} {
		if err := w.Append([]byte(line)); err != nil {
			t.Fatal(err)
		}
	}
	lines := readLines(t, todayFile(root))
	if len(lines) != 2 || string(lines[0]) != `{"a":1}` || string(lines[1]) != `{"b":2}` {
		t.Fatalf("got %q", lines)
	}
	info, err := os.Stat(todayFile(root))
	if err != nil {
		t.Fatal(err)
	}
	if mode := info.Mode().Perm(); mode != 0o600 {
		t.Fatalf("file mode %o, want 600", mode)
	}
	for _, dir := range []string{filepath.Join(root, App), root} {
		info, err := os.Stat(dir)
		if err != nil {
			t.Fatal(err)
		}
		if mode := info.Mode().Perm(); mode != 0o700 {
			t.Fatalf("%s mode %o, want 700", dir, mode)
		}
	}
}

func TestTheWriterRefusesWhatTheContractRefuses(t *testing.T) {
	root := heldoutRoot(t)
	w := newTestWriter(t, root)
	if err := w.Append(bytes.Repeat([]byte("x"), MaxRecordBytes+1)); !errors.Is(err, ErrRecordTooLarge) {
		t.Fatalf("over 64 KiB: %v", err)
	}
	if err := w.Append(bytes.Repeat([]byte("x"), MaxRecordBytes)); err != nil {
		t.Fatalf("exactly 64 KiB is legal: %v", err)
	}
	if err := w.Append([]byte("a\nb")); err == nil {
		t.Fatal("an embedded newline is two lines")
	}
	if got := w.Dropped(); got != 2 {
		t.Fatalf("dropped %d, want 2", got)
	}
	if _, err := NewWriter(filepath.Join(t.TempDir(), "records"), nil); err == nil {
		t.Fatal("a root with no held-out segment must be refused")
	}
	if _, err := NewWriter("heldout/records", nil); err == nil {
		t.Fatal("a relative root must be refused")
	}
}

func TestRecordingStopsAtTheDayCap(t *testing.T) {
	root := heldoutRoot(t)
	w := newTestWriter(t, root)
	if err := w.Append([]byte(`{}`)); err != nil {
		t.Fatal(err)
	}
	// Sparse on APFS: no real megabytes are written.
	if err := os.Truncate(todayFile(root), MaxDayFileBytes); err != nil {
		t.Fatal(err)
	}
	if err := w.Append([]byte(`{}`)); !errors.Is(err, ErrDayFileFull) {
		t.Fatalf("got %v", err)
	}
	info, err := os.Stat(todayFile(root))
	if err != nil {
		t.Fatal(err)
	}
	if info.Size() != MaxDayFileBytes || w.Dropped() != 1 {
		t.Fatalf("size %d dropped %d", info.Size(), w.Dropped())
	}
}

func TestRecordingStopsAtTheStoreCap(t *testing.T) {
	root := heldoutRoot(t)
	w := newTestWriter(t, root)
	if err := w.Append([]byte(`{}`)); err != nil {
		t.Fatal(err)
	}
	old := filepath.Join(root, App, "2026-01-01.jsonl")
	f, err := os.OpenFile(old, os.O_CREATE|os.O_WRONLY, 0o600)
	if err != nil {
		t.Fatal(err)
	}
	if err := f.Close(); err != nil {
		t.Fatal(err)
	}
	if err := os.Truncate(old, MaxStoreBytes); err != nil {
		t.Fatal(err)
	}
	if err := w.Append([]byte(`{}`)); !errors.Is(err, ErrStoreFull) {
		t.Fatalf("got %v", err)
	}
	if w.Dropped() != 1 {
		t.Fatalf("dropped %d", w.Dropped())
	}
}

func TestAFailedWriteIsLoggedOnceAndCountedEveryTime(t *testing.T) {
	var log bytes.Buffer
	w, err := NewWriter(heldoutRoot(t), &log)
	if err != nil {
		t.Fatal(err)
	}
	for i := 0; i < 3; i++ {
		if err := w.Append(bytes.Repeat([]byte("x"), MaxRecordBytes+1)); err == nil {
			t.Fatal("accepted")
		}
	}
	if w.Dropped() != 3 || strings.Count(log.String(), "\n") != 1 {
		t.Fatalf("dropped %d, log %q", w.Dropped(), log.String())
	}
}

func TestConcurrentAppendsProduceWholeLinesOnly(t *testing.T) {
	root := heldoutRoot(t)
	writers := []*Writer{newTestWriter(t, root), newTestWriter(t, root)}
	const goroutines, each = 16, 25
	var wg sync.WaitGroup
	errs := make(chan error, goroutines*each)
	for g := 0; g < goroutines; g++ {
		wg.Add(1)
		go func(g int) {
			defer wg.Done()
			w := writers[g%len(writers)]
			for i := 0; i < each; i++ {
				line := fmt.Sprintf(`{"g":%d,"i":%d,"pad":%q}`, g, i, strings.Repeat("p", 2000+g))
				if err := w.Append([]byte(line)); err != nil {
					errs <- err
				}
			}
		}(g)
	}
	wg.Wait()
	close(errs)
	for err := range errs {
		t.Fatal(err)
	}
	lines := readLines(t, todayFile(root))
	if len(lines) != goroutines*each {
		t.Fatalf("%d lines, want %d", len(lines), goroutines*each)
	}
	seen := map[string]bool{}
	for _, line := range lines {
		var v struct {
			G, I int
			Pad  string
		}
		if err := json.Unmarshal(line, &v); err != nil || len(v.Pad) != 2000+v.G {
			t.Fatalf("a torn line: %.80q (%v)", line, err)
		}
		seen[fmt.Sprint(v.G, ".", v.I)] = true
	}
	if len(seen) != goroutines*each {
		t.Fatalf("%d distinct lines", len(seen))
	}
}

func TestDecisionLineRefusesAnInconsistentLappiBlock(t *testing.T) {
	id := strings.Repeat("ab", 16)
	slots := map[string]SlotAnswer{DefectClassSlot: {Noul: true}}
	for name, res := range map[string]Result{
		"zero_reading":        {},
		"answer_no_slots":     {Reading: ModelAnswered, Backend: "b"},
		"answer_no_backend":   {Reading: ModelAbstained, Slots: slots},
		"answer_with_kind":    {Reading: ModelAnswered, Backend: "b", Slots: slots, Kind: "x"},
		"refusal_no_kind":     {Reading: RequestRefused},
		"refusal_with_slots":  {Reading: RequestRefused, Kind: "x", Slots: slots},
		"unavailable_no_kind": {Reading: Unavailable},
		"not_asked_with_kind": {Reading: NotAsked, Kind: "x"},
	} {
		if _, err := DecisionLine(id, "0.2.4", fixedNow, Facts{}, AppChoice{}, res); err == nil {
			t.Errorf("%s: accepted", name)
		}
	}
	for _, bad := range []string{"", strings.Repeat("A", 32), strings.Repeat("a", 31), strings.Repeat("g", 32)} {
		if _, err := DecisionLine(bad, "0.2.4", fixedNow, Facts{}, AppChoice{}, notAsked("x")); err == nil {
			t.Errorf("record id %q accepted", bad)
		}
	}
}

func TestEveryReadingRendersAValidDecisionLine(t *testing.T) {
	slots := map[string]SlotAnswer{
		DefectClassSlot: {Value: json.RawMessage(`"logic"`), ConformalSet: []string{"logic"}, Score: 0.9},
		DefectSpanSlot:  {Score: 0.1, Noul: true},
	}
	for _, res := range []Result{
		{Reading: ModelAnswered, Backend: "qd-metal/x", Slots: slots, Latency: 140 * time.Millisecond},
		{Reading: ModelAbstained, Backend: "reference-deterministic-v1", Slots: slots},
		{Reading: RequestRefused, Kind: "calibration_entry_missing"},
		{Reading: BackendFailed, Kind: "deadline_exceeded"},
		{Reading: Unavailable, Kind: UnavailableSocketNotFound},
		notAsked(SkipBinary),
	} {
		line, err := DecisionLine(strings.Repeat("0f", 16), "0.2.4", fixedNow,
			Facts{Language: "go", Hunks: 2, LinesAdded: 4, LinesDeleted: 1},
			AppChoice{Status: "verified", BlockingGaps: 0}, res)
		if err != nil {
			t.Fatalf("%s: %v", res.Reading, err)
		}
		checkDecisionLine(t, line)
	}
}

func TestFromEnvironmentIsOffByDefaultAndHonoursTheOverride(t *testing.T) {
	home := shortDir(t)
	cases := []struct {
		env             map[string]string
		on, ask, record bool
	}{
		{map[string]string{"HOME": home}, false, false, false},
		{map[string]string{"HOME": home, AskEnv: "true"}, false, false, false},
		{map[string]string{"HOME": home, AskEnv: "1"}, true, true, false},
		{map[string]string{"HOME": home, CollectEnv: "1"}, true, false, true},
		{map[string]string{"HOME": home, CollectEnv: "1", CollectOverrideEnv: "0"}, false, false, false},
		{map[string]string{"HOME": home, AskEnv: "1", CollectEnv: "1", CollectOverrideEnv: "0"}, true, true, false},
		{map[string]string{"HOME": home, AskEnv: "1", CollectEnv: "1"}, true, true, true},
	}
	for i, tc := range cases {
		a, err := FromEnvironment(func(k string) string { return tc.env[k] }, nil)
		if err != nil {
			t.Fatalf("case %d: %v", i, err)
		}
		if (a != nil) != tc.on || a.Asking() != tc.ask || (a.Writer() != nil) != tc.record {
			t.Fatalf("case %d: on=%v ask=%v record=%v", i, a != nil, a.Asking(), a.Writer() != nil)
		}
	}
	if _, err := FromEnvironment(func(k string) string { return map[string]string{CollectEnv: "1"}[k] }, nil); err == nil {
		t.Fatal("collecting with no HOME must not fall back to a relative store")
	}
}

// diffOfFiles is n askable Go files.
func diffOfFiles(n int) string {
	var b strings.Builder
	for i := 0; i < n; i++ {
		fmt.Fprintf(&b, "diff --git a/f%d.go b/f%d.go\n--- a/f%d.go\n+++ b/f%d.go\n@@ -1 +1 @@\n-a\n+b\n", i, i, i, i)
	}
	return b.String()
}

func TestARunIsBoundedToSixteenFilesAndSaysHowManyThereWere(t *testing.T) {
	root := heldoutRoot(t)
	w := newTestWriter(t, root)
	a, err := NewAsker(nil, w)
	if err != nil {
		t.Fatal(err)
	}
	out := a.Run(context.Background(), diffOfFiles(20), AppChoice{Status: "verified"})
	if len(out.Decisions) != MaxFilesPerRun || out.TotalSections != 20 || out.RecordsWritten != MaxFilesPerRun {
		t.Fatalf("decisions %d total %d written %d", len(out.Decisions), out.TotalSections, out.RecordsWritten)
	}
	for _, d := range out.Decisions {
		if d.Result.Reading != NotAsked || d.Result.NotAskedReason != SkipAskingOff {
			t.Fatalf("collect-only must not ask: %+v", d.Result)
		}
	}
}

func TestARunAsksSequentiallyAndRecordsEveryFileWithoutItsText(t *testing.T) {
	agent := startAgent(t, reply(refused))
	client, err := NewClient(agent.path, 2*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	root := heldoutRoot(t)
	a, err := NewAsker(client, newTestWriter(t, root))
	if err != nil {
		t.Fatal(err)
	}
	a.now = func() time.Time { return time.Now() }
	out := a.Run(context.Background(), multiFileDiff, AppChoice{Status: "blocked", BlockingGaps: 2})
	if len(out.Decisions) != 7 || out.RecordsWritten != 7 || out.RecordsDropped != 0 {
		t.Fatalf("%+v", out)
	}
	wantReading := []Reading{RequestRefused, RequestRefused, NotAsked, NotAsked, NotAsked, NotAsked, RequestRefused}
	for i, d := range out.Decisions {
		if d.Result.Reading != wantReading[i] {
			t.Errorf("file %d: %s, want %s", i, d.Result.Reading, wantReading[i])
		}
	}
	if n := agent.accepts.Load(); n != 3 {
		t.Fatalf("the agent saw %d requests, want 3", n)
	}
	// The request carries the record's id as its example id.
	first := <-agent.lines
	if !bytes.Contains(first, []byte(`"example_id":"devcouncil:code.defect_class:`+out.Decisions[0].RecordID+`"`)) {
		t.Fatalf("example_id does not carry the record id: %s", first)
	}

	path := todayFile(root)
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	for _, leak := range []string{"src/add.go", "pkg/m.py", "my file", "old.rs", "img.png", "return a", "secret/path", "no calibration entry"} {
		if bytes.Contains(raw, []byte(leak)) {
			t.Fatalf("a caller record carries %q", leak)
		}
	}
	for i, line := range readLines(t, path) {
		top := checkDecisionLine(t, line)
		var choice AppChoice
		if err := json.Unmarshal(top["app_choice"], &choice); err != nil || choice != (AppChoice{Status: "blocked", BlockingGaps: 2}) {
			t.Fatalf("line %d app_choice %s", i, top["app_choice"])
		}
	}
}

func TestTheRunBudgetIsSharedAcrossFiles(t *testing.T) {
	agent := startAgent(t, func(conn net.Conn) { time.Sleep(2 * time.Second) })
	client, err := NewClient(agent.path, MaxExchangeTimeout)
	if err != nil {
		t.Fatal(err)
	}
	a, err := NewAsker(client, nil)
	if err != nil {
		t.Fatal(err)
	}
	a.budget = 300 * time.Millisecond
	start := time.Now()
	out := a.Run(context.Background(), diffOfFiles(4), AppChoice{})
	if elapsed := time.Since(start); elapsed > 1500*time.Millisecond {
		t.Fatalf("a 300 ms budget took %v", elapsed)
	}
	if out.Decisions[0].Result.Kind != UnavailableDeadline {
		t.Fatalf("first: %+v", out.Decisions[0].Result)
	}
	for _, d := range out.Decisions[1:] {
		if d.Result.Reading != NotAsked || d.Result.NotAskedReason != SkipBudgetSpent {
			t.Fatalf("after the budget: %+v", d.Result)
		}
	}
}

// TestRecordsPassTheRustChecker writes one record of every reading into a
// held-out store and runs Lappi's qd-caller-records over it when
// QD_CALLER_RECORDS_BIN names a build.
func TestRecordsPassTheRustChecker(t *testing.T) {
	root := heldoutRoot(t)
	w := newTestWriter(t, root)
	slots := map[string]SlotAnswer{
		DefectClassSlot: {Value: json.RawMessage(`"logic"`), ConformalSet: []string{"logic"}, Score: 0.91},
		DefectSpanSlot:  {Value: json.RawMessage(`{"start_line":4,"end_line":4}`), Score: 0.6},
	}
	results := []Result{
		{Reading: ModelAnswered, Backend: "qd-metal/qwen3.5-2b-base/tessl", Slots: slots, Latency: 140 * time.Millisecond},
		{Reading: ModelAbstained, Backend: "reference-deterministic-v1", Slots: slots},
		{Reading: RequestRefused, Kind: "calibration_entry_missing"},
		{Reading: BackendFailed, Kind: "deadline_exceeded"},
		{Reading: Unavailable, Kind: UnavailableDeadline},
		notAsked(SkipDeleted),
	}
	for _, res := range results {
		id, err := NewRecordID()
		if err != nil {
			t.Fatal(err)
		}
		line, err := DecisionLine(id, "0.2.4", fixedNow, Facts{Language: "go", Hunks: 1, LinesAdded: 1, LinesDeleted: 1},
			AppChoice{Status: "verified"}, res)
		if err != nil {
			t.Fatal(err)
		}
		if err := w.Append(line); err != nil {
			t.Fatal(err)
		}
	}
	for _, line := range readLines(t, todayFile(root)) {
		checkDecisionLine(t, line)
	}
	bin := os.Getenv("QD_CALLER_RECORDS_BIN")
	if bin == "" {
		t.Log("cross-check NOT RUN: QD_CALLER_RECORDS_BIN is unset (build Lappi-decision's qd-caller-records and point it here)")
		t.Skip("cross-check NOT RUN")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, bin, todayFile(root))
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	out, err := cmd.Output()
	if err != nil {
		t.Fatalf("qd-caller-records refused the store: %v\nstdout: %s\nstderr: %s", err, out, stderr.String())
	}
	var report struct {
		Lines     int            `json:"lines"`
		Decisions int            `json:"decisions"`
		Readings  map[string]int `json:"readings"`
	}
	if err := json.Unmarshal(out, &report); err != nil {
		t.Fatalf("checker report is not one JSON object: %v: %s", err, out)
	}
	if report.Decisions != len(results) || len(report.Readings) != len(results) {
		t.Fatalf("checker report %+v from %s", report, out)
	}
}
