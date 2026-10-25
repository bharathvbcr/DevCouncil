package verify

import (
	"context"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

// SubjectPrefix marks a subject hash computed by WorkingTreeSubject.
const SubjectPrefix = "git-tree:"

// subjectBound bounds the whole computation. Staging a working tree hashes
// every changed file, so it is given longer than one git read.
const subjectBound = 5 * time.Minute

// WorkingTreeSubject names the exact content of root's working tree, for a
// review to be bound to: "git-tree:" and the object id of the tree git would
// commit if every change — tracked, untracked and deleted, minus what
// .gitignore excludes and minus the tool state verification never judges
// (policy.ToolStateDirs) — were staged.
//
// A tree id rather than a hash of the diff, because the diff moves when the
// content does not: the same content reviewed before a commit and verified
// after it has a different diff against a different HEAD, and the same tree.
// Two working trees with byte-identical content have one subject; any change
// to any file verify would judge gives another. A clean tree's subject is its
// HEAD's tree.
//
// It runs against a scratch copy of the index (GIT_INDEX_FILE), so the
// repository's own index — what the person has staged — is untouched. Staging
// writes blob objects for changed files into the object store, unreferenced
// and pruned by git's ordinary gc; nothing else is written. Every git call
// goes through proc.GitSession, so the repository's filter drivers and
// fsmonitor are neutralised as for every other read here. A submodule
// contributes the commit it is checked out at, not its own working tree.
func WorkingTreeSubject(ctx context.Context, root string) (string, error) {
	ctx, cancel := context.WithTimeout(ctx, subjectBound)
	defer cancel()
	git, err := proc.NewGitSession(root)
	if err != nil {
		return "", err
	}
	indexOut, err := runGit(ctx, git, 1<<12, "rev-parse", "--git-path", "index")
	if err != nil {
		return "", fmt.Errorf("locating the index: %w", err)
	}
	index := strings.TrimSpace(string(indexOut))
	if !filepath.IsAbs(index) {
		index = filepath.Join(root, index)
	}
	scratchDir, err := os.MkdirTemp("", "devcouncil-subject-")
	if err != nil {
		return "", err
	}
	defer os.RemoveAll(scratchDir)
	scratch := filepath.Join(scratchDir, "index")
	// The real index is copied so its stat cache spares git rehashing every
	// unchanged file. A repository with no index yet (nothing ever staged)
	// starts from an empty one, which git reads a missing file as.
	if err := copyFile(index, scratch); err != nil && !errors.Is(err, fs.ErrNotExist) {
		return "", fmt.Errorf("copying the index: %w", err)
	}
	env := []string{"GIT_INDEX_FILE=" + scratch}
	if _, err := runGitEnv(ctx, git, 1<<20, subjectBound, env, append([]string{"add", "--all"}, toolStateExcludes()...)...); err != nil {
		return "", fmt.Errorf("staging the working tree into a scratch index: %w", err)
	}
	treeOut, err := runGitEnv(ctx, git, 1<<10, subjectBound, env, "write-tree")
	if err != nil {
		return "", fmt.Errorf("writing the working tree's tree: %w", err)
	}
	tree := strings.TrimSpace(string(treeOut))
	if !isObjectID(tree) {
		return "", fmt.Errorf("git write-tree answered %q, which is not an object id", tree)
	}
	return SubjectPrefix + tree, nil
}

func copyFile(from, to string) error {
	src, err := os.Open(from)
	if err != nil {
		return err
	}
	defer src.Close()
	dst, err := os.OpenFile(to, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o600)
	if err != nil {
		return err
	}
	if _, err := io.Copy(dst, src); err != nil {
		dst.Close()
		return err
	}
	return dst.Close()
}

// isObjectID reports whether s is a SHA-1 or SHA-256 object id in lowercase
// hex.
func isObjectID(s string) bool {
	if len(s) != 40 && len(s) != 64 {
		return false
	}
	for _, c := range s {
		if !(c >= '0' && c <= '9' || c >= 'a' && c <= 'f') {
			return false
		}
	}
	return true
}
