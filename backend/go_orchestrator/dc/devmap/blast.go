package devmap

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"regexp"
	"sort"
	"strings"
)

// maxBlastBytes bounds a blast document read from a file or a pipe. The
// subprocess path is already capped by Client.invoke; this is the same bound
// for callers that hand DecodeBlast bytes they read themselves.
const maxBlastBytes = defaultMaxOutput

// Blast is `devmap blast --since <rev> --json`: what a change affects.
//
// It is the forward mirror of Advanced's `affected`, and it is decoded here
// rather than passed through because its callers decide what to run from it.
// The producer's own words for incompleteness are `complete` and the
// `unavailable` list, not `walk_incomplete`, `truncated` or `degraded` (those
// belong to `affected`), so a reader written against the other shape would see
// no signal at all and read every partial answer as complete.
type Blast struct {
	// Change is the producer's description of the diff, `<since>..<head sha>`
	// for a revision range.
	Change string
	// Complete is false whenever any of Unavailable or Unattributed is
	// non-empty. On a real change it is false almost always, because the
	// inbound walk stops at its depth cap.
	Complete     bool
	ChangedFiles []BlastChangedFile
	// Seeds are the symbols the change touched; the walk starts from them.
	Seeds []BlastSeed
	// Impacted is the inbound walk's reach, distance 1 being a direct caller.
	Impacted []BlastImpact
	// Tests are paths only. A path may be a file under tests/ or a source file
	// that holds #[cfg(test)] code; it does not name test symbols.
	Tests []BlastTest
	// Unattributed are changed lines that landed in no symbol, each with the
	// producer's reason. A change to a module constant lives here, and has no
	// inbound edges to walk.
	Unattributed []BlastUnattributed
	// Unavailable names every way the answer is a lower bound.
	Unavailable []BlastUnavailable
}

// BlastChangedFile is one file of the diff. Status is the producer's kind:
// modified, added, deleted, binary or unreadable.
type BlastChangedFile struct {
	Path        string
	Status      string
	RenamedFrom string
}

// BlastSeed is one changed symbol.
type BlastSeed struct {
	FilePath      string `json:"file_path"`
	QualifiedName string `json:"qualified_name"`
	StartLine     int    `json:"start_line"`
	EndLine       int    `json:"end_line"`
}

// BlastImpact is one symbol the walk reached.
type BlastImpact struct {
	Distance      int    `json:"distance"`
	FilePath      string `json:"file_path"`
	QualifiedName string `json:"qualified_name"`
}

// BlastTest is one test-bearing file the walk reached.
type BlastTest struct {
	Distance int    `json:"distance"`
	Path     string `json:"path"`
}

// BlastUnattributed is a changed range with no symbol to start a walk from.
type BlastUnattributed struct {
	Path string
	// Reason is the producer's kind: outside_every_symbol, file_not_indexed,
	// no_post_image_lines, basis_mismatch, beyond_end_of_file or
	// content_unavailable. Unknown kinds are kept, not rejected.
	Reason string
	// NearestSymbol is set only for outside_every_symbol.
	NearestSymbol string
}

// BlastUnavailable is one declared limit on the answer.
//
// Kind is kept as the producer spelled it, and an unknown kind decodes: a
// reader that refused a new kind would fail the whole run on a producer
// upgrade, and a reader that ignored it would treat a new way of being partial
// as none. Raw carries the whole entry for a log line.
type BlastUnavailable struct {
	Kind string
	// Depth and Reached are set for impact_incomplete: the cap the walk
	// stopped at and the symbols it had reached.
	Depth   *int
	Reached *int
	// Lines and Ranges are set for change_unattributed.
	Lines  *uint64
	Ranges *int
	Raw    json.RawMessage
}

// DecodeBlast reads a blast document and refuses one that lacks the
// completeness envelope. Go's decoder fills absent fields with zero values and
// reports nothing, and a blast with no `unavailable` key would then read as
// "nothing is missing". Every key a decision rests on must therefore be
// present and of the right type.
func DecodeBlast(raw []byte) (*Blast, error) {
	if len(raw) > maxBlastBytes {
		return nil, fmt.Errorf("devmap blast document exceeds %d bytes", maxBlastBytes)
	}
	trimmed := bytes.TrimSpace(raw)
	if len(trimmed) == 0 || trimmed[0] != '{' {
		return nil, errors.New("devmap blast did not return a JSON object")
	}
	var w struct {
		Change       *string `json:"change"`
		Complete     *bool   `json:"complete"`
		ChangedFiles *[]struct {
			Path        string `json:"path"`
			RenamedFrom string `json:"renamed_from"`
			Status      *struct {
				Kind string `json:"kind"`
			} `json:"status"`
		} `json:"changed_files"`
		Seeds        *[]BlastSeed   `json:"seeds"`
		Impacted     *[]BlastImpact `json:"impacted"`
		Tests        *[]BlastTest   `json:"tests"`
		Unattributed *[]struct {
			Path   string `json:"path"`
			Reason *struct {
				Kind          string `json:"kind"`
				NearestSymbol string `json:"nearest_symbol"`
			} `json:"reason"`
		} `json:"unattributed"`
		Unavailable *[]json.RawMessage `json:"unavailable"`
	}
	if err := json.Unmarshal(trimmed, &w); err != nil {
		return nil, fmt.Errorf("devmap blast returned unparseable output: %w", err)
	}
	var missing []string
	for name, present := range map[string]bool{
		"change": w.Change != nil, "complete": w.Complete != nil,
		"changed_files": w.ChangedFiles != nil, "seeds": w.Seeds != nil,
		"impacted": w.Impacted != nil, "tests": w.Tests != nil,
		"unattributed": w.Unattributed != nil, "unavailable": w.Unavailable != nil,
	} {
		if !present {
			missing = append(missing, name)
		}
	}
	if len(missing) > 0 {
		// Map iteration order is random; a message that changes between runs
		// cannot be compared or grepped.
		sort.Strings(missing)
		return nil, fmt.Errorf("devmap blast lacks required field(s) %s; its completeness cannot be qualified", strings.Join(missing, ", "))
	}

	b := &Blast{Change: *w.Change, Complete: *w.Complete, Seeds: *w.Seeds, Impacted: *w.Impacted, Tests: *w.Tests}
	for i, f := range *w.ChangedFiles {
		if err := checkRepoPath(f.Path); err != nil {
			return nil, fmt.Errorf("devmap blast changed_files[%d]: %w", i, err)
		}
		if f.Status == nil || f.Status.Kind == "" {
			return nil, fmt.Errorf("devmap blast changed_files[%d] (%s) has no status kind", i, f.Path)
		}
		b.ChangedFiles = append(b.ChangedFiles, BlastChangedFile{Path: f.Path, Status: f.Status.Kind, RenamedFrom: f.RenamedFrom})
	}
	for i, s := range b.Seeds {
		if err := checkRepoPath(s.FilePath); err != nil || s.QualifiedName == "" {
			return nil, fmt.Errorf("devmap blast seeds[%d] is not an identified symbol: %+v", i, s)
		}
	}
	for i, t := range b.Tests {
		if err := checkRepoPath(t.Path); err != nil || t.Distance < 0 {
			return nil, fmt.Errorf("devmap blast tests[%d] is not a usable repository path: %+v", i, t)
		}
	}
	for i, u := range *w.Unattributed {
		if err := checkRepoPath(u.Path); err != nil {
			return nil, fmt.Errorf("devmap blast unattributed[%d]: %w", i, err)
		}
		if u.Reason == nil || u.Reason.Kind == "" {
			return nil, fmt.Errorf("devmap blast unattributed[%d] (%s) has no reason kind", i, u.Path)
		}
		b.Unattributed = append(b.Unattributed, BlastUnattributed{Path: u.Path, Reason: u.Reason.Kind, NearestSymbol: u.Reason.NearestSymbol})
	}
	for i, entry := range *w.Unavailable {
		var u struct {
			Kind    string  `json:"kind"`
			Depth   *int    `json:"depth"`
			Reached *int    `json:"reached"`
			Lines   *uint64 `json:"lines"`
			Ranges  *int    `json:"ranges"`
		}
		if err := json.Unmarshal(entry, &u); err != nil || u.Kind == "" {
			return nil, fmt.Errorf("devmap blast unavailable[%d] carries no kind: %s", i, entry)
		}
		b.Unavailable = append(b.Unavailable, BlastUnavailable{
			Kind: u.Kind, Depth: u.Depth, Reached: u.Reached, Lines: u.Lines, Ranges: u.Ranges, Raw: entry,
		})
	}
	return b, nil
}

// checkRepoPath accepts a slash-separated path inside the repository. A path
// that escapes it, or names an absolute location, cannot be matched against
// the tree the caller will map it onto.
func checkRepoPath(p string) error {
	switch {
	case p == "":
		return errors.New("empty path")
	case strings.ContainsRune(p, 0) || strings.Contains(p, "\\"):
		return fmt.Errorf("path %q is not slash-separated text", p)
	case strings.HasPrefix(p, "/") || (len(p) > 1 && p[1] == ':'):
		return fmt.Errorf("path %q is absolute", p)
	}
	for _, part := range strings.Split(p, "/") {
		if part == ".." || part == "" || part == "." {
			return fmt.Errorf("path %q is not a normalised repository path", p)
		}
	}
	return nil
}

var fullSHA = regexp.MustCompile(`^[0-9a-f]{40}$`)

// Head is the commit the index was built at, which is the post-image of the
// change: blast always measures against it. It is the text after the last
// `..` of Change, and empty when Change carries no full commit id (an `--at`
// location, or a producer that stopped printing it). A caller that compares it
// with `git rev-parse HEAD` learns whether the index it queried is the tree it
// is about to test.
func (b *Blast) Head() string {
	at := strings.LastIndex(b.Change, "..")
	if at < 0 {
		return ""
	}
	if head := b.Change[at+2:]; fullSHA.MatchString(head) {
		return head
	}
	return ""
}

// Blast runs `devmap blast --since <since>` against the client's repository.
//
// The revision is attached with `=` and refused when it could be read as a
// flag, because it arrives from CI context and a value of `-x` would otherwise
// be parsed by clap as an option and answer for a different question.
func (c *Client) Blast(ctx context.Context, since string) (*Blast, error) {
	if since == "" || strings.HasPrefix(since, "-") || strings.ContainsAny(since, "\x00\r\n") {
		return nil, fmt.Errorf("blast --since needs a revision, got %q", since)
	}
	out, err := c.invoke(ctx, c.Timeout, nil, "blast", "--since="+since)
	if err != nil {
		return nil, err
	}
	if out.runErr != nil {
		return nil, out.failure("blast")
	}
	return DecodeBlast(out.stdout)
}
