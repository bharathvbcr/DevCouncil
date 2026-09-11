package devcouncil

import (
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/fnmatch"
)

// NormalizePlannedCandidate strips a leading "./" the way write authorization does.
func NormalizePlannedCandidate(path string) string {
	normalized := strings.ReplaceAll(path, "\\", "/")
	for strings.HasPrefix(normalized, "./") {
		normalized = normalized[2:]
	}
	return normalized
}

// MatchesPlannedPath reports whether path matches a planned file exactly or via glob.
func MatchesPlannedPath(path string, planned []string) bool {
	normalized := NormalizePlannedCandidate(path)
	for _, entry := range planned {
		plannedPath := NormalizePlannedCandidate(entry)
		if normalized == plannedPath || fnmatch.Match(plannedPath, normalized) {
			return true
		}
	}
	return false
}
