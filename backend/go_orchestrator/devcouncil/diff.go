package devcouncil

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

const (
	// DiffTimeout bounds every git probe used by get_diff. Matches Python
	// CLI_TIMEOUT_SECONDS for the MCP handler's _run_git (120s), except the
	// repo-state probe which uses GIT_TIMEOUT (60s).
	DiffTimeout      = 120 * time.Second
	RepoStateTimeout = 60 * time.Second

	// DiffOutputLimit matches Python _CLI_OUTPUT_LIMIT.
	DiffOutputLimit = 20_000

	binaryProbeBytes = 8192
)

// DiffFile is one path in a get_diff payload.
type DiffFile struct {
	Path      string `json:"path"`
	Status    string `json:"status"`
	Additions int    `json:"additions"`
	Deletions int    `json:"deletions"`
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
	out, err := runGit(ctx, root, "rev-parse", "--is-inside-work-tree")
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
	if len(paths) > 0 {
		diffArgs = append(diffArgs, "--")
		diffArgs = append(diffArgs, paths...)
		numstatArgs = append(numstatArgs, "--")
		numstatArgs = append(numstatArgs, paths...)
		namestatusArgs = append(namestatusArgs, "--")
		namestatusArgs = append(namestatusArgs, paths...)
	}

	ctx, cancel := context.WithTimeout(ctx, DiffTimeout)
	defer cancel()

	diffOut, err := runGit(ctx, root, diffArgs...)
	if err != nil && !isGitFail(err, diffOut) {
		return DiffResult{OK: false, Files: []DiffFile{}, Staged: staged, Error: err.Error()}, nil
	}
	numOut, err := runGit(ctx, root, numstatArgs...)
	if err != nil && !isGitFail(err, numOut) {
		return DiffResult{OK: false, Files: []DiffFile{}, Staged: staged, Error: err.Error()}, nil
	}
	nameOut, err := runGit(ctx, root, namestatusArgs...)
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
		uFiles, uDiff, uErr := collectUntracked(root, paths, known)
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
	unified, truncated := TruncateText(combined, DiffOutputLimit)
	return DiffResult{
		OK: true, Files: files, UnifiedDiff: unified, Truncated: truncated, Staged: staged,
	}, nil
}

func collectUntracked(root string, paths []string, known map[string]struct{}) ([]DiffFile, string, string) {
	args := []string{"ls-files", "--others", "--exclude-standard", "-z"}
	if len(paths) > 0 {
		args = append(args, "--")
		args = append(args, paths...)
	}
	ctx, cancel := context.WithTimeout(context.Background(), DiffTimeout)
	defer cancel()
	out, err := runGit(ctx, root, args...)
	if err != nil && out.returnCode == 0 {
		return nil, "", err.Error()
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
	var files []DiffFile
	var fragments []string
	for _, rel := range strings.Split(out.stdout, "\x00") {
		rel = strings.TrimSpace(strings.ReplaceAll(rel, "\\", "/"))
		if rel == "" {
			continue
		}
		if _, ok := known[rel]; ok {
			continue
		}
		full := filepath.Join(root, filepath.FromSlash(rel))
		info, err := os.Stat(full)
		if err != nil || !info.Mode().IsRegular() {
			continue
		}
		fragment, additions := formatUntrackedFileDiff(rel, full)
		if fragment == "" {
			continue
		}
		files = append(files, DiffFile{Path: rel, Status: "A", Additions: additions, Deletions: 0})
		fragments = append(fragments, strings.TrimRight(fragment, "\n"))
	}
	unified := strings.Join(fragments, "\n")
	if unified != "" {
		unified += "\n"
	}
	return files, unified, ""
}

func formatUntrackedFileDiff(relPath, fullPath string) (string, int) {
	raw, err := os.ReadFile(fullPath)
	if err != nil {
		return "", 0
	}
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
}

func runGit(ctx context.Context, root string, args ...string) (gitOut, error) {
	cmd := exec.CommandContext(ctx, "git", args...)
	cmd.Dir = root
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	// Same bound and group isolation as every other subprocess boundary in
	// this module: CommandContext alone does not cover a wedged Start, and
	// killing only the direct child leaves grandchildren holding the pipes.
	proc.ConfigureGroup(cmd)
	cmd.WaitDelay = 2 * time.Second
	err, timedOut := proc.RunBounded(ctx, cmd.Run)
	out := gitOut{stdout: stdout.String(), stderr: stderr.String()}
	if timedOut {
		out.returnCode = 124
		if out.stderr == "" {
			out.stderr = fmt.Sprintf("timed out after %s", RepoStateTimeout)
		}
		return out, ctx.Err()
	}
	if err != nil {
		if ctx.Err() == context.DeadlineExceeded || isTimeout(err) {
			out.returnCode = 124
			if out.stderr == "" {
				out.stderr = fmt.Sprintf("timed out after %s", RepoStateTimeout)
			}
			return out, err
		}
		if ee, ok := err.(*exec.ExitError); ok {
			out.returnCode = ee.ExitCode()
			return out, err
		}
		out.returnCode = -1
		return out, err
	}
	return out, nil
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
