package platform

import (
	"os"
	"path/filepath"
	"strings"
)

// MigrateToUnifiedHome relocates per-user state written by older Gortex
// versions (which split files across ~/.config/gortex, ~/.cache/gortex,
// and a flat ~/.gortex) into the unified ~/.gortex tree this package now
// resolves. It is best-effort and idempotent: a destination that already
// exists is never overwritten, and individual move failures are reported
// to logf but never abort the caller.
//
// It is a no-op when any XDG_*_HOME variable is set to an absolute path —
// that signals the user opted into the XDG layout, so there is nothing to
// unify. logf may be nil. Because every step short-circuits once its
// destination exists, the function is cheap to call on every startup; it
// only logs (and only does work) on the first run after the upgrade.
func MigrateToUnifiedHome(logf func(format string, args ...any)) {
	if logf == nil {
		logf = func(string, ...any) {}
	}
	// Respect an explicit XDG opt-in: relocate nothing.
	for _, v := range []string{"XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"} {
		if val := os.Getenv(v); val != "" && filepath.IsAbs(val) {
			return
		}
	}
	home, err := os.UserHomeDir()
	if err != nil || home == "" {
		return
	}
	root := filepath.Join(home, homeDir) // ~/.gortex

	// 1. Global config moves out of ~/.config/gortex into the root.
	migrateInto(logf, filepath.Join(home, ".config", gortexDir, "config.yaml"), filepath.Join(root, "config.yaml"))
	migrateInto(logf, filepath.Join(home, ".config", gortexDir, "servers.toml"), filepath.Join(root, "servers.toml"))

	// 2. The old ~/.cache/gortex tree folds into ~/.gortex/cache, except
	//    downloaded models (durable data, kept out of cache so a cache
	//    wipe doesn't discard them) and the stale daemon socket / pid
	//    (regenerated on the next start).
	oldCache := filepath.Join(home, ".cache", gortexDir)
	if entries, err := os.ReadDir(oldCache); err == nil {
		for _, e := range entries {
			switch e.Name() {
			case "daemon.sock", "daemon.pid":
				continue
			case "models":
				migrateInto(logf, filepath.Join(oldCache, "models"), filepath.Join(root, "models"))
			default:
				migrateInto(logf, filepath.Join(oldCache, e.Name()), filepath.Join(root, cacheSub, e.Name()))
			}
		}
	}

	// 3. In-place reorg of the ~/.gortex root: the backend store (and its
	//    WAL/shm sidecars) move under store/, and the old memories-cache
	//    directory becomes memories/.
	if entries, err := os.ReadDir(root); err == nil {
		for _, e := range entries {
			name := e.Name()
			if e.IsDir() {
				continue
			}
			if strings.HasSuffix(name, ".store") || strings.HasPrefix(name, "store.sqlite") {
				migrateInto(logf, filepath.Join(root, name), filepath.Join(root, "store", name))
			}
		}
	}
	migrateInto(logf, filepath.Join(root, "memories-cache"), filepath.Join(root, "memories"))
}

// migrateInto moves src to dst when src exists and dst does not. The move
// is a rename (atomic within a filesystem); a cross-device failure is
// logged and the source left in place rather than risking a partial copy
// of a live store. Idempotent: a pre-existing dst short-circuits.
func migrateInto(logf func(string, ...any), src, dst string) {
	if src == dst {
		return
	}
	if _, err := os.Lstat(src); err != nil {
		return // nothing to migrate
	}
	if _, err := os.Lstat(dst); err == nil {
		return // already present — never clobber
	}
	if err := os.MkdirAll(filepath.Dir(dst), 0o755); err != nil {
		logf("gortex: migrate %s: mkdir parent failed: %v", dst, err)
		return
	}
	if err := os.Rename(src, dst); err != nil {
		logf("gortex: could not migrate %s -> %s (move it manually): %v", src, dst, err)
		return
	}
	logf("gortex: migrated %s -> %s", src, dst)
}
