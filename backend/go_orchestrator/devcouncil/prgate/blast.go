package prgate

import (
	"fmt"
	"sort"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/devmap"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/gitrev"
)

// Blast error, degraded, and notice codes (fixed vocabulary).
const (
	BlastErrUnavailable          = "blast_unavailable"
	BlastErrHeadMismatch         = "blast_head_mismatch"
	BlastErrUnattributedMismatch = "blast_unattributed_mismatch"
	BlastErrInvalidPath          = "blast_invalid_path"

	BlastDegFileNotIndexed     = "blast_file_not_indexed"
	BlastDegBasisMismatch      = "blast_basis_mismatch"
	BlastDegContentUnavailable = "blast_content_unavailable"
	BlastDegBeyondEndOfFile    = "blast_beyond_end_of_file"
	BlastDegUnknownUnavailable = "blast_unknown_unavailable"
	BlastDegUnknownUnattrib    = "blast_unknown_unattributed"

	BlastNoticeImpactIncomplete   = "blast_impact_incomplete"
	BlastNoticeTestsIncomplete    = "blast_affected_tests_incomplete"
	BlastNoticeOwnersUnavailable  = "blast_owners_unavailable"
	BlastNoticeOutsideEverySymbol = "blast_outside_every_symbol"
	BlastNoticeNoPostImageLines   = "blast_no_post_image_lines"
)

// QualifyBlast validates a DevMap blast result against headSHA and classifies
// every completeness signal according to the fail-closed PR Gate contract:
//
//   - Decode/invocation failure, Head() != headSHA, invalid repository paths,
//     or change_unattributed/unattributed list disagreement -> Error.
//   - impact_incomplete, affected_tests_incomplete, owners_unavailable,
//     outside_every_symbol, no_post_image_lines -> Notices only.
//   - file_not_indexed, basis_mismatch, content_unavailable,
//     beyond_end_of_file, or any unknown unavailable/unattributed kind -> Degraded.
func QualifyBlast(blast *devmap.Blast, blastErr error, headSHA string) BlastQualification {
	q := BlastQualification{
		AffectedTests: []string{},
		Degraded:      []DegradedItem{},
		Notices:       []NoticeItem{},
	}
	if blastErr != nil || blast == nil {
		q.Error = true
		q.ErrorCode = BlastErrUnavailable
		return q
	}

	gotHead := blast.Head()
	q.HeadSHA = gotHead
	if gotHead == "" || !gitrev.IsSHA40(headSHA) || gotHead != headSHA {
		q.Error = true
		q.ErrorCode = BlastErrHeadMismatch
		return q
	}

	// Validate every repository path in the blast payload before trusting it.
	for _, f := range blast.ChangedFiles {
		if err := gitrev.CheckRepoPath(f.Path); err != nil {
			q.Error = true
			q.ErrorCode = BlastErrInvalidPath
			return q
		}
	}
	for _, t := range blast.Tests {
		if err := gitrev.CheckRepoPath(t.Path); err != nil {
			q.Error = true
			q.ErrorCode = BlastErrInvalidPath
			return q
		}
	}
	for _, u := range blast.Unattributed {
		if err := gitrev.CheckRepoPath(u.Path); err != nil {
			q.Error = true
			q.ErrorCode = BlastErrInvalidPath
			return q
		}
	}

	// Check change_unattributed consistency against the unattributed list.
	hasChangeUnattrib := false
	for _, u := range blast.Unavailable {
		if u.Kind == "change_unattributed" {
			hasChangeUnattrib = true
			break
		}
	}
	if hasChangeUnattrib != (len(blast.Unattributed) > 0) {
		q.Error = true
		q.ErrorCode = BlastErrUnattributedMismatch
		return q
	}

	q.Available = true
	q.Complete = blast.Complete
	q.ChangedFilesCount = len(blast.ChangedFiles)
	q.SeedsCount = len(blast.Seeds)
	q.ImpactedCount = len(blast.Impacted)
	q.UnattributedCount = len(blast.Unattributed)

	seenTests := make(map[string]struct{}, len(blast.Tests))
	for _, t := range blast.Tests {
		if _, dup := seenTests[t.Path]; !dup {
			seenTests[t.Path] = struct{}{}
			q.AffectedTests = append(q.AffectedTests, t.Path)
		}
	}
	sort.Strings(q.AffectedTests)

	for _, u := range blast.Unavailable {
		switch u.Kind {
		case "impact_incomplete":
			msg := "Impact walk stopped at depth limit; impact counts are a lower bound."
			if u.Depth != nil && u.Reached != nil {
				msg = fmt.Sprintf("Impact walk stopped at depth %d (%d symbols reached); impact counts are a lower bound.", *u.Depth, *u.Reached)
			}
			q.Notices = append(q.Notices, NoticeItem{
				Code:    BlastNoticeImpactIncomplete,
				Message: msg,
			})
		case "affected_tests_incomplete":
			q.Notices = append(q.Notices, NoticeItem{
				Code:    BlastNoticeTestsIncomplete,
				Message: "Affected-test walk stopped at limit; test list is a lower bound.",
			})
		case "owners_unavailable":
			q.Notices = append(q.Notices, NoticeItem{
				Code:    BlastNoticeOwnersUnavailable,
				Message: "Code ownership data unavailable; not required for PR gate.",
			})
		case "change_unattributed":
			// Resolved per-entry via blast.Unattributed below.
		default:
			q.Degraded = append(q.Degraded, DegradedItem{
				Code:    BlastDegUnknownUnavailable,
				Message: "DevMap blast reported an unrecognized completeness limitation.",
			})
		}
	}

	for _, u := range blast.Unattributed {
		switch u.Reason {
		case "outside_every_symbol":
			q.Notices = append(q.Notices, NoticeItem{
				Code:    BlastNoticeOutsideEverySymbol,
				Path:    u.Path,
				Message: "Changed lines lie outside any indexed symbol; impact is a lower bound.",
			})
		case "no_post_image_lines":
			q.Notices = append(q.Notices, NoticeItem{
				Code:    BlastNoticeNoPostImageLines,
				Path:    u.Path,
				Message: "Changed hunk has no post-image lines to map to a symbol.",
			})
		case "file_not_indexed":
			q.Degraded = append(q.Degraded, DegradedItem{
				Code:    BlastDegFileNotIndexed,
				Path:    u.Path,
				Message: "Changed file is not indexed by DevMap.",
			})
		case "basis_mismatch":
			q.Degraded = append(q.Degraded, DegradedItem{
				Code:    BlastDegBasisMismatch,
				Path:    u.Path,
				Message: "DevMap index basis does not match file revision.",
			})
		case "content_unavailable":
			q.Degraded = append(q.Degraded, DegradedItem{
				Code:    BlastDegContentUnavailable,
				Path:    u.Path,
				Message: "DevMap could not read file content for blast analysis.",
			})
		case "beyond_end_of_file":
			q.Degraded = append(q.Degraded, DegradedItem{
				Code:    BlastDegBeyondEndOfFile,
				Path:    u.Path,
				Message: "Changed range falls beyond indexed file length.",
			})
		default:
			q.Degraded = append(q.Degraded, DegradedItem{
				Code:    BlastDegUnknownUnattrib,
				Path:    u.Path,
				Message: "Changed range has an unrecognized DevMap unattributed reason.",
			})
		}
	}

	q.Degraded = sortDegradedItems(q.Degraded)
	q.Notices = sortNoticeItems(q.Notices)
	return q
}
