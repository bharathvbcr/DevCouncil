package proc

import (
	"slices"
	"strings"
)

// baseEnvAllow names the host variables a child DevCouncil starts from code it
// does not own (a repository's verification command, another app's script)
// may inherit. Everything else is dropped. The list is what the common
// toolchains need to find themselves and their caches (PATH, HOME, the Go,
// Cargo, Rustup, Node, Python and JVM roots) and to behave like a
// terminal-less CI run; it holds no credential. A caller that needs more names
// them in ScrubEnv's extra.
//
// DevCouncil owns this list. It is not shared with any host that embeds
// DevCouncil: the embedder is downstream and decides its own.
var baseEnvAllow = map[string]struct{}{
	"PATH": {}, "HOME": {}, "USER": {}, "LOGNAME": {}, "SHELL": {},
	"TMPDIR": {}, "TMP": {}, "TEMP": {}, "TZ": {}, "TERM": {}, "COLORTERM": {}, "NO_COLOR": {}, "CI": {},
	"LANG": {}, "LANGUAGE": {},
	"GOCACHE": {}, "GOMODCACHE": {}, "GOPATH": {}, "GOROOT": {}, "GOFLAGS": {}, "GOTOOLCHAIN": {}, "GOENV": {},
	"CARGO_HOME": {}, "CARGO_TARGET_DIR": {}, "RUSTUP_HOME": {}, "RUSTUP_TOOLCHAIN": {},
	"NVM_DIR": {}, "NODE_PATH": {}, "PYENV_ROOT": {}, "VIRTUAL_ENV": {}, "JAVA_HOME": {},
	"XDG_CACHE_HOME": {}, "XDG_CONFIG_HOME": {}, "XDG_DATA_HOME": {}, "XDG_RUNTIME_DIR": {},
	"DEVELOPER_DIR": {}, "SDKROOT": {},
}

// baseEnvAllowPrefixes admits families: LC_ALL, LC_CTYPE and the rest.
var baseEnvAllowPrefixes = []string{"LC_"}

// baseEnvAllowed reports whether name is on the base allowlist. Names are
// matched exactly, as the process environment spells them.
func baseEnvAllowed(name string) bool {
	if _, ok := baseEnvAllow[name]; ok {
		return true
	}
	for _, prefix := range baseEnvAllowPrefixes {
		if strings.HasPrefix(name, prefix) {
			return true
		}
	}
	return false
}

// ScrubEnv filters environ (KEY=VALUE entries, as os.Environ returns them)
// down to the base allowlist plus the names in extra. It always returns a
// non-nil slice: a nil Cmd.Env means "inherit everything", which is exactly
// what this exists to prevent.
func ScrubEnv(environ []string, extra ...string) []string {
	out := make([]string, 0, len(environ))
	for _, kv := range environ {
		name, _, ok := strings.Cut(kv, "=")
		if !ok {
			continue
		}
		if baseEnvAllowed(name) || slices.Contains(extra, name) {
			out = append(out, kv)
		}
	}
	return out
}
