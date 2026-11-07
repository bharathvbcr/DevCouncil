package main

import (
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"regexp"
	"sort"
	"strings"
	"testing"
	"time"
)

// These tests read the workflow files as text. The repository has no YAML
// dependency and takes none for this; the workflow's shape (where an `if:` or a
// permission sits) is what they pin, and a step is a block that starts at
// `      - ` under `steps:`.

const workflowDir = "../../../../.github/workflows"

type wfStep struct {
	name  string
	lines []string
}

// has reports whether the step's own YAML, comments excluded, contains sub.
// A comment above the next step is split onto this one, and must not count.
func (s wfStep) has(sub string) bool {
	var code []string
	for _, l := range s.lines {
		if !strings.HasPrefix(strings.TrimSpace(l), "#") {
			code = append(code, l)
		}
	}
	return strings.Contains(strings.Join(code, "\n"), sub)
}

type wfFile struct {
	text   string
	header []string // everything before `steps:`
	steps  []wfStep
}

func loadWorkflow(t *testing.T, name string) wfFile {
	t.Helper()
	body, err := os.ReadFile(filepath.Join(workflowDir, name))
	if err != nil {
		t.Fatal(err)
	}
	f := wfFile{text: string(body)}
	inSteps := false
	for _, line := range strings.Split(f.text, "\n") {
		if !inSteps {
			f.header = append(f.header, line)
			if line == "    steps:" {
				inSteps = true
			}
			continue
		}
		if strings.HasPrefix(line, "      - ") {
			f.steps = append(f.steps, wfStep{})
		}
		if len(f.steps) > 0 {
			cur := &f.steps[len(f.steps)-1]
			cur.lines = append(cur.lines, line)
			if m := regexp.MustCompile(`^      - (?:name|uses): (.*)$`).FindStringSubmatch(line); m != nil && cur.name == "" {
				cur.name = strings.TrimSpace(m[1])
			}
			if m := regexp.MustCompile(`^        name: (.*)$`).FindStringSubmatch(line); m != nil {
				cur.name = strings.TrimSpace(m[1])
			}
		}
	}
	return f
}

func (f wfFile) step(t *testing.T, prefix string) wfStep {
	t.Helper()
	for _, s := range f.steps {
		if strings.HasPrefix(s.name, prefix) {
			return s
		}
	}
	t.Fatalf("no step starting %q; steps are %v", prefix, func() (n []string) {
		for _, s := range f.steps {
			n = append(n, s.name)
		}
		return
	}())
	return wfStep{}
}

// gated is the condition of every step that needs the selector binary: it runs
// whatever earlier steps did, but only when the selector was built.
const gated = "if: ${{ !cancelled() && steps.selector.outcome == 'success' }}"

func TestTheWorkflowRunsThePlanAndTheFullSuiteWhateverEarlierStepsDid(t *testing.T) {
	f := loadWorkflow(t, "selected-tests.yml")
	for _, name := range []string{"Plan the selection", "Full suite", "Audit the selection"} {
		if s := f.step(t, name); !s.has(gated) {
			t.Errorf("step %q lacks `%s`: a failed devmap build or index would skip it, and the full suite and the artifact with it", name, gated)
		}
	}
	if full := f.step(t, "Full suite"); full.has("continue-on-error") {
		t.Error("the Full suite step is the gate and must not continue on error")
	}
	upload := f.step(t, "Upload selection-recall.json")
	if !upload.has("if: ${{ always() }}") {
		t.Error("the artifact must be uploaded on every run")
	}
}

func TestADevmapBuildOrIndexFailureIsAWarningNotAFailedJob(t *testing.T) {
	f := loadWorkflow(t, "selected-tests.yml")
	for _, name := range []string{"Build devmap", "Index this commit"} {
		s := f.step(t, name)
		if !s.has("continue-on-error: true") {
			t.Errorf("step %q must continue on error: its failure is a blast error, which runs everything", name)
		}
	}
	index := f.step(t, "Index this commit")
	if !index.has("::warning::") {
		t.Error("a failed index must say so with ::warning::")
	}
	for _, name := range []string{"Index this commit", "Plan the selection"} {
		if !f.step(t, name).has("timeout-minutes:") {
			t.Errorf("step %q has no timeout-minutes", name)
		}
	}
}

func TestTheJobCanOnlyReadActionsAndTheTokenIsInOneStepOnly(t *testing.T) {
	f := loadWorkflow(t, "selected-tests.yml")
	header := strings.Join(f.header, "\n")
	if !regexp.MustCompile(`(?m)^    permissions:\n      contents: read\n      actions: read\n`).MatchString(header) {
		t.Errorf("job permissions must be exactly contents: read and actions: read:\n%s", header)
	}
	if strings.Contains(header, "actions: write") || strings.Contains(f.text, "actions: write\n") {
		t.Error("actions: write is wider than gh run list/download need, and upload-artifact needs none")
	}
	if strings.Contains(header, "GH_TOKEN") {
		t.Error("GH_TOKEN is in the job's environment, so cargo build and test run the changed code with it")
	}
	holders := 0
	for _, s := range f.steps {
		if s.has("GH_TOKEN") {
			holders++
			if !strings.HasPrefix(s.name, "Fetch the base") {
				t.Errorf("step %q holds GH_TOKEN; only the base fetch may", s.name)
			}
		}
	}
	if holders != 1 {
		t.Errorf("GH_TOKEN is in %d steps, want exactly the base fetch", holders)
	}
}

func TestTheBaseIsOnlyACompletedPushRunOfTheBaseBranchAndTheAuditIsToldWhoItIs(t *testing.T) {
	f := loadWorkflow(t, "selected-tests.yml")
	fetch := f.step(t, "Fetch the base")
	for _, want := range []string{`--branch "$BASE_BRANCH"`, "--event push", "--status completed", "cancelled"} {
		if !fetch.has(want) {
			t.Errorf("the base fetch lacks %q", want)
		}
	}
	audit := f.step(t, "Audit the selection")
	for _, want := range []string{`--ref "$GITHUB_REF"`, `--event "$GITHUB_EVENT_NAME"`, `--run-id "$GITHUB_RUN_ID"`, `--base-branch "$BASE_BRANCH"`} {
		if !audit.has(want) {
			t.Errorf("the audit step does not pass %s", want)
		}
	}
}

func TestTheWorkflowRunsTheSelectorsEndToEndTestsUnderTheE2ETagAndOnlyThose(t *testing.T) {
	f := loadWorkflow(t, "selected-tests.yml")
	s := f.step(t, "Selector end-to-end")
	if !s.has("-tags e2e") || !s.has("-run '^TestE2E'") || !s.has("./cmd/dctestselect") {
		t.Errorf("the end-to-end step must run `go test -tags e2e ./cmd/dctestselect -run '^TestE2E'`:\n%s", strings.Join(s.lines, "\n"))
	}
	// go test's default package timeout is 10 minutes and panics the binary, so
	// the 25-minute bound inside the whole-crate test only works if -timeout
	// is passed and is longer than it.
	m := regexp.MustCompile(`-timeout (\S+)`).FindStringSubmatch(strings.Join(s.lines, "\n"))
	if m == nil {
		t.Fatalf("the end-to-end step passes no -timeout, so go test's 10-minute default kills the 25-minute bound:\n%s", strings.Join(s.lines, "\n"))
	}
	if d, err := time.ParseDuration(m[1]); err != nil || d <= 25*time.Minute {
		t.Errorf("-timeout %s must be a duration longer than the 25-minute in-test bound", m[1])
	}
}

// The fetch step's esc() is the shell twin of escapeData: gh's error text goes
// into a ::warning:: line, and a %, CR or LF in it must not end the annotation.
func TestTheFetchStepsEscapeMatchesEscapeData(t *testing.T) {
	f := loadWorkflow(t, "selected-tests.yml")
	fetch := f.step(t, "Fetch the base")
	def := ""
	for _, l := range fetch.lines {
		if strings.HasPrefix(strings.TrimSpace(l), "esc() {") {
			def = strings.TrimSpace(l)
		}
	}
	if def == "" {
		t.Fatal("the fetch step defines no esc() for gh's error text")
	}
	if strings.Contains(strings.Join(fetch.lines, "\n"), "tr '\\n' ' '") {
		t.Error("a warning still flattens newlines with tr, which leaves % and CR unescaped")
	}
	for _, v := range []string{"plain", "a\nb", "100%\r\nx", "::error::injected", "%0A%"} {
		out, err := exec.Command("bash", "-c", def+`; esc "$1"`, "bash", v).Output()
		if err != nil {
			t.Fatal(err)
		}
		if string(out) != escapeData(v) {
			t.Errorf("esc(%q) = %q, escapeData = %q", v, out, escapeData(v))
		}
	}
	for _, want := range []string{`$(esc "$(head -c 300`, `$(esc "$(head -c 200`} {
		if !fetch.has(want) {
			t.Errorf("the fetch step does not pass its gh error text through esc: missing %q", want)
		}
	}
}

func TestOnlyTheBaseBranchSavesTheCache(t *testing.T) {
	f := loadWorkflow(t, "selected-tests.yml")
	if s := f.step(t, "Swatinem/rust-cache"); !s.has("save-if: ${{ github.ref == 'refs/heads/main' }}") {
		t.Error("rust-cache must save only on main: another cache entry per branch is cost with no reader")
	}
}

func TestTheSelectedAndFullPhasesAreExplainedAsDifferingInFeatures(t *testing.T) {
	f := loadWorkflow(t, "selected-tests.yml")
	if !strings.Contains(f.text, "feature-unification") || !strings.Contains(f.text, "selected_phase_failed_only") {
		t.Error("the workflow must say why a selected-only red is possible and where it is recorded")
	}
}

var excludeFlag = regexp.MustCompile(`--exclude\s+(\S+)`)

// rust.yml's Test step is the definition this job's universe copies. A new
// crate excluded there and not here would be tested twice and mean different
// things; one dropped there and kept here would stop being tested here.
func TestTheExcludeListIsRustYmlsTestStepList(t *testing.T) {
	f := loadWorkflow(t, "rust.yml")
	var test wfStep
	found := false
	for _, s := range f.steps {
		// The first: the `test` job's, whose universe this job copies.
		if s.name == "Test" && !found {
			test, found = s, true
		}
	}
	if !found {
		t.Fatal("rust.yml has no step named Test")
	}
	var got []string
	for _, line := range test.lines {
		if strings.Contains(line, "cargo test") {
			for _, m := range excludeFlag.FindAllStringSubmatch(line, -1) {
				got = append(got, m[1])
			}
		}
	}
	want := append([]string{}, DefaultRustExcludes...)
	sort.Strings(got)
	sort.Strings(want)
	if len(got) == 0 || !reflect.DeepEqual(got, want) {
		t.Fatalf("rust.yml's Test step excludes %v, DefaultRustExcludes is %v", got, want)
	}
}

func TestAFailedSelectorBuildSaysSoInsteadOfFailingWithExit127(t *testing.T) {
	f := loadWorkflow(t, "selected-tests.yml")
	build := f.step(t, "Build the selector")
	if !build.has("id: selector") || build.has("continue-on-error") {
		t.Error("the selector build must have id `selector` and must stay fatal: the job is red when it fails")
	}
	gate := f.step(t, "Selector build gate")
	if !gate.has("steps.selector.outcome != 'success'") || !gate.has("::error::selector build failed; full suite not run") || !gate.has("exit 1") {
		t.Errorf("the gate step must name the failure and keep the job red:\n%s", strings.Join(gate.lines, "\n"))
	}
	for _, name := range []string{"Plan the selection", "Selected tests", "Full suite", "Audit the selection"} {
		if !f.step(t, name).has("steps.selector.outcome == 'success'") {
			t.Errorf("step %q runs the selector binary without checking that it was built", name)
		}
	}
}

func TestTheBaseFetchDoesNotSwallowGhErrors(t *testing.T) {
	f := loadWorkflow(t, "selected-tests.yml")
	fetch := f.step(t, "Fetch the base")
	if fetch.has("|| true") {
		t.Error("`|| true` hides why gh failed")
	}
	for _, want := range []string{"::warning::gh run list failed", "gh.err", "base_failed is unknown"} {
		if !fetch.has(want) {
			t.Errorf("the fetch step lacks %q: a failure must be logged as a warning with its reason", want)
		}
	}
}
