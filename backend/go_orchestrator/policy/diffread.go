package policy

import (
	"path"
	"strconv"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/fnmatch"
)

// WithheldSecretPath is the marker a reader is given in place of the contents
// of a file under SecretPathPatterns.
const WithheldSecretPath = "secret_path"

// UnattributedDiffText is the name WithholdSecretDiff reports for text it
// removed because no file section owned it.
const UnattributedDiffText = "(unattributed diff text)"

// SecretPathSpelling reports whether a repository-relative path, as a tool
// printed it, names a credential file.
//
// It is textual on purpose: it judges the names git writes into a diff, which
// are already repository-relative and need no filesystem to mean something.
// It tries the spelling as written and the one the write gate settles it to,
// and a spelling that will not settle counts as secret, because a check that
// cannot decide must not answer "readable".
func SecretPathSpelling(name string) bool {
	slashed := normalizeSlashes(name)
	plain := strings.TrimPrefix(path.Clean(slashed), "./")
	if fnmatch.MatchAnyFold(SecretPathPatterns, plain) {
		return true
	}
	settled, ok := settlePath(slashed)
	if !ok {
		return true
	}
	return fnmatch.MatchAnyFold(SecretPathPatterns, strings.TrimPrefix(settled, "./"))
}

// WithheldUnreadable is the rule WithholdDiff reports for a section it removed
// because its names could not be read, and for text no section owned.
const WithheldUnreadable = "unreadable_diff"

// WithheldDiff is one name WithholdDiff left out of a diff, and the rule that
// left it out.
type WithheldDiff struct {
	Path string
	Rule string
}

// WithholdSecretDiff removes from a unified diff every file section that
// touches a path under SecretPathPatterns, and returns the names it withheld.
//
// It is the one rule every reader of a working-tree diff applies before the
// text can reach a model: get_diff hands it to the agent, and verify hands it
// to Lappi. Filtering the text — rather than asking git to leave the paths out
// — means the decision is made about the bytes actually returned, so a file
// that became modified between two git invocations cannot slip through.
//
// It is WithholdDiff with the secret-path rule alone; a caller with a second
// reason to refuse a name (a file's link count, say, which no spelling shows)
// calls WithholdDiff with its own judgement instead of filtering the result a
// second time.
func WithholdSecretDiff(diff string) (string, []string) {
	kept, withheld := WithholdDiff(diff, func(name string) string {
		if SecretPathSpelling(name) {
			return WithheldSecretPath
		}
		return ""
	})
	var names []string
	for _, w := range withheld {
		names = append(names, w.Path)
	}
	return kept, names
}

// WithholdDiff removes from a unified diff every file section one of whose
// names refuse answers with a rule, and returns what it withheld and why.
//
// A section's names come from its "---"/"+++" lines and its rename and copy
// lines, which git C-quotes whenever a name is unusual. Every hunk is preceded
// by "---"/"+++", so a section carrying content always has a name to judge;
// one whose names cannot be read is withheld anyway, because content of
// unknown provenance is exactly what this exists to stop. For the same reason
// any non-empty text before the first section header is removed: git writes
// none, so text there is output this filter does not understand — colored
// output was once exactly that, and copying it through returned every secret.
// A section is removed whole, header included, so what remains is still a diff
// a parser accepts.
//
// refuse is asked once per name as git wrote it and, for a "---"/"+++" name,
// once more without git's one-segment prefix. It must fail closed: an answer
// of "" means the name may be read.
func WithholdDiff(diff string, refuse func(name string) string) (string, []WithheldDiff) {
	if diff == "" {
		return diff, nil
	}
	lines := strings.SplitAfter(diff, "\n")
	var out strings.Builder
	out.Grow(len(diff))
	var withheld []WithheldDiff
	seen := map[string]struct{}{}
	note := func(name, rule string) {
		if _, dup := seen[name]; !dup {
			seen[name] = struct{}{}
			withheld = append(withheld, WithheldDiff{Path: name, Rule: rule})
		}
	}
	for i := 0; i < len(lines); {
		if !isDiffSectionStart(lines[i]) {
			// Only reachable before the first section: a section runs to the
			// next header or the end.
			if strings.TrimSpace(lines[i]) != "" {
				note(UnattributedDiffText, WithheldUnreadable)
			} else {
				out.WriteString(lines[i])
			}
			i++
			continue
		}
		end := i + 1
		for end < len(lines) && !isDiffSectionStart(lines[end]) {
			end++
		}
		names, hasContent, readable := diffSectionNames(lines[i:end])
		rule := ""
		for _, name := range names {
			if r := refuse(name); r != "" {
				rule = r
				break
			}
		}
		if rule == "" && hasContent && !readable {
			rule = WithheldUnreadable
		}
		if rule != "" {
			for _, name := range names {
				note(name, rule)
			}
		} else {
			for _, line := range lines[i:end] {
				out.WriteString(line)
			}
		}
		i = end
	}
	return out.String(), withheld
}

// isDiffSectionStart reports whether a line opens one file's section. A hunk
// line always starts with ' ', '+', '-' or '\', so no content line can be
// mistaken for one.
func isDiffSectionStart(line string) bool {
	return strings.HasPrefix(line, "diff --git ") ||
		strings.HasPrefix(line, "diff --cc ") ||
		strings.HasPrefix(line, "diff --combined ")
}

// diffSectionNames reads the names one section's header gives, each both as
// written and without git's one-segment "a/"/"b/" prefix. A repository may set
// any prefix (diff.noprefix, diff.mnemonicPrefix, diff.srcPrefix); none can
// hide a secret path, because every SecretPathPatterns entry either has a
// "**/" twin or crosses "/", so a name still matches with a prefix before it. hasContent says the
// section carries hunks or a binary patch; readable is false when it does and
// no header line could be read.
func diffSectionNames(section []string) (names []string, hasContent bool, readable bool) {
	readable = true
	add := func(name string, prefixed bool) {
		if name == "" || name == "/dev/null" {
			return
		}
		names = append(names, name)
		if prefixed {
			if _, rest, ok := strings.Cut(name, "/"); ok && rest != "" {
				names = append(names, rest)
			}
		}
	}
	for _, raw := range section[1:] {
		line := strings.TrimRight(raw, "\r\n")
		if strings.HasPrefix(line, "@@") || strings.HasPrefix(line, "GIT binary patch") {
			hasContent = true
			break
		}
		for _, h := range []struct {
			prefix   string
			prefixed bool
		}{
			{"--- ", true}, {"+++ ", true},
			{"rename from ", false}, {"rename to ", false},
			{"copy from ", false}, {"copy to ", false},
		} {
			rest, ok := strings.CutPrefix(line, h.prefix)
			if !ok {
				continue
			}
			name, ok := unquoteDiffName(rest)
			if !ok {
				readable = false
				continue
			}
			add(name, h.prefixed)
		}
	}
	if hasContent && len(names) == 0 {
		readable = false
	}
	return names, hasContent, readable
}

// unquoteDiffName undoes git's C-style quoting, and the tab git appends to a
// "---"/"+++" name that contains a space.
func unquoteDiffName(raw string) (string, bool) {
	raw = strings.TrimSuffix(raw, "\t")
	if !strings.HasPrefix(raw, `"`) {
		return raw, true
	}
	name, err := strconv.Unquote(raw)
	if err != nil {
		return "", false
	}
	return name, true
}
