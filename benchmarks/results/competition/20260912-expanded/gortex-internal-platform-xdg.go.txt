package platform

import (
	"os"
	"path/filepath"
)

// Gortex keeps all per-user state under one directory tree. By default
// that tree is $HOME/.gortex, holding config, cache, the on-disk store,
// downloaded models, and development memories side by side — a single
// place to find, back up, or delete.
//
// The XDG Base Directory variables remain an explicit escape hatch:
// when XDG_CONFIG_HOME / XDG_DATA_HOME / XDG_CACHE_HOME is set to an
// absolute path it wins, and that category's files live under
// "<XDG_*_HOME>/gortex" (standard XDG layout) instead of inside the
// unified ~/.gortex tree. This keeps XDG-strict setups, sandboxes, and
// the test suite working while giving everyone else one folder.

const (
	// gortexDir is the application sub-directory Gortex owns inside an
	// XDG base directory when an XDG_*_HOME override is in effect.
	gortexDir = "gortex"
	// homeDir is the unified per-user directory ($HOME/.gortex) used
	// when no XDG override applies.
	homeDir = ".gortex"
	// cacheSub disambiguates cache from config/data inside the unified
	// ~/.gortex tree. Under an XDG_CACHE_HOME override the base is
	// already cache-specific, so this sub-path is not added there.
	cacheSub = "cache"
)

// Home returns the unified per-user Gortex directory ($HOME/.gortex),
// falling back to a temp-dir equivalent when $HOME can't be resolved.
// This is the root the cache / store / models / memories sub-paths hang
// off when no XDG override is in play.
func Home() string {
	home, err := os.UserHomeDir()
	if err != nil || home == "" {
		return filepath.Join(os.TempDir(), homeDir)
	}
	return filepath.Join(home, homeDir)
}

// unifiedDir resolves a Gortex base for one XDG category. An absolute
// $envVar wins ("<envVar>/gortex" — the standard XDG location), so
// XDG-strict setups, sandboxes, and the test suite keep working.
// Otherwise the category collapses into the unified ~/.gortex tree,
// with homeSub distinguishing cache ("cache") from config/data ("").
//
// A non-absolute $envVar is ignored, as the XDG Base Directory
// specification mandates ("If [the variable] is set to a relative path
// the value MUST be ignored").
func unifiedDir(envVar, homeSub string) string {
	if v := os.Getenv(envVar); v != "" && filepath.IsAbs(v) {
		return filepath.Join(v, gortexDir)
	}
	return filepath.Join(Home(), homeSub)
}

// ConfigDir is where Gortex reads/writes configuration (config.yaml,
// servers.toml). Default ~/.gortex; an absolute $XDG_CONFIG_HOME
// relocates it to "<XDG_CONFIG_HOME>/gortex".
func ConfigDir() string { return unifiedDir("XDG_CONFIG_HOME", "") }

// DataDir is the root for durable, non-disposable state (the on-disk
// store, downloaded models, development memories). Default ~/.gortex;
// an absolute $XDG_DATA_HOME relocates it to "<XDG_DATA_HOME>/gortex".
func DataDir() string { return unifiedDir("XDG_DATA_HOME", "") }

// CacheDir is where Gortex keeps disposable state (the daemon socket /
// pid / log, snapshots, eval and token caches). Default ~/.gortex/cache;
// an absolute $XDG_CACHE_HOME relocates it to "<XDG_CACHE_HOME>/gortex".
func CacheDir() string { return unifiedDir("XDG_CACHE_HOME", cacheSub) }

// OSCacheDir is retained for callers that historically rooted their
// cache at os.UserCacheDir(); under the unified layout it resolves to
// the same directory as CacheDir.
func OSCacheDir() string { return CacheDir() }

// StoreDir is where the on-disk backend persists its store:
// <DataDir>/store (~/.gortex/store by default).
func StoreDir() string { return filepath.Join(DataDir(), "store") }

// ModelsDir is where downloaded embedding models live: <DataDir>/models
// (~/.gortex/models by default). Models live under DataDir rather than
// CacheDir so a cache wipe doesn't discard multi-hundred-MB downloads.
func ModelsDir() string { return filepath.Join(DataDir(), "models") }

// MemoriesDir is where cross-session development memories persist:
// <DataDir>/memories (~/.gortex/memories by default).
func MemoriesDir() string { return filepath.Join(DataDir(), "memories") }

// TelemetryDir is where opt-in anonymous usage telemetry buffers its daily
// rollups and stores the install id: <DataDir>/telemetry (~/.gortex/telemetry
// by default). Under DataDir (durable) rather than CacheDir so a cache wipe
// doesn't silently reset the stable install id or drop unsent completed days.
func TelemetryDir() string { return filepath.Join(DataDir(), "telemetry") }
