package verify

import (
	"strings"
)

// SanitizedAmendment is a test-amendment diff stripped of comments, docstrings,
// and natural-language rationales so an independent critic evaluates only the
// executable assertion changes without persuasive prose framing.
type SanitizedAmendment struct {
	SanitizedDiff        string
	CodeLinesAdded       int
	CodeLinesRemoved     int
	StrippedCommentLines int
}

// SanitizeTestAmendmentDiff removes comment-only added/removed lines (`//`, `#`,
// `--`, `/* ... */` blocks, and Python triple-quoted docstrings) from a unified
// diff while preserving file headers (`diff --git`, `---`, `+++`, `@@`) and
// stripping inline trailing comments on executable lines.
func SanitizeTestAmendmentDiff(unifiedDiff string) SanitizedAmendment {
	var out strings.Builder
	var added, removed, stripped int
	inBlockComment := false
	inPyDocstring := ""

	for _, rawLine := range strings.Split(unifiedDiff, "\n") {
		if strings.HasPrefix(rawLine, "diff --git ") ||
			strings.HasPrefix(rawLine, "index ") ||
			strings.HasPrefix(rawLine, "--- ") ||
			strings.HasPrefix(rawLine, "+++ ") ||
			strings.HasPrefix(rawLine, "@@ ") {
			inBlockComment = false
			inPyDocstring = ""
			out.WriteString(rawLine)
			out.WriteByte('\n')
			continue
		}
		if rawLine == "" {
			continue
		}
		prefix := rawLine[0]
		if prefix != '+' && prefix != '-' && prefix != ' ' {
			out.WriteString(rawLine)
			out.WriteByte('\n')
			continue
		}
		body := rawLine[1:]
		trimmed := strings.TrimSpace(body)
		if trimmed == "" {
			continue
		}

		if inPyDocstring != "" {
			stripped++
			if strings.Contains(trimmed, inPyDocstring) {
				inPyDocstring = ""
			}
			continue
		}
		if inBlockComment {
			stripped++
			if idx := strings.Index(trimmed, "*/"); idx >= 0 {
				inBlockComment = false
				rem := strings.TrimSpace(trimmed[idx+2:])
				if rem != "" {
					emitSanitizedLine(&out, prefix, rem, &added, &removed)
				}
			}
			continue
		}
		for _, delim := range []string{`"""`, `'''`} {
			if strings.HasPrefix(trimmed, delim) {
				stripped++
				rest := trimmed[len(delim):]
				if !strings.Contains(rest, delim) {
					inPyDocstring = delim
				}
				trimmed = ""
				break
			}
		}
		if trimmed == "" {
			continue
		}
		if strings.HasPrefix(trimmed, "/*") {
			stripped++
			if idx := strings.Index(trimmed[2:], "*/"); idx >= 0 {
				rem := strings.TrimSpace(trimmed[2+idx+2:])
				if rem != "" {
					emitSanitizedLine(&out, prefix, rem, &added, &removed)
				}
			} else {
				inBlockComment = true
			}
			continue
		}
		if strings.HasPrefix(trimmed, "//") || strings.HasPrefix(trimmed, "#") || strings.HasPrefix(trimmed, "--") {
			stripped++
			continue
		}
		cleaned := stripTrailingLineComment(body)
		if strings.TrimSpace(cleaned) == "" {
			stripped++
			continue
		}
		emitSanitizedLine(&out, prefix, cleaned, &added, &removed)
	}
	return SanitizedAmendment{
		SanitizedDiff:        out.String(),
		CodeLinesAdded:       added,
		CodeLinesRemoved:     removed,
		StrippedCommentLines: stripped,
	}
}

func emitSanitizedLine(out *strings.Builder, prefix byte, code string, added, removed *int) {
	if prefix == '+' {
		*added++
	} else if prefix == '-' {
		*removed++
	}
	out.WriteByte(prefix)
	out.WriteString(code)
	out.WriteByte('\n')
}

func stripTrailingLineComment(line string) string {
	inDouble, inSingle, inBacktick, escaped := false, false, false, false
	for i := 0; i < len(line); i++ {
		c := line[i]
		if inBacktick {
			if c == '`' {
				inBacktick = false
			}
			continue
		}
		if escaped {
			escaped = false
			continue
		}
		if (inDouble || inSingle) && c == '\\' {
			escaped = true
			continue
		}
		if !inSingle && c == '"' {
			inDouble = !inDouble
			continue
		}
		if !inDouble && c == '\'' {
			inSingle = !inSingle
			continue
		}
		if !inDouble && !inSingle {
			if c == '`' {
				inBacktick = true
				continue
			}
			if c == '#' {
				return strings.TrimRight(line[:i], " \t")
			}
			if c == '/' && i+1 < len(line) && line[i+1] == '/' {
				return strings.TrimRight(line[:i], " \t")
			}
		}
	}
	return line
}

// VerifyAmendmentStartsBroken enforces the `starts_broken` anti-tautology check
// on any amended or newly added regression test:
//  1. Against the pre-fix code (`preFix`), the test command MUST fail cleanly
//     (ExitCode > 0, neither malformed nor timed out) — proving the test actually
//     detects the defect rather than passing tautologically on every revision;
//  2. Against the post-fix code (`postFix`), the test command MUST exit 0 AND
//     pass runner attestation (`InspectTestRunnerOutcome`).
func VerifyAmendmentStartsBroken(taskID, testCommand string, preFix, postFix CommandOutcome) []Gap {
	var gaps []Gap
	cmd := testCommand
	if preFix.TimedOut || CommandIsMalformed(preFix) {
		gaps = append(gaps, Gap{
			ID:               StableGapID(taskID, "STARTSBROKEN-PRE-INVALID-"+testCommand),
			Severity:         "high",
			GapType:          "invalid_verification_command",
			TaskID:           taskID,
			Description:      "Pre-fix baseline run for amended test '" + testCommand + "' could not execute cleanly, so starts_broken was not proven.",
			Evidence:         []string{truncate(preFix.Summary, 500)},
			RecommendedFix:   "Ensure the amended test compiles and runs against the pre-fix baseline so its failure on buggy code is witnessed.",
			Blocking:         true,
			SuggestedCommand: &cmd,
		})
	} else if preFix.ExitCode == 0 {
		gaps = append(gaps, Gap{
			ID:               StableGapID(taskID, "STARTSBROKEN-VACUOUS-"+testCommand),
			Severity:         "high",
			GapType:          "acceptance_criteria_unproven",
			TaskID:           taskID,
			Description:      "Amended test '" + testCommand + "' passed (exit 0) against the pre-fix code; a tautological or weakened test does not prove the fix.",
			Evidence:         []string{"pre-fix exit code: 0"},
			RecommendedFix:   "Strengthen the regression assertion so it fails against the unmodified pre-fix behavior and passes only with the fix.",
			Blocking:         true,
			SuggestedCommand: &cmd,
		})
	}

	if postFix.ExitCode != 0 || postFix.TimedOut || CommandIsMalformed(postFix) {
		gaps = append(gaps, Gap{
			ID:               StableGapID(taskID, "STARTSBROKEN-POST-FAIL-"+testCommand),
			Severity:         "high",
			GapType:          "test_failed",
			TaskID:           taskID,
			Description:      "Amended test '" + testCommand + "' failed against the post-fix code.",
			Evidence:         []string{truncate(postFix.Summary, 500)},
			RecommendedFix:   "Fix the implementation so the regression test passes cleanly, then re-run: " + testCommand,
			Blocking:         true,
			SuggestedCommand: &cmd,
		})
	} else if reason, unattested := InspectTestRunnerOutcome(testCommand, postFix); unattested {
		gaps = append(gaps, Gap{
			ID:               StableGapID(taskID, "STARTSBROKEN-POST-UNATTESTED-"+testCommand),
			Severity:         "high",
			GapType:          "unattested_test_run",
			TaskID:           taskID,
			Description:      "Post-fix run of '" + testCommand + "' exited 0 without runner attestation: " + reason + ".",
			Evidence:         []string{reason},
			RecommendedFix:   "Remove early process exits and ensure at least one test executes and reports passing.",
			Blocking:         true,
			SuggestedCommand: &cmd,
		})
	}
	return gaps
}
