package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
)

func readJSON(path string, v any) error {
	raw, err := os.ReadFile(path)
	if err != nil {
		return err
	}
	return json.Unmarshal(raw, v)
}

func readJSONL(path string, each func(json.RawMessage) error) error {
	f, err := os.Open(path)
	if err != nil {
		return err
	}
	defer f.Close()
	sc := bufio.NewScanner(f)
	sc.Buffer(make([]byte, 1<<20), 16<<20)
	for sc.Scan() {
		line := strings.TrimSpace(sc.Text())
		if line == "" {
			continue
		}
		if err := each(json.RawMessage(line)); err != nil {
			return err
		}
	}
	return sc.Err()
}

// stat reduces samples to the benchmark's headline triple: the minimum, with
// the median and maximum recorded beside it.
func stat(xs []float64) Cell {
	if len(xs) == 0 {
		return missing()
	}
	s := append([]float64(nil), xs...)
	sort.Float64s(s)
	med := s[len(s)/2]
	if len(s)%2 == 0 {
		med = (s[len(s)/2-1] + s[len(s)/2]) / 2
	}
	return Cell{s[0], med, s[len(s)-1]}
}

func mean(xs []float64) float64 {
	if len(xs) == 0 {
		return math.NaN()
	}
	t := 0.0
	for _, x := range xs {
		t += x
	}
	return t / float64(len(xs))
}

// ---- effectiveness (hidden-test score, A/B/C arms) -------------------------

type effArm struct {
	Seconds  float64  `json:"seconds"`
	Verdict  *string  `json:"verdict"`
	CostUSD  float64  `json:"cost_usd"`
	Fraction *float64 `json:"fraction"`
}

type effFile struct {
	Records []struct {
		Task string            `json:"task"`
		Arms map[string]effArm `json:"arms"`
	} `json:"records"`
}

// EffRun is one effectiveness run reduced to what the charts need.
type EffRun struct {
	Name     string
	Score    map[string]float64
	Scored   map[string]int
	Seconds  map[string][]float64
	Verdicts map[string]int
	CostB    float64
	Tasks    int
}

func loadEffectiveness(root string) ([]EffRun, error) {
	var paths []string
	for _, pat := range []string{"benchmarks/results/2026*.json", "benchmarks/results/diag/2026*.json"} {
		m, err := filepath.Glob(filepath.Join(root, pat))
		if err != nil {
			return nil, err
		}
		paths = append(paths, m...)
	}
	sort.Slice(paths, func(i, j int) bool { return filepath.Base(paths[i]) < filepath.Base(paths[j]) })
	var runs []EffRun
	for _, p := range paths {
		var f effFile
		if err := readJSON(p, &f); err != nil {
			return nil, fmt.Errorf("%s: %w", p, err)
		}
		if len(f.Records) == 0 {
			continue
		}
		r := EffRun{Name: runLabel(strings.TrimSuffix(filepath.Base(p), ".json")), Score: map[string]float64{}, Scored: map[string]int{}, Seconds: map[string][]float64{}, Verdicts: map[string]int{}, Tasks: len(f.Records)}
		sums := map[string]float64{}
		for _, rec := range f.Records {
			for arm, a := range rec.Arms {
				if a.Fraction != nil {
					sums[arm] += *a.Fraction
					r.Scored[arm]++
				}
				if a.Seconds > 0 {
					r.Seconds[arm] = append(r.Seconds[arm], a.Seconds)
				}
				if arm == "B" {
					r.CostB += a.CostUSD
					if a.Verdict != nil {
						r.Verdicts[*a.Verdict]++
					}
				}
			}
		}
		for arm, s := range sums {
			r.Score[arm] = s / float64(r.Scored[arm])
		}
		runs = append(runs, r)
	}
	return runs, nil
}

// runLabel turns 20260620T150003Z into "06-20 15:00".
func runLabel(stem string) string {
	if len(stem) >= 13 && stem[8] == 'T' {
		return fmt.Sprintf("%s-%s %s:%s", stem[4:6], stem[6:8], stem[9:11], stem[11:13])
	}
	if i := strings.LastIndex(stem, "_"); i >= 0 && len(stem)-i > 8 {
		return runLabel(stem[i+1:])
	}
	return stem
}

// ---- local monitor (acceptance-criterion verdict grid) ---------------------

type monitorFile struct {
	Samples int `json:"samples"`
	Tasks   map[string]struct {
		Arms map[string]map[string]struct {
			Verdict string `json:"verdict"`
		} `json:"arms"`
	} `json:"tasks"`
}

// MonitorRun counts how the local monitor judged acceptance criteria.
type MonitorRun struct {
	Name                    string
	Samples                 int
	RefProven, RefTotal     int
	BuggyFailed, BuggyTotal int
	RefSplit, BuggySplit    int
}

func loadMonitor(root string) ([]MonitorRun, error) {
	paths, err := filepath.Glob(filepath.Join(root, "benchmarks/results/local_monitor_*.json"))
	if err != nil {
		return nil, err
	}
	sort.Strings(paths)
	var out []MonitorRun
	for _, p := range paths {
		var f monitorFile
		if err := readJSON(p, &f); err != nil {
			return nil, fmt.Errorf("%s: %w", p, err)
		}
		r := MonitorRun{Name: runLabel(strings.TrimSuffix(filepath.Base(p), ".json")), Samples: f.Samples}
		for _, t := range f.Tasks {
			for arm, acs := range t.Arms {
				for _, ac := range acs {
					switch arm {
					case "reference":
						r.RefTotal++
						if ac.Verdict == "proven" {
							r.RefProven++
						}
						if ac.Verdict == "split" {
							r.RefSplit++
						}
					case "buggy":
						r.BuggyTotal++
						if ac.Verdict == "failed" {
							r.BuggyFailed++
						}
						if ac.Verdict == "split" {
							r.BuggySplit++
						}
					}
				}
			}
		}
		out = append(out, r)
	}
	return out, nil
}

// ---- map history (devmap stage timings over time) --------------------------

// MapPoint is one map run's cold/warm/touch timings on one corpus. Repo is the
// repository's directory name; Synthetic marks generated corpora, which are
// not any repository.
type MapPoint struct {
	Name, Repo, Label string
	Cold, Warm, Touch Cell
	Files             int
	Synthetic         bool
}

// MapRepo is one repository's timings from the cross-repository map run.
type MapRepo struct {
	Name              string
	Cold, Warm, Touch Cell
	Files, Symbols    int
}

func num(m map[string]any, k string) float64 {
	if v, ok := m[k].(float64); ok {
		return v
	}
	return math.NaN()
}

func sub(m map[string]any, k string) map[string]any {
	v, _ := m[k].(map[string]any)
	return v
}

func loadMap(root string) ([]MapPoint, []MapRepo, error) {
	paths, err := filepath.Glob(filepath.Join(root, "benchmarks/results/map/2026*.json"))
	if err != nil {
		return nil, nil, err
	}
	sort.Strings(paths)
	var pts []MapPoint
	var repos []MapRepo
	for _, p := range paths {
		var d map[string]any
		if err := readJSON(p, &d); err != nil {
			return nil, nil, fmt.Errorf("%s: %w", p, err)
		}
		name := runLabel(strings.TrimSuffix(filepath.Base(p), ".json"))
		if s := sub(d, "summary"); s != nil {
			// cross-repository schema
			names := make([]string, 0, len(s))
			for k := range s {
				names = append(names, k)
			}
			sort.Strings(names)
			for _, rn := range names {
				rm := sub(s, rn)
				cell := func(stage string) Cell {
					c := sub(rm, stage)
					return Cell{num(c, "min"), num(c, "median"), num(c, "max")}
				}
				g := sub(rm, "graph")
				repos = append(repos, MapRepo{Name: rn, Cold: cell("cold"), Warm: cell("warm"), Touch: cell("touch"), Files: int(num(g, "files_indexed")), Symbols: int(num(g, "symbols"))})
				if rn == "DevCouncil" {
					pts = append(pts, MapPoint{Name: name, Repo: rn, Cold: cell("cold"), Warm: cell("warm"), Touch: cell("touch"), Files: int(num(g, "files_indexed"))})
				}
			}
			continue
		}
		st := sub(d, "stages")
		if st == nil {
			continue
		}
		cell := func(stage string) Cell {
			c := sub(st, stage)
			return Cell{num(c, "min_s"), num(c, "median_s"), num(c, "max_s")}
		}
		repo, _ := d["repo"].(string)
		label, _ := d["label"].(string)
		kind, _ := d["corpus_kind"].(string)
		pts = append(pts, MapPoint{Name: name, Repo: filepath.Base(repo), Label: label, Cold: cell("cold"), Warm: cell("warm"), Touch: cell("touch"), Files: int(num(sub(d, "corpus"), "code_files")), Synthetic: kind == "synthetic"})
	}
	return pts, repos, nil
}

// ---- competition (code-graph tool comparison) ------------------------------

var toolOrder = []string{"devmap", "codegraph", "cbm", "gitnexus", "graphify", "gortex", "ripgrep"}

var toolColor = map[string]string{
	"devmap": "#2a78d6", "codegraph": "#eb6834", "cbm": "#1baf7a", "gitnexus": "#eda100",
	"graphify": "#e87ba4", "gortex": "#6250d6", "ripgrep": "#898781",
}

var toolSet = func() map[string]bool {
	m := map[string]bool{}
	for _, t := range toolOrder {
		m[t] = true
	}
	return m
}()

type measRow struct {
	Label    string  `json:"label"`
	Seconds  float64 `json:"seconds"`
	RSS      float64 `json:"rss_bytes"`
	TreeRSS  float64 `json:"sampled_tree_peak_rss_bytes"`
	Exit     int     `json:"exit_code"`
	TimedOut bool    `json:"timed_out"`
	OK       *bool   `json:"measurement_ok"`
}

// Meas is one successful timed command from a competition run.
type Meas struct {
	Stage, Repo, Tool, Target string
	Seconds, RSS              float64
}

func parseLabel(label string) (Meas, bool) {
	toks := strings.Split(label, "-")
	if n := len(toks); n > 1 {
		if _, err := strconv.Atoi(toks[n-1]); err == nil {
			toks = toks[:n-1]
		}
	}
	for i := 1; i < len(toks); i++ {
		if toolSet[toks[i]] {
			return Meas{Stage: toks[0], Repo: strings.Join(toks[1:i], "-"), Tool: toks[i], Target: strings.Join(toks[i+1:], "-")}, true
		}
	}
	return Meas{}, false
}

func loadMeasurements(path string) ([]Meas, error) {
	var out []Meas
	err := readJSONL(path, func(raw json.RawMessage) error {
		var r measRow
		if err := json.Unmarshal(raw, &r); err != nil {
			return err
		}
		if r.Exit != 0 || r.TimedOut || (r.OK != nil && !*r.OK) {
			return nil
		}
		m, ok := parseLabel(r.Label)
		if !ok {
			return nil
		}
		m.Seconds = r.Seconds
		m.RSS = math.Max(r.RSS, r.TreeRSS)
		out = append(out, m)
		return nil
	})
	return out, err
}

// CompRun is one directory under benchmarks/results/competition.
type CompRun struct {
	Dir, Title string
	Meas       []Meas
}

var compRuns = []CompRun{
	{Dir: "20260912-52d63a1a", Title: "2026-09-12 52d63a1a (DevMap vs GitNexus)"},
	{Dir: "20260912-expanded", Title: "2026-09-12 expanded (six tools, one repository)"},
	{Dir: "20260913-48cd3c7", Title: "2026-09-13 build 48cd3c7 (six tools, one repository)"},
	{Dir: "20260913-v0.2.1", Title: "2026-09-13 v0.2.1 (six tools, one repository)"},
	{Dir: "20260913-multirepo", Title: "2026-09-13 multirepo (five tools, four repositories)"},
	{Dir: "20260914-v0.2.2", Title: "2026-09-14 v0.2.2 (six tools, four repositories)"},
}

func loadCompetition(root string) ([]CompRun, error) {
	out := make([]CompRun, 0, len(compRuns))
	for _, r := range compRuns {
		ms, err := loadMeasurements(filepath.Join(root, "benchmarks/results/competition", r.Dir, "measurements.jsonl"))
		if err != nil {
			return nil, fmt.Errorf("%s: %w", r.Dir, err)
		}
		r.Meas = ms
		out = append(out, r)
	}
	return out, nil
}

// StageStat gathers the samples of one stage for one tool (and repo).
func (r CompRun) StageStat(stage, repo, tool string) Cell {
	var xs []float64
	for _, m := range r.Meas {
		if m.Stage == stage && m.Repo == repo && m.Tool == tool {
			xs = append(xs, m.Seconds)
		}
	}
	return stat(xs)
}

// QueryStat averages the per-target statistics of a query stage, so a stage
// measured against several symbols is one comparable number per tool.
func (r CompRun) QueryStat(stage, tool string) Cell {
	per := map[string][]float64{}
	for _, m := range r.Meas {
		if m.Stage == stage && m.Tool == tool {
			per[m.Target] = append(per[m.Target], m.Seconds)
		}
	}
	var mins, meds, maxs []float64
	for _, xs := range per {
		c := stat(xs)
		mins, meds, maxs = append(mins, c.V), append(meds, c.Med), append(maxs, c.Max)
	}
	if len(mins) == 0 {
		return missing()
	}
	return Cell{mean(mins), mean(meds), mean(maxs)}
}

func (r CompRun) Repos() []string {
	seen := map[string]bool{}
	var out []string
	for _, m := range r.Meas {
		if m.Repo != "" && !seen[m.Repo] {
			seen[m.Repo] = true
			out = append(out, m.Repo)
		}
	}
	sort.Strings(out)
	return out
}

func (r CompRun) Tools(stages ...string) []string {
	has := map[string]bool{}
	for _, m := range r.Meas {
		for _, s := range stages {
			if m.Stage == s {
				has[m.Tool] = true
			}
		}
	}
	var out []string
	for _, t := range toolOrder {
		if has[t] {
			out = append(out, t)
		}
	}
	return out
}

// ColdRSS is the peak resident set of a tool's cold index, in bytes.
func (r CompRun) ColdRSS(repo, tool string) Cell {
	var xs []float64
	for _, m := range r.Meas {
		if m.Stage == "cold" && m.Repo == repo && m.Tool == tool && m.RSS > 0 {
			xs = append(xs, m.RSS)
		}
	}
	if len(xs) == 0 {
		return missing()
	}
	c := stat(xs)
	return Cell{c.Med, absent, c.Max}
}

type v022Comparison struct {
	IndexBytes  map[string]map[string]float64 `json:"index_bytes"`
	GortexGates []struct {
		Repo  string  `json:"repo"`
		Ready float64 `json:"query_ready_seconds"`
	} `json:"gortex_gates"`
}

func loadV022(root string) (v022Comparison, error) {
	var c v022Comparison
	err := readJSON(filepath.Join(root, "benchmarks/results/competition/20260914-v0.2.2/comparison.json"), &c)
	return c, err
}

// ---- v0.2.1 -> v0.2.2 A/B ---------------------------------------------------

type abRow struct {
	Binary string  `json:"binary"`
	Repo   string  `json:"repo"`
	Cold   float64 `json:"cold"`
	Warm   float64 `json:"warm"`
	Touch  float64 `json:"touch"`
}

type queryRow struct {
	Binary  string  `json:"binary"`
	Op      string  `json:"op"`
	Seconds float64 `json:"seconds"`
}

func loadAB(root string) ([]abRow, []queryRow, error) {
	dir := filepath.Join(root, "benchmarks/results/competition/20260914-v0.2.2")
	var ab []abRow
	if err := readJSONL(filepath.Join(dir, "ab-measurements.jsonl"), func(raw json.RawMessage) error {
		var r abRow
		if err := json.Unmarshal(raw, &r); err != nil {
			return err
		}
		ab = append(ab, r)
		return nil
	}); err != nil {
		return nil, nil, err
	}
	var q []queryRow
	if err := readJSONL(filepath.Join(dir, "query-ab.jsonl"), func(raw json.RawMessage) error {
		var r queryRow
		if err := json.Unmarshal(raw, &r); err != nil {
			return err
		}
		q = append(q, r)
		return nil
	}); err != nil {
		return nil, nil, err
	}
	return ab, q, nil
}

// ---- build-cost A/Bs --------------------------------------------------------

type triple struct {
	Min    float64 `json:"min"`
	Median float64 `json:"median"`
	Max    float64 `json:"max"`
}

func (t triple) cell() Cell { return Cell{t.Min, t.Median, t.Max} }

type coldAB struct {
	Rounds int                          `json:"rounds"`
	Arms   map[string]map[string]triple `json:"arms"`
}

type teardownAB struct {
	Rounds int                          `json:"rounds"`
	Arms   map[string]map[string]triple `json:"arms"`
}

type editLatency struct {
	Repeat  int `json:"repeat"`
	Summary map[string]struct {
		N      int     `json:"n"`
		Min    float64 `json:"min_ms"`
		Median float64 `json:"median_ms"`
		Max    float64 `json:"max_ms"`
	} `json:"summary"`
}

func loadBuildCost(root string) (coldAB, teardownAB, editLatency, error) {
	dir := filepath.Join(root, "benchmarks/results/competition/20261007-build-cost")
	var c coldAB
	var t teardownAB
	var e editLatency
	if err := readJSON(filepath.Join(dir, "cold-index-ab/summary.json"), &c); err != nil {
		return c, t, e, err
	}
	if err := readJSON(filepath.Join(dir, "teardown-ab/summary.json"), &t); err != nil {
		return c, t, e, err
	}
	err := readJSON(filepath.Join(dir, "edit-latency/edit-latency.json"), &e)
	return c, t, e, err
}
