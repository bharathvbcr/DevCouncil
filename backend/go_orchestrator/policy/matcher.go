package policy

import (
	"fmt"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/fnmatch"
)

// Matcher answers the gates' pattern questions: does name match any of
// patterns, case-sensitively (fnmatch.MatchAny) or case-folded
// (fnmatch.MatchAnyFold). Every answer must equal what fnmatch gives for the
// same arguments; only the error is new.
//
// The error is the point of the interface. A matcher that can fail — the
// in-process Rust engine behind gussetfn — has no honest bool for "I could
// not answer": either value is a wrong allow or a wrong deny. The gates turn
// an error into a denial under RulePathEngineUnavailable or
// RuleCommandEngineUnavailable, never under the rung that asked, so the
// record says the engine failed rather than that the path was a secret.
//
// This package stays free of cgo: GoMatcher is the default, and a host that
// links the engine injects it through FileGate.Matcher and
// CommandGate.Matcher.
type Matcher interface {
	MatchAny(patterns []string, name string) (bool, error)
	MatchAnyFold(patterns []string, name string) (bool, error)
}

// GoMatcher is fnmatch itself. It never fails.
var GoMatcher Matcher = goMatcher{}

type goMatcher struct{}

func (goMatcher) MatchAny(patterns []string, name string) (bool, error) {
	return fnmatch.MatchAny(patterns, name), nil
}

func (goMatcher) MatchAnyFold(patterns []string, name string) (bool, error) {
	return fnmatch.MatchAnyFold(patterns, name), nil
}

// engineFailure carries a matcher error from deep in the ladder to the gate's
// entry point. The rungs are bool-valued helpers several calls down; a panic
// recovered at the entry point keeps each rung's order and shape intact
// instead of threading an error through every one (encoding/json unwinds its
// decoder the same way). It never escapes this package.
type engineFailure struct{ err error }

// matching wraps a Matcher for the ladder's bool-valued helpers.
type matching struct{ m Matcher }

// defaultMatcher answers for a gate whose Matcher is nil. It is GoMatcher
// everywhere except under the gussetengine test tag, which points it at the
// engine so the whole policy suite runs through gussetfn.
var defaultMatcher = GoMatcher

func matcherOf(m Matcher) matching {
	if m == nil {
		m = defaultMatcher
	}
	return matching{m: m}
}

func (x matching) any(patterns []string, name string) bool {
	if len(patterns) == 0 {
		return false
	}
	ok, err := x.m.MatchAny(patterns, name)
	if err != nil {
		panic(engineFailure{err})
	}
	return ok
}

func (x matching) anyFold(patterns []string, name string) bool {
	if len(patterns) == 0 {
		return false
	}
	ok, err := x.m.MatchAnyFold(patterns, name)
	if err != nil {
		panic(engineFailure{err})
	}
	return ok
}

// failClosed converts an engineFailure unwinding out of a gate into a denial
// under rule. Any other panic is re-raised untouched.
func failClosed(d *Decision, rule RuleID, target, taskID string) {
	r := recover()
	if r == nil {
		return
	}
	f, ok := r.(engineFailure)
	if !ok {
		panic(r)
	}
	*d = deny(rule, fmt.Sprintf(
		"The pattern engine could not answer (%v), so this is refused rather than guessed: "+
			"a matcher that cannot answer has no safe allow.", f.err), target, taskID)
}
