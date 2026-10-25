package policy

import (
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
)

// An agent whose task allows every `dev` command, or every command at all,
// still cannot record the review its own llm_review criterion is discharged
// by, however the command is spelled.
func TestRecordingAReviewIsRefusedInEveryAgentShell(t *testing.T) {
	root := t.TempDir()
	task := &dc.Task{ID: "TASK-1", AllowedCommands: []string{"dev *", "devcouncil *", "dcstore *", "*"}}
	gates := map[string]CommandGate{
		"task allows everything":   {HardRules: true, Root: root},
		"global allows everything": {HardRules: true, Root: root, GlobalAllowedCommands: []string{"*"}},
	}
	refused := []string{
		"dev review record --file r.json",
		"devcouncil review record --file r.json",
		"devcouncil review record",
		"devcouncil review   record --file r.json",
		"/opt/devcouncil/bin/devcouncil review record --file r.json",
		".venv/bin/dev review record --file r.json",
		"devcouncil.exe review record --file r.json",
		"env DEVCOUNCIL_X=1 devcouncil review record --file r.json",
		"nice -n 5 dev review record --file r.json",
		"echo r.json | xargs devcouncil review record --file",
		"git status && devcouncil review record --file r.json",
		"devcouncil 'review' record --file r.json",
		`devcouncil re"view" rec'ord' --file r.json`,
		`dev rev\iew record --file r.json`,
		"dcstore --db state.sqlite review-append --task T --acceptance-criterion-id AC --subject-hash h --record {}",
		"/usr/local/bin/dcstore review-append --task T",
		"dcstore --db=x review-append",
		// What the shell expands into `review record` without the line
		// containing it (adversarial round on 48a66ac3).
		"devcouncil review {record,}",
		"dev review {record,} --file r.json",
		"devcouncil review rec{o,}rd --file r.json",
		"devcouncil review rec?rd --file r.json",
		"devcouncil review rec[o]rd --file r.json",
		"devcouncil review $'record' --file r.json",
		`devcouncil review $'\x72ecord' --file r.json`,
		`devcouncil review "$(echo record)" --file r.json`,
		"a=rec; devcouncil review ${a}ord --file r.json",
		"devcouncil {review,} record --file r.json",
		"devcouncil $SUB record --file r.json",
		"DEVCOUNCIL review record --file r.json",
		`C:\bin\devcouncil.exe review record --file r.json`,
		"x;devcouncil review record --file r.json",
		"(devcouncil review record --file r.json)",
		"dcstore {review-append,} --task T --acceptance-criterion-id AC --subject-hash h --record r",
		"dcstore review-ap{p,}end --task T",
		"dcstore\treview-append --task T",
		"rust/target/debug/dcstore review-append --task T",
		"sh -c 'devcouncil review record --file r.json'",
	}
	for name, gate := range gates {
		for _, command := range refused {
			d := gate.EvaluateCommand(command, task)
			if d.Action != Deny || d.Severity != Hard || d.Rule != RuleCommandHarnessOnly {
				t.Errorf("%s: %q: %s %s/%s, want a hard %s", name, command, d.Action, d.Rule, d.Severity, RuleCommandHarnessOnly)
			}
		}
	}
}

// Reading reviews, naming the content, and everything that merely contains
// the words is the agent's to run.
func TestReadingReviewsAndLookalikesAreNotHarnessOnly(t *testing.T) {
	for _, command := range []string{
		"devcouncil review list --task TASK-1",
		"devcouncil review subject",
		"dev review",
		"devcouncil verify TASK-1",
		"echo review record",
		"mydev review record",
		"devcouncilx review record",
		"dcstore reviews --task T",
		"grep -r review-append docs",
		"git log --grep 'review record'",
		"devcouncil review --help",
		"devcouncil review list --task 'TASK-1' --json",
		"dcstore --db s.sqlite reviews --task T",
		"devcouncil task show TASK-1",
	} {
		if reason := harnessOnlyViolation(command); reason != "" {
			t.Errorf("%q refused as harness-only: %s", command, reason)
		}
	}
}

// The rule is what the line does, so it outranks the refusals for what the
// gate could not read: a heredoc beside it must not be the reason given.
func TestHarnessOnlyIsNotAnUnreadableRule(t *testing.T) {
	if IsUnreadableRule(RuleCommandHarnessOnly) {
		t.Fatal("harness_only would rank below an unreadable refusal and could be hidden by one")
	}
	if SeverityOf(RuleCommandHarnessOnly) != Hard || SubjectOf(RuleCommandHarnessOnly) != SubjectCommand {
		t.Fatalf("severity %s subject %v", SeverityOf(RuleCommandHarnessOnly), SubjectOf(RuleCommandHarnessOnly))
	}
}
