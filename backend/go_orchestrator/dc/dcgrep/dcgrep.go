// Package dcgrep is the harness's repository-search boundary.
//
// It does not implement search. Ripgrep's engine already does that — ignore-rule
// resolution, a line-oriented searcher, a regex compiler tuned for it — and a
// second implementation here would be a second set of answers to keep in step
// with the first. This package execs the `dcgrep` binary that links it and
// reads its JSON, exactly as the store and verifier boundaries do, and for the
// same reason: a process keeps CGO_ENABLED=0 and the single static binary
// intact, and a pattern that makes the engine misbehave cannot take the agent
// loop with it.
//
// What this package owns is the part a search service must not get wrong, which
// is refusing to answer when it cannot:
//
//   - A missing binary is an error naming what to build, never an empty match
//     list. `{"count":0}` means "this search ran and matched nothing", and a
//     model handed that for a search which never happened will act on it.
//     That is not hypothetical here: it is the exact shape of the regression
//     that cost a run when an unparseable pattern returned no matches.
//
//   - A binary speaking a different schema is refused rather than decoded
//     through the wrong shape.
package dcgrep

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"os/exec"
	"strings"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/safefile"
)

// BinaryEnv overrides binary discovery, so an operator who names a path means
// it. It is declared here rather than at the call site so the message that
// names the remedy and the code that reads it cannot drift apart.
const BinaryEnv = "MANVI_GREP_BINARY"

// SchemaVersion must match dcgrep's. A mismatch is refused rather than decoded
// through the wrong shape.
const SchemaVersion = 1

// identity is how the binary names itself to a health probe. A binary that
// answers with anything else is some other program that happens to sit at the
// configured path.
const identity = "dc-grep"

const (
	// defaultTimeout bounds one search. A cold walk of a large repository with
	// ignore rules to resolve is seconds, not minutes; anything past this is a
	// wedged child rather than a slow one.
	defaultTimeout = 30 * time.Second

	// maxOutput bounds the reply. The searcher caps its own match count, so a
	// legitimate result is kilobytes; this is the bound on a child gone wrong,
	// applied during the copy rather than checked after it so a runaway cannot
	// allocate the whole thing before anyone looks.
	//
	// It is load-bearing rather than nominal: 5,000 matches at the searcher's
	// 64 KiB line ceiling is 320 MiB, so this is what stands between a
	// pathological repository and that allocation.
	maxOutput = 16 << 20

	// maxListOutput bounds a *listing's* reply, which is legitimately larger.
	// A match carries a line; a path carries a path. Raising the listing
	// ceiling without raising this would have moved the truncation rather than
	// removed it.
	//
	// Measured, not estimated: listing a 200,000-file tree of deeply nested
	// paths produced 25.4 MB, 127 bytes per path including JSON quoting, in
	// 689 ms and 39 MB of child RSS. That is well over the 16 MiB above and
	// leaves 2.6x headroom here, which is what a bound on a child gone wrong
	// should look like — comfortably above every legitimate reply and nowhere
	// near the heap.
	maxListOutput = 64 << 20

	// maxStderr bounds the diagnostic half. It exists to make a failure
	// reportable, not to capture a log.
	maxStderr = 64 << 10

	// waitDelay matches the other process boundaries. Killing a child does not
	// unblock Wait while something still holds its stdout pipe.
	waitDelay = 2 * time.Second
)

// Client runs the dcgrep binary against a repository.
type Client struct {
	// Binary is the path to `dcgrep`.
	Binary string
	// Root is the repository searched. Every result path is relative to it.
	Root string
	// Timeout bounds one invocation. Zero means defaultTimeout.
	Timeout time.Duration

	// maxOutput and maxStderr bound what one invocation may hand back, in
	// bytes. They are fields rather than the constants they replaced so a test
	// can drive the bound without generating megabytes to reach it — the same
	// reason the devmap boundary carries them as fields. Nothing outside this
	// package sets them.
	maxOutput     int
	maxListOutput int
	maxStderr     int
}

// New builds a client with defaults.
func New(binary, root string) *Client {
	return &Client{
		Binary: binary, Root: root, Timeout: defaultTimeout,
		maxOutput: maxOutput, maxListOutput: maxListOutput, maxStderr: maxStderr,
	}
}

// Request is one search.
type Request struct {
	// Pattern is the regular expression to find.
	Pattern string `json:"pattern"`
	// Root is the repository. Set by Search from the client.
	Root string `json:"root"`
	// Path scopes the search under Root. Empty means the whole repository.
	Path string `json:"path,omitempty"`
	// MaxResults bounds the match list. Zero means the searcher's default.
	MaxResults int `json:"max_results,omitempty"`
	// IncludeIgnored searches files the ignore rules would exclude.
	IncludeIgnored bool `json:"include_ignored,omitempty"`
	// CaseInsensitive matches without regard to case.
	CaseInsensitive bool `json:"case_insensitive,omitempty"`
}

// Match is one matching line.
type Match struct {
	Path       string `json:"path"`
	LineNumber int    `json:"line_number"`
	Line       string `json:"line"`
	// LineTruncated marks a line the searcher clipped. A clipped line must
	// never read as the whole line.
	LineTruncated bool `json:"line_truncated,omitempty"`
}

// Skipped counts what the walk could not look inside. Every field is a hole in
// the answer's coverage, reported so a caller never infers completeness from
// the absence of a complaint.
type Skipped struct {
	TooLarge   int `json:"too_large"`
	Binary     int `json:"binary"`
	Unreadable int `json:"unreadable"`
	// UnrepresentableName counts files whose names are not valid UTF-8, so a
	// match is never reported under a path that names no file. Reachable on
	// Linux, where filenames are bytes; not on macOS, which rejects them.
	UnrepresentableName int `json:"unrepresentable_name"`
}

// Total is how many files went unsearched for any reason.
//
// Every reason belongs in this sum: it decides whether the caller reports the
// coverage note at all, so a reason left out is a file silently dropped from
// the warning that exists to say files were dropped.
func (s Skipped) Total() int {
	return s.TooLarge + s.Binary + s.Unreadable + s.UnrepresentableName
}

// Result is what the searcher returned.
type Result struct {
	OK      bool    `json:"ok"`
	Error   string  `json:"error"`
	Pattern string  `json:"pattern"`
	Count   int     `json:"count"`
	Matches []Match `json:"matches"`
	// Truncated reports that the match limit stopped the walk early.
	Truncated bool `json:"truncated"`
	// Limit is the bound that did the stopping, present when Truncated is.
	Limit         int `json:"limit"`
	FilesSearched int `json:"files_searched"`
	// FilesPruned counts files the searcher proved could not change the
	// answer and so never opened. It is only ever set alongside Truncated.
	//
	// Carried separately from FilesSearched, and deliberately not added into
	// Skipped.Total: a pruned file is not a hole in the coverage. Folding it
	// into either number would make a caller report missing coverage that was
	// proved absent, and leaving it out entirely would make FilesSearched look
	// like the searcher had gone blind over most of the tree.
	FilesPruned        int     `json:"files_pruned"`
	Skipped            Skipped `json:"skipped"`
	IgnoreRulesApplied bool    `json:"ignore_rules_applied"`
}

// ErrNoBinary is returned when no searcher is configured or none can be found.
// Callers turn it into a message naming the remedy rather than an empty result.
var ErrNoBinary = errors.New("no dcgrep binary configured")

// Search runs one search.
//
// Every error here names a fault, and none of them is reachable by a search
// that simply found nothing: that is a Result with Count zero.
func (c *Client) Search(ctx context.Context, req Request) (*Result, error) {
	if c == nil || c.Binary == "" {
		return nil, ErrNoBinary
	}
	req.Root = c.Root

	var out Result
	if err := c.run(ctx, "search", req, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// run is the one place this package execs the searcher.
//
// Both operations go through it so the bound, the caps, the group isolation
// and — most of all — the refusal to read a reply as an answer unless it says
// ok cannot be present at one call site and missing at the other.
func (c *Client) run(ctx context.Context, command string, req any, out any) error {
	body, err := json.Marshal(req)
	if err != nil {
		return fmt.Errorf("request could not be rendered: %w", err)
	}

	timeout := c.Timeout
	if timeout <= 0 {
		timeout = defaultTimeout
	}
	ctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()

	// #nosec G204 -- c.Binary is the searcher this harness configured and
	// command is one of the four literals this file passes ("search", "files",
	// "index", "rank"). Neither reaches here from a caller, let alone from a
	// model.
	cmd := exec.CommandContext(ctx, c.Binary, command)
	// See proc.ConfigureGroup. The searcher spawns nothing today, which is
	// exactly the argument that was made at the boundaries where a grandchild
	// later appeared and held the pipe open past the deadline.
	proc.ConfigureGroup(cmd)
	cmd.Stdin = bytes.NewReader(body)
	// A listing answers a different question and is bounded by a different
	// number; see maxListOutput.
	replyBound := c.outputBound()
	if command == "files" {
		replyBound = c.listOutputBound()
	}
	stdout := &proc.CappedBuffer{Limit: replyBound}
	stderr := &proc.CappedBuffer{Limit: c.stderrBound()}
	cmd.Stdout = stdout
	cmd.Stderr = stderr
	cmd.WaitDelay = waitDelay

	runErr, timedOut := proc.RunBounded(ctx, cmd.Run)
	if timedOut {
		// RunBounded abandons the goroutine on a deadline, so the buffers may
		// still be written. Nothing below reads them on this path.
		return fmt.Errorf("searcher timed out after %s", timeout)
	}
	if stdout.Overflowed() {
		// Refused rather than decoded from the prefix. A truncated reply can
		// still be valid JSON — the match array simply ends early — and
		// accepting it would report a capped sample as the whole result.
		return fmt.Errorf("searcher produced more than %d bytes", replyBound)
	}

	if err := json.Unmarshal(bytes.TrimSpace(stdout.Bytes()), out); err != nil {
		if runErr != nil {
			return fmt.Errorf("searcher failed: %w (stderr: %s)", runErr, bytes.TrimSpace(stderr.Bytes()))
		}
		return fmt.Errorf("searcher returned unparseable output: %w (%q)", err, stdout.String())
	}
	// The OK flag is the only thing separating a real empty answer from a
	// reply that decoded into a zero value — `null` and `[]` both unmarshal
	// without error, leaving every field at zero.
	if ok, reason := okOf(out); !ok {
		if reason == "" {
			reason = "no reason given"
		}
		return errors.New(reason)
	}
	// And saying ok is not the same as being usable. Everything below is a
	// property the caller is about to rely on, checked here rather than assumed
	// because the thing that produced it is a separate program in a different
	// language that this build does not compile.
	return validate(out)
}

// validate refuses a reply that said ok but cannot be believed.
//
// Every check here was written against a reply the fuzzer produced and the
// client accepted:
//
//   - `{"OK":true,"Count":1}` — Go matches JSON field names case-insensitively,
//     so a reply can claim one match while sending none. A caller reading
//     `count` and a caller reading `matches` then disagree about the same
//     answer, and one of them acts on a match that does not exist.
//   - `{"ok":true,"count":1,"paths":["/etc/passwd"]}` and `["../escape"]` — an
//     absolute or climbing path presented as repository contents. The searcher
//     does not emit these; that is exactly why the client must not depend on it
//     not emitting them. Containment asserted at one end only is containment
//     that a change at the other end silently removes.
//   - `{"ok":true,"truncated":true,"limit":0}` — truncation with no limit, so
//     the caller cannot say how much it is missing.
//   - `{"ok":true,"skipped":{"too_large":-5}}` — a negative count, which makes
//     the coverage note arithmetic meaningless.
func validate(out any) error {
	switch reply := out.(type) {
	case *Result:
		if reply.Count != len(reply.Matches) {
			return fmt.Errorf("searcher reported %d matches and sent %d", reply.Count, len(reply.Matches))
		}
		for _, match := range reply.Matches {
			if err := validPath(match.Path); err != nil {
				return fmt.Errorf("match path: %w", err)
			}
			if match.LineNumber < 1 {
				return fmt.Errorf("match in %s carries line number %d, which names no line",
					match.Path, match.LineNumber)
			}
		}
		if reply.Truncated && reply.Limit < 1 {
			return errors.New("searcher reported truncation without the limit that caused it")
		}
		return validSkipped(reply.Skipped, reply.FilesSearched)
	case *RankedResult:
		if reply.Count != len(reply.Files) {
			return fmt.Errorf("searcher reported %d ranked files and sent %d",
				reply.Count, len(reply.Files))
		}
		last := math.Inf(1)
		for _, file := range reply.Files {
			if err := validPath(file.Path); err != nil {
				return fmt.Errorf("ranked path: %w", err)
			}
			// A score that is not a number compares false against every
			// threshold, so a caller filtering by relevance would silently
			// drop the file rather than see a bad value.
			if math.IsNaN(file.Score) || math.IsInf(file.Score, 0) {
				return fmt.Errorf("ranked file %s carries score %v, which is not a number",
					file.Path, file.Score)
			}
			if file.Score <= 0 {
				return fmt.Errorf("ranked file %s scored %v, so it is not an answer",
					file.Path, file.Score)
			}
			// The order is the answer. A list that is not descending has
			// either been reordered in transit or ranked by something other
			// than the score it reports, and a caller reading the first row
			// as "best" would be wrong either way.
			if file.Score > last {
				return fmt.Errorf("ranked files are not in descending order at %s (%v after %v)",
					file.Path, file.Score, last)
			}
			last = file.Score
			if file.TermsPresent < 0 {
				return fmt.Errorf("ranked file %s holds %d terms, which is not a count",
					file.Path, file.TermsPresent)
			}
		}
		if reply.Truncated && reply.Limit < 1 {
			return errors.New("searcher reported truncation without the limit that caused it")
		}
		if reply.Vocabulary == "" {
			return errors.New("searcher ranked without naming the vocabulary it ranked in")
		}
		if reply.StaleFiles > len(reply.Files) {
			return fmt.Errorf("searcher reported %d stale files out of %d returned",
				reply.StaleFiles, len(reply.Files))
		}
		// Coverage. A ranking over a prefix of the repository is a ranking whose
		// silence means nothing, so every way the report can contradict itself
		// is refused rather than passed to a caller who would believe the
		// favourable half.
		for name, count := range map[string]int{
			"files_indexed":     reply.FilesIndexed,
			"files_seen":        reply.FilesSeen,
			"walk_errors":       reply.WalkErrors,
			"lexical_unindexed": reply.LexicalUnindexed,
		} {
			if count < 0 {
				return fmt.Errorf("ranking reported %s = %d, which is not a count", name, count)
			}
		}
		// A build cannot have indexed more files than its walk reached. Only
		// checked when the report carries a walk at all: a searcher that
		// predates `files_seen` omits it, and that is read as "not known to be
		// complete" below rather than as a contradiction.
		if (reply.FilesSeen > 0 || reply.Complete) && reply.FilesIndexed > reply.FilesSeen {
			return fmt.Errorf("ranking drew on %d files but says its build saw %d",
				reply.FilesIndexed, reply.FilesSeen)
		}
		// A file is a document or it was left out, never both, so the two counts
		// cannot exceed what the walk saw. The searcher's own reader refuses the
		// same thing. Written as a subtraction because the counts are untrusted
		// and a sum of two large ones overflows.
		if (reply.FilesSeen > 0 || reply.Complete) && reply.FilesIndexed > reply.FilesSeen-reply.LexicalUnindexed {
			return fmt.Errorf("ranking drew on %d files and says it left out %d, from a build that saw %d",
				reply.FilesIndexed, reply.LexicalUnindexed, reply.FilesSeen)
		}
		// A ceiling the client has never heard of is not one: it would be
		// passed to a reader as an explanation, and an unknown string explains
		// nothing about what the ranking is missing.
		if err := knownLimit(rankedLimitReasons, "lexical_limit_reason", reply.LexicalLimitReason); err != nil {
			return err
		}
		// Complete and stopped by a ceiling cannot both be true.
		if reply.Complete && reply.LexicalLimitReason != "" {
			return fmt.Errorf("ranking claims a complete index and names limit %q",
				reply.LexicalLimitReason)
		}
		// Nor can complete and a walk that could not read what it was shown, or
		// that left admissible files out: those are shortfalls with no ceiling
		// to name, which is exactly where a favourable `complete` would hide.
		if reply.Complete && (reply.WalkErrors > 0 || reply.LexicalUnindexed > 0) {
			return fmt.Errorf("ranking claims a complete index with %d walk errors and %d files left out",
				reply.WalkErrors, reply.LexicalUnindexed)
		}
		return nil
	case *IndexResult:
		if reply.FilesIndexed > reply.FilesSeen {
			return fmt.Errorf("index reported %d files indexed of %d seen",
				reply.FilesIndexed, reply.FilesSeen)
		}
		if reply.LexicalFiles > reply.FilesSeen {
			return fmt.Errorf("index ranked %d files but its walk only saw %d",
				reply.LexicalFiles, reply.FilesSeen)
		}
		if reply.LexicalFiles > reply.FilesSeen-reply.LexicalUnindexed {
			return fmt.Errorf("index ranked %d files and left out %d, from a walk that saw %d",
				reply.LexicalFiles, reply.LexicalUnindexed, reply.FilesSeen)
		}
		// More files ranked than trigram-indexed has exactly one cause: the
		// trigram index filled while the ranked one carried on. Every file the
		// ranked index holds is read, trusted, plain UTF-8 text, which is
		// precisely what the trigram side admits, so an index that did not fill
		// holds at least as many. A report that ranks more without naming the
		// postings ceiling is two halves disagreeing about what was published.
		if reply.LexicalFiles > reply.FilesIndexed && reply.LimitReason != "postings" {
			return fmt.Errorf("index ranked %d files but trigram-indexed %d without its postings ceiling (limit %q)",
				reply.LexicalFiles, reply.FilesIndexed, reply.LimitReason)
		}
		// A model name without a model vocabulary, or the reverse, means the
		// two halves of the report disagree about what was published — and a
		// caller deciding whether to trust a ranking reads exactly these.
		if (reply.LexicalModel != "") != (reply.LexicalVocabulary != "" &&
			reply.LexicalVocabulary != "code-v1") {
			return fmt.Errorf("index names model %q in vocabulary %q",
				reply.LexicalModel, reply.LexicalVocabulary)
		}
		// A build that stopped short must say so. `traversal_complete` is
		// what a caller reads to decide whether an empty search result means
		// anything, so it may not be true while a limit is named.
		if reply.TraversalComplete && reply.LimitReason != "" {
			return fmt.Errorf("index claims a complete traversal and names limit %q",
				reply.LimitReason)
		}
		if err := knownLimit(trigramLimitReasons, "limit_reason", reply.LimitReason); err != nil {
			return err
		}
		if err := knownLimit(rankedLimitReasons, "lexical_limit_reason", reply.LexicalLimitReason); err != nil {
			return err
		}
		for name, count := range map[string]int{
			"files_seen":        reply.FilesSeen,
			"files_indexed":     reply.FilesIndexed,
			"files_unindexed":   reply.FilesUnindexed,
			"walk_errors":       reply.WalkErrors,
			"postings":          reply.Postings,
			"lexical_files":     reply.LexicalFiles,
			"lexical_postings":  reply.LexicalPostings,
			"lexical_unindexed": reply.LexicalUnindexed,
			"lexical_unmatched": reply.LexicalUnmatched,
		} {
			if count < 0 {
				return fmt.Errorf("index reported %s = %d, which is not a count", name, count)
			}
		}
		return validLeftOut(reply.LexicalUnindexed, reply.LexicalUnindexedByClass)
	case *ListResult:
		if reply.Count != len(reply.Paths) {
			return fmt.Errorf("searcher reported %d paths and sent %d", reply.Count, len(reply.Paths))
		}
		for _, path := range reply.Paths {
			if err := validPath(path); err != nil {
				return fmt.Errorf("listed path: %w", err)
			}
		}
		if reply.Truncated && reply.Limit < 1 {
			return errors.New("searcher reported truncation without the limit that caused it")
		}
		return validSkipped(reply.Skipped, 0)
	default:
		return errors.New("the client validated a reply shape it does not know")
	}
}

// The ceilings the searcher can name, as closed sets. Empty is "none" and is
// always allowed; anything else must be one of these, so a limit the client does
// not know is refused rather than rendered as if it explained something.
//
// The ranked index's own are lexical_postings and lexical_bytes; the walk's are
// shared by both indexes and spelled the same in both fields. The trigram index
// has the one of its own, postings. A new ceiling is a new entry here and in the
// searcher at once, which is the point of closing the set.
var (
	rankedLimitReasons = map[string]bool{
		"lexical_postings": true, "lexical_bytes": true,
		"files": true, "duration": true, "input_bytes": true,
	}
	trigramLimitReasons = map[string]bool{
		"postings": true,
		"files":    true, "duration": true, "input_bytes": true,
	}
)

// leftOutClasses is the closed set of causes the searcher gives for an admissible
// file being absent from the ranked index. A cause the client does not know is
// refused, for the reason a limit it does not know is: it would be shown to a
// reader as an explanation and explain nothing.
var leftOutClasses = map[string]bool{
	"unrepresentable_name":   true,
	"unopenable":             true,
	"oversized_text":         true,
	"not_a_regular_file":     true,
	"untrusted_version":      true,
	"unreadable":             true,
	"no_metadata_after_read": true,
	"changed_while_read":     true,
	"not_in_encoding":        true,
	"term_cap":               true,
}

// MaxLeftOutSample is the most paths the searcher lists per cause, mirrored
// from its LEFT_OUT_SAMPLE so a reply that lists more is recognisably not it.
const MaxLeftOutSample = 50

// validLeftOut holds the per-cause breakdown to the total it explains.
//
// An absent breakdown is accepted: a searcher that predates it reports only the
// total, and a total with no explanation is what that searcher always sent.
// A present one must add up exactly, with no cause the client does not know,
// no sample longer than the searcher's bound or than its own count, and no path
// that is not a repository-relative one.
func validLeftOut(total int, classes map[string]LeftOut) error {
	if len(classes) == 0 {
		return nil
	}
	remaining := total
	for class, left := range classes {
		if !leftOutClasses[class] {
			return fmt.Errorf("searcher named left-out cause %q, which this client does not know", class)
		}
		if left.Count < 0 {
			return fmt.Errorf("left-out cause %s counts %d, which is not a count", class, left.Count)
		}
		if len(left.Sample) > MaxLeftOutSample {
			return fmt.Errorf("left-out cause %s lists %d paths, over the searcher's %d",
				class, len(left.Sample), MaxLeftOutSample)
		}
		if len(left.Sample) > left.Count {
			return fmt.Errorf("left-out cause %s lists %d paths for a count of %d",
				class, len(left.Sample), left.Count)
		}
		for _, path := range left.Sample {
			if err := validPath(path); err != nil {
				return fmt.Errorf("left-out cause %s: %w", class, err)
			}
		}
		// Subtracted rather than summed: the counts are untrusted, and a sum of
		// large ones wraps to a number that looks like it agrees.
		if left.Count > remaining {
			return fmt.Errorf("left-out causes count more than the %d files the index says it left out", total)
		}
		remaining -= left.Count
	}
	if remaining != 0 {
		return fmt.Errorf("left-out causes account for %d of the %d files the index says it left out",
			total-remaining, total)
	}
	return nil
}

func knownLimit(known map[string]bool, field, reason string) error {
	if reason == "" || known[reason] {
		return nil
	}
	return fmt.Errorf("searcher named %s %q, which is not a ceiling this client knows", field, reason)
}

// validPath refuses anything that is not a repository-relative file path.
//
// The rule is the same one containedRelOf applies on the tool side, asserted
// again here because this is where bytes from another process become a path a
// caller will open.
//
// A thin adapter over safefile.ValidRepoPath, which owns the rule. This used
// to be a second full implementation of it, byte-identical to the one in
// dc/dcverify — two containment checks at two subprocess boundaries, either of
// which could be tightened without the other. Delegating keeps the local name
// the call sites already use while leaving exactly one place where the rule
// can change.
func validPath(path string) error {
	return safefile.ValidRepoPath(path)
}

// validSkipped refuses counts that cannot be counts.
//
// They matter because the caller sums them to decide whether to warn that its
// answer is incomplete. A negative reason can cancel a positive one and turn a
// partial result into one that reports full coverage.
func validSkipped(skipped Skipped, filesSearched int) error {
	for name, count := range map[string]int{
		"too_large":            skipped.TooLarge,
		"binary":               skipped.Binary,
		"unreadable":           skipped.Unreadable,
		"unrepresentable_name": skipped.UnrepresentableName,
		"files_searched":       filesSearched,
	} {
		if count < 0 {
			return fmt.Errorf("searcher reported %s = %d, which is not a count", name, count)
		}
	}
	return nil
}

// okOf reads the ok flag and error text off whichever reply shape was decoded.
//
// A type switch rather than reflection or an interface on the wire structs:
// there are two shapes, both defined in this file, and a switch that stops
// compiling when a third is added is the behaviour wanted. Silently returning
// true for an unknown shape is the one thing this must not do.
func okOf(out any) (bool, string) {
	switch reply := out.(type) {
	case *Result:
		return reply.OK, reply.Error
	case *ListResult:
		return reply.OK, reply.Error
	case *RankedResult:
		return reply.OK, reply.Error
	case *IndexResult:
		return reply.OK, reply.Error
	default:
		return false, "the client decoded a reply shape it does not know how to check"
	}
}

// MaxListResults is the ceiling the searcher enforces on a listing, mirrored
// here so a caller that needs every candidate can ask for all of them by name
// rather than by guessing a large number.
//
// It must not exceed the searcher's own MAX_LIST_RESULTS; asking for more is
// clamped silently on that side, and a caller that believed the larger number
// would think it had the whole tree.
//
// This was 5000 — the searcher's *search* ceiling, which a listing used to
// share. That is the right bound on match lines a model reads and the wrong
// one on file names: the repository this package lives in crossed it at 5,408
// files, so "give me every candidate" had been returning a prefix. The
// searcher now reports both ceilings from `health`, and
// TestMaxListResultsIsNotSilentlyClampedByTheSearcher reads them rather than
// inferring one from a tree it would have to be enormous to detect.
const MaxListResults = 200000

// ListRequest is one file listing.
type ListRequest struct {
	// Root is the repository. Set by List from the client.
	Root string `json:"root"`
	// Path scopes the listing under Root. Empty means the whole repository.
	Path string `json:"path,omitempty"`
	// MaxResults bounds the path list. Zero means the searcher's default.
	MaxResults int `json:"max_results,omitempty"`
	// IncludeIgnored lists files the ignore rules would exclude.
	IncludeIgnored bool `json:"include_ignored,omitempty"`
}

// ListResult is the set of files a search would open.
type ListResult struct {
	OK                 bool     `json:"ok"`
	Error              string   `json:"error"`
	Count              int      `json:"count"`
	Paths              []string `json:"paths"`
	Truncated          bool     `json:"truncated"`
	Limit              int      `json:"limit"`
	Skipped            Skipped  `json:"skipped"`
	IgnoreRulesApplied bool     `json:"ignore_rules_applied"`
}

// List enumerates the files a search would open.
//
// It exists so the harness has one answer to "what files are in this
// repository". While the two tools walked separately, `devcouncil_find_files`
// reported `dist/generated.go` and `devcouncil_grep` would never open it —
// two readers of one repository, disagreeing, with no way for the agent
// holding both answers to tell which was about the tree it was editing.
func (c *Client) List(ctx context.Context, req ListRequest) (*ListResult, error) {
	if c == nil || c.Binary == "" {
		return nil, ErrNoBinary
	}
	req.Root = c.Root
	var out ListResult
	if err := c.run(ctx, "files", req, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// IndexRequest builds the cached indexes for a repository.
//
// One request builds both: the trigram index the exact searcher uses to skip
// files, and the ranked index Rank scores against. They are one artifact in
// one published slot, so there is no way to hold a fresh one and a stale one.
type IndexRequest struct {
	// Root is the repository. Set by Index from the client.
	Root string `json:"root"`
	// MaxFiles bounds the build. Zero means the searcher's ceiling.
	MaxFiles int `json:"max_files,omitempty"`
	// Sparse is the output of a learned sparse encoder run offline against
	// this repository. dc-sparse-encode writes it, on Apple silicon. Empty
	// means the ranked index is built from the searcher's own tokeniser and
	// BM25, which needs no model.
	//
	// The searcher never loads a model either way: a learned index carries
	// the token and weight tables a query is scored against, so this path is
	// read at build time and never again.
	Sparse string `json:"sparse,omitempty"`
}

// IndexResult is what one index build did, including what it could not do.
type IndexResult struct {
	OK                 bool   `json:"ok"`
	Error              string `json:"error"`
	Engine             string `json:"engine"`
	FilesSeen          int    `json:"files_seen"`
	FilesIndexed       int    `json:"files_indexed"`
	FilesUnindexed     int    `json:"files_unindexed"`
	WalkErrors         int    `json:"walk_errors"`
	TraversalComplete  bool   `json:"traversal_complete"`
	LimitReason        string `json:"limit_reason"`
	InputBytes         int64  `json:"input_bytes"`
	Postings           int    `json:"postings"`
	CacheWarning       string `json:"cache_warning"`
	LexicalFiles       int    `json:"lexical_files"`
	LexicalPostings    int    `json:"lexical_postings"`
	LexicalUnindexed   int    `json:"lexical_unindexed"`
	LexicalLimitReason string `json:"lexical_limit_reason"`
	LexicalVocabulary  string `json:"lexical_vocabulary"`
	// LexicalModel names the encoder whose weights were imported, and is
	// empty when the searcher computed them itself.
	LexicalModel string `json:"lexical_model"`
	// LexicalUnmatched counts documents the encoding described that the walk
	// never reached — deleted, ignored, or never in this repository. They are
	// dropped rather than indexed, so a large number means the encoding is
	// stale and the ranking covers less than the caller expects.
	LexicalUnmatched int `json:"lexical_unmatched"`
	// LexicalUnindexedByClass explains LexicalUnindexed by cause, with the first
	// paths of each. Empty from a searcher that predates it.
	LexicalUnindexedByClass map[string]LeftOut `json:"lexical_unindexed_by_class"`
}

// LeftOut is one cause for an admissible file being absent from the ranked
// index: how many, and the first few by repository-relative path.
type LeftOut struct {
	Count  int      `json:"count"`
	Sample []string `json:"sample"`
}

// RankedRequest asks which files are about something.
type RankedRequest struct {
	// Query is words, not a regular expression.
	Query string `json:"query"`
	// Root is the repository. Set by Rank from the client.
	Root string `json:"root"`
	// Path scopes the ranking under Root.
	Path string `json:"path,omitempty"`
	// MaxResults bounds the file list. Zero means the searcher's default.
	MaxResults int `json:"max_results,omitempty"`
}

// RankedFile is one file and how much it is about the query.
type RankedFile struct {
	Path  string  `json:"path"`
	Score float64 `json:"score"`
	// TermsPresent is how many of the query's terms this file holds.
	TermsPresent int `json:"terms_present"`
	// Stale marks a file whose indexed copy is no longer what is on disk.
	// The file is still ranked — it was about the query when it was read —
	// but quoting it without reading it first quotes history.
	Stale bool `json:"stale"`
}

// RankedResult is one ranking.
type RankedResult struct {
	OK        bool         `json:"ok"`
	Error     string       `json:"error"`
	Query     string       `json:"query"`
	Count     int          `json:"count"`
	Files     []RankedFile `json:"files"`
	Truncated bool         `json:"truncated"`
	Limit     int          `json:"limit"`
	// Vocabulary names the term space the ranking used, so a caller can tell
	// a BM25 ranking from a learned one without asking a second question.
	Vocabulary string `json:"vocabulary"`
	// FilesIndexed is the documents the ranking drew on.
	FilesIndexed int `json:"files_indexed"`
	// FilesSeen is the files the build that wrote the index reached, indexable
	// or not. Zero when the searcher predates the field.
	FilesSeen int `json:"files_seen"`
	// Complete is true only when the searcher said so: no ceiling stopped the
	// build that wrote this index, the walk read everything it was shown, and no
	// admissible file was left out. "Admissible" is the searcher's own rule —
	// text, so a binary or non-UTF-8 file is not a shortfall. A reply that omits
	// the field decodes to false, which is the safe reading — a ranking over part
	// of the repository must never be mistaken for one over all of it because an
	// older searcher had no way to say otherwise.
	Complete bool `json:"complete"`
	// LexicalLimitReason names the ceiling that ended the build when Complete is
	// false: "lexical_postings" or "lexical_bytes" for the ranked index's own,
	// or "files", "duration" or "input_bytes" for the walk's. Empty does not mean
	// complete; read Complete.
	LexicalLimitReason string `json:"lexical_limit_reason"`
	// WalkErrors is the directory entries the build's walk could not read.
	// Whatever was under one is absent from the ranking and from FilesSeen.
	WalkErrors int `json:"walk_errors"`
	// LexicalUnindexed is the admissible files the walk reached and the ranking
	// does not contain: unreadable, over the read bound, changed mid-read, over
	// the per-document term cap, or missing from an imported encoding.
	LexicalUnindexed int `json:"lexical_unindexed"`
	TermsUnknown     int `json:"terms_unknown"`
	TermsTotal       int `json:"terms_total"`
	TermsDropped     int `json:"terms_dropped"`
	StaleFiles       int `json:"stale_files"`
}

// Partial says, in a sentence a caller can put in front of a reader, that the
// ranking does not cover the whole repository. It is empty only when the
// searcher reported a complete index.
//
// One owner for the wording so every surface that renders a ranking says the
// same thing, and so none of them has to decide what a missing field means. The
// absence of a file from a partial ranking is not evidence that nothing in it is
// about the query.
func (r *RankedResult) Partial() string {
	if r == nil || r.Complete {
		return ""
	}
	covered := "an unknown share of the repository"
	if r.FilesSeen > 0 {
		covered = fmt.Sprintf("%d of the %d files its build reached", r.FilesIndexed, r.FilesSeen)
	}
	var causes []string
	if r.LexicalLimitReason != "" {
		causes = append(causes, fmt.Sprintf("the index build stopped at %s", r.LexicalLimitReason))
	}
	if r.WalkErrors > 0 {
		causes = append(causes, fmt.Sprintf("%d directory entries could not be read", r.WalkErrors))
	}
	if r.LexicalUnindexed > 0 {
		causes = append(causes, fmt.Sprintf("%d admissible text files were left out", r.LexicalUnindexed))
	}
	if len(causes) == 0 {
		causes = append(causes, "the searcher did not report the index as complete")
	}
	return fmt.Sprintf("ranking covers %s: %s; a file missing from it may simply not have been read",
		covered, strings.Join(causes, "; "))
}

// Index builds the cached indexes for the repository.
//
// The searcher works without them — every search falls back to a live walk —
// so this is acceleration and ranking, never correctness. A build that fails
// is reported; it does not make the repository unsearchable.
func (c *Client) Index(ctx context.Context, req IndexRequest) (*IndexResult, error) {
	if c == nil || c.Binary == "" {
		return nil, ErrNoBinary
	}
	req.Root = c.Root
	var out IndexResult
	if err := c.run(ctx, "index", req, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// Rank returns the files most about a query, in rank order.
//
// This is the question Search cannot answer. Search takes a pattern and finds
// the lines that contain it; a reader asking about "json response parsing" is
// not naming a substring, and for that question every file is a better or
// worse answer rather than a hit or a miss.
//
// An error here includes "this repository has no index", which is deliberately
// not an empty list: a repository nobody has indexed and a repository where
// nothing is about the query are different facts, and a caller that cannot
// tell them apart will report the second when it means the first.
func (c *Client) Rank(ctx context.Context, req RankedRequest) (*RankedResult, error) {
	if c == nil || c.Binary == "" {
		return nil, ErrNoBinary
	}
	req.Root = c.Root
	var out RankedResult
	if err := c.run(ctx, "rank", req, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// Available reports whether the searcher can be reached, by asking it to
// identify itself rather than by the absence of an error.
func (c *Client) Available(ctx context.Context) error {
	if c == nil || c.Binary == "" {
		return ErrNoBinary
	}
	ctx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()

	// #nosec G204 -- the configured binary and a literal subcommand.
	cmd := exec.CommandContext(ctx, c.Binary, "health")
	proc.ConfigureGroup(cmd)
	stdout := &proc.CappedBuffer{Limit: c.stderrBound()}
	cmd.Stdout = stdout
	cmd.WaitDelay = waitDelay
	if runErr, timedOut := proc.RunBounded(ctx, cmd.Run); timedOut {
		return errors.New("health probe timed out")
	} else if runErr != nil {
		return runErr
	}

	var health struct {
		OK            bool   `json:"ok"`
		Searcher      string `json:"searcher"`
		SchemaVersion int    `json:"schema_version"`
	}
	if err := json.Unmarshal(bytes.TrimSpace(stdout.Bytes()), &health); err != nil {
		return fmt.Errorf("unparseable health reply: %w", err)
	}
	if !health.OK || health.Searcher != identity {
		return fmt.Errorf("%s identified itself as %q, not %q", c.Binary, health.Searcher, identity)
	}
	if health.SchemaVersion != SchemaVersion {
		return fmt.Errorf("%s speaks schema %d, this harness speaks %d",
			c.Binary, health.SchemaVersion, SchemaVersion)
	}
	return nil
}

// outputBound, listOutputBound and stderrBound resolve the per-invocation
// caps, so a zero-valued Client built by a caller that did not go through New
// is bounded rather than unbounded. A cap that defaults to "none" is the
// failure the cap exists for.
func (c *Client) outputBound() int {
	if c.maxOutput <= 0 {
		return maxOutput
	}
	return c.maxOutput
}

func (c *Client) listOutputBound() int {
	if c.maxListOutput <= 0 {
		return maxListOutput
	}
	return c.maxListOutput
}

func (c *Client) stderrBound() int {
	if c.maxStderr <= 0 {
		return maxStderr
	}
	return c.maxStderr
}
