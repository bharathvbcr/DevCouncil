// Command plots renders every committed benchmark result under
// benchmarks/results as a standalone SVG chart, so the docs show the shape of
// the data and each plot can be regenerated from the JSON it came from.
//
//	GOWORK=off go run ./benchmarks/plots -root . -out docs/assets/benchmarks
//
// It reads only; it never runs a benchmark. Timing charts plot each
// benchmark's own headline statistic, the minimum of interleaved repeats, with
// a tick at the median and a whisker to the maximum.
package main

import (
	"flag"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"sort"
)

func main() {
	root := flag.String("root", ".", "repository root")
	out := flag.String("out", "docs/assets/benchmarks", "output directory, relative to -root")
	flag.Parse()
	if err := run(*root, filepath.Join(*root, *out)); err != nil {
		fmt.Fprintln(os.Stderr, "plots:", err)
		os.Exit(1)
	}
}

type writer struct {
	dir   string
	count int
}

func (w *writer) write(name, svg string) error {
	w.count++
	fmt.Println(name)
	return os.WriteFile(filepath.Join(w.dir, name), []byte(svg), 0o644)
}

func run(root, out string) error {
	if err := os.MkdirAll(out, 0o755); err != nil {
		return err
	}
	w := &writer{dir: out}
	steps := []func(string, *writer) error{effectiveness, localMonitor, mapHistory, competition, competitionDrift, v022Detail, abCompare, buildCost}
	for _, s := range steps {
		if err := s(root, w); err != nil {
			return err
		}
	}
	fmt.Printf("%d charts written to %s\n", w.count, out)
	return nil
}

func seconds(v float64) string {
	if v >= 100 {
		return fmt.Sprintf("%.0fs", v)
	}
	if v >= 1 {
		return fmt.Sprintf("%.3gs", v)
	}
	if v >= 0.001 {
		return fmt.Sprintf("%.3gs", v)
	}
	return fmt.Sprintf("%.2gs", v)
}

func ms(v float64) string {
	if v >= 1000 {
		return fmt.Sprintf("%.3gs", v/1000)
	}
	return fmt.Sprintf("%.3gms", v)
}

func mb(v float64) string {
	if v >= 1e9 {
		return fmt.Sprintf("%.1f GB", v/1e9)
	}
	return fmt.Sprintf("%.0f MB", v/1e6)
}

func effectiveness(root string, w *writer) error {
	runs, err := loadEffectiveness(root)
	if err != nil {
		return err
	}
	kept := scoredRuns(runs)
	labels := make([]string, len(kept))
	for i, r := range kept {
		labels[i] = fmt.Sprintf("%s (n=%d)", r.Name, r.Scored["B"])
	}
	arms := []Series{{"A: raw agent", "#898781"}, {"B: DevCouncil gated loop", "#2a78d6"}, {"C: raw agent + elaborated spec", "#1baf7a"}}
	letters := []string{"A", "B", "C"}
	cells := make([][]Cell, len(kept))
	times := make([][]Cell, len(kept))
	for i, r := range kept {
		cells[i] = make([]Cell, 3)
		times[i] = make([]Cell, 3)
		for j, l := range letters {
			if v, ok := r.Score[l]; ok {
				cells[i][j] = single(v)
			} else {
				cells[i][j] = missing()
			}
			if xs := r.Seconds[l]; len(xs) > 0 {
				c := stat(xs)
				times[i][j] = Cell{c.Med, absent, c.Max}
			} else {
				times[i][j] = missing()
			}
		}
	}
	if err := w.write("effectiveness-score.svg", barChart(BarOpts{
		Title: "Effectiveness: hidden-test score per run", Sub: "Mean fraction of hidden ground-truth checks passed; only runs with both arm A and arm B (n = tasks).",
		YLabel: "mean hidden-test pass fraction", Groups: labels, Series: arms, Cells: cells, YMax: 1, Fmt: fmtPct, Labels: true})); err != nil {
		return err
	}
	if err := w.write("effectiveness-wall-time.svg", barChart(BarOpts{
		Title: "Effectiveness: wall time per task", Sub: "Median seconds per task (whisker: slowest task). Log scale: the gated loop plans, executes and verifies.",
		YLabel: "seconds per task", Groups: labels, Series: arms, Cells: times, Log: true, Fmt: seconds})); err != nil {
		return err
	}
	vs := []Series{{"passed", "#1baf7a"}, {"incomplete", "#eda100"}, {"blocked", "#e34948"}, {"error", "#898781"}}
	var vruns []EffRun
	for _, r := range runs {
		if len(r.Verdicts) > 0 {
			vruns = append(vruns, r)
		}
	}
	vl := make([]string, len(vruns))
	vc := make([][]Cell, len(vruns))
	for i, r := range vruns {
		vl[i] = fmt.Sprintf("%s (n=%d)", r.Name, r.Tasks)
		vc[i] = make([]Cell, len(vs))
		for j, s := range vs {
			vc[i][j] = single(float64(r.Verdicts[s.Name]))
		}
	}
	if err := w.write("effectiveness-verdicts.svg", barChart(BarOpts{
		Title: "Effectiveness: DevCouncil verdicts per run", Sub: "Arm B verdict counts, all runs that have an arm B (including runs with no baseline arm).",
		YLabel: "tasks", Groups: vl, Series: vs, Cells: vc, Fmt: func(v float64) string { return fmt.Sprintf("%.0f", v) }, Labels: true})); err != nil {
		return err
	}
	cost := make([][]Cell, len(vruns))
	for i, r := range vruns {
		cost[i] = []Cell{single(r.CostB)}
	}
	return w.write("effectiveness-cost.svg", barChart(BarOpts{
		Title: "Effectiveness: planning cost of arm B", Sub: "Summed cost_usd of the gated loop per run (arm A and C run on the executor's own plan).",
		YLabel: "USD per run", Groups: vl, Series: []Series{{"arm B cost", "#2a78d6"}}, Cells: cost,
		Fmt: func(v float64) string { return fmt.Sprintf("$%.2f", v) }, Labels: true}))
}

func localMonitor(root string, w *writer) error {
	runs, err := loadMonitor(root)
	if err != nil {
		return err
	}
	labels := make([]string, len(runs))
	cells := make([][]Cell, len(runs))
	for i, r := range runs {
		labels[i] = fmt.Sprintf("%s (samples=%d)", r.Name, r.Samples)
		cells[i] = []Cell{single(float64(r.RefProven) / float64(r.RefTotal)), single(float64(r.BuggyFailed) / float64(r.BuggyTotal))}
	}
	return w.write("local-monitor.svg", barChart(BarOpts{
		Title: "Local monitor: do acceptance checks discriminate?", Sub: "Share of criteria judged proven on the correct reference, and judged failed on the deliberately buggy variant. Split verdicts count as neither.",
		YLabel: "share of acceptance criteria", Groups: labels,
		Series: []Series{{"reference proven", "#1baf7a"}, {"buggy failed", "#e34948"}}, Cells: cells, YMax: 1, Fmt: fmtPct, Labels: true}))
}

// splitMap separates runs on the real DevCouncil corpus, which form a history,
// from runs on other repositories or generated corpora, which do not.
func splitMap(pts []MapPoint) (history, other []MapPoint) {
	for _, p := range pts {
		if p.Repo == "DevCouncil" && !p.Synthetic {
			history = append(history, p)
		} else {
			other = append(other, p)
		}
	}
	return history, other
}

// scoredRuns keeps the runs whose score chart is meaningful: both the raw
// agent (A) and DevCouncil (B) were scored, and not at zero on both, which marks
// a broken run rather than a measurement.
func scoredRuns(runs []EffRun) []EffRun {
	var kept []EffRun
	for _, r := range runs {
		_, hasA := r.Score["A"]
		_, hasB := r.Score["B"]
		if hasA && hasB && (r.Score["A"] != 0 || r.Score["B"] != 0) {
			kept = append(kept, r)
		}
	}
	return kept
}

func mapHistory(root string, w *writer) error {
	pts, repos, err := loadMap(root)
	if err != nil {
		return err
	}
	hist, scaling := splitMap(pts)
	labels := make([]string, len(hist))
	vals := [][]float64{make([]float64, len(hist)), make([]float64, len(hist)), make([]float64, len(hist))}
	lo, hi := math.MaxInt, 0
	for i, p := range hist {
		labels[i] = p.Name
		vals[0][i], vals[1][i], vals[2][i] = p.Cold.V, p.Warm.V, p.Touch.V
		lo, hi = min(lo, p.Files), max(hi, p.Files)
	}
	if err := w.write("map-history.svg", lineChart(LineOpts{
		Title: "devmap map performance over time (DevCouncil repository)", Sub: fmt.Sprintf("Minimum of repeats per run, real DevCouncil corpus only (%d to %d files as each run counted them). The 09-02 runs are optimization passes.", lo, hi),
		YLabel: "seconds (log)", XLabels: labels, Series: []Series{{"cold index", "#2a78d6"}, {"warm refresh", "#1baf7a"}, {"one-file touch", "#eb6834"}},
		Vals: vals, Log: true, Fmt: seconds})); err != nil {
		return err
	}
	sg := make([]string, len(scaling))
	sc := make([][]Cell, len(scaling))
	for i, p := range scaling {
		what := p.Repo
		if p.Synthetic {
			what = "synthetic corpus"
		}
		sg[i] = fmt.Sprintf("%s · %s (%d files)", p.Name, what, p.Files)
		sc[i] = []Cell{p.Cold, p.Warm, p.Touch}
	}
	if err := w.write("map-scaling.svg", barChart(BarOpts{
		Title: "devmap map performance on other corpora", Sub: "The scholarlm repository and generated synthetic corpora, from results/map/. Minimum (bar), median (tick), maximum (whisker).",
		YLabel: "seconds (log)", Groups: sg, Series: []Series{{"cold index", "#2a78d6"}, {"warm refresh", "#1baf7a"}, {"one-file touch", "#eb6834"}},
		Cells: sc, Log: true, Fmt: seconds, Labels: true})); err != nil {
		return err
	}
	groups := make([]string, len(repos))
	cells := make([][]Cell, len(repos))
	for i, r := range repos {
		groups[i] = fmt.Sprintf("%s (%d files)", r.Name, r.Files)
		cells[i] = []Cell{r.Cold, r.Warm, r.Touch}
	}
	return w.write("map-repositories.svg", barChart(BarOpts{
		Title: "devmap across four repositories (2026-09-13)", Sub: "Minimum of 5 repeats (bar), median (tick) and maximum (whisker), on a loaded machine.",
		YLabel: "seconds (log)", Groups: groups, Series: []Series{{"cold index", "#2a78d6"}, {"warm refresh", "#1baf7a"}, {"one-file touch", "#eb6834"}},
		Cells: cells, Log: true, Fmt: seconds, Labels: true}))
}

func toolSeries(tools []string) []Series {
	s := make([]Series, len(tools))
	for i, t := range tools {
		s[i] = Series{t, toolColor[t]}
	}
	return s
}

func competition(root string, w *writer) error {
	runs, err := loadCompetition(root)
	if err != nil {
		return err
	}
	cmp, err := loadV022(root)
	if err != nil {
		return err
	}
	for _, r := range runs {
		stages := []string{"cold", "warm", "touch", "edit"}
		tools := r.Tools(stages...)
		repos := r.Repos()
		gortex := r.Dir == "20260914-v0.2.2"
		if gortex {
			tools = append(tools, "gortex")
		}
		var groups []string
		var rows [][]Cell
		repoList := repos
		if len(repoList) == 0 {
			repoList = []string{""}
		}
		for _, stage := range stages {
			for _, repo := range repoList {
				row := make([]Cell, len(tools))
				any := false
				for j, t := range tools {
					if t == "gortex" {
						row[j] = missing()
						if stage == "cold" {
							var xs []float64
							for _, g := range cmp.GortexGates {
								if g.Repo == repo {
									xs = append(xs, g.Ready)
								}
							}
							row[j] = stat(xs)
							any = any || row[j].has()
						}
						continue
					}
					row[j] = r.StageStat(stage, repo, t)
					any = any || row[j].has()
				}
				if !any {
					continue
				}
				name := stage
				if stage == "edit" {
					name = "edit (one-file)"
				}
				if repo != "" {
					name += " · " + repo
				}
				groups = append(groups, name)
				rows = append(rows, row)
			}
		}
		sub := "Minimum of repeats (bar), median (tick), maximum (whisker); lower is better. Tools do not do identical work."
		if gortex {
			sub = "Minimum of 3 interleaved repeats. Gortex cold is its resident daemon's query-ready gate, not a CLI run."
		}
		if err := w.write("competition-"+r.Dir+"-indexing.svg", barChart(BarOpts{
			Title: "Code-graph tools, indexing: " + r.Title, Sub: sub, YLabel: "seconds (log)",
			Groups: groups, Series: toolSeries(tools), Cells: rows, Log: true, Fmt: seconds})); err != nil {
			return err
		}
		qstages := []string{"search", "callers", "impact", "context"}
		qtools := r.Tools(qstages...)
		if len(qtools) == 0 {
			continue
		}
		var qg []string
		var qr [][]Cell
		for _, stage := range qstages {
			row := make([]Cell, len(qtools))
			any := false
			for j, t := range qtools {
				row[j] = r.QueryStat(stage, t)
				any = any || row[j].has()
			}
			if any {
				qg = append(qg, stage)
				qr = append(qr, row)
			}
		}
		if err := w.write("competition-"+r.Dir+"-queries.svg", barChart(BarOpts{
			Title: "Code-graph tools, queries: " + r.Title, Sub: "Per-symbol minimum averaged over the symbols queried; lower is better. ripgrep is plain text search.",
			YLabel: "seconds (log)", Groups: qg, Series: toolSeries(qtools), Cells: qr, Log: true, Fmt: seconds, Labels: true})); err != nil {
			return err
		}
	}
	return nil
}

func competitionDrift(root string, w *writer) error {
	runs, err := loadCompetition(root)
	if err != nil {
		return err
	}
	labels := make([]string, len(runs))
	vals := [][]float64{make([]float64, len(runs)), make([]float64, len(runs)), make([]float64, len(runs))}
	for i, r := range runs {
		labels[i] = r.Dir
		repo := ""
		if len(r.Repos()) > 0 {
			repo = "DevCouncil"
		}
		touch := r.StageStat("touch", repo, "devmap")
		if !touch.has() {
			touch = r.StageStat("edit", repo, "devmap")
		}
		vals[0][i], vals[1][i], vals[2][i] = r.StageStat("cold", repo, "devmap").V, r.StageStat("warm", repo, "devmap").V, touch.V
	}
	return w.write("competition-devmap-drift.svg", lineChart(LineOpts{
		Title: "devmap in the competition runs, over time", Sub: "Minimum seconds on the DevCouncil corpus per run. Corpus commit, host load and the one-file edit stage differ between runs.",
		YLabel: "seconds (log)", XLabels: labels, Series: []Series{{"cold index", "#2a78d6"}, {"warm refresh", "#1baf7a"}, {"one-file edit", "#eb6834"}},
		Vals: vals, Log: true, Fmt: seconds}))
}

func v022Detail(root string, w *writer) error {
	runs, err := loadCompetition(root)
	if err != nil {
		return err
	}
	cmp, err := loadV022(root)
	if err != nil {
		return err
	}
	var r CompRun
	for _, c := range runs {
		if c.Dir == "20260914-v0.2.2" {
			r = c
		}
	}
	repos := r.Repos()
	sort.Slice(repos, func(i, j int) bool { return cmp.IndexBytes[repos[i]]["devmap"] < cmp.IndexBytes[repos[j]]["devmap"] })
	tools := []string{"devmap", "codegraph", "graphify", "gitnexus", "gortex"}
	rows := make([][]Cell, len(repos))
	for i, repo := range repos {
		rows[i] = make([]Cell, len(tools))
		for j, t := range tools {
			if v, ok := cmp.IndexBytes[repo][t]; ok {
				rows[i][j] = single(v)
			} else {
				rows[i][j] = missing()
			}
		}
	}
	if err := w.write("competition-v0.2.2-index-size.svg", barChart(BarOpts{
		Title: "Index size on disk: v0.2.2, four repositories", Sub: "Bytes the tool keeps for its index after a cold build (codebase-memory-mcp ran without persistence and leaves none).",
		YLabel: "index size (log)", Groups: repos, Series: toolSeries(tools), Cells: rows, Log: true, Fmt: mb, Labels: true})); err != nil {
		return err
	}
	rtools := []string{"devmap", "codegraph", "cbm", "gitnexus", "graphify"}
	rr := make([][]Cell, len(repos))
	for i, repo := range repos {
		rr[i] = make([]Cell, len(rtools))
		for j, t := range rtools {
			rr[i][j] = r.ColdRSS(repo, t)
		}
	}
	return w.write("competition-v0.2.2-peak-rss.svg", barChart(BarOpts{
		Title: "Peak memory of a cold index: v0.2.2, four repositories", Sub: "Median sampled process-tree peak resident set of 3 repeats (bar) and the largest (whisker); lower is better.",
		YLabel: "peak RSS (log)", Groups: repos, Series: toolSeries(rtools), Cells: rr, Log: true, Fmt: mb, Labels: true}))
}

func abCompare(root string, w *writer) error {
	ab, q, err := loadAB(root)
	if err != nil {
		return err
	}
	bins := []string{"v0.2.1", "v0.2.2"}
	repoSet := map[string]bool{}
	for _, r := range ab {
		repoSet[r.Repo] = true
	}
	var repos []string
	for r := range repoSet {
		repos = append(repos, r)
	}
	sort.Strings(repos)
	pick := func(repo, bin, stage string) Cell {
		var xs []float64
		for _, r := range ab {
			if r.Repo == repo && r.Binary == bin {
				switch stage {
				case "cold":
					xs = append(xs, r.Cold)
				case "warm":
					xs = append(xs, r.Warm)
				default:
					xs = append(xs, r.Touch)
				}
			}
		}
		return stat(xs)
	}
	var groups []string
	var rows [][]Cell
	for _, stage := range []string{"cold", "warm", "touch"} {
		for _, repo := range repos {
			groups = append(groups, stage+" · "+repo)
			rows = append(rows, []Cell{pick(repo, bins[0], stage), pick(repo, bins[1], stage)})
		}
	}
	series := []Series{{"devmap v0.2.1", "#898781"}, {"devmap v0.2.2", "#2a78d6"}}
	if err := w.write("ab-v0.2.1-vs-v0.2.2-indexing.svg", barChart(BarOpts{
		Title: "devmap v0.2.1 vs v0.2.2: indexing A/B", Sub: "Same corpora, interleaved rounds; minimum (bar), median (tick), maximum (whisker).",
		YLabel: "seconds (log)", Groups: groups, Series: series, Cells: rows, Log: true, Fmt: seconds})); err != nil {
		return err
	}
	opSet := map[string]bool{}
	for _, r := range q {
		opSet[r.Op] = true
	}
	var ops []string
	for o := range opSet {
		ops = append(ops, o)
	}
	sort.Strings(ops)
	qrows := make([][]Cell, len(ops))
	for i, op := range ops {
		qrows[i] = make([]Cell, len(bins))
		for j, b := range bins {
			var xs []float64
			for _, r := range q {
				if r.Op == op && r.Binary == b {
					xs = append(xs, r.Seconds)
				}
			}
			qrows[i][j] = stat(xs)
		}
	}
	return w.write("ab-v0.2.1-vs-v0.2.2-queries.svg", barChart(BarOpts{
		Title: "devmap v0.2.1 vs v0.2.2: query A/B", Sub: "Per-query seconds across rounds and symbols; minimum (bar), median (tick), maximum (whisker).",
		YLabel: "seconds (log)", Groups: ops, Series: series, Cells: qrows, Log: true, Fmt: seconds, Labels: true}))
}

func buildCost(root string, w *writer) error {
	cold, td, edit, err := loadBuildCost(root)
	if err != nil {
		return err
	}
	ab := []Series{{"A: HEAD", "#898781"}, {"B: variant", "#2a78d6"}}
	metrics := []struct{ key, label string }{{"wall_s", "wall"}, {"persist_write_s", "persist: write"}, {"persist_unresolved_s", "persist: unresolved calls"}, {"persist_commit_s", "persist: commit"}}
	var g []string
	var rows [][]Cell
	for _, m := range metrics {
		g = append(g, m.label)
		rows = append(rows, []Cell{cold.Arms["a"][m.key].cell(), cold.Arms["b"][m.key].cell()})
	}
	if err := w.write("build-cost-cold-index.svg", barChart(BarOpts{
		Title: "Build cost: cold index, HEAD vs variant without the callee index", Sub: fmt.Sprintf("%d interleaved rounds on a 1310-file corpus under heavy host load; minimum (bar), median (tick), maximum (whisker).", cold.Rounds),
		YLabel: "seconds (log)", Groups: g, Series: ab, Cells: rows, Log: true, Fmt: seconds, Labels: true})); err != nil {
		return err
	}
	g, rows = nil, nil
	for _, m := range []struct{ key, label string }{{"line_to_exit_ms", "last output line to process exit"}, {"total_ms", "total command time"}} {
		g = append(g, m.label)
		rows = append(rows, []Cell{td.Arms["a"][m.key].cell(), td.Arms["b"][m.key].cell()})
	}
	if err := w.write("build-cost-teardown.svg", barChart(BarOpts{
		Title: "Build cost: process exit, HEAD vs variant that mem::forgets build structures", Sub: fmt.Sprintf("%d interleaved rounds; minimum (bar), median (tick), maximum (whisker).", td.Rounds),
		YLabel: "milliseconds (log)", Groups: g, Series: ab, Cells: rows, Log: true, Fmt: ms, Labels: true})); err != nil {
		return err
	}
	var arms []string
	for _, a := range []string{"codegraph", "devmap", "devmap_b"} {
		arms = append(arms, a)
	}
	names := map[string]string{"codegraph": "codegraph", "devmap": "devmap HEAD", "devmap_b": "devmap variant (mem::forget)"}
	g, rows = nil, nil
	for _, a := range arms {
		g = append(g, names[a])
		row := make([]Cell, 2)
		for j, kind := range []string{"edit", "restore"} {
			s, ok := edit.Summary[a+"/"+kind]
			if !ok {
				row[j] = missing()
				continue
			}
			row[j] = Cell{s.Min, s.Median, s.Max}
		}
		rows = append(rows, row)
	}
	return w.write("build-cost-edit-latency.svg", barChart(BarOpts{
		Title: "Build cost: edit-to-query latency", Sub: fmt.Sprintf("Time to see an edit (and its restore) after touching a file; %d repeats; minimum (bar), median (tick), maximum (whisker).", edit.Repeat),
		YLabel: "milliseconds (log)", Groups: g, Series: []Series{{"edit", "#2a78d6"}, {"restore", "#eb6834"}}, Cells: rows, Log: true, Fmt: ms, Labels: true}))
}
