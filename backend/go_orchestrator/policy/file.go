package policy

import (
	"errors"
	"io/fs"
	"os"
	pathpkg "path"
	"path/filepath"
	"reflect"
	"strconv"
	"strings"
	"sync/atomic"
	"unicode"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/fnmatch"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/safefile"
)

// Path pattern sets, ported verbatim from DevCouncil's policy engine. They are
// Python fnmatch patterns and are matched with those semantics — see
// internal/fnmatch for why that distinction is load-bearing.

// ProtectedWritePatterns are high-impact files: allowed, but flagged.
var ProtectedWritePatterns = []string{
	"package.json",
	"pyproject.toml",
	"uv.lock",
	"package-lock.json",
	"yarn.lock",
	"pnpm-lock.yaml",
	"Dockerfile",
	"docker-compose.yml",
	".github/workflows/*.yml",
	".github/workflows/*.yaml",
	"schema.prisma",
	"wrangler.toml",
	"index.html",
}

// SecretPathPatterns are never writable, by any task, under any grant.
//
// Every "**/x" entry needs a bare "x" beside it, and that is a rule about this
// list rather than a stylistic preference. "**/" matches one or more leading
// segments, never zero — the fnmatch parity fixture pins `**/.env  .env
// false` against Python, so the engine is right and the list must carry the
// root case itself. This is the same defect the RestrictedPathPatterns comment
// below describes for ".git".
//
// It had drifted: ".env", "id_rsa", "id_ed25519", ".npmrc", ".pypirc" and
// ".netrc" had their bare twins and "secrets/", "credentials/", "*.pem",
// "*.key", "id_dsa" and "id_ecdsa" did not, so a write to ./server.key was
// allowed while sub/server.key was refused. A pattern added here without its
// bare form is a hole at the repository root, which is where a checkout puts
// deploy keys.
var SecretPathPatterns = []string{
	".env",
	".env.*",
	"**/.env",
	"**/.env.*",
	"credentials/**",
	"**/credentials/**",
	"secrets/**",
	"**/secrets/**",
	// "*" crosses "/" in this engine, as it does in Python's fnmatch, so these
	// two subsume the "**/*.pem" and "**/*.key" forms they replace.
	"*.pem",
	"*.key",
	"**/id_rsa",
	"**/id_dsa",
	"**/id_ecdsa",
	"**/id_ed25519",
	"id_rsa",
	"id_dsa",
	"id_ecdsa",
	"id_ed25519",
	".npmrc",
	"**/.npmrc",
	".pypirc",
	"**/.pypirc",
	".netrc",
	"**/.netrc",
	// `.env.*` does not match `.envrc` (no dot after env). These are credential
	// files the `*.pem` / `credentials/**` forms also miss: a file named
	// credentials is not a directory, and `*` does not require a dot.
	".aws/credentials",
	"**/.aws/credentials",
	".envrc",
	"**/.envrc",
	".git-credentials",
	"**/.git-credentials",
	"*.p12",
}

// RestrictedPathPatterns protect the repository's own machinery — including
// the agent client configs, so an agent cannot disarm the gate that is
// checking it.
var RestrictedPathPatterns = []string{
	// These two are a deliberate addition to the incumbent's list. In a git
	// worktree or a submodule, ".git" is a *file* holding "gitdir: ...", and
	// rewriting it repoints the repository at another one — which is the whole
	// of what this rung protects, achieved without touching ".git/". Python's
	// rule is a glob plus a prefix test, and ".git".startswith(".git/") is
	// false, so the incumbent misses this. Matching it is strictly safer.
	//
	// Only these two ever got a bare twin by hand, and the omission was the
	// hole: ".claude", ".codex", ".cursor", ".gemini", ".grok", ".opencode"
	// and ".agents" carried "/*" and "/**" forms only, so a fresh checkout
	// could take a *file* named ".claude" and pre-empt the config directory
	// this rung exists to protect. Hand-added twins fix the cases; the rule
	// they state belongs to matchesRestricted, which now compares whole path
	// components and therefore covers the bare form of every entry here,
	// including any added later.
	".git",
	".devcouncil",
	".devmap",
	".git/*",
	".devcouncil/*",
	".devmap/*",
	".claude/*",
	".claude/**",
	".codex/*",
	".codex/**",
	".cursor/*",
	".cursor/**",
	".gemini/*",
	".gemini/**",
	".grok/*",
	".grok/**",
	".opencode/*",
	".opencode/**",
	".agents/*",
	".agents/**",
	"opencode.json",
}

// ToolStateDirs are the directories DevCouncil's own tools write state into:
// the task store, config and verification run logs under .devcouncil, and the
// code graph and repo map under .devmap. They are a subset of
// RestrictedPathPatterns — the write policy refuses an agent's write there — and
// the same list is what verification leaves out of the diff it judges: a file
// the harness wrote is not work the agent did, and blaming it told an obedient
// agent to revert (that is, delete) the task store.
var ToolStateDirs = []string{".devcouncil", ".devmap"}

// IsToolState reports whether a repository-relative path is inside a directory
// a DevCouncil tool owns. Folded and slash-normalised like the restricted rung,
// because that is the rung it must agree with.
func IsToolState(path string) bool {
	cleaned := dc.NormalizePlanPath(strings.TrimSpace(path))
	folded := fnmatch.Fold(cleaned)
	for _, dir := range ToolStateDirs {
		if folded == dir || strings.HasPrefix(folded, dir+"/") {
			return true
		}
	}
	return false
}

// SubsystemMap answers the two questions the neighbour rule needs. It is an
// interface so its absence is a reported degradation rather than a swallowed
// exception: the Python implementation wraps the whole neighbour lookup in a
// bare `except Exception: return None`, which turns an unreadable repo map into
// a silent fall-through to deny.
type SubsystemMap interface {
	// AreaForPath returns the subsystem owning a path, and whether one was found.
	AreaForPath(path string) (string, bool)
	// AreNeighbors reports whether two subsystems are declared neighbours.
	AreNeighbors(a, b string) bool
	// NeighborsArePermissive reports whether the neighbour relation covers so
	// much of the repository that the rung is close to allowing everything.
	//
	// It is on the interface, and not an optional one discovered by type
	// assertion, because the answer must never be absent by default. A map that
	// silently declined to answer would let every allow look tight, which is
	// the exact condition this exists to expose.
	NeighborsArePermissive() bool
}

// FileGate evaluates file writes against a task's declared scope.
type FileGate struct {
	Root string
	// Subsystems may be nil. When it is, the neighbour rung cannot run and
	// every decision that would have consulted it is marked Degraded.
	Subsystems SubsystemMap
	// AllowNeighbors mirrors flags.PolicyNeighborScope.
	AllowNeighbors bool
	// AllowSameDir mirrors flags.PolicyScopeSameDir. It only ever matters when
	// AllowNeighbors is on: the same-directory rung is the neighbour rung's
	// fallback, not a second, independent way to widen scope, so switching the
	// rung off switches off what it falls back to.
	AllowSameDir bool
	// HardRules mirrors flags.PolicyHardRules. False is reported, never silent.
	HardRules bool
	// Matcher answers the ladder's pattern questions. Nil is GoMatcher. A
	// matcher that fails turns the decision into a denial under
	// RulePathEngineUnavailable.
	Matcher Matcher
}

// EvaluateFileChange walks DevCouncil's decision ladder in its original order.
// Order is the contract: the secret-path rung must run before the task rung, or
// a task could authorise a write to .env by listing it as a planned file.
func (g FileGate) EvaluateFileChange(path string, task *dc.Task, op dc.Operation, internal bool) (d Decision) {
	normalized, outside := NormalizeRepoPath(g.Root, path)
	taskID := ""
	if task != nil {
		taskID = task.ID
	}
	mt := matcherOf(g.Matcher)
	defer failClosed(&d, RulePathEngineUnavailable, normalized, taskID, g.HardRules)

	// The directory walk behind onDiskSpelling is the dearest thing this ladder
	// does, and two rungs want its answer: ask once.
	var diskName string
	var diskDiffers, diskDone bool
	diskOnce := func() (string, bool) {
		if !diskDone {
			diskName, diskDiffers = onDiskSpelling(g.Root, normalized)
			diskDone = true
		}
		return diskName, diskDiffers
	}

	if d, done := g.preTaskDecision(mt, path, normalized, outside, internal, diskOnce, taskID); done {
		return d
	}

	if task == nil {
		d := deny(RuleNoTask, "No running DevCouncil task authorizes this file write.", normalized, "")
		return g.noteHardRules(d)
	}

	// Folded like the secret and restricted rungs above: ".ENV and .env are
	// one file" on the case-insensitive filesystems this harness ships on,
	// and a prohibition the planner wrote is voided by a case change unless
	// the match folds too. (The planned-file match below stays
	// case-sensitive on purpose: its failure direction is denial, which is
	// safe, and folding it would let a differently-cased path claim another
	// entry's authorisation.)
	forbidden := forbiddenEntries(task.ForbiddenChanges)
	if mt.anyFold(forbidden, normalized) {
		return g.noteHardRules(deny(RuleForbiddenChange, "Path is listed in forbidden_changes.", normalized, task.ID))
	}
	if entry, aliased := forbiddenAlias(g.Root, normalized, forbidden); aliased {
		return g.noteHardRules(deny(RuleForbiddenChange,
			"Path is another name for `"+entry+"`, which is listed in forbidden_changes.",
			normalized, task.ID))
	}

	planned, widened := plannedFileFor(mt, normalized, task)
	if planned == nil {
		return g.noteHardRules(g.unplannedDecision(normalized, task))
	}

	if refusal, refused := operationRefusal(planned, op, normalized, task); refused {
		// Scope an executor appended to its own task may widen what that
		// executor can reach; it must never narrow it. Without this, appending
		// "src/a.go" as a modify would turn a later *delete* of the same path
		// from scope.unplanned — soft, and on the agent-grantable list — into
		// scope.operation, which is soft but which no agent may clear. The
		// widening would have taken away a recovery the agent had before it.
		//
		// So an appended entry that does not cover the operation stands aside
		// and the path is judged as what it was before anyone appended it. The
		// planner's entries do not get this treatment: a read-only file, or one
		// planned for creation only, is a decision someone made on purpose, and
		// the whole point of the search order above is that an appended entry
		// cannot reach past it.
		if widened != "" {
			return g.noteHardRules(g.unplannedDecision(normalized, task))
		}
		return g.noteHardRules(refusal)
	}

	// Folded like the hard rungs above, and for the same reason: on APFS and
	// NTFS "PACKAGE.JSON" and "package.json" are one file, so a case-sensitive
	// match here read the pattern list and still let the write through with no
	// warning and no verification gate — the rung silently absent for exactly
	// the spelling an agent would reach for to make it absent. Over-blocking a
	// file that differs from a manifest only by case costs a warning, which is
	// all this rung ever produces.
	if mt.anyFold(ProtectedWritePatterns, normalized) {
		d := warn(RuleProtectedWrite,
			normalized+" is a protected high-impact file; verification gates must approve it.", normalized, task.ID)
		d.Widened = widened
		return g.noteHardRules(d)
	}

	if widened != "" {
		d := allow("File change is allowed by scope task "+task.ID+
			" appended at runtime (`"+widened+"`), not by the plan it was created with.",
			normalized, task.ID)
		d.Widened = widened
		return g.noteHardRules(d)
	}
	return g.noteHardRules(allow("File change is allowed.", normalized, task.ID))
}

// preTaskDecision is every rung of the ladder that runs before the task is
// read: malformed paths, the temp-dir allowance, outside-root, secret,
// restricted, their on-disk aliases, and prompt inputs. EvaluateFileChange
// returns its decision when it has one, and HardDenied asks the same function,
// so the set of paths no plan can authorise is one list rather than a copy that
// can drift from the gate.
func (g FileGate) preTaskDecision(mt matching, path, normalized string, outside, internal bool,
	diskOnce func() (string, bool), taskID string) (Decision, bool) {
	if g.HardRules {
		// Checked on the raw input and again on the normalized result: a ".."
		// segment can promote an interior component to the final one, so
		// "a /b/.." resolves to the aliasing path "a " from an input whose own
		// last component is harmless.
		if reason, bad := malformedPath(path); bad {
			return deny(RuleMalformedPath, reason, "", taskID), true
		}
		if reason, bad := malformedPath(normalized); bad {
			return deny(RuleMalformedPath, reason, normalized, taskID), true
		}
		if outside && tempWrite(g.Root, normalized) {
			return allow("Write to a temporary directory allowed.", normalized, taskID), true
		}
		if outside {
			return deny(RuleOutsideRoot, "Path is outside the project root.", normalized, taskID), true
		}
		// Case-folded: on APFS and NTFS ".ENV" and ".env" are one file, so a
		// case-sensitive check would read the pattern and still allow the write.
		if mt.anyFold(SecretPathPatterns, normalized) {
			return deny(RuleSecretPath, "Secret and credential paths are never writable.", normalized, taskID), true
		}
		if !internal && matchesRestricted(mt, normalized) {
			return deny(RuleRestrictedPath, "Protected repository paths cannot be modified.", normalized, taskID), true
		}
		// The same two rungs against the name the filesystem stores, for a path
		// that exists: a spelling the text rules do not fold to the pattern can
		// still be the file the pattern protects. See onDiskSpelling.
		if disk, differs := diskOnce(); differs {
			if mt.anyFold(SecretPathPatterns, disk) {
				return deny(RuleSecretPath, "Path is another name for `"+disk+"`, a secret or credential path, which is never writable.", normalized, taskID), true
			}
			if !internal && matchesRestricted(mt, disk) {
				return deny(RuleRestrictedPath, "Path is another name for `"+disk+"`, a protected repository path.", normalized, taskID), true
			}
		}
		// A hard link is another name the text rules cannot see: the path spells
		// nothing protected and its inode is a secret's. See protectedHardLink
		// for what is searched and what a search that cannot finish answers.
		switch kind, other := protectedHardLink(mt, g.Root, normalized, internal); kind {
		case hardLinkSecret:
			return deny(RuleSecretPath, "Path is a hard link to `"+other+"`, a secret or credential path, which is never writable.", normalized, taskID), true
		case hardLinkRestricted:
			return deny(RuleRestrictedPath, "Path is a hard link to `"+other+"`, a protected repository path.", normalized, taskID), true
		case hardLinkUnknown:
			return deny(RuleRestrictedPath, "Path has other hard links and the repository could not be searched for their names ("+other+"); "+
				"an unchecked file is not treated as clean.", normalized, taskID), true
		}
	}

	// Outside the hard-rules block on purpose: see PromptInputPatterns for what
	// that buys and what it does not. Internal writes are the host's own — a
	// host-trust assertion, as for the restricted rung — and are not judged.
	if !internal {
		if reason := g.promptInputReason(path, normalized, outside, diskOnce); reason != "" {
			return deny(RuleRestrictedPath, reason, normalized, taskID), true
		}
	}

	return Decision{}, false
}

// hardLinkWalkCap bounds the directory entries one hard-link search reads. A
// var so a test can spend it; production never writes it.
var hardLinkWalkCap = 250000

type hardLinkKind int

const (
	hardLinkNone hardLinkKind = iota
	hardLinkSecret
	hardLinkRestricted
	// hardLinkUnknown means the file has other names and the search for them
	// could not finish. It is a refusal, never a pass: a check that could not run
	// must not read the same as one that found nothing.
	hardLinkUnknown
)

// protectedHardLink reports whether the existing regular file at rel (a resolved
// repository-relative path) has another name that is a secret, a restricted
// path or a prompt input, and returns that name. Identity, not spelling: a hard
// link spells nothing protected and is the same inode, so a glob plan covering
// its directory would otherwise let the write land on the secret.
//
// A link count of 1 ends it, which is every ordinary write: one lstat on unix
// (the count is in the stat it already made, so a write-only file is no harder
// to judge than a readable one) and, where lstat carries no count, one open and
// safefile.LinkCount on the handle. os.SameFile compares dev+inode on unix and
// volume+file index on windows. Only a file with other names pays for a walk of
// the repository, and the walk stats each regular file before it classifies any
// name, so pattern matching is spent on the one or two names that are the same
// file. It does not follow symbolic links below the root and skips no directory,
// .git included: a link into .git/objects is a write to repository state.
//
// The root is resolved first (a symlinked root would otherwise read as one
// entry), and the names found are counted against the link count. A file with
// more names than the walk could see has names outside the repository (a secret
// in the home directory, a hook of the main repository seen from a linked
// worktree, a package store), and those cannot be classified: it is refused,
// with that reason, rather than read as clean. Past hardLinkWalkCap entries, or
// with a directory it cannot read, a search that found nothing is refused too.
func protectedHardLink(mt matching, root, rel string, internal bool) (hardLinkKind, string) {
	if root == "" || rel == "" {
		return hardLinkNone, ""
	}
	realRoot, err := filepath.EvalSymlinks(root)
	if err != nil {
		if _, statErr := os.Lstat(filepath.Join(root, filepath.FromSlash(rel))); statErr != nil {
			return hardLinkNone, ""
		}
		return hardLinkUnknown, "the repository root could not be resolved: " + err.Error()
	}
	info, err := os.Lstat(filepath.Join(realRoot, filepath.FromSlash(rel)))
	if err != nil || !info.Mode().IsRegular() {
		return hardLinkNone, ""
	}
	links, err := linkCount(info, filepath.Join(realRoot, filepath.FromSlash(rel)))
	if err != nil {
		return hardLinkUnknown, "its link count could not be read: " + err.Error()
	}
	if links <= 1 {
		return hardLinkNone, ""
	}

	var (
		kind       = hardLinkNone
		found      string
		incomplete string
		seen       int
		names      uint64
	)
	errStop := errors.New("search ended")
	// The error is only the stop signal or what the callback already recorded.
	_ = filepath.WalkDir(realRoot, func(p string, d fs.DirEntry, err error) error {
		if err != nil {
			// A directory that cannot be read may hold the other name; keep
			// going, and say so if nothing is found.
			incomplete = "a directory could not be read: " + err.Error()
			if d != nil && d.IsDir() {
				return fs.SkipDir
			}
			return nil
		}
		seen++
		if seen > hardLinkWalkCap {
			incomplete = "more than " + strconv.Itoa(hardLinkWalkCap) + " entries"
			return errStop
		}
		if !d.Type().IsRegular() {
			return nil
		}
		entry, statErr := d.Info()
		if statErr != nil || !os.SameFile(info, entry) {
			return nil
		}
		names++
		name, relErr := filepath.Rel(realRoot, p)
		if relErr != nil {
			return nil
		}
		name = filepath.ToSlash(name)
		if name == rel {
			return nil
		}
		switch {
		case mt.anyFold(SecretPathPatterns, name):
			kind = hardLinkSecret
		case !internal && (matchesRestricted(mt, name) || IsPromptInput(name)):
			kind = hardLinkRestricted
		default:
			return nil
		}
		found = name
		return errStop
	})
	if kind != hardLinkNone {
		return kind, found
	}
	if incomplete != "" {
		return hardLinkUnknown, incomplete
	}
	if names < links {
		return hardLinkUnknown, strconv.FormatUint(links, 10) + " names for this file but only " + strconv.FormatUint(names, 10) +
			" are inside the repository: the others are outside it or not visible, and cannot be checked against the protected paths"
	}
	return hardLinkNone, ""
}

// linkCount is the number of names of the regular file info describes. On unix
// the stat already holds it (Nlink); elsewhere it is read from an open handle.
func linkCount(info os.FileInfo, full string) (uint64, error) {
	if sys := reflect.ValueOf(info.Sys()); sys.Kind() == reflect.Pointer && !sys.IsNil() && sys.Elem().Kind() == reflect.Struct {
		if n := sys.Elem().FieldByName("Nlink"); n.IsValid() && n.CanUint() {
			return n.Uint(), nil
		}
	}
	f, err := os.Open(full)
	if err != nil {
		return 0, err
	}
	defer f.Close()
	return safefile.LinkCount(f)
}

// hardDeniedEvaluations counts HardDenied's runs of the pre-task rungs, so a
// test can bound how often a caller pays for them.
var hardDeniedEvaluations atomic.Int64

// HardDeniedEvaluations is how many times HardDenied has run in this process.
func HardDeniedEvaluations() int64 { return hardDeniedEvaluations.Load() }

// textOnlyRoot is the root HardDenied asks with when its caller has none: a
// directory that cannot exist, so every on-disk rung (aliases, hard links)
// finds nothing and only the textual rungs speak.
const textOnlyRoot = "/nonexistent/devcouncil-text-only-root"

// HardDenied reports whether the write gate refuses path before it ever reads a
// task: a malformed spelling (raw or cleaned), a path outside the root, a
// secret, restricted or tool-state path, a prompt input, or the on-disk alias of
// one. It asks preTaskDecision, the function EvaluateFileChange itself calls, so
// the two cannot drift. root may be "" when the caller has none; then only the
// textual rungs run, which is a lower bound, never a license. An engine that
// cannot answer counts as denied, as it does in the gate.
func HardDenied(root, path string) bool {
	hardDeniedEvaluations.Add(1)
	d, outside := hardDecision(root, path)
	return outside || d.Blocked()
}

func hardDecision(root, path string) (d Decision, outside bool) {
	mt := matching{m: GoMatcher}
	defer failClosed(&d, RulePathEngineUnavailable, "", "", true)
	g := FileGate{Root: root, HardRules: true}
	var normalized string
	diskOnce := func() (string, bool) { return "", false }
	if root == "" {
		g.Root = textOnlyRoot
		cleaned, settled := settlePath(normalizeSlashes(path))
		outside = !settled || filepath.IsAbs(cleaned) || cleaned == ".." || strings.HasPrefix(cleaned, "../")
		normalized = cleaned
	} else {
		normalized, outside = NormalizeRepoPath(root, path)
		diskOnce = func() (string, bool) { return onDiskSpelling(root, normalized) }
	}
	if outside {
		return Decision{}, true
	}
	d, _ = g.preTaskDecision(mt, path, normalized, outside, false, diskOnce, "")
	return d, false
}

// lendsScope reports whether a planned entry may lend its neighbourhood to the
// unplanned-scope rungs. An allow-list, not "anything but read_only": an entry
// whose change is not one this gate knows must lend nothing, since the only
// reading that never widens is the one where an unknown word grants nothing.
func lendsScope(pf dc.PlannedFile) bool {
	switch pf.AllowedChange {
	case dc.ChangeCreate, dc.ChangeModify, dc.ChangeDelete:
		return true
	}
	return false
}

// operationRefusal reports whether a planned entry forbids the operation being
// attempted, and the denial that says so.
//
// Split out because the answer is consumed twice — once to refuse, and once to
// decide whether an appended entry should stand aside — and a second copy of
// this ladder rung would be a second thing to keep in step with DevCouncil.
func operationRefusal(planned *dc.PlannedFile, op dc.Operation, path string, task *dc.Task) (Decision, bool) {
	if planned.AllowedChange == dc.ChangeReadOnly {
		return deny(RuleReadOnly, "Planned file is read-only.", path, task.ID), true
	}
	if op == dc.OpWrite {
		if planned.AllowedChange != dc.ChangeCreate && planned.AllowedChange != dc.ChangeModify {
			return deny(RuleOperation,
				"Operation "+string(op)+" not allowed for planned file.", path, task.ID), true
		}
		return Decision{}, false
	}
	if string(planned.AllowedChange) != string(op) {
		return deny(RuleOperation,
			"Operation "+string(op)+" not allowed for planned file.", path, task.ID), true
	}
	return Decision{}, false
}

// unplannedDecision is the soft-block rung: a write outside the plan is allowed
// when it lands close enough to it — in the same subsystem as a planned file,
// in a declared neighbour of one, or, when the repo map cannot say, in the same
// directory as one.
//
// Only the *plan* lends its neighbourhood, never scope an executor appended to
// itself. Otherwise the widening ratchets: an agent argues one file into scope,
// and that file's subsystem — or directory — becomes writable, which is a
// larger permission than the one anybody granted it.
// EvaluateRead decides whether this agent may see the bytes at path.
//
// It is deliberately not EvaluateFileChange with a read flag. A write is judged
// against the task's declared scope, because writing outside a plan is what
// scope exists to catch; reading is how an agent orients, and refusing every
// read outside the plan would make the harness unusable while protecting
// nothing. So this rung asks one narrower question — is this a path whose
// contents are credentials — and leaves scope alone.
//
// Nothing here consults the task, and that is the point: no plan and no grant
// makes a deploy key readable. It answers only when hard rules are on, exactly
// like the write rung it sits beside, so a posture that turns enforcement off
// turns this off too rather than pretending otherwise.
func (g FileGate) EvaluateRead(path string, task *dc.Task) (d Decision) {
	normalized, outside := NormalizeRepoPath(g.Root, path)
	taskID := ""
	if task != nil {
		taskID = task.ID
	}
	mt := matcherOf(g.Matcher)
	defer failClosed(&d, RulePathEngineUnavailable, normalized, taskID, g.HardRules)
	if !g.HardRules {
		return g.noteHardRules(allow("Read is not gated when hard rules are off.", normalized, taskID))
	}
	if reason, bad := malformedPath(path); bad {
		return deny(RuleMalformedPath, reason, "", taskID)
	}
	if reason, bad := malformedPath(normalized); bad {
		return deny(RuleMalformedPath, reason, normalized, taskID)
	}
	if outside {
		return deny(RuleOutsideRoot, "Path is outside the project root.", normalized, taskID)
	}
	// Case-folded for the same reason the write rung is: ".ENV" and ".env" are
	// one file on APFS and NTFS.
	if mt.anyFold(SecretPathPatterns, normalized) {
		return deny(RuleSecretRead,
			"Secret and credential paths are not readable. Their contents would enter the model's "+
				"context, which no later redaction can undo.", normalized, taskID)
	}
	if disk, differs := onDiskSpelling(g.Root, normalized); differs && mt.anyFold(SecretPathPatterns, disk) {
		return deny(RuleSecretRead,
			"Path is another name for `"+disk+"`, a secret or credential path. Their contents would enter "+
				"the model's context, which no later redaction can undo.", normalized, taskID)
	}
	return allow("Read is allowed.", normalized, taskID)
}

// ReadClass is what a reader is told about one path: allowed, refused because
// it escapes the root, or refused because its contents are credentials.
type ReadClass int

const (
	ReadAllowed ReadClass = iota
	// ReadOutsideRoot is a path that resolves outside the repository. A caller
	// filtering a result set treats it as an attack on the read, not as a file
	// to leave out.
	ReadOutsideRoot
	// ReadSecretPath is a path under SecretPathPatterns. A caller filtering a
	// result set omits its contents and says so, rather than failing the whole
	// result: one credential file must not make every other file unreadable.
	ReadSecretPath
)

// ClassifyRead reports whether a path would be refused to a reader, and why,
// without building a decision for it.
//
// It exists for the callers that filter a result set — a diff returning hunks
// from many files — where the question is asked once per file and a full
// decision per hit would be recorded noise rather than a record.
//
// It uses fnmatch directly, not a Matcher. It has nowhere to put an engine
// error except "refused", and it is asked once per file of a result; its
// answer is the one a FileGate with GoMatcher gives, which every Matcher is
// required to equal.
func ClassifyRead(root, path string) ReadClass {
	normalized, outside := NormalizeRepoPath(root, path)
	if outside {
		return ReadOutsideRoot
	}
	if fnmatch.MatchAnyFold(SecretPathPatterns, normalized) {
		return ReadSecretPath
	}
	if disk, differs := onDiskSpelling(root, normalized); differs && fnmatch.MatchAnyFold(SecretPathPatterns, disk) {
		return ReadSecretPath
	}
	return ReadAllowed
}

// scopeRemedy closes every unplanned-scope refusal. It used to say "Expand
// scope with `dev scope update`", a Python CLI command that exits 2 on the Go
// host and has no equivalent in it or in Manvi: no tool widens a task's planned
// files, so the honest remedy names the person who can.
const scopeRemedy = "Widening a task's planned files is a decision for the task's owner; " +
	"ask them rather than writing outside the plan."

func (g FileGate) unplannedDecision(path string, task *dc.Task) Decision {
	base := deny(RuleUnplannedScope,
		"Task "+task.ID+" does not authorize changes to "+path+".", path, task.ID)

	if !g.AllowNeighbors {
		base.Degraded = append(base.Degraded, "scope.neighbors.disabled")
		return base
	}
	if g.Subsystems == nil {
		// The Python engine returns None here and falls through to deny. Same
		// outcome, but recorded: a deny reached without the map is not evidence
		// that the path is outside the plan's neighbourhood.
		base.Degraded = append(base.Degraded, "repo_map.unavailable")
		return g.sameDirDecision(path, task, base)
	}

	targetArea, ok := g.Subsystems.AreaForPath(path)
	if !ok || targetArea == "" {
		base.Degraded = append(base.Degraded, "subsystem.unknown_for_target")
		return g.sameDirDecision(path, task, base)
	}
	if isRootArea(targetArea) {
		base.Degraded = append(base.Degraded, "scope.root_not_a_subsystem")
		return g.sameDirDecision(path, task, base)
	}

	plannedAreas := make([]string, 0, len(task.PlannedFiles))
	seen := map[string]bool{}
	droppedRoot := false
	for _, pf := range task.PlannedFiles {
		// A read-only entry says "look at this, do not change it". Lending its
		// subsystem as writable would turn that into a permission over every
		// file beside it, which is the opposite of what it declares.
		if !lendsScope(pf) {
			continue
		}
		area, ok := g.Subsystems.AreaForPath(normalizeSlashes(pf.Path))
		if !ok || area == "" || seen[area] {
			continue
		}
		if isRootArea(area) {
			// A planned file at the root lends nothing, for the reason
			// isRootArea states. Recorded so the refusal below names the rule
			// rather than reporting a map that answered as one that did not.
			droppedRoot = true
			continue
		}
		seen[area] = true
		plannedAreas = append(plannedAreas, area)
	}
	if len(plannedAreas) == 0 {
		if droppedRoot {
			base.Degraded = append(base.Degraded, "scope.root_not_a_subsystem")
		} else {
			base.Degraded = append(base.Degraded, "subsystem.unknown_for_planned")
		}
		return g.sameDirDecision(path, task, base)
	}

	for _, area := range plannedAreas {
		if area == targetArea {
			d := allow("File is in the same subsystem as a planned file.", path, task.ID)
			if g.Subsystems.NeighborsArePermissive() {
				// The map reports its own rungs as near-total, and this one
				// degenerates faster than the neighbour rung below it: a graph
				// holding a single named area puts every indexed file in "the
				// same subsystem as a planned file", so the question has one
				// possible answer and every unplanned write was recorded as a
				// scope check that ran and passed. isRootArea already covers a
				// whole-repo area spelled ".", which is why the named-single-area
				// shape survived — same condition, a spelling the guard did not
				// recognise.
				//
				// Its own marker, not the neighbour one: the neighbour relation
				// did not decide this allow, and a signal that names the wrong
				// rung is a signal an operator learns to discount. Recorded
				// rather than refused, for the reason the neighbour rung states.
				d.Degraded = append(d.Degraded, "scope.subsystem.permissive")
			}
			return d
		}
	}
	for _, area := range plannedAreas {
		if g.Subsystems.AreNeighbors(targetArea, area) {
			d := allow("File is in a neighboring subsystem of a planned file.", path, task.ID)
			if g.Subsystems.NeighborsArePermissive() {
				// The rung ran and said yes, and in this repository it says yes
				// to nearly everything: the widest area neighbours at least
				// half the map, so "is the target next door to the plan" is a
				// question with almost one answer.
				//
				// Recorded rather than refused. The relation is a fact about
				// how the repository is coupled, not a misconfiguration, and
				// denying on it would break every task in a densely connected
				// tree. What must not happen is this allow reading like a
				// scope check that meant something — `manvi map status` has
				// been reporting the condition all along, to an operator who
				// would have to know to go and look.
				d.Degraded = append(d.Degraded, "scope.neighbors.permissive")
			}
			return d
		}
	}

	// The map answered, and its answer was no. Proximity is a fallback for a
	// question that could not be asked, never a second opinion on one that was.
	base.Reason = "Task " + task.ID + " does not authorize changes to " + path +
		" (subsystem `" + targetArea + "` is outside planned files and not a declared neighbor). " +
		scopeRemedy
	return base
}

// sameDirDecision is the neighbour rung's fallback: with no repo map to consult,
// a write is allowed when it lands in the same directory as a file the plan
// authorises changing.
//
// This exists because the alternative is a cliff. Without a built repo map the
// subsystem rung cannot run at all, so *every* write outside the enumerated
// paths is refused — including the test file beside the file being fixed, and
// the helper it calls. The map is an artifact a repository may simply not have
// built, and "we have no map" is not evidence that a sibling file is out of
// scope. A directory is the coarsest honest stand-in for a subsystem: it is
// what the repository itself groups by, and it is bounded by construction.
//
// Two limits keep it a fallback rather than a hole. The repository root is not a
// directory for this purpose — it is where the build, the CI config, and the
// dependency manifests live, and one planned file at the top level would
// otherwise lend writability to all of them. And the allow keeps the degradation
// that brought it here, so a run that leaned on proximity can never be reported
// as one where the scope rules were fully checked.
func (g FileGate) sameDirDecision(path string, task *dc.Task, base Decision) Decision {
	if !g.AllowSameDir {
		base.Degraded = append(base.Degraded, "scope.same_dir.disabled")
		return base
	}

	dir, ok := scopeDir(path)
	if !ok {
		base.Reason = "Task " + task.ID + " does not authorize changes to " + path +
			", the repo map that would place it in a subsystem is unavailable, and a path " +
			"at the repository root has no directory that could stand in for one. " +
			scopeRemedy
		return base
	}

	for _, pf := range task.PlannedFiles {
		if !lendsScope(pf) {
			continue
		}
		plannedDir, ok := scopeDir(pf.Path)
		if !ok || plannedDir != dir {
			continue
		}
		allowed := allow("File is in the same directory (`"+dir+"`) as planned file `"+
			pf.Path+"`; the repo map that would place it in a subsystem is unavailable.",
			path, task.ID)
		// The reason the map could not answer travels onto the allow. A pass
		// reached by proximity is not the pass the subsystem rung would have
		// produced, and nothing downstream may treat the two as one.
		allowed.Degraded = base.Degraded
		return allowed
	}

	base.Reason = "Task " + task.ID + " does not authorize changes to " + path +
		" (no planned file is writable in `" + dir + "`, and the repo map that would " +
		"place it in a subsystem is unavailable). " + scopeRemedy
	return base
}

// scopeDir is the directory a path lends or borrows scope through, and whether
// it has one at all.
//
// It has none in two cases. A path at the repository root gets no directory, for
// the reason sameDirDecision states. A pattern whose directory part still holds
// glob metacharacters — `src/*/main.go` — names a set of directories rather than
// one, and comparing that set's textual form against a real directory would
// either never match or match by accident.
// isRootArea reports whether a subsystem is the repository root.
//
// This is scopeDir's rule applied one rung higher, and it was missing there.
// scopeDir refuses to let a path at the root lend scope through its directory
// because the root is where the build, the CI config and the dependency
// manifests live. A code graph groups top-level files under `.` — this
// repository's own graph does, for verify.sh — which offers the subsystem rung
// exactly that widening under a different name, past a guard that only ever
// existed in the fallback beneath it. One planned file at the top level would
// have made every other one writable, and the allow would have read
// `File is in the same subsystem as a planned file.`
func isRootArea(area string) bool {
	switch strings.TrimSpace(normalizeSlashes(area)) {
	case "", ".", "/", "./":
		return true
	}
	return false
}

func scopeDir(p string) (string, bool) {
	normalized := dc.NormalizePlanPath(p)
	idx := strings.LastIndex(normalized, "/")
	if idx <= 0 {
		return "", false
	}
	dir := normalized[:idx]
	if strings.ContainsAny(dir, "*?[") {
		return "", false
	}
	return dir, true
}

// noteHardRules marks any decision reached while the hard rules were disabled.
// Turning them off is a startup-only, human-only act, and every decision made
// under that configuration says so.
func (g FileGate) noteHardRules(d Decision) Decision {
	if !g.HardRules {
		d.Degraded = append(d.Degraded, "policy.hard_rules.disabled")
	}
	return d
}

// malformedPath rejects paths whose textual form and whose meaning to the
// kernel are not the same thing. A NUL terminates the path at every syscall
// boundary, so "src/a.go\x00.env" is matched against the secret patterns as one
// string and opened as another. Nothing legitimate contains one.
func malformedPath(raw string) (string, bool) {
	// Control characters are refused as a class, and NUL is named separately
	// because its failure mode is the worst: it terminates the path at every
	// syscall boundary, so "src/a.go\x00.env" is matched against the secret
	// patterns as one string and opened as another.
	//
	// The rest are refused for two reasons that reinforce each other. A newline
	// in a filename aliases — surrounding whitespace is trimmed from a path on
	// its way in, so "a\n" and "a" become the same string on a second pass but
	// name different files — and it also breaks every line-oriented consumer
	// downstream: the diff parser that finds the changed files, the
	// newline-delimited transcript, the terminal renderer. Nothing legitimate
	// has one.
	if i := strings.IndexFunc(raw, unicode.IsControl); i >= 0 {
		if raw[i] == 0 {
			return "Path contains a NUL byte, which the filesystem would truncate at.", true
		}
		return "Path contains the control character " + strconv.QuoteRune(rune(raw[i])) +
			", which aliases between names and breaks every line-oriented consumer of this path.", true
	}
	// Whitespace padding or a trailing dot on any component aliases to a
	// different file. Windows strips both when opening, so ".env " and ".env"
	// are one file there and two here; and this normalizer itself trims
	// surrounding whitespace, so " a" and "a" collapse to one string on a
	// second pass while naming two files on disk. That is the same class of bug
	// as the case-variant bypass — a check that is a statement about strings
	// rather than about files — and nothing legitimate has one, so it is
	// refused rather than normalised into something that looks valid.
	cleaned := normalizeSlashes(cleanInput(raw))
	for _, component := range strings.Split(cleaned, "/") {
		if component == "." || component == ".." || component == "" {
			continue
		}
		if trimmed := strings.TrimSpace(component); trimmed != component {
			return "Path component " + strconv.Quote(component) +
				" is padded with whitespace, which aliases: this path is cleaned of surrounding " +
				"whitespace on its way in, so it and its trimmed form would be one string here " +
				"and two files on disk.", true
		}
		if component != strings.TrimRight(component, ".") {
			return "Path component " + strconv.Quote(component) +
				" ends in a dot, which some filesystems strip when opening — " +
				"it would not name the file the check examined.", true
		}
	}
	return "", false
}

// cleanInput strips the wrapping a path may arrive with — surrounding
// whitespace, and a matched pair of surrounding quotes — until it settles.
//
// Two defects a fuzzer found are fixed here, and both had the same shape: the
// function did not settle, so the same input normalized to two different paths
// on successive calls, and a normalizer that does not settle is one where the
// matcher and the filesystem can be looking at different strings.
//
//   - Whitespace was trimmed before quotes, so a quote could hide trailing
//     whitespace that then survived. Hence the loop.
//   - Quotes were stripped with strings.Trim, which removes them from either
//     end independently — so a path legitimately ending in a quote character
//     lost it, even though nothing was wrapping it. Only a matched pair is
//     unwrapping; a lone quote is part of the name.
func cleanInput(raw string) string {
	for {
		trimmed := strings.TrimSpace(raw)
		if len(trimmed) >= 2 && trimmed[0] == '"' && trimmed[len(trimmed)-1] == '"' {
			trimmed = trimmed[1 : len(trimmed)-1]
		}
		if trimmed == raw {
			return trimmed
		}
		raw = trimmed
	}
}

// tempDirs are the machine's temporary directories. A variable so a test can
// point it at directories of its own.
var tempDirs = func() []string { return []string{os.TempDir(), "/tmp", "/private/tmp"} }

// tempWrite reports whether a path NormalizeRepoPath placed outside root lies
// inside one of tempDirs, each with its own symlinks resolved, while root does
// not.
//
// Writes there are allowed although they leave the project (owner decision,
// 2026-10-24). A host tells its agent to keep scratch files in exactly these
// places — Claude Code names a per-session scratchpad under the temporary
// directory — and refusing them made the harness contradict the host on every
// session, while nothing the repository or the person's credentials hold lives
// there. The path given is the one NormalizeRepoPath returned, which has
// already followed every symlink that exists, dangling ones included, so a link
// under /tmp that leads to ~/.ssh is judged where it leads and is still
// outside.
//
// A root that itself lives in a temporary directory gets no such allowance:
// there the temporary directory is the repository's own surroundings — its
// parent, its siblings — which is what the outside-root rung protects, and a
// `../x` from it would otherwise read as a scratch write.
func tempWrite(root, resolved string) bool {
	if resolved == "" || !filepath.IsAbs(filepath.FromSlash(resolved)) {
		return false
	}
	rootResolved, ok := resolveExisting(root)
	if !ok {
		return false
	}
	path := filepath.Clean(filepath.FromSlash(resolved))
	inside := false
	for _, dir := range tempDirs() {
		if dir == "" {
			continue
		}
		base, ok := resolveExisting(dir)
		if !ok {
			continue
		}
		if within(base, rootResolved) || base == rootResolved {
			return false
		}
		inside = inside || within(base, path)
	}
	return inside
}

// within reports whether path lies strictly below base.
func within(base, path string) bool {
	rel, err := filepath.Rel(base, path)
	if err != nil || rel == "." || rel == ".." {
		return false
	}
	return !strings.HasPrefix(rel, ".."+string(filepath.Separator))
}

// NormalizeRepoPath resolves raw against root and reports containment.
//
// It returns (posix path relative to root, isOutsideRoot). Callers fail closed
// on the second value: anything that escapes the project — an absolute path
// elsewhere, a ".." traversal, a symlink pointing out — is outside.
func NormalizeRepoPath(root, raw string) (string, bool) {
	// A NUL terminates the path at every syscall boundary, so the kernel and
	// the pattern matcher would be looking at different strings: "a.go\x00.env"
	// matches no secret pattern and opens ".env"-adjacent paths depending on the
	// caller. Nothing legitimate contains one, so it is refused as uncontained
	// rather than sanitised into something that looks valid.
	if _, bad := malformedPath(raw); bad {
		// Defence in depth for the caller that does not run the ladder: a path
		// whose text and whose meaning to the kernel differ is never contained.
		return "", true
	}
	cleaned, settled := settlePath(normalizeSlashes(raw))
	if !settled {
		// A path whose normalisation does not converge is one where this
		// function and the filesystem can be looking at different strings, which
		// is the whole failure this normalizer exists to prevent. Refused as
		// uncontained rather than used at whatever round the loop gave up on.
		return "", true
	}

	rootResolved, rootOK := resolveExisting(root)
	if !rootOK {
		// Without a resolved root there is nothing to be contained *by*, so
		// every answer below would be a guess. Uncontained is the honest one.
		return "", true
	}

	var candidate string
	if filepath.IsAbs(cleaned) {
		candidate = cleaned
	} else {
		candidate = filepath.Join(rootResolved, cleaned)
	}
	resolved, resolvedOK := resolveExisting(candidate)
	if !resolvedOK {
		return "", true
	}

	rel, err := filepath.Rel(rootResolved, resolved)
	if err != nil {
		return dc.NormalizePlanPath(cleaned), true
	}
	rel = filepath.ToSlash(rel)
	if rel == ".." || strings.HasPrefix(rel, "../") {
		return filepath.ToSlash(resolved), true
	}
	if rel == "." {
		return "", true
	}
	return rel, false
}

// settlePath applies the two normalisations to a fixed point, and reports
// whether it reached one.
//
// Neither step is idempotent with respect to the other, so a fixed number of
// rounds is a guess about how many an input needs. Running each of them once,
// then once more, was such a guess, and a fuzzer found the input that needs a
// third: `""0"/"/` has its trailing slash removed by Clean, which exposes a
// quote pair that the second cleanInput unwraps to `"0"/` — still holding a
// trailing slash, and still holding quotes. It normalised to `"0"`, and `"0"`
// normalised again to `0`: two spellings the gate would evaluate as two
// different files while the filesystem opens one.
//
// Iterating removes the guess. The bound is a backstop, not the mechanism —
// each round can only shorten the string or leave it unchanged, so convergence
// is quick — and exhausting it is reported rather than swallowed, because the
// caller must fail closed on a path that will not settle.
func settlePath(raw string) (string, bool) {
	cleaned := raw
	for i := 0; i < 16; i++ {
		next := filepath.ToSlash(filepath.Clean(cleanInput(cleaned)))
		if next == cleaned {
			return cleaned, true
		}
		cleaned = next
	}
	return "", false
}

// resolveExisting resolves symlinks as far up the path as actually exists,
// then reattaches the remainder. Go's EvalSymlinks fails outright on a path
// that does not exist yet, and a write gate is asked about new files
// constantly — but skipping symlink resolution entirely would let a symlinked
// directory carry a write out of the repository.
// The second return value is whether the path could be resolved at all.
// Callers fail closed on false: a path this function could not follow is one
// where it and the kernel may be looking at different files, which is the whole
// condition it exists to prevent.
//
// A component that does not resolve is not automatically a component that does
// not exist, and conflating the two was a hole. EvalSymlinks reports ENOENT for
// a *dangling* symlink — one whose target is missing — exactly as it does for a
// name that was never created. The old loop peeled such a component off and
// reattached it literally, which discarded the link and answered about the
// link's own name rather than where it points: `notes.md ->
// /elsewhere/authorized_keys` normalised to `notes.md`, was reported contained,
// and the write that *created* the target landed outside the repository. Only a
// second write, once the target existed, was refused — so the escaping write was
// always the first one. Git carries dangling symlinks in a tree, so a clone can
// deliver this.
func resolveExisting(p string) (string, bool) {
	abs, err := filepath.Abs(p)
	if err != nil {
		return "", false
	}
	cur := abs
	rest := ""
	// One bound for both kinds of step: peeling a component and following a
	// link. Real paths are bounded by the OS and link chains by ELOOP, so
	// exhausting this means the input was built to exhaust it — reported rather
	// than answered at whatever round the loop gave up on.
	for steps := 0; steps < 256; steps++ {
		if resolved, err := filepath.EvalSymlinks(cur); err == nil {
			if rest == "" {
				return resolved, true
			}
			return filepath.Join(resolved, rest), true
		}
		// Dangling link: follow it by hand and keep resolving from its target,
		// carrying whatever tail has already been peeled.
		if fi, lerr := os.Lstat(cur); lerr == nil && fi.Mode()&fs.ModeSymlink != 0 {
			target, rerr := os.Readlink(cur)
			if rerr != nil {
				return "", false
			}
			if !filepath.IsAbs(target) {
				target = filepath.Join(filepath.Dir(cur), target)
			}
			cur = filepath.Clean(target)
			continue
		}
		parent := filepath.Dir(cur)
		if parent == cur {
			return filepath.Clean(abs), true
		}
		if rest == "" {
			rest = filepath.Base(cur)
		} else {
			rest = filepath.Join(filepath.Base(cur), rest)
		}
		cur = parent
	}
	return "", false
}

// MatchesPlannedPath reports whether path matches a planned file exactly or by
// glob. It is the planned-path judge verify, get_diff and WouldBlockVerification
// ask, and the write gate reads the plan through the same scopeMatch it is built
// on, so a plan entry means the same thing on every surface. Entries and path are
// spelled through dc.NormalizePlanPath, the one strip rule for plan entries.
// Matching is case-sensitive on purpose (see EvaluateFileChange): a
// differently-cased path must not claim another entry's authorisation. It has no
// root, so the on-disk alias rungs do not run; see PlannedScopeIn.
func MatchesPlannedPath(path string, planned []dc.PlannedFile) bool {
	editable, readOnly := PlannedScope(path, planned)
	return editable || readOnly
}

// PlannedScope is PlannedScopeIn with no root: text only, the on-disk rungs do
// not run (see ScopeAnswer.OnDiskChecked). Callers that have a root use
// PlannedScopeIn.
func PlannedScope(path string, planned []dc.PlannedFile) (editable, readOnly bool) {
	return PlannedScopeIn("", path, planned)
}

// PlannedScopeIn classifies path against the plan entry by entry: editable when
// some entry that is not read_only matches, readOnly when some read_only entry
// matches. Both can be true when entries overlap, and read_only wins: the gate
// reads the plan through scopeMatch too, so it refuses the write, and
// verification raises oracle_tampering, whatever the order or spelling of the
// entries.
//
// A glob entry never covers a path HardDenied says the gate refuses before it
// reads the plan (secret, restricted, tool-state, prompt input, malformed, or
// the on-disk alias of one; root enables the alias rungs): a glob such as "*"
// must not make verification treat a change to ".env" or "CLAUDE.md" as planned.
// An entry that names such a path exactly is read as it always was (existing
// verify tests pin a planned secret path as planned, so its secret scan, not an
// orphan_diff, speaks for it); the gate refuses the write either way.
func PlannedScopeIn(root, path string, planned []dc.PlannedFile) (editable, readOnly bool) {
	return NewScopeJudge(root).Scope(path, planned)
}

// ScopeJudge is PlannedScopeIn for a caller that asks about many plan entries
// over the same paths. HardDenied reads the filesystem, so asking it once per
// (entry, path) pair made verification quadratic; a judge asks it once per path
// and remembers the answer. It is not safe for concurrent use, and its memory is
// one bool per distinct path asked about, so keep one per detector call rather
// than one per process: the repository can change between calls.
type ScopeJudge struct {
	root   string
	denied map[string]bool
}

// NewScopeJudge returns a judge for root. With root "" the on-disk rungs (alias
// spellings, symbolic and hard links) cannot run and every answer says so.
func NewScopeJudge(root string) *ScopeJudge {
	return &ScopeJudge{root: root, denied: map[string]bool{}}
}

// ScopeAnswer is a judge's verdict on one path. OnDiskChecked is false when the
// judge had no root: the answer then comes from the path's text alone, and a
// symbolic or hard link to a protected file reads as planned.
type ScopeAnswer struct {
	Editable      bool
	ReadOnly      bool
	OnDiskChecked bool
}

// Judge classifies path against planned, as PlannedScopeIn does.
func (j *ScopeJudge) Judge(path string, planned []dc.PlannedFile) ScopeAnswer {
	guarded, seen := j.denied[path]
	if !seen {
		guarded = HardDenied(j.root, path)
		j.denied[path] = guarded
	}
	ed, ro := scopeMatch(matching{m: GoMatcher}, dc.NormalizePlanPath(path), planned, guarded)
	return ScopeAnswer{Editable: ed != nil, ReadOnly: ro != nil, OnDiskChecked: j.root != ""}
}

// Scope is Judge without the on-disk marker.
func (j *ScopeJudge) Scope(path string, planned []dc.PlannedFile) (editable, readOnly bool) {
	a := j.Judge(path, planned)
	return a.Editable, a.ReadOnly
}

// scopeMatch is the one place plan entries are compared with a path: the first
// matching editable entry and the first matching read_only entry. With guarded
// set (the path is hard-denied) only entries that name the path exactly count.
// The gate passes guarded=false because it has already refused every hard-denied
// path.
func scopeMatch(mt matching, want string, planned []dc.PlannedFile, guarded bool) (editable, readOnly *dc.PlannedFile) {
	for i := range planned {
		pf := &planned[i]
		if !pathMatches(mt, pf.Path, want) {
			continue
		}
		if guarded && dc.NormalizePlanPath(pf.Path) != want {
			continue
		}
		if pf.AllowedChange == dc.ChangeReadOnly {
			if readOnly == nil {
				readOnly = pf
			}
		} else if editable == nil {
			editable = pf
		}
	}
	return editable, readOnly
}

// NormalizePlannedCandidate is dc.NormalizePlanPath, kept as an exported name
// because Manvi's verify (manvi/devcouncil/verify.go) calls it and links this
// module live.
func NormalizePlannedCandidate(path string) string { return dc.NormalizePlanPath(path) }

// plannedFileFor finds the entry that authorises a path, and reports the
// appended pattern when the entry came from scope an executor added to its own
// task rather than from the plan.
//
// The plan is searched first, and the order is the rule rather than an
// optimisation. An appended entry must never reach past what the planner said
// about the same path: a file planned read-only stays read-only however many
// times an agent appends it, because the plan is searched first and, within
// it, a read_only entry decides over an editable one whatever their order.
func plannedFileFor(mt matching, path string, task *dc.Task) (*dc.PlannedFile, string) {
	if pf := matchPlannedFile(mt, path, task.PlannedFiles); pf != nil {
		return pf, ""
	}
	if pf := matchPlannedFile(mt, path, task.AgentAppendedPlannedFiles); pf != nil {
		return pf, pf.Path
	}
	return nil, ""
}

// matchPlannedFile returns the entry that decides for path: a matching
// read_only entry if there is one, else the first matching editable entry. It is
// scopeMatch, the same comparison PlannedScopeIn makes for verification.
func matchPlannedFile(mt matching, path string, planned []dc.PlannedFile) *dc.PlannedFile {
	editable, readOnly := scopeMatch(mt, path, planned, false)
	if readOnly != nil {
		return readOnly
	}
	return editable
}

// forbiddenEntries reads forbidden_changes entries the way the filesystem reads
// the names they stand for. An entry is a prohibition, so the reading that never
// voids one is the right one: a ".." segment is resolved lexically ("src/../lib/x"
// forbids lib/x, which is the file the kernel opens, symlinks aside), and an entry
// ending in "/" or "/." names a directory and forbids the directory and everything
// under it. Left as written, both matched nothing and silently allowed what they
// named. An entry that resolves to somewhere above the root names nothing this
// gate is responsible for and is kept as written. This is the gate's reading, not
// the decoder's, so a task built in code gets it too; planned entries are not
// read this way, because a planned entry that matches nothing only narrows scope.
func forbiddenEntries(raw []string) []string {
	if raw == nil {
		return nil
	}
	out := make([]string, 0, len(raw))
	for _, entry := range raw {
		entry = dc.NormalizePlanPath(entry)
		dir := strings.HasSuffix(entry, "/") || strings.HasSuffix(entry, "/.") || entry == "."
		if strings.Contains(entry, "..") || dir {
			if cleaned := pathpkg.Clean(entry); cleaned != "." && cleaned != ".." && !strings.HasPrefix(cleaned, "../") && !strings.HasPrefix(cleaned, "/") {
				entry = cleaned
			}
		}
		out = append(out, entry)
		if dir {
			out = append(out, entry+"/**")
		}
	}
	return out
}

// forbiddenAlias reports which forbidden_changes entry names the same *file*
// as path under a different name, and whether one does.
//
// The pattern rung above compares strings, and a filesystem hands out more than
// one string for the same file. APFS is normalisation-insensitive as well as
// case-insensitive, so "café.txt" spelled NFC and spelled NFD are two strings
// and one file: a prohibition written in one form was cleared by submitting the
// other, and the matcher saw a name it had never been told about while the
// kernel opened the file it had. A hard link does the same thing for pure
// ASCII, and so does any spelling a future filesystem decides to fold.
//
// Normalising the strings would need Unicode tables this module does not carry,
// and it would be the wrong answer anyway on a normalisation-sensitive
// filesystem, where the two spellings really are two files and refusing the
// second would be an over-refusal. Asking the kernel which file each name opens
// is exact on both kinds of filesystem. It costs one stat per glob-free entry,
// and only for a task that declared a prohibition at all.
//
// Two bounds, stated rather than hidden. A glob entry names a set, not a file,
// so resolving one would mean walking the repository on every write; glob
// entries are left to the pattern rung. And a prohibition on a file that does
// not exist yet has no identity to compare against, so the alias spelling can
// still create it — the rung protects a file, and there is not yet a file.
func forbiddenAlias(root, path string, forbidden []string) (string, bool) {
	if root == "" || path == "" || len(forbidden) == 0 {
		return "", false
	}
	target, err := os.Stat(filepath.Join(root, filepath.FromSlash(path)))
	if err != nil {
		return "", false
	}
	for _, raw := range forbidden {
		entry := dc.NormalizePlanPath(raw)
		if entry == "" || entry == path || strings.ContainsAny(entry, "*?[") {
			continue
		}
		if filepath.IsAbs(entry) || entry == ".." || strings.HasPrefix(entry, "../") ||
			strings.Contains(entry, "/../") || strings.HasSuffix(entry, "/..") {
			// An entry that leaves the repository names nothing this gate is
			// responsible for; stat'ing it would only widen what a planner
			// string can reach.
			continue
		}
		fi, err := os.Stat(filepath.Join(root, filepath.FromSlash(entry)))
		if err != nil {
			continue
		}
		if os.SameFile(fi, target) {
			return raw, true
		}
	}
	return "", false
}

func pathMatches(mt matching, pattern, path string) bool {
	p := dc.NormalizePlanPath(pattern)
	return path == p || mt.any([]string{p}, path)
}

// matchesRestricted mirrors the Python rule, which tests both a glob match and
// a prefix match against the pattern with its stars stripped — so ".git/*"
// also catches a bare ".git" and everything beneath it.
//
// The prefix half compares whole path components, and that is the fix for two
// defects that were the same defect. Python's rule is `path.startswith(pattern
// .strip("*"))`, a test on characters: ".git" is a character prefix of
// ".gitignore", ".gitattributes", ".gitmodules" and ".github/workflows/ci.yml",
// none of which are the repository's own machinery, and every one of them was
// denied Hard. That also made the two ".github/workflows/*" entries in
// ProtectedWritePatterns dead code — a rung that runs earlier can never let
// them fire — so the "allowed, but flagged" behaviour those entries document
// did not exist, and a routine CI edit was unreachable rather than reviewed.
//
// In the other direction the character test needed the pattern to end in "/"
// to cover a subtree, which meant it never covered the directory itself:
// ".claude/*" stripped to ".claude/", and ".claude".startswith(".claude/") is
// false. Bare ".git" and ".devcouncil" were added to the list by hand to
// paper over that; the other seven agent-config entries were not. Trimming the
// separator and comparing components states the rule once, for every entry.
func matchesRestricted(mt matching, path string) bool {
	// One question for the whole glob half: "any pattern matches" is the OR
	// the loop below used to compute pattern by pattern, and the prefix half
	// does not depend on it.
	if mt.anyFold(RestrictedPathPatterns, path) {
		return true
	}
	lower := fnmatch.Fold(path)
	for _, pattern := range RestrictedPathPatterns {
		// Folded here for the same filesystem reason as the glob above.
		prefix := fnmatch.Fold(strings.TrimSuffix(strings.Trim(pattern, "*"), "/"))
		if prefix == "" {
			continue
		}
		if lower == prefix || strings.HasPrefix(lower, prefix+"/") {
			return true
		}
	}
	return false
}

func normalizeSlashes(p string) string { return strings.ReplaceAll(p, `\`, "/") }

// WouldBlockVerification reports whether a write this decision lets through
// will be raised as an unplanned file by verification. Verification judges the
// planned set (exact entries and globs); the write policy also lends a planned file's subsystem,
// neighbours and directory, and a posture can demote an unplanned refusal, so a
// write can be allowed here and blocked there. The answer is the same
// question verification asks — is the path one the task plans — asked of the
// path the decision names, so the two verdicts have one definition of scope.
// A refused write never reaches verification.
//
// root is the repository root, so the planned-path judge runs its on-disk rungs
// (a symbolic or hard link to a protected file is not planned). There is no
// root-less form: it could not tell a link to a secret from an ordinary file.
func WouldBlockVerificationIn(root string, d Decision, task *dc.Task) bool {
	if d.Blocked() || task == nil || d.Target == "" {
		return false
	}
	return !PlansPathIn(root, task, d.Target)
}

// PlansPathIn reports whether some planned entry names path, exactly or by glob,
// which is the set verification's orphan check compares the changed files with.
// A symbolic or hard link to a secret or restricted file is not read as planned
// by a glob; root enables those rungs.
func PlansPathIn(root string, task *dc.Task, path string) bool {
	editable, readOnly := PlannedScopeIn(root, path, task.PlannedFiles)
	return editable || readOnly
}
