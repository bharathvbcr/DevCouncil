package gatescfg

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/flags"
)

// WorkbenchDBKey is the config key naming the database the host reads its tasks
// from. Tasks are workbench items, and leases, gaps, evidence and runs live in
// the same file, so this is the whole store.
const WorkbenchDBKey = "workbench.db"

// WorkbenchDB says which database the host reads and where that was decided.
type WorkbenchDB struct {
	// Path is the absolute path of the database file.
	Path string `json:"path"`
	// Origin is OriginDefault (no key) or OriginConfig.
	Origin     string `json:"origin"`
	ConfigPath string `json:"config_path"`
}

// DefaultWorkbenchDB is where a repository keeps its store.
func DefaultWorkbenchDB(root string) string {
	return filepath.Join(root, ".devcouncil", "state.sqlite")
}

// ResolveWorkbenchDB reads `workbench.db` from `.devcouncil/config.yaml` and
// returns the database the host opens.
//
// The host reads only a repo-local store. The key exists so a repository can
// name another file, but the file must be inside the repository's `.devcouncil/`
// directory: never GitPulse's profile database, never anything an operator's
// typo or a hostile config could point at. Items reach this store by being
// synced into it (see docs/quickstart.md), so the host and the board are never
// two writers on one SQLite file.
//
// A config that is present but unreadable, a value that leaves `.devcouncil/`,
// a path through a symlink, or a database file that is itself a symlink is an
// error: there is no fallback to the default, because quietly reading a
// different store than the one configured is the failure this check exists for.
func ResolveWorkbenchDB(root string) (WorkbenchDB, error) {
	configPath := filepath.Join(root, ".devcouncil", "config.yaml")
	out := WorkbenchDB{Path: DefaultWorkbenchDB(root), Origin: OriginDefault, ConfigPath: configPath}
	f, err := os.Open(configPath)
	if err != nil {
		if os.IsNotExist(err) {
			return out, checkRepoLocal(root, out.Path)
		}
		return out, fmt.Errorf("%s: %w", configPath, err)
	}
	defer func() { _ = f.Close() }()
	values, err := flags.ParseConfig(f)
	if err != nil {
		return out, fmt.Errorf("%s: %w", configPath, err)
	}
	raw, set := values[WorkbenchDBKey]
	if !set || strings.TrimSpace(raw) == "" {
		return out, checkRepoLocal(root, out.Path)
	}
	raw = strings.TrimSpace(raw)
	if strings.ContainsAny(raw, "\x00$~") {
		return out, fmt.Errorf("%s: %s %q contains a character that is never expanded (NUL, $ or ~); name a path inside .devcouncil/", configPath, WorkbenchDBKey, raw)
	}
	path := raw
	if !filepath.IsAbs(path) {
		path = filepath.Join(root, path)
	}
	out.Path, out.Origin = filepath.Clean(path), OriginConfig
	if err := checkRepoLocal(root, out.Path); err != nil {
		return out, fmt.Errorf("%s: %s: %w", configPath, WorkbenchDBKey, err)
	}
	return out, nil
}

// checkRepoLocal refuses a database path that is not a plain file inside
// `<root>/.devcouncil/`, resolved through symlinks so a link cannot lead out.
func checkRepoLocal(root, path string) error {
	dir := filepath.Join(root, ".devcouncil")
	realDir, err := filepath.EvalSymlinks(dir)
	if err != nil {
		if os.IsNotExist(err) {
			// Nothing to resolve yet; the lexical check is all there is.
			return lexicallyInside(dir, path)
		}
		return err
	}
	if err := lexicallyInside(dir, path); err != nil {
		return err
	}
	parent, err := filepath.EvalSymlinks(filepath.Dir(path))
	if err != nil {
		if os.IsNotExist(err) {
			return nil
		}
		return err
	}
	if err := lexicallyInside(realDir, filepath.Join(parent, filepath.Base(path))); err != nil {
		return fmt.Errorf("%q resolves outside %s through a symlink", path, dir)
	}
	if info, err := os.Lstat(path); err == nil {
		if info.Mode()&os.ModeSymlink != 0 {
			return fmt.Errorf("%q is a symlink; the store must be a regular file inside %s", path, dir)
		}
		if info.IsDir() {
			return fmt.Errorf("%q is a directory, not a database file", path)
		}
	}
	return nil
}

func lexicallyInside(dir, path string) error {
	rel, err := filepath.Rel(dir, path)
	if err != nil || rel == "." || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
		return fmt.Errorf("%q is not inside %s; the host reads only a repo-local store", path, dir)
	}
	return nil
}
