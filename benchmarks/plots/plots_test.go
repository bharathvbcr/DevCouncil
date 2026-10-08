package main

import (
	"math"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
)

func TestParseLabel(t *testing.T) {
	cases := []struct {
		label                     string
		stage, repo, tool, target string
		ok                        bool
	}{
		{"cold-devmap-1", "cold", "", "devmap", "", true},
		{"cold-DevCouncil-devmap-2", "cold", "DevCouncil", "devmap", "", true},
		{"callers-cbm-db_size_gate_bytes-3", "callers", "", "cbm", "db_size_gate_bytes", true},
		{"touch-scholarlm-gitnexus-1", "touch", "scholarlm", "gitnexus", "", true},
		{"pilot-unknowntool", "", "", "", "", false},
	}
	for _, c := range cases {
		m, ok := parseLabel(c.label)
		if ok != c.ok || (ok && (m.Stage != c.stage || m.Repo != c.repo || m.Tool != c.tool || m.Target != c.target)) {
			t.Errorf("parseLabel(%q) = %+v, %v", c.label, m, ok)
		}
	}
}

func TestStat(t *testing.T) {
	if c := stat([]float64{3, 1, 2}); c.V != 1 || c.Med != 2 || c.Max != 3 {
		t.Errorf("odd: %+v", c)
	}
	if c := stat([]float64{4, 1, 3, 2}); c.V != 1 || c.Med != 2.5 || c.Max != 4 {
		t.Errorf("even: %+v", c)
	}
	if stat(nil).has() {
		t.Error("empty samples must be absent, not zero")
	}
}

// A synthetic or foreign corpus must never enter the DevCouncil history.
func TestSplitMapKeepsOnlyRealDevCouncilRuns(t *testing.T) {
	pts := []MapPoint{
		{Name: "a", Repo: "DevCouncil"},
		{Name: "b", Repo: "scholarlm"},
		{Name: "c", Repo: "DevCouncil", Synthetic: true},
		{Name: "d", Repo: "synthetic-2000", Synthetic: true},
	}
	hist, other := splitMap(pts)
	if len(hist) != 1 || hist[0].Name != "a" {
		t.Errorf("history = %+v", hist)
	}
	if len(other) != 3 {
		t.Errorf("other = %+v", other)
	}
}

func TestScoredRuns(t *testing.T) {
	run := func(name string, score map[string]float64) EffRun { return EffRun{Name: name, Score: score} }
	got := scoredRuns([]EffRun{
		run("both", map[string]float64{"A": 0.5, "B": 0.7}),
		run("zero", map[string]float64{"A": 0, "B": 0}),
		run("b-only", map[string]float64{"B": 1}),
		run("a-zero-b-not", map[string]float64{"A": 0, "B": 0.2}),
	})
	var names []string
	for _, r := range got {
		names = append(names, r.Name)
	}
	if strings.Join(names, ",") != "both,a-zero-b-not" {
		t.Errorf("kept %v", names)
	}
}

func TestBarChartRendersCleanSVG(t *testing.T) {
	for _, log := range []bool{false, true} {
		svg := barChart(BarOpts{
			Title: "t & <u>", Groups: []string{"g1", "g2"}, Series: []Series{{"a", "#111111"}, {"b", "#222222"}},
			Cells: [][]Cell{{{1, 2, 3}, single(0.5)}, {missing(), {10, 12, 40}}}, Log: log, Labels: true,
		})
		for _, bad := range []string{"NaN", "Inf", "<u>"} {
			if strings.Contains(svg, bad) {
				t.Errorf("log=%v: output contains %q", log, bad)
			}
		}
		if !strings.HasSuffix(strings.TrimSpace(svg), "</svg>") {
			t.Errorf("log=%v: unterminated svg", log)
		}
		if !strings.Contains(svg, `class="bg"`) || !strings.Contains(svg, "prefers-color-scheme: dark") {
			t.Errorf("log=%v: a standalone chart needs its own themed background", log)
		}
		if got := strings.Count(svg, "<rect") - 3; got != 3 { // background and two legend swatches; one cell is absent
			t.Errorf("log=%v: %d bars, want 3", log, got)
		}
	}
}

// Rotated labels are end-anchored, so a long label under the first group
// reaches left of the axis and used to be clipped.
func TestLabelPadWidensChartForLongLabels(t *testing.T) {
	if p := labelPad([]string{"a", "b"}, 400); p != 0 {
		t.Errorf("short labels need no pad, got %d", p)
	}
	long := "09-02 17:39 · scholarlm (3674 files)"
	if p := labelPad([]string{long, "b", "c", "d"}, 220); p <= 0 {
		t.Errorf("long first label must pad, got %d", p)
	}
	pad := labelPad([]string{long, "b", "c", "d"}, 220)
	svg := barChart(BarOpts{Groups: []string{long, "b", "c", "d"}, Series: []Series{{"s", "#111111"}}, Cells: [][]Cell{{single(1)}, {single(1)}, {single(1)}, {single(1)}}})
	if !strings.Contains(svg, `viewBox="0 0 `+strconv.Itoa(chartW+pad)+" ") {
		t.Errorf("viewBox must grow by %d: %.120s", pad, svg)
	}
}

// The same rotation hangs a long label below the axis; the chart must be tall
// enough to hold it or the label is cut off at the bottom.
func TestLabelDropGrowsChartHeight(t *testing.T) {
	short, long := []string{"cold", "warm"}, []string{"09-02 17:39 · scholarlm (3674 files)", "b"}
	if labelDrop(long) <= labelDrop(short) {
		t.Fatalf("drop long %d <= short %d", labelDrop(long), labelDrop(short))
	}
	h := strconv.Itoa(marginT + plotH + labelDrop(long))
	svg := barChart(BarOpts{Groups: long, Series: []Series{{"s", "#111111"}}, Cells: [][]Cell{{single(1)}, {single(1)}}})
	if !strings.Contains(svg, ` `+h+`" width=`) {
		t.Errorf("chart height must be %s: %.160s", h, svg)
	}
}

func TestLogAxisPlacesHigherValuesHigher(t *testing.T) {
	a := newAxis(true, 0.01, 100, 0)
	if !(a.y(1) < a.y(0.1) && a.y(0.1) < a.y(0.01)) {
		t.Errorf("log axis not monotonic: %v %v %v", a.y(1), a.y(0.1), a.y(0.01))
	}
	if math.Abs((a.y(0.1)-a.y(1))-(a.y(1)-a.y(10))) > 1e-6 {
		t.Error("decades must be equally spaced")
	}
}

func TestLineChartBreaksAtGaps(t *testing.T) {
	svg := lineChart(LineOpts{XLabels: []string{"a", "b", "c"}, Series: []Series{{"s", "#111111"}}, Vals: [][]float64{{1, math.NaN(), 3}}})
	if strings.Contains(svg, "NaN") || strings.Count(svg, "M") < 2 {
		t.Errorf("a NaN must lift the pen, not draw a segment: %s", svg)
	}
}

// Every committed result must load and render; a schema drift fails here
// instead of silently dropping a chart.
func TestRunAgainstCommittedResults(t *testing.T) {
	root := filepath.Join("..", "..")
	if _, err := os.Stat(filepath.Join(root, "benchmarks", "results")); err != nil {
		t.Skip("no committed results in this checkout")
	}
	out := t.TempDir()
	if err := run(root, out); err != nil {
		t.Fatal(err)
	}
	files, err := filepath.Glob(filepath.Join(out, "*.svg"))
	if err != nil || len(files) < 20 {
		t.Fatalf("%d charts, err %v", len(files), err)
	}
	for _, f := range files {
		raw, err := os.ReadFile(f)
		if err != nil {
			t.Fatal(err)
		}
		s := string(raw)
		if !strings.HasSuffix(strings.TrimSpace(s), "</svg>") || strings.Contains(s, "NaN") || strings.Contains(s, "Inf") {
			t.Errorf("%s malformed", filepath.Base(f))
		}
	}
}
