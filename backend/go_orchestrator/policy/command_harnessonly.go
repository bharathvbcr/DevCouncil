package policy

import (
	"regexp"
	"strings"
)

// The commands only the harness may run. See RuleCommandHarnessOnly.
//
// Matched anywhere in the clause rather than at its head, so a wrapper
// (`env X=1 dev review record`, `nice`, `xargs`, a `.venv/bin/devcouncil`
// path) does not walk past it, and on the clause with its quotes and
// backslashes removed, so neither do `dev 're'view record` nor a command
// handed to `sh -c '…'` as one quoted word. The binary name is matched at a
// path boundary: `/opt/bin/devcouncil review record` is the host;
// `mydev review record` is not.
var (
	hostReviewRecordRe = regexp.MustCompile(`(?:^|[\s/;&|(=])(?:dev|devcouncil)(?:\.exe)?\s+review\s+record(?:\s|$|[;&|)])`)
	storeReviewWriteRe = regexp.MustCompile(`(?:^|[\s/;&|(=])dcstore(?:\.exe)?\s(?:.*\s)?review-append(?:\s|$|[;&|)=])`)
)

// harnessOnlyViolation reports why a clause runs a command only the harness
// may run, or "" when it does not.
func harnessOnlyViolation(clause string) string {
	bare := strings.NewReplacer(`'`, "", `"`, "", `\`, "").Replace(clause)
	switch {
	case hostReviewRecordRe.MatchString(bare):
		return "`devcouncil review record` writes the independent review an llm_review acceptance " +
			"criterion is discharged by. It is run by the harness that ran the reviewers, or by a " +
			"person, never by the agent whose work was reviewed: an agent that could record its own " +
			"review could discharge the criterion by asserting it. Finish the change and ask for the review."
	case storeReviewWriteRe.MatchString(bare):
		return "`dcstore review-append` writes the independent review an llm_review acceptance " +
			"criterion is discharged by, which only the harness that ran the reviewers may record."
	}
	return ""
}
