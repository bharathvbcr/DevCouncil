package verify

import (
	"bufio"
	"context"
	"encoding/json"
	"net"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/gating"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/lappi"
)

func answerSlots(value string) map[string]lappi.SlotAnswer {
	return map[string]lappi.SlotAnswer{
		lappi.DefectClassSlot: {Value: json.RawMessage(`"` + value + `"`), ConformalSet: []string{value}, Score: 0.91},
		lappi.DefectSpanSlot:  {Score: 0.2, Noul: true},
	}
}

// everyReading is one decision per reading Lappi can give, for distinct files.
func everyReading() []lappi.FileDecision {
	file := func(p string) lappi.FileDiff { return lappi.FileDiff{Path: p, Language: "go"} }
	return []lappi.FileDecision{
		{File: file("a.go"), Result: lappi.Result{Reading: lappi.ModelAnswered, Backend: "qd-metal/x", Slots: answerSlots("logic")}},
		{File: file("b.go"), Result: lappi.Result{Reading: lappi.ModelAnswered, Backend: "qd-metal/x", Slots: answerSlots("stub")}},
		{File: file("c.go"), Result: lappi.Result{Reading: lappi.ModelAnswered, Backend: "qd-metal/x", Slots: answerSlots("clean")}},
		{File: file("d.go"), Result: lappi.Result{Reading: lappi.ModelAbstained, Backend: "reference-deterministic-v1", Slots: answerSlots("stub")}},
		{File: file("e.go"), Result: lappi.Result{Reading: lappi.RequestRefused, Kind: "calibration_entry_missing"}},
		{File: file("f.go"), Result: lappi.Result{Reading: lappi.BackendFailed, Kind: "overloaded"}},
		{File: file("g.go"), Result: lappi.Result{Reading: lappi.Unavailable, Kind: lappi.UnavailableSocketNotFound}},
		{File: file("h.go"), Result: lappi.Result{Reading: lappi.NotAsked, NotAskedReason: lappi.SkipBinary}},
		// The same file answered twice still earns one gap.
		{File: file("a.go"), Result: lappi.Result{Reading: lappi.ModelAnswered, Backend: "qd-metal/x", Slots: answerSlots("cosmetic")}},
	}
}

func gap(gapType string, blocking bool, severity string) Gap {
	f := "src/x.go"
	return Gap{ID: StableGapID("T", gapType, f), Severity: severity, GapType: gapType, TaskID: "T",
		Description: gapType, Blocking: blocking, File: &f}
}

func TestOnlyANamedDefectFromAModelEarnsAnAdvisoryGap(t *testing.T) {
	gaps := lappiAdvisoryGaps("T", everyReading())
	if len(gaps) != 2 {
		t.Fatalf("got %d gaps, want 2 (a.go logic, b.go stub): %+v", len(gaps), gaps)
	}
	for i, want := range []string{"a.go", "b.go"} {
		g := gaps[i]
		if g.File == nil || *g.File != want || g.GapType != GapTypeLappiDefectClass {
			t.Fatalf("gap %d: %+v", i, g)
		}
		if g.Blocking || g.RequirementID != nil || g.AcceptanceCriterionID != nil || g.Severity != "low" {
			t.Fatalf("gap %d is not advisory: %+v", i, g)
		}
		// Not demotable, because it never blocks in the first place.
		if gating.MayDemote(g.GapType, g.Blocking) || !isAdmissibleLappiGap(g) {
			t.Fatalf("gap %d is in MayDemote's path", i)
		}
		if _, ranked := severityRank[g.Severity]; !ranked {
			t.Fatalf("severity %q is unranked and would sort above critical", g.Severity)
		}
		if !strings.Contains(g.Evidence[0], "score=0.910") || !strings.Contains(g.Evidence[0], "backend=qd-metal/x") {
			t.Fatalf("evidence %q", g.Evidence)
		}
	}
	if !strings.Contains(gaps[0].Evidence[0], "defect_class=logic") {
		t.Fatalf("first answer wins for a file: %q", gaps[0].Evidence)
	}
}

func TestAnInadmissibleLappiGapIsDroppedNotCoerced(t *testing.T) {
	req, ac := "REQ-1", "AC-1"
	good := lappiAdvisoryGaps("T", everyReading()[:1])[0]
	blocking := good
	blocking.Blocking = true
	withReq := good
	withReq.RequirementID = &req
	withAC := good
	withAC.AcceptanceCriterionID = &ac
	foreign := good
	foreign.GapType = "test_failed"
	got := appendLappiAdvisory(nil, []Gap{blocking, withReq, withAC, foreign, good})
	if len(got) != 1 || !reflect.DeepEqual(got[0], good) {
		t.Fatalf("got %+v", got)
	}
}

// TestLappiNeverChangesAVerifyStatus is the admission rule as a table: for
// every gap set and every gate mode, adding Lappi's gaps changes neither the
// status nor any existing gap.
func TestLappiNeverChangesAVerifyStatus(t *testing.T) {
	lappiGaps := lappiAdvisoryGaps("T", everyReading())
	if len(lappiGaps) == 0 {
		t.Fatal("the table needs Lappi gaps to add")
	}
	// A hand-built blocking Lappi gap rides along; the guard must drop it.
	rogue := lappiGaps[0]
	rogue.Blocking = true
	rogue.Description = "rogue"
	adding := append(append([]Gap(nil), lappiGaps...), rogue)

	sets := map[string][]Gap{
		"empty":                {},
		"advisory_only":        {gap("missing_test", false, "medium")},
		"one_soft_blocking":    {gap("test_failed", true, "high")},
		"one_hard_blocking":    {gap("stub_detected", true, "high")},
		"all_blocking":         {gap("test_failed", true, "high"), gap("stub_detected", true, "critical"), gap("orphan_diff", true, "high"), gap("rigor_check_unavailable", true, "high")},
		"all_blocking_soft":    {gap("test_failed", true, "high"), gap("missing_test", true, "medium")},
		"mixed":                {gap("test_failed", true, "high"), gap("missing_test", false, "low"), gap("security_risk", true, "critical")},
		"same_file_as_lappi":   {func() Gap { g := gap("test_failed", true, "high"); f := "a.go"; g.File = &f; return g }()},
		"already_has_advisory": {lappiGaps[0]},
	}
	modes := []string{"", "off", "advisory", "warn", "enforce", "true", "ENFORCE", "bogus"}
	for name, set := range sets {
		for _, mode := range modes {
			before := NormalizeGaps(append([]Gap(nil), set...))
			after := NormalizeGaps(appendLappiAdvisory(append([]Gap(nil), set...), adding))
			s1, p1 := StatusFromGaps(before, mode)
			s2, p2 := StatusFromGaps(after, mode)
			if s1 != s2 || p1 != p2 {
				t.Fatalf("%s/%q: status %s/%v became %s/%v", name, mode, s1, p1, s2, p2)
			}
			// Every gap that was there is still there, unchanged.
			var rest []Gap
			for _, g := range after {
				if g.Blocking && strings.HasPrefix(g.GapType, LappiAdvisoryPrefix) {
					t.Fatalf("%s/%q: a blocking Lappi gap got through", name, mode)
				}
				if !strings.HasPrefix(g.GapType, LappiAdvisoryPrefix) {
					rest = append(rest, g)
				}
			}
			var beforeRest []Gap
			for _, g := range before {
				if !strings.HasPrefix(g.GapType, LappiAdvisoryPrefix) {
					beforeRest = append(beforeRest, g)
				}
			}
			if !reflect.DeepEqual(rest, beforeRest) {
				t.Fatalf("%s/%q: existing gaps changed:\n%+v\n%+v", name, mode, beforeRest, rest)
			}
			blocking := func(gs []Gap) int {
				n := 0
				for _, g := range gs {
					if g.Blocking {
						n++
					}
				}
				return n
			}
			if blocking(before) != blocking(after) {
				t.Fatalf("%s/%q: blocking count changed", name, mode)
			}
		}
	}
}

// startLappiAgent serves reply to every request on a short socket path.
func startLappiAgent(t *testing.T, reply string) string {
	t.Helper()
	dir, err := os.MkdirTemp("", "qd")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := os.RemoveAll(dir); err != nil {
			t.Errorf("cleanup: %v", err)
		}
	})
	path := filepath.Join(dir, "a.sock")
	l, err := net.Listen("unix", path)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := l.Close(); err != nil {
			t.Errorf("close: %v", err)
		}
	})
	go func() {
		for {
			conn, err := l.Accept()
			if err != nil {
				return
			}
			go func() {
				defer conn.Close()
				if _, err := bufio.NewReader(conn).ReadBytes('\n'); err != nil {
					return
				}
				_, _ = conn.Write([]byte(reply + "\n"))
			}()
		}
	}()
	return path
}

const refusedReply = `{"status":"refused","schema_version":1,"refusal":{"kind":"calibration_entry_missing","slot":"defect_span","slot_type":"span","rows":0},"message":"no calibration entry"}`

const logicReply = `{"status":"ok","schema_version":1,"backend":"qd-metal/qwen3.5-2b-base/tessl","degraded":false,"slots":{` +
	`"defect_class":{"value":"logic","conformal_set":["logic"],"score":0.91,"noul":false,"degraded":false},` +
	`"defect_span":{"value":{"start_line":4,"end_line":4},"conformal_set":null,"score":0.6,"noul":false,"degraded":false}}}`

const runDiff = "diff --git a/src/a.go b/src/a.go\n--- a/src/a.go\n+++ b/src/a.go\n@@ -1 +1 @@\n-a\n+b\n" +
	"diff --git a/img.png b/img.png\nBinary files a/img.png and b/img.png differ\n"

func runInput(asker *lappi.Asker) Input {
	return Input{
		Task: &store.Task{ID: "T-LAPPI", PlannedFiles: []dc.PlannedFile{
			{Path: "src/a.go", AllowedChange: dc.ChangeModify},
		}},
		GateMode:     "enforce",
		ChangedFiles: []string{"src/a.go", "img.png"},
		DiffContent:  runDiff,
		WorkPresent:  true,
		Lappi:        asker,
	}
}

func askerFor(t *testing.T, socket string) *lappi.Asker {
	t.Helper()
	client, err := lappi.NewClient(socket, 2*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	a, err := lappi.NewAsker(client, nil)
	if err != nil {
		t.Fatal(err)
	}
	return a
}

// TestTodaysRefusalLeavesAVerifyRunExactlyAsItWas: the v0.1 preview refuses
// code.defect_class, so turning asking on must change nothing.
func TestTodaysRefusalLeavesAVerifyRunExactlyAsItWas(t *testing.T) {
	ctx := context.Background()
	baseGaps, baseMeta := Run(ctx, runInput(nil))
	gaps, meta := Run(ctx, runInput(askerFor(t, startLappiAgent(t, refusedReply))))
	if !reflect.DeepEqual(baseGaps, gaps) || !reflect.DeepEqual(baseMeta, meta) {
		t.Fatalf("a refused ask changed the run:\n%+v\n%+v", baseGaps, gaps)
	}
	// No agent at all: also unchanged.
	gone := filepath.Join(os.TempDir(), "qd-none.sock")
	gaps, meta = Run(ctx, runInput(askerFor(t, gone)))
	if !reflect.DeepEqual(baseGaps, gaps) || !reflect.DeepEqual(baseMeta, meta) {
		t.Fatal("an absent agent changed the run")
	}
}

func TestAModelAnswerAddsOneAdvisoryGapAndNoStatusChange(t *testing.T) {
	ctx := context.Background()
	baseGaps, baseMeta := Run(ctx, runInput(nil))
	gaps, meta := Run(ctx, runInput(askerFor(t, startLappiAgent(t, logicReply))))
	if !reflect.DeepEqual(baseMeta, meta) {
		t.Fatal("meta changed")
	}
	var advisory []Gap
	for _, g := range gaps {
		if strings.HasPrefix(g.GapType, LappiAdvisoryPrefix) {
			advisory = append(advisory, g)
		}
	}
	if len(advisory) != 1 || *advisory[0].File != "src/a.go" || len(gaps) != len(baseGaps)+1 {
		t.Fatalf("want one advisory gap for src/a.go (the binary file is not asked): %+v", advisory)
	}
	s1, p1 := StatusFromGaps(baseGaps, "enforce")
	s2, p2 := StatusFromGaps(gaps, "enforce")
	if s1 != s2 || p1 != p2 {
		t.Fatalf("status %s/%v became %s/%v", s1, p1, s2, p2)
	}
	res := ToMCP("T-LAPPI", gaps, meta)
	for _, b := range res.BlockingGaps {
		if strings.HasPrefix(b.GapType, LappiAdvisoryPrefix) {
			t.Fatal("a Lappi gap is listed as blocking")
		}
	}
	for _, a := range res.NextActions {
		if strings.HasPrefix(a.GapType, LappiAdvisoryPrefix) {
			t.Fatal("a Lappi gap became a blocking next action")
		}
	}
}

func TestLappiIsNotAskedWhenTheSettingIsOff(t *testing.T) {
	t.Setenv(lappi.AskEnv, "")
	t.Setenv(lappi.CollectEnv, "")
	if LappiAsker() != nil {
		t.Fatal("Lappi must be off by default")
	}
}
