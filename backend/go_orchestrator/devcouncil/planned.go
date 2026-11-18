package devcouncil

import (
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
)

// MatchesPlannedPath reports whether path matches a planned file exactly or via
// glob. It is a delegate: policy.MatchesPlannedPath is the one planned-path
// judge, so get_diff, the write gate and verification cannot drift apart.
func MatchesPlannedPath(path string, planned []string) bool {
	entries := make([]dc.PlannedFile, len(planned))
	for i, p := range planned {
		entries[i] = dc.PlannedFile{Path: p, AllowedChange: dc.ChangeModify}
	}
	return policy.MatchesPlannedPath(path, entries)
}
