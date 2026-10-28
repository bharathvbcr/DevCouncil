package verify

import (
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

// gitlinkReasonPrefix starts every degraded reason a gitlink produces.
const gitlinkReasonPrefix = "submodule commit range not scanned: "

// gitlinkReasons names each submodule gitlink the change touches as a check
// that did not run, in the degraded reasons Run already carries.
//
// Owner decision (2026-10-28): the commits a gitlink moved across are never
// read. `git diff HEAD` shows a move as one `Subproject commit` line, and a
// credential added in one submodule commit and removed in the next is in the
// history and in neither end, so a run that says nothing about the move has
// said it is fine. Every record with a gitlink on either side gets one reason,
// whatever its shape, and the run is degraded, never verified.
//
// A reason carries the path and a shape word and nothing from the record: no
// commit id, since the one thing known is that the commits were not read. A
// path spelled like a credential file prints as policy.WithheldSecretPath, and
// a path with a control character is quoted the way git quotes it, so a name
// cannot forge a second reason line.
func gitlinkReasons(moved []proc.GitlinkChange) []string {
	reasons := make([]string, 0, len(moved))
	for _, change := range moved {
		path := quoteDiffPath("", change.Path)
		if policy.SecretPathSpelling(change.Path) {
			path = policy.WithheldSecretPath
		}
		reasons = append(reasons, gitlinkReasonPrefix+path+" ("+change.Shape+")")
	}
	return reasons
}
