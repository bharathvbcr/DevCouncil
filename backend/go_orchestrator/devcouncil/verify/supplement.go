package verify

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
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
// tracked is the set of paths `git diff HEAD --name-only -z` named, which is
// exactly the set of paths the diff carries a section for. It is read from that
// NUL-delimited listing rather than parsed out of `diff --git` headers, which
// quote some names and are ambiguous for others (a name holding " b/").
//
// dirty names the submodules whose working trees hold uncommitted changes. A
// dirty submodule is never "present" because it is tracked: when its checked-out
// commit moved, the diff carries a header for it — a `Subproject commit` hunk
// that holds none of the bytes of the files edited inside it.
//
// The result is what dcverify already scans. It is capped by gitDiffLimit,
// including the diff git already returned. A path that is a symlink, escapes
// the root, or cannot be read fails the whole read — a gap, not a skip, and
// not a partial diff that looks complete. Secret paths are included; the
// host withholds their values when it builds gap evidence.
func attachMissingDiffBytes(ctx context.Context, git *proc.GitSession, root string, changed []string, tracked map[string]struct{}, dirty []string, diff string) (out string, err error) {
	if len(changed) == 0 {
		return diff, nil
	}
	isDirty := make(map[string]struct{}, len(dirty))
	for _, path := range dirty {
		isDirty[path] = struct{}{}
	}
	present := make(map[string]struct{}, len(tracked))
	for path := range tracked {
		if _, ok := isDirty[path]; !ok {
			present[path] = struct{}{}
		}
	}
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
		changes, err := git.SubmoduleWorktreeFiles(ctx, path)
		if err != nil {
			return err
		}
		// A dirty submodule for which git names no change at all is a
		// disagreement between two reads, not a deletion: nothing is known about
		// its bytes. A submodule whose every change is a deletion is different —
		// those changes have no bytes, and each is recorded as a deletion below.
		if changes.Empty() {
			return fmt.Errorf("cannot include %s: the submodule is dirty but git named no changed file in it", path)
		}
		for _, file := range changes.Files {
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
		for _, file := range changes.Deleted {
			if err := ctx.Err(); err != nil {
				return err
			}
			if _, ok := present[file]; ok {
				continue
			}
			if err := appendDeletion(ctx, file, b); err != nil {
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
	return appendFragment(ctx, b, formatSyntheticDiff(rel, raw))
}

// appendDeletion records a file deleted inside a dirty submodule as a
// header-only deletion section. There are no bytes to scan, but the change is
// named, so a diff that carries nothing else is not read as "nothing changed".
func appendDeletion(ctx context.Context, rel string, b *strings.Builder) error {
	if !pathOK(rel) {
		return fmt.Errorf("cannot include %q in the diff", rel)
	}
	a := quoteDiffPath("a/", rel)
	fragment := strings.Join([]string{
		fmt.Sprintf("diff --git %s %s", a, quoteDiffPath("b/", rel)),
		"deleted file mode 100644",
		"--- " + a,
		"+++ /dev/null",
	}, "\n")
	return appendFragment(ctx, b, fragment)
}

// appendFragment writes one diff section under the diff cap.
func appendFragment(ctx context.Context, b *strings.Builder, fragment string) error {
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
		return nil, fmt.Errorf("cannot include %s: refused: hard-linked file", rel)
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

// pathOK reports whether rel is a plain relative path inside the root. Which
// characters its name holds does not matter here: a name with a newline, a
// quote or a backslash is written into the synthetic diff C-quoted, as git
// writes it, so it is read back as the one file it names.
func pathOK(rel string) bool {
	return rel != "" && !strings.ContainsRune(rel, 0) && filepath.IsLocal(filepath.FromSlash(rel))
}

// quoteDiffPath spells prefix+name the way git spells it in a diff header: raw
// unless the name holds a control character, a quote or a backslash, in which
// case it is C-quoted. (Non-ASCII bytes stay raw: the diffs git is asked for
// carry core.quotePath=false.)
func quoteDiffPath(prefix, name string) string {
	full := prefix + name
	plain := true
	for i := 0; i < len(full); i++ {
		if c := full[i]; c < 0x20 || c == 0x7f || c == '"' || c == '\\' {
			plain = false
			break
		}
	}
	if plain {
		return full
	}
	var b strings.Builder
	b.WriteByte('"')
	for i := 0; i < len(full); i++ {
		switch c := full[i]; c {
		case '\a':
			b.WriteString(`\a`)
		case '\b':
			b.WriteString(`\b`)
		case '\t':
			b.WriteString(`\t`)
		case '\n':
			b.WriteString(`\n`)
		case '\v':
			b.WriteString(`\v`)
		case '\f':
			b.WriteString(`\f`)
		case '\r':
			b.WriteString(`\r`)
		case '"':
			b.WriteString(`\"`)
		case '\\':
			b.WriteString(`\\`)
		default:
			if c < 0x20 || c == 0x7f {
				fmt.Fprintf(&b, `\%03o`, c)
			} else {
				b.WriteByte(c)
			}
		}
	}
	b.WriteByte('"')
	return b.String()
}

func formatSyntheticDiff(rel string, raw []byte) string {
	a, b := quoteDiffPath("a/", rel), quoteDiffPath("b/", rel)
	header := []string{
		fmt.Sprintf("diff --git %s %s", a, b),
		"new file mode 100644",
		"--- /dev/null",
		"+++ " + b,
	}
	probe := raw
	if len(probe) > binaryProbeBytes {
		probe = probe[:binaryProbeBytes]
	}
	if bytes.IndexByte(probe, 0) >= 0 {
		header = append(header, fmt.Sprintf("Binary files %s and %s differ", a, b))
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
