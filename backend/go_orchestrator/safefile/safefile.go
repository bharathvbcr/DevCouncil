// Package safefile owns the platform boundary for non-destructive file opens
// and pinned filesystem identity. It does not replace the caller's containment
// walk or authorization policy.
package safefile

import (
	"errors"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
)

// Identity holds an eagerly captured OS identity. On Windows, os.Stat may defer
// fetching the volume/file index until SameFile is called. Initialize it here,
// at the pin boundary, so later comparisons never resolve that old path again.
// Root.Stat and File.Stat already capture identity from their kernel handles.
type Identity struct{ info fs.FileInfo }

func PinIdentity(info fs.FileInfo) (Identity, bool) {
	if info == nil || !os.SameFile(info, info) {
		return Identity{}, false
	}
	return Identity{info: info}, true
}

func (id Identity) Equal(other Identity) bool {
	return id.info != nil && other.info != nil && os.SameFile(id.info, other.info)
}

// OpenNoFollow opens without truncating. A nil root uses an ordinary path;
// otherwise every ancestor is pinned without following links beneath the held directory.
// Callers must still validate regular-file type, identity, link count and their
// held directory chain before reading or truncating the returned handle.
func OpenNoFollow(root *os.Root, name string, flag int, perm fs.FileMode) (*os.File, error) {
	if flag&os.O_TRUNC != 0 {
		return nil, errors.New("truncation requires a validated handle")
	}
	if root != nil {
		parent, leaf, err := noFollowParent(root, name)
		if err != nil {
			return nil, err
		}
		if parent != root {
			f, err := OpenNoFollow(parent, leaf, flag, perm)
			closeErr := parent.Close()
			if closeErr != nil {
				if f != nil {
					closeErr = errors.Join(closeErr, f.Close())
				}
				return nil, errors.Join(err, closeErr)
			}
			return f, err
		}
		name = leaf
	}
	var f *os.File
	var err error
	if root == nil {
		f, err = os.OpenFile(name, flag|noFollowFlags, perm)
	} else {
		f, err = root.OpenFile(name, flag|noFollowFlags, perm)
	}
	if err != nil {
		return nil, err
	}
	if err := validateHandle(f); err != nil {
		return nil, errors.Join(err, f.Close())
	}
	if root != nil {
		// os.Root can resolve an in-root symlink itself after the kernel
		// refuses it. Reject that resolution before exposing the handle.
		named, err := root.Lstat(name)
		if err != nil {
			return nil, errors.Join(err, f.Close())
		}
		opened, err := f.Stat()
		if err != nil {
			return nil, errors.Join(err, f.Close())
		}
		if named.Mode()&fs.ModeSymlink != 0 || !os.SameFile(named, opened) {
			return nil, errors.Join(errors.New("opened entry changed identity or is a symbolic link"), f.Close())
		}
	}
	return f, nil
}

// noFollowParent pins each native path component before advancing. Comparing
// the named and opened directory prevents an in-root link swap from changing
// the authorization target even where os.Root would otherwise follow it.
func noFollowParent(root *os.Root, name string) (*os.Root, string, error) {
	if !filepath.IsLocal(name) {
		return nil, "", errors.New("file name must be relative to the held root")
	}
	parts := strings.Split(filepath.Clean(name), string(filepath.Separator))
	parent := root
	fail := func(err error) (*os.Root, string, error) {
		if parent != root {
			err = errors.Join(err, parent.Close())
		}
		return nil, "", err
	}
	for _, part := range parts[:len(parts)-1] {
		named, err := parent.Lstat(part)
		if err != nil {
			return fail(err)
		}
		if !named.IsDir() || named.Mode()&fs.ModeSymlink != 0 {
			return fail(errors.New("file parent is not an ordinary directory"))
		}
		child, err := parent.OpenRoot(part)
		if err != nil {
			return fail(err)
		}
		opened, err := child.Stat(".")
		if err != nil {
			return fail(errors.Join(err, child.Close()))
		}
		after, err := parent.Lstat(part)
		if err != nil {
			return fail(errors.Join(err, child.Close()))
		}
		if after.Mode()&fs.ModeSymlink != 0 || !os.SameFile(named, opened) || !os.SameFile(after, opened) {
			return fail(errors.Join(errors.New("file parent changed identity or is a symbolic link"), child.Close()))
		}
		if parent != root {
			if err := parent.Close(); err != nil {
				return nil, "", errors.Join(err, child.Close())
			}
		}
		parent = child
	}
	return parent, parts[len(parts)-1], nil
}

// WriteAtomic writes content through OpenNoFollow into a sibling temp file
// and renames it onto path. A symlink at the temp name is refused rather than
// followed; truncation is never requested of OpenNoFollow.
func WriteAtomic(path string, content []byte, perm fs.FileMode) error {
	if perm == 0 {
		perm = 0o644
	}
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	tmp, err := os.CreateTemp(dir, ".safefile-*")
	if err != nil {
		return err
	}
	tmpName := tmp.Name()
	if err := tmp.Close(); err != nil {
		_ = os.Remove(tmpName)
		return err
	}
	f, err := OpenNoFollow(nil, tmpName, os.O_WRONLY, perm)
	if err != nil {
		_ = os.Remove(tmpName)
		return err
	}
	if _, err := f.Write(content); err != nil {
		return errors.Join(err, f.Close(), os.Remove(tmpName))
	}
	if err := f.Close(); err != nil {
		_ = os.Remove(tmpName)
		return err
	}
	if err := os.Chmod(tmpName, perm); err != nil {
		_ = os.Remove(tmpName)
		return err
	}
	if err := os.Rename(tmpName, path); err != nil {
		_ = os.Remove(tmpName)
		return err
	}
	return nil
}
