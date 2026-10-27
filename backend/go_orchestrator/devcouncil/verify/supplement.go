package verify

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/safefile"
)

// binaryProbeBytes is how much of a file is inspected for a NUL before the
// supplemental diff calls it binary. Git's own binary check uses the first
// 8000 bytes; this matches the untracked-diff reader.
const binaryProbeBytes = 8192

// attachMissingDiffBytes appends a synthetic unified diff for changed paths
// whose bytes `git diff HEAD` did not carry: untracked files, and tracked
// files inside a dirty submodule (that diff is run with
// --ignore-submodules=dirty).
//
// The result is what dcverify already scans. It is capped by gitDiffLimit,
// including the diff git already returned. A path that is a symlink, escapes
// the root, or cannot be read fails the whole read — a gap, not a skip, and
// not a partial diff that looks complete. Secret paths are included; the
// host withholds their values when it builds gap evidence.
func attachMissingDiffBytes(ctx context.Context, git *proc.GitSession, root string, changed []string, diff string) (out string, err error) {
	if len(changed) == 0 {
		return diff, nil
	}
	present := diffHeaderPaths(diff)
	var missing []string
	for _, path := range changed {
		if _, ok := present[path]; ok {
			continue
		}
		missing = append(missing, path)
	}
	if len(missing) == 0 {
		return diff, nil
	}
	if err := ctx.Err(); err != nil {
		return "", err
	}
	if len(diff) > gitDiffLimit {
		return "", fmt.Errorf("the diff exceeds %d bytes", gitDiffLimit)
	}
	rootFS, err := os.OpenRoot(root)
	if err != nil {
		return "", err
	}
	defer func() { err = errors.Join(err, rootFS.Close()) }()
	var b strings.Builder
	b.WriteString(diff)
	if diff != "" && !strings.HasSuffix(diff, "\n") {
		b.WriteByte('\n')
	}
	for _, path := range missing {
		if err = ctx.Err(); err != nil {
			return "", err
		}
		if err = appendMissingPath(ctx, git, rootFS, path, &b, present); err != nil {
			return "", err
		}
	}
	return b.String(), nil
}

func appendMissingPath(ctx context.Context, git *proc.GitSession, rootFS *os.Root, path string, b *strings.Builder, present map[string]struct{}) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	if !pathOK(path) {
		return fmt.Errorf("cannot include %q in the diff", path)
	}
	info, err := rootFS.Lstat(path)
	if err != nil {
		return fmt.Errorf("cannot include %s: %v", path, err)
	}
	if info.Mode()&os.ModeSymlink != 0 {
		return fmt.Errorf("refusing to follow symlink %s", path)
	}
	if info.IsDir() {
		files, err := git.SubmoduleWorktreeFiles(ctx, path)
		if err != nil {
			return err
		}
		if len(files) == 0 {
			return fmt.Errorf("cannot include %s: no file bytes were readable", path)
		}
		for _, file := range files {
			if err := ctx.Err(); err != nil {
				return err
			}
			if _, ok := present[file]; ok {
				continue
			}
			if err := appendRegularFile(ctx, rootFS, file, b); err != nil {
				return err
			}
			present[file] = struct{}{}
		}
		return nil
	}
	if !info.Mode().IsRegular() {
		return fmt.Errorf("cannot include %s: not a regular file", path)
	}
	return appendRegularFile(ctx, rootFS, path, b)
}

func appendRegularFile(ctx context.Context, rootFS *os.Root, rel string, b *strings.Builder) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	if !pathOK(rel) {
		return fmt.Errorf("cannot include %q in the diff", rel)
	}
	remaining := gitDiffLimit - b.Len()
	if remaining < 0 {
		return fmt.Errorf("the diff exceeds %d bytes", gitDiffLimit)
	}
	raw, err := readCappedFile(rootFS, rel, int64(remaining))
	if err != nil {
		return err
	}
	if err := ctx.Err(); err != nil {
		return err
	}
	fragment := formatSyntheticDiff(rel, raw)
	if !strings.HasSuffix(fragment, "\n") {
		fragment += "\n"
	}
	if b.Len()+len(fragment) > gitDiffLimit {
		return fmt.Errorf("the diff exceeds %d bytes", gitDiffLimit)
	}
	if err := ctx.Err(); err != nil {
		return err
	}
	b.WriteString(fragment)
	return nil
}

// readCappedFile reads a regular file under root without following a symlink.
// The size is checked before the read. The read itself stops at limit+1 so a
// file that grows past the cap is refused rather than buffered whole.
func readCappedFile(rootFS *os.Root, rel string, limit int64) (raw []byte, err error) {
	info, err := rootFS.Lstat(rel)
	if err != nil {
		return nil, fmt.Errorf("cannot include %s: %v", rel, err)
	}
	if info.Mode()&os.ModeSymlink != 0 {
		return nil, fmt.Errorf("refusing to follow symlink %s", rel)
	}
	if !info.Mode().IsRegular() {
		return nil, fmt.Errorf("cannot include %s: not a regular file", rel)
	}
	if limit < 0 || info.Size() > limit {
		return nil, fmt.Errorf("the diff exceeds %d bytes", gitDiffLimit)
	}
	f, err := safefile.OpenNoFollow(rootFS, rel, os.O_RDONLY, 0)
	if err != nil {
		return nil, fmt.Errorf("cannot include %s: %v", rel, err)
	}
	defer func() { err = errors.Join(err, f.Close()) }()
	opened, err := f.Stat()
	if err != nil {
		return nil, fmt.Errorf("cannot include %s: %v", rel, err)
	}
	if !opened.Mode().IsRegular() || opened.Mode()&os.ModeSymlink != 0 {
		return nil, fmt.Errorf("refusing to follow symlink %s", rel)
	}
	links, err := safefile.LinkCount(f)
	if err != nil {
		return nil, fmt.Errorf("cannot include %s: %v", rel, err)
	}
	if links != 1 {
		return nil, fmt.Errorf("cannot include %s: hard-linked files are not read", rel)
	}
	raw, err = io.ReadAll(io.LimitReader(f, limit+1))
	if err != nil {
		return nil, fmt.Errorf("cannot include %s: %v", rel, err)
	}
	if int64(len(raw)) > limit {
		return nil, fmt.Errorf("the diff exceeds %d bytes", gitDiffLimit)
	}
	return raw, nil
}

func pathOK(rel string) bool {
	if rel == "" || strings.ContainsAny(rel, "\n\r\"") {
		return false
	}
	return !strings.HasPrefix(rel, "/") && !strings.Contains(rel, "../") && rel != ".."
}

func diffHeaderPaths(diff string) map[string]struct{} {
	set := map[string]struct{}{}
	for _, line := range strings.Split(diff, "\n") {
		rest, ok := strings.CutPrefix(line, "diff --git ")
		if !ok {
			continue
		}
		if path, ok := headerBPath(rest); ok {
			set[path] = struct{}{}
		}
	}
	return set
}

func headerBPath(rest string) (string, bool) {
	at := strings.LastIndex(rest, " b/")
	if at < 0 {
		return "", false
	}
	name := rest[at+3:]
	if name == "" || strings.Contains(name, "\"") {
		return "", false
	}
	return name, true
}

func formatSyntheticDiff(rel string, raw []byte) string {
	header := []string{
		fmt.Sprintf("diff --git a/%s b/%s", rel, rel),
		"new file mode 100644",
		"--- /dev/null",
		fmt.Sprintf("+++ b/%s", rel),
	}
	probe := raw
	if len(probe) > binaryProbeBytes {
		probe = probe[:binaryProbeBytes]
	}
	if bytes.IndexByte(probe, 0) >= 0 {
		header = append(header, fmt.Sprintf("Binary files a/%s and b/%s differ", rel, rel))
		return strings.Join(header, "\n")
	}
	text := string(raw)
	if text == "" {
		return strings.Join(header, "\n")
	}
	lines := strings.Split(text, "\n")
	if strings.HasSuffix(text, "\n") || strings.HasSuffix(text, "\r") {
		if len(lines) > 0 && lines[len(lines)-1] == "" {
			lines = lines[:len(lines)-1]
		}
	}
	out := append(header, fmt.Sprintf("@@ -0,0 +1,%d @@", len(lines)))
	for _, line := range lines {
		out = append(out, "+"+line)
	}
	return strings.Join(out, "\n")
}
