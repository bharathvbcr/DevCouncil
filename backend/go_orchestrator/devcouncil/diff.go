package devcouncil

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/safefile"
)

const (
	// DiffTimeout bounds every git probe used by get_diff. Matches Python
	// CLI_TIMEOUT_SECONDS for the MCP handler's _run_git (120s), except the
	// repo-state probe which uses GIT_TIMEOUT (60s).
	DiffTimeout      = 120 * time.Second
	RepoStateTimeout = 60 * time.Second

	// DiffOutputLimit matches Python _CLI_OUTPUT_LIMIT.
	DiffOutputLimit = 20_000

	binaryProbeBytes    = 8192
	untrackedInputLimit = 8 * 1024 * 1024
	// gitReadLimit caps every git read get_diff makes. A diff past it is
	// returned as Truncated; a listing past it is an error, because a partial
	// file list would be read as the whole change.
	gitReadLimit       = 32 << 20
	untrackedFileLimit = 4096
)

// DiffFile is one path in a get_diff payload.
//
// Withheld names why its hunks are absent from unified_diff; today the only
// reason is policy.WithheldSecretPath. The file is still listed, so a reader
// knows it changed without being shown what it holds.
type DiffFile struct {
	Path      string `json:"path"`
	Status    string `json:"status"`
	Additions int    `json:"additions"`
	Deletions int    `json:"deletions"`
	Withheld  string `json:"withheld,omitempty"`
}

// DiffResult is the Python MCP get_diff success shape.
type DiffResult struct {
	OK          bool       `json:"ok"`
	Files       []DiffFile `json:"files"`
	UnifiedDiff string     `json:"unified_diff"`
	Truncated   bool       `json:"truncated"`
	Staged      bool       `json:"staged"`
	Error       string     `json:"error,omitempty"`
	Code        string     `json:"code,omitempty"`
}

// ErrorPayload is the fail-closed MCP error envelope (ok:false).
type ErrorPayload struct {
	OK    bool   `json:"ok"`
	Error string `json:"error"`
	Code  string `json:"code"`
}

// GitRepoState is the tri-state probe: true / false / undetermined.
// undetermined is distinct from false so a timeout never looks like "not a repo".
type GitRepoState struct {
	InRepo *bool // nil = undetermined
	Reason string
}

// ProbeGitRepoState mirrors Python git_repo_state.
func ProbeGitRepoState(ctx context.Context, root string) GitRepoState {
	ctx, cancel := context.WithTimeout(ctx, RepoStateTimeout)
	defer cancel()
	git, err := proc.NewGitSession(root)
	if err != nil {
		return GitRepoState{Reason: fmt.Sprintf("could not run git: %v", err)}
	}
	out, err := runGit(ctx, git, "rev-parse", "--is-inside-work-tree")
	if err != nil {
		if isTimeout(err) || out.returnCode == 124 {
			return GitRepoState{Reason: timeoutReason(err, out)}
		}
		if isExecError(err) {
			return GitRepoState{Reason: fmt.Sprintf("could not run git: %v", err)}
		}
		falseVal := false
		reason := strings.TrimSpace(out.stderr)
		if reason == "" {
			reason = fmt.Sprintf("git rev-parse exited %d", out.returnCode)
		}
		return GitRepoState{InRepo: &falseVal, Reason: reason}
	}
	answer := strings.TrimSpace(out.stdout)
	switch answer {
	case "true":
		trueVal := true
		return GitRepoState{InRepo: &trueVal}
	case "false":
		falseVal := false
		return GitRepoState{InRepo: &falseVal, Reason: "bare repository"}
	default:
		if out.returnCode == 124 {
			return GitRepoState{Reason: timeoutReason(err, out)}
		}
		falseVal := false
		reason := strings.TrimSpace(out.stderr)
		if reason == "" {
			reason = fmt.Sprintf("git rev-parse exited %d", out.returnCode)
		}
		return GitRepoState{InRepo: &falseVal, Reason: reason}
	}
}

// GetDiffArgs are the MCP/CLI arguments for get_diff.
type GetDiffArgs struct {
	TaskID        string
	Paths         []string
	Staged        bool
	PlannedFiles  []string // when TaskID set: planned paths from the task
	TaskFound     bool     // false + TaskID set => not_found
	DBInitialized bool     // false + TaskID set => not_initialized
}

// GetDiff implements Python handlers/git.py handle_get_diff + git_diff.
func GetDiff(ctx context.Context, root string, args GetDiffArgs) (any, error) {
	state := ProbeGitRepoState(ctx, root)
	if state.InRepo == nil {
		return ErrorPayload{
			OK:    false,
			Code:  "git_repo_undetermined",
			Error: "get_diff could not determine whether this is a git repository: " + state.Reason,
		}, nil
	}
	if !*state.InRepo {
		return ErrorPayload{
			OK:    false,
			Code:  "not_a_git_repo",
			Error: "get_diff requires a git repository.",
		}, nil
	}

	scopePaths := make([]string, 0, len(args.Paths))
	for _, p := range args.Paths {
		scopePaths = append(scopePaths, strings.ReplaceAll(p, "\\", "/"))
	}
	taskScoped := false
	if args.TaskID != "" {
		if !args.DBInitialized {
			return ErrorPayload{
				OK:    false,
				Code:  "not_initialized",
				Error: "DevCouncil not initialized in this directory.",
			}, nil
		}
		if !args.TaskFound {
			return ErrorPayload{
				OK:    false,
				Code:  "not_found",
				Error: fmt.Sprintf("Task %s not found.", args.TaskID),
			}, nil
		}
		taskScoped = true
		planned := make([]string, 0, len(args.PlannedFiles))
		for _, p := range args.PlannedFiles {
			planned = append(planned, strings.ReplaceAll(p, "\\", "/"))
		}
		if len(scopePaths) > 0 {
			filtered := make([]string, 0, len(scopePaths))
			for _, p := range scopePaths {
				if MatchesPlannedPath(p, planned) {
					filtered = append(filtered, p)
				}
			}
			scopePaths = filtered
		} else {
			scopePaths = planned
		}
	}

	if taskScoped && len(scopePaths) == 0 {
		return emptyDiff(args.Staged), nil
	}
	return gitDiff(ctx, root, scopePaths, args.Staged)
}

func emptyDiff(staged bool) DiffResult {
	return DiffResult{OK: true, Files: []DiffFile{}, UnifiedDiff: "", Truncated: false, Staged: staged}
}

func gitDiff(ctx context.Context, root string, paths []string, staged bool) (DiffResult, error) {
	diffArgs := []string{"diff"}
	numstatArgs := []string{"diff", "--numstat"}
	namestatusArgs := []string{"diff", "--name-status", "-z"}
	if staged {
		diffArgs = append(diffArgs, "--cached")
		numstatArgs = append(numstatArgs, "--cached")
		namestatusArgs = append(namestatusArgs, "--cached")
	}
	if len(paths) == 0 {
		// The whole tree, less what DevCouncil's own tools wrote: a state
		// database is not a hunk an agent can read or should be told to revert.
		paths = []string{"."}
		for _, dir := range policy.ToolStateDirs {
			paths = append(paths, ":(top,exclude)"+dir)
		}
	}
	diffArgs = append(append(diffArgs, "--"), paths...)
	numstatArgs = append(append(numstatArgs, "--"), paths...)
	namestatusArgs = append(append(namestatusArgs, "--"), paths...)

	ctx, cancel := context.WithTimeout(ctx, DiffTimeout)
	defer cancel()

	// One session for every read of this operation, so the repository's
	// config is read once rather than once per read.
	git, err := proc.NewGitSession(root)
	if err != nil {
		return DiffResult{OK: false, Files: []DiffFile{}, Staged: staged, Error: err.Error()}, nil
	}
	diffOut, err := runGit(ctx, git, diffArgs...)
	if err != nil && !isGitFail(err, diffOut) {
		return DiffResult{OK: false, Files: []DiffFile{}, Staged: staged, Error: err.Error()}, nil
	}
	numOut, err := runGit(ctx, git, numstatArgs...)
	if err != nil && !isGitFail(err, numOut) {
		return DiffResult{OK: false, Files: []DiffFile{}, Staged: staged, Error: err.Error()}, nil
	}
	nameOut, err := runGit(ctx, git, namestatusArgs...)
	if err != nil && !isGitFail(err, nameOut) {
		return DiffResult{OK: false, Files: []DiffFile{}, Staged: staged, Error: err.Error()}, nil
	}

	for _, item := range []struct {
		label string
		out   gitOut
	}{
		{"git diff", diffOut},
		{"git diff --numstat", numOut},
		{"git diff --name-status", nameOut},
	} {
		if item.out.returnCode != 0 {
			detail := strings.TrimSpace(item.out.stderr)
			if detail == "" {
				detail = strings.TrimSpace(item.out.stdout)
			}
			if detail == "" {
				detail = fmt.Sprintf("%s exited %d", item.label, item.out.returnCode)
			}
			return DiffResult{OK: false, Files: []DiffFile{}, Staged: staged, Error: detail, Truncated: false}, nil
		}
	}

	for _, item := range []struct {
		label string
		out   gitOut
	}{{"git diff --numstat", numOut}, {"git diff --name-status", nameOut}} {
		if item.out.truncated {
			return DiffResult{OK: false, Files: []DiffFile{}, Staged: staged,
				Error: fmt.Sprintf("%s output exceeded %d bytes; narrow the requested paths", item.label, gitReadLimit)}, nil
		}
	}

	statusByPath := parseNameStatusZ(nameOut.stdout)
	files := make([]DiffFile, 0)
	for _, line := range strings.Split(numOut.stdout, "\n") {
		if line == "" {
			continue
		}
		parts := strings.Split(line, "\t")
		if len(parts) < 3 {
			continue
		}
		addedStr, deletedStr, filePath := parts[0], parts[1], parts[len(parts)-1]
		if i := strings.Index(filePath, " => "); i >= 0 {
			filePath = filePath[i+4:]
		}
		filePath = strings.ReplaceAll(filePath, "\\", "/")
		additions, _ := strconv.Atoi(addedStr)
		deletions, _ := strconv.Atoi(deletedStr)
		if !isDigits(addedStr) {
			additions = 0
		}
		if !isDigits(deletedStr) {
			deletions = 0
		}
		status := statusByPath[filePath]
		if status == "" {
			status = "M"
		}
		files = append(files, DiffFile{
			Path: filePath, Status: status, Additions: additions, Deletions: deletions,
		})
	}

	unifiedParts := []string{}
	if strings.TrimRight(diffOut.stdout, "\n") != "" {
		unifiedParts = append(unifiedParts, strings.TrimRight(diffOut.stdout, "\n"))
	}

	if !staged {
		known := map[string]struct{}{}
		for _, f := range files {
			known[f.Path] = struct{}{}
		}
		// The diff reads pass --ignore-submodules=dirty, so git runs no status
		// inside a submodule; a guarded read of each one reports the dirty
		// ones, as `M` with no line counts, which is how git listed them.
		dirty, err := git.DirtySubmodules(ctx, paths...)
		if err != nil {
			return DiffResult{OK: false, Files: []DiffFile{}, Staged: staged, Error: err.Error()}, nil
		}
		for _, path := range dirty {
			path = strings.ReplaceAll(path, "\\", "/")
			if _, ok := known[path]; !ok {
				files = append(files, DiffFile{Path: path, Status: "M"})
				known[path] = struct{}{}
			}
		}
		uFiles, uDiff, uErr := collectUntracked(ctx, git, root, paths, known)
		if uErr != "" {
			return DiffResult{OK: false, Files: []DiffFile{}, Staged: staged, Error: uErr}, nil
		}
		files = append(files, uFiles...)
		if uDiff != "" {
			unifiedParts = append(unifiedParts, strings.TrimRight(uDiff, "\n"))
		}
	}

	combined := strings.Join(filterNonEmpty(unifiedParts), "\n")
	if combined != "" {
		combined += "\n"
	}
	// Tracked and untracked text alike, and before truncation, so the limit is
	// spent on what the reader may see.
	combined, withheldNames := policy.WithholdSecretDiff(combined)
	withheld := make(map[string]struct{}, len(withheldNames))
	for _, name := range withheldNames {
		withheld[name] = struct{}{}
	}
	for i := range files {
		_, named := withheld[files[i].Path]
		if named || policy.SecretPathSpelling(files[i].Path) {
			files[i].Withheld = policy.WithheldSecretPath
		}
	}
	unified, truncated := TruncateText(combined, DiffOutputLimit)
	return DiffResult{
		OK: true, Files: files, UnifiedDiff: unified, Truncated: truncated || diffOut.truncated, Staged: staged,
	}, nil
}

func collectUntracked(ctx context.Context, git *proc.GitSession, root string, paths []string, known map[string]struct{}) (files []DiffFile, unified string, failure string) {
	held, err := os.OpenRoot(root)
	if err != nil {
		return nil, "", err.Error()
	}
	defer func() {
		if err := held.Close(); err != nil {
			files, unified, failure = nil, "", errors.Join(errors.New(failure), err).Error()
		}
	}()
	args := []string{"ls-files", "--others", "--exclude-standard", "-z"}
	if len(paths) > 0 {
		args = append(args, "--")
		args = append(args, paths...)
	}
	// The caller's context, so cancelling GetDiff reaches this read too.
	ctx, cancel := context.WithTimeout(ctx, DiffTimeout)
	defer cancel()
	out, err := runGit(ctx, git, args...)
	if err != nil && out.returnCode == 0 {
		return nil, "", err.Error()
	}
	if out.truncated {
		return nil, "", fmt.Sprintf("untracked listing exceeded %d bytes; narrow the requested paths", gitReadLimit)
	}
	if out.returnCode != 0 {
		detail := strings.TrimSpace(out.stderr)
		if detail == "" {
			detail = strings.TrimSpace(out.stdout)
		}
		if detail == "" {
			detail = fmt.Sprintf("git ls-files exited %d", out.returnCode)
		}
		return nil, "", detail
	}
	var fragments []string
	remaining := int64(untrackedInputLimit)
	for _, rel := range strings.Split(out.stdout, "\x00") {
		if rel == "" || policy.IsToolState(rel) {
			continue
		}
		if _, ok := known[rel]; ok {
			continue
		}
		if len(files) >= untrackedFileLimit {
			return nil, "", fmt.Sprintf("untracked diff exceeds %d-file input limit; narrow the requested paths", untrackedFileLimit)
		}
		// Git's NUL-delimited names are native file names. Use policy only
		// for authorization; never reopen its normalized spelling. A secret
		// path is listed but never opened; a path escaping the root fails the
		// whole read, because that is an attack on it, not a file to skip.
		switch policy.ClassifyRead(root, rel) {
		case policy.ReadOutsideRoot:
			return nil, "", fmt.Sprintf("untracked diff read refused by file policy: %q", rel)
		case policy.ReadSecretPath:
			files = append(files, DiffFile{Path: rel, Status: "A", Withheld: policy.WithheldSecretPath})
			continue
		}
		raw, err := readUntrackedFile(held, filepath.FromSlash(rel), remaining)
		if err != nil {
			return nil, "", fmt.Sprintf("untracked diff %q: %v", rel, err)
		}
		remaining -= int64(len(raw))
		fragment, additions := formatUntrackedFileDiff(rel, raw)
		if fragment == "" {
			continue
		}
		files = append(files, DiffFile{Path: rel, Status: "A", Additions: additions, Deletions: 0})
		fragments = append(fragments, strings.TrimRight(fragment, "\n"))
	}
	unified = strings.Join(fragments, "\n")
	if unified != "" {
		unified += "\n"
	}
	return files, unified, ""
}

// readUntrackedFile authorizes content from the opened object, never a second
// path lookup. Reads and total collection input are bounded before allocation.
func readUntrackedFile(root *os.Root, name string, limit int64) (raw []byte, resultErr error) {
	f, err := safefile.OpenNoFollow(root, name, os.O_RDONLY, 0)
	if err != nil {
		return nil, err
	}
	defer func() { resultErr = errors.Join(resultErr, f.Close()) }()
	before, err := f.Stat()
	if err != nil {
		return nil, err
	}
	if !before.Mode().IsRegular() {
		return nil, errors.New("not an ordinary file")
	}
	links, err := safefile.LinkCount(f)
	if err != nil {
		return nil, err
	}
	if links != 1 {
		return nil, errors.New("hard-linked files are not readable through untracked diffs")
	}
	if before.Size() > limit {
		return nil, errors.New("untracked diff input byte limit exceeded; narrow the requested paths")
	}
	raw, err = io.ReadAll(io.LimitReader(f, limit+1))
	if err != nil {
		return nil, err
	}
	if int64(len(raw)) > limit {
		return nil, errors.New("untracked diff input byte limit exceeded during read")
	}
	after, err := f.Stat()
	if err != nil {
		return nil, err
	}
	named, err := root.Lstat(name)
	if err != nil {
		return nil, err
	}
	if !os.SameFile(before, after) || !os.SameFile(after, named) || named.Mode()&os.ModeSymlink != 0 ||
		before.Size() != after.Size() || !before.ModTime().Equal(after.ModTime()) {
		return nil, errors.New("untracked file changed during read")
	}
	return raw, nil
}

func formatUntrackedFileDiff(relPath string, raw []byte) (string, int) {
	header := []string{
		fmt.Sprintf("diff --git a/%s b/%s", relPath, relPath),
		"new file mode 100644",
		"--- /dev/null",
		fmt.Sprintf("+++ b/%s", relPath),
	}
	probe := raw
	if len(probe) > binaryProbeBytes {
		probe = probe[:binaryProbeBytes]
	}
	if bytes.IndexByte(probe, 0) >= 0 {
		return strings.Join(append(header, fmt.Sprintf("Binary files /dev/null and b/%s differ", relPath)), "\n"), 0
	}
	text := string(raw)
	if text == "" {
		return strings.Join(header, "\n") + "\n", 0
	}
	lines := strings.Split(text, "\n")
	// Preserve Python: splitlines() drops a trailing empty from a final newline.
	if strings.HasSuffix(text, "\n") || strings.HasSuffix(text, "\r") {
		if len(lines) > 0 && lines[len(lines)-1] == "" {
			lines = lines[:len(lines)-1]
		}
	}
	lineCount := len(lines)
	if !strings.HasSuffix(text, "\n") && !strings.HasSuffix(text, "\r") {
		if lineCount == 0 {
			lineCount = 1
		}
	} else if lineCount == 0 {
		lineCount = 0
	}
	diffLines := append(header, fmt.Sprintf("@@ -0,0 +1,%d @@", lineCount))
	for _, line := range lines {
		diffLines = append(diffLines, "+"+line)
	}
	return strings.Join(diffLines, "\n"), lineCount
}

func parseNameStatusZ(data string) map[string]string {
	statusByPath := map[string]string{}
	parts := strings.Split(data, "\x00")
	i := 0
	for i < len(parts) {
		status := parts[i]
		if status == "" {
			i++
			continue
		}
		kind := status[0]
		if (kind == 'R' || kind == 'C') && i+2 < len(parts) {
			newPath := strings.ReplaceAll(parts[i+2], "\\", "/")
			if newPath != "" {
				statusByPath[newPath] = status
			}
			i += 3
			continue
		}
		if i+1 < len(parts) {
			path := strings.ReplaceAll(parts[i+1], "\\", "/")
			if path != "" {
				statusByPath[path] = status
			}
			i += 2
			continue
		}
		break
	}
	return statusByPath
}

// TruncateText matches Python truncate_text.
func TruncateText(value string, limit int) (string, bool) {
	if len(value) <= limit {
		return value, false
	}
	marker := fmt.Sprintf("\n...[truncated to %d characters]", limit)
	return value[:limit] + marker, true
}

type gitOut struct {
	stdout     string
	stderr     string
	returnCode int
	// truncated is set when the read hit gitReadLimit: the output is a prefix.
	truncated bool
}

// runGit runs a git read built by the operation's proc.GitSession: a git the
// repository supplies, or a program its .git/config names, does not get to
// describe the repository. A refusal comes back as an *exec.Error, like a git
// that is missing, so callers report that git could not run rather than that
// root is not a repository.
func runGit(ctx context.Context, git *proc.GitSession, args ...string) (gitOut, error) {
	res, err := git.Run(ctx, gitReadLimit, args...)
	out := gitOut{
		stdout:     string(res.Stdout),
		stderr:     string(res.Stderr),
		returnCode: res.ExitCode,
		truncated:  res.Truncated,
	}
	if res.TimedOut && out.stderr == "" {
		out.stderr = fmt.Sprintf("timed out after %s", RepoStateTimeout)
	}
	return out, err
}

func isTimeout(err error) bool {
	if err == nil {
		return false
	}
	return strings.Contains(err.Error(), "deadline") || strings.Contains(err.Error(), "signal: killed")
}

func isExecError(err error) bool {
	if err == nil {
		return false
	}
	_, ok := err.(*exec.Error)
	return ok
}

func isGitFail(err error, out gitOut) bool {
	return err != nil && out.returnCode > 0
}

func timeoutReason(err error, out gitOut) string {
	if strings.TrimSpace(out.stderr) != "" {
		return strings.TrimSpace(out.stderr)
	}
	return "timed out after 60.0s"
}

func isDigits(s string) bool {
	if s == "" {
		return false
	}
	for _, c := range s {
		if c < '0' || c > '9' {
			return false
		}
	}
	return true
}

func filterNonEmpty(parts []string) []string {
	out := make([]string, 0, len(parts))
	for _, p := range parts {
		if p != "" {
			out = append(out, p)
		}
	}
	return out
}
