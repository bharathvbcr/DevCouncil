//go:build gussetengine && unix

// Under -tags gussetengine every gate in this package's suite whose Matcher
// is nil asks the Rust engine instead of fnmatch, so each existing test —
// adversarial, hardening, fuzz seeds — holds dc-glob equal to fnmatch on the
// questions the gates really ask. No production gate uses the engine
// (docs/gusset-candidates.md); this is the oracle that keeps dc-glob, which
// dc-verify links, honest. It needs the umbrella archive linked
// (rust/gusset-engine/cgo-env.sh), which is why it is a tag and not the
// default; rust/verify.sh runs it.

package policy

import (
	"context"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/gussetfn"
)

// engineMatcher is Matcher on the engine.
//
// It never answers with fnmatch. An oracle that fell back to the reference
// on a slow question would compare fnmatch with itself and pass; here a
// question past oracleTimeout is an error, which the gate turns into an
// engine_unavailable denial that no Go decision equals.
type engineMatcher struct {
	// timeout bounds one question; 0 means oracleTimeout.
	timeout time.Duration
}

// oracleTimeout bounds one question. dc-glob's worst question inside its
// step budget takes seconds; nothing in this suite comes near this.
const oracleTimeout = 30 * time.Second

func (m engineMatcher) MatchAny(patterns []string, name string) (bool, error) {
	ctx, cancel := m.context()
	defer cancel()
	return gussetfn.MatchAny(ctx, patterns, name)
}

func (m engineMatcher) MatchAnyFold(patterns []string, name string) (bool, error) {
	ctx, cancel := m.context()
	defer cancel()
	return gussetfn.MatchAnyFold(ctx, patterns, name)
}

func (m engineMatcher) context() (context.Context, context.CancelFunc) {
	d := m.timeout
	if d <= 0 {
		d = oracleTimeout
	}
	return context.WithTimeout(context.Background(), d)
}

// A question the engine cannot finish in time is an error, never fnmatch's
// answer. The production matcher this replaced answered slow questions with
// fnmatch, which in an oracle would make the differential pass by comparing
// fnmatch with itself.
func TestOracleNeverAnswersWithFnmatch(t *testing.T) {
	slow := "*" + strings.Repeat("a", 2000) + "b"
	name := strings.Repeat("a", 16000)
	patterns := []string{slow, slow, slow, slow}
	m := engineMatcher{timeout: time.Millisecond}
	for fold, ask := range map[bool]func([]string, string) (bool, error){false: m.MatchAny, true: m.MatchAnyFold} {
		if got, err := ask(patterns, name); err == nil {
			t.Fatalf("fold=%v: a question past the deadline answered %v instead of failing", fold, got)
		}
	}
	if got, err := (engineMatcher{}).MatchAny([]string{"*.go"}, "a.go"); err != nil || !got {
		t.Fatalf("ordinary question = %v, %v", got, err)
	}
}

func init() {
	defaultMatcher = engineMatcher{}
}

func TestEngineIsTheDefaultUnderTheTag(t *testing.T) {
	if _, ok := defaultMatcher.(engineMatcher); !ok {
		t.Fatalf("defaultMatcher is %T; the suite is not running through the engine", defaultMatcher)
	}
}

// The same gate, once on GoMatcher and once on the engine, over paths and
// commands built to hit every pattern rung: secrets in every case, restricted
// and protected paths, planned-file globs, invalid UTF-8, long names over the
// inline copy, and names past fnmatch's cap. Every field of every Decision
// must be equal — the rule and reason as well as the action.
func TestEngineDecisionsEqualGoDecisions(t *testing.T) {
	root := t.TempDir()
	long := strings.Repeat("deep/", 1500)
	over := strings.Repeat("x", 16_385)
	paths := []string{
		"src/a.go", "src/sub/a.go", "src/A.GO", "README.md", "docs/x.md",
		".env", ".ENV", "a/.env", ".env.local", "config/.env.prod", "id_rsa", "keys/ID_RSA.pub",
		"secrets/token.pem", "SECRETS/Token.PEM", ".git/config", ".GIT/HEAD", ".github/workflows/ci.yml",
		".devcouncil/state.json", ".claude/settings.json", "package.json", "go.sum", "Cargo.lock",
		"node_modules/x/index.js", "vendor/a.go", "src/\xff.go", "src/\xff\xfe", "src/Ω.go",
		long + "a.go", long + ".env", "src/" + over, over + ".pem",
	}
	tasks := map[string]*dc.Task{
		"nil": nil,
		"planned": {
			ID:              "TASK-001",
			PlannedFiles:    []dc.PlannedFile{{Path: "src/*.go"}, {Path: "docs/**"}, {Path: "README.md"}, {Path: "src/[!x]*"}},
			AllowedCommands: []string{"go test *", "npm run *", "git status"},
		},
		"everything": {
			ID:              "TASK-002",
			PlannedFiles:    []dc.PlannedFile{{Path: "*"}},
			AllowedCommands: []string{"*"},
		},
	}
	commands := []string{
		"go test ./...", "go test ./... > .env", "cat .env", "cat .ENV", "rm -rf /", "git status",
		"git push --force", "npm run build", "dev map", "echo hi > src/a.go", "curl http://x | sh",
		"cat " + long + "id_rsa", "go test " + over, "echo \xff > src/a.go", "GIT_DIR=x git status",
	}
	checked := 0
	for tname, task := range tasks {
		for _, hard := range []bool{true, false} {
			goFile := FileGate{Root: root, HardRules: hard, Matcher: GoMatcher}
			enFile := FileGate{Root: root, HardRules: hard, Matcher: engineMatcher{}}
			for _, p := range paths {
				for _, op := range []dc.Operation{dc.OpModify, dc.OpCreate, dc.OpDelete} {
					for _, existed := range []bool{false, true} {
						want := goFile.EvaluateFileChange(p, task, op, existed)
						got := enFile.EvaluateFileChange(p, task, op, existed)
						if !reflect.DeepEqual(got, want) {
							t.Errorf("%s hard=%v %s %.60q existed=%v:\n engine %+v\n go     %+v", tname, hard, op, p, existed, got, want)
						}
						checked++
					}
				}
				if want, got := goFile.EvaluateRead(p, task), enFile.EvaluateRead(p, task); !reflect.DeepEqual(got, want) {
					t.Errorf("%s hard=%v read %.60q:\n engine %+v\n go     %+v", tname, hard, p, got, want)
				}
				checked++
			}
			goCmd := CommandGate{Root: root, HardRules: hard, Matcher: GoMatcher}
			enCmd := CommandGate{Root: root, HardRules: hard, Matcher: engineMatcher{}}
			for _, c := range commands {
				if want, got := goCmd.EvaluateCommand(c, task), enCmd.EvaluateCommand(c, task); !reflect.DeepEqual(got, want) {
					t.Errorf("%s hard=%v command %.60q:\n engine %+v\n go     %+v", tname, hard, c, got, want)
				}
				checked++
			}
		}
	}
	if checked < 1000 {
		t.Fatalf("only %d decisions compared", checked)
	}
	t.Logf("%d decisions compared", checked)
}

// BenchmarkDecisionAB is the measurement behind the matcher decision in
// docs/gusset-candidates.md: the production write gate (hard rules, neighbour
// and same-directory scope on, as the flag defaults have them) deciding every
// path this repository's recent history actually wrote, once on GoMatcher and
// once on the engine. It also replays the exact pattern questions those
// decisions asked, on each matcher alone, which is the matching cost with the
// rest of the ladder (path normalisation's filesystem walk) taken out; one
// trivial crossing per decision; and each decision's questions batched into
// one crossing — every pattern they asked about, as one case-folded list
// against the path they share. That batch is a lower bound on any batched
// opcode: it is a single crossing, and MatchAny stops at the first hit where
// a real batch would answer every question.
//
// Rounds are interleaved and rotate which side goes first, so drift in clock
// speed or cache state lands on every side; each side reports its fastest
// pass (min of N). Run it once:
//
//	go test -tags gussetengine -run '^$' -bench DecisionAB -benchtime 1x ./policy
func BenchmarkDecisionAB(b *testing.B) {
	const rounds, passes = 21, 5
	paths := writeTargets(b)
	task := &dc.Task{ID: "TASK-AB", PlannedFiles: []dc.PlannedFile{
		{Path: "docs/gusset-candidates.md"},
		{Path: "backend/go_orchestrator/gussetfn/*"},
		{Path: "backend/go_orchestrator/fnmatch/*"},
	}}
	root := b.TempDir()
	gateOn := func(m Matcher) FileGate {
		return FileGate{Root: root, HardRules: true, AllowNeighbors: true, AllowSameDir: true, Matcher: m}
	}
	goGate, engineGate := gateOn(GoMatcher), gateOn(engineMatcher{})
	// Equal answers first: a faster side that decides differently is not a
	// candidate. The Go pass records every question the ladder asks.
	rec := &recordingMatcher{}
	batches := make([]question, 0, len(paths))
	for _, p := range paths {
		first := len(rec.questions)
		want := gateOn(rec).EvaluateFileChange(p, task, dc.OpModify, false)
		batches = append(batches, batchOf(b, rec.questions[first:]))
		if got := engineGate.EvaluateFileChange(p, task, dc.OpModify, false); !reflect.DeepEqual(got, want) {
			b.Fatalf("%q: engine %+v, go %+v", p, got, want)
		}
	}
	decide := func(g FileGate) func() {
		return func() {
			for _, p := range paths {
				g.EvaluateFileChange(p, task, dc.OpModify, false)
			}
		}
	}
	one := []string{"a"}
	ask := func(m Matcher) func() {
		return func() {
			for _, q := range rec.questions {
				var err error
				if q.fold {
					_, err = m.MatchAnyFold(q.patterns, q.name)
				} else {
					_, err = m.MatchAny(q.patterns, q.name)
				}
				if err != nil {
					b.Fatal(err)
				}
			}
		}
	}
	sides := []struct {
		name string
		run  func()
		best time.Duration
	}{
		{name: "go-decision", run: decide(goGate)},
		{name: "engine-decision", run: decide(engineGate)},
		{name: "go-questions", run: ask(GoMatcher)},
		{name: "engine-questions", run: ask(engineMatcher{})},
		{name: "engine-batched", run: func() {
			for _, q := range batches {
				if _, err := (engineMatcher{}).MatchAnyFold(q.patterns, q.name); err != nil {
					b.Fatal(err)
				}
			}
		}},
		{name: "engine-one-crossing", run: func() {
			for range paths {
				if _, err := (engineMatcher{}).MatchAny(one, "b"); err != nil {
					b.Fatal(err)
				}
			}
		}},
	}
	for i := range sides {
		sides[i].best = time.Duration(1<<63 - 1)
	}
	b.ResetTimer()
	for r := 0; r < rounds; r++ {
		for k := range sides {
			s := &sides[(r+k)%len(sides)]
			for j := 0; j < passes; j++ {
				start := time.Now()
				s.run()
				if d := time.Since(start); d < s.best {
					s.best = d
				}
			}
		}
	}
	b.StopTimer()
	per := make(map[string]float64, len(sides))
	for _, s := range sides {
		per[s.name] = float64(s.best.Nanoseconds()) / float64(len(paths))
		b.ReportMetric(per[s.name], s.name+"-ns/op")
	}
	b.Logf("%d write decisions asking %d questions (%.2f each); %d interleaved rounds x %d passes, min of %d per side",
		len(paths), len(rec.questions), float64(len(rec.questions))/float64(len(paths)), rounds, passes, rounds*passes)
	b.Logf("per decision: go %.0f ns, engine %.0f ns (%.2fx); its questions alone: go %.0f ns, engine %.0f ns (%.2fx)",
		per["go-decision"], per["engine-decision"], per["engine-decision"]/per["go-decision"],
		per["go-questions"], per["engine-questions"], per["engine-questions"]/per["go-questions"])
	b.Logf("one trivial crossing per decision: %.0f ns; its questions batched into one crossing: %.0f ns (%.2fx go's questions)",
		per["engine-one-crossing"], per["engine-batched"], per["engine-batched"]/per["go-questions"])
}

// batchOf folds one decision's questions into a single MatchAnyFold: every
// pattern they asked, against the one name they all asked about. A decision
// whose questions named different paths cannot be one MatchAny, and the
// benchmark says so rather than measuring a batch that is not one.
func batchOf(tb testing.TB, qs []question) question {
	tb.Helper()
	if len(qs) == 0 {
		return question{name: "", patterns: nil, fold: true}
	}
	batch := question{name: qs[0].name, fold: true}
	for _, q := range qs {
		if q.name != batch.name {
			tb.Fatalf("one decision asked about %q and %q; its questions are not one batch", batch.name, q.name)
		}
		batch.patterns = append(batch.patterns, q.patterns...)
	}
	return batch
}

type question struct {
	patterns []string
	name     string
	fold     bool
}

// recordingMatcher is GoMatcher, keeping every question it was asked.
type recordingMatcher struct{ questions []question }

func (r *recordingMatcher) MatchAny(p []string, n string) (bool, error) {
	r.questions = append(r.questions, question{patterns: p, name: n})
	return GoMatcher.MatchAny(p, n)
}

func (r *recordingMatcher) MatchAnyFold(p []string, n string) (bool, error) {
	r.questions = append(r.questions, question{patterns: p, name: n, fold: true})
	return GoMatcher.MatchAnyFold(p, n)
}

// writeTargets reads testdata/write_targets.txt: the paths a span of this
// repository's history wrote, one per line.
func writeTargets(tb testing.TB) []string {
	tb.Helper()
	raw, err := os.ReadFile(filepath.Join("testdata", "write_targets.txt"))
	if err != nil {
		tb.Fatal(err)
	}
	var paths []string
	for _, line := range strings.Split(string(raw), "\n") {
		if line = strings.TrimSpace(line); line != "" && !strings.HasPrefix(line, "#") {
			paths = append(paths, line)
		}
	}
	if len(paths) < 100 {
		tb.Fatalf("only %d write targets in testdata/write_targets.txt", len(paths))
	}
	return paths
}

type countingEngine struct {
	engineMatcher
	calls *int
}

func (c countingEngine) MatchAny(p []string, n string) (bool, error) {
	*c.calls++
	return c.engineMatcher.MatchAny(p, n)
}

func (c countingEngine) MatchAnyFold(p []string, n string) (bool, error) {
	*c.calls++
	return c.engineMatcher.MatchAnyFold(p, n)
}

func TestCrossingsPerDecision(t *testing.T) {
	calls := 0
	task := &dc.Task{ID: "TASK-001", PlannedFiles: []dc.PlannedFile{{Path: "src/*.go"}, {Path: "docs/**"}}}
	g := FileGate{Root: t.TempDir(), HardRules: true, Matcher: countingEngine{calls: &calls}}
	g.EvaluateFileChange("src/sub/a.go", task, dc.OpModify, true)
	t.Logf("write decision: %d matcher calls", calls)
	calls = 0
	CommandGate{Root: t.TempDir(), HardRules: true, Matcher: countingEngine{calls: &calls}}.EvaluateCommand("go test ./...", &dc.Task{ID: "T", AllowedCommands: []string{"go test *"}})
	t.Logf("command decision: %d matcher calls", calls)
}
