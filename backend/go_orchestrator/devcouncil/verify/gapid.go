package verify

import (
	"crypto/sha256"
	"encoding/hex"
	"regexp"
	"sort"
	"strings"
)

var nonAlnum = regexp.MustCompile(`[^A-Za-z0-9]`)

// StableGapID mirrors Python verification.gap_ids.stable_gap_id so golden
// fixtures keep matching across the hard cut.
func StableGapID(taskID, kind string, identity ...string) string {
	id := kind
	if len(identity) > 0 && identity[0] != "" {
		id = identity[0]
	}
	key := taskID + "|" + kind + "|" + id
	sum := sha256.Sum256([]byte(key))
	digest := hex.EncodeToString(sum[:])[:10]
	safe := nonAlnum.ReplaceAllString(kind, "")
	if len(safe) > 16 {
		safe = safe[:16]
	}
	if safe == "" {
		safe = "GAP"
	}
	return "GAP-" + taskID + "-" + safe + "-" + digest
}

var severityRank = map[string]int{
	"critical": 0,
	"high":     1,
	"medium":   2,
	"low":      3,
}

func gapIdentity(g Gap) string {
	file := ""
	if g.File != nil {
		file = *g.File
	}
	line := ""
	if g.Line != nil {
		line = itoa(*g.Line)
	}
	ac := ""
	if g.AcceptanceCriterionID != nil {
		ac = *g.AcceptanceCriterionID
	}
	return strings.Join([]string{
		g.GapType, file, line, ac, strings.TrimSpace(g.Description),
	}, "|")
}

// NormalizeGaps dedupes and sorts gaps the way Python normalize_verify_gaps does.
func NormalizeGaps(gaps []Gap) []Gap {
	seen := make(map[string]Gap, len(gaps))
	order := make([]string, 0, len(gaps))
	for _, g := range gaps {
		key := gapIdentity(g)
		existing, ok := seen[key]
		if !ok {
			seen[key] = g
			order = append(order, key)
			continue
		}
		if g.Blocking && !existing.Blocking {
			seen[key] = g
			continue
		}
		if g.Blocking == existing.Blocking {
			if severityRank[g.Severity] < severityRank[existing.Severity] {
				seen[key] = g
			}
		}
	}
	out := make([]Gap, 0, len(seen))
	for _, key := range order {
		if g, ok := seen[key]; ok {
			out = append(out, g)
			delete(seen, key)
		}
	}
	for _, g := range seen {
		out = append(out, g)
	}
	sort.SliceStable(out, func(i, j int) bool {
		a, b := out[i], out[j]
		if a.Blocking != b.Blocking {
			return a.Blocking
		}
		sa, sb := severityRank[a.Severity], severityRank[b.Severity]
		if sa != sb {
			return sa < sb
		}
		if a.GapType != b.GapType {
			return a.GapType < b.GapType
		}
		fa, fb := "", ""
		if a.File != nil {
			fa = *a.File
		}
		if b.File != nil {
			fb = *b.File
		}
		if fa != fb {
			return fa < fb
		}
		la, lb := 0, 0
		if a.Line != nil {
			la = *a.Line
		}
		if b.Line != nil {
			lb = *b.Line
		}
		if la != lb {
			return la < lb
		}
		return a.ID < b.ID
	})
	return out
}

func itoa(n int) string {
	if n == 0 {
		return "0"
	}
	neg := n < 0
	if neg {
		n = -n
	}
	var b [20]byte
	i := len(b)
	for n > 0 {
		i--
		b[i] = byte('0' + n%10)
		n /= 10
	}
	if neg {
		i--
		b[i] = '-'
	}
	return string(b[i:])
}

// PythonListRepr formats a string slice the way Python list.__repr__ does
// (single-quoted elements). Golden fixtures pin this exact evidence string.
func PythonListRepr(items []string) string {
	if items == nil {
		items = []string{}
	}
	parts := make([]string, len(items))
	for i, s := range items {
		parts[i] = "'" + strings.ReplaceAll(s, "'", "\\'") + "'"
	}
	return "[" + strings.Join(parts, ", ") + "]"
}
