package verify

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/flags"
)

// envAllowKey is the config.yaml key that extends the base allowlist.
const envAllowKey = "verify.env_allow"

// baseEnvAllow names the host variables a verification command may inherit.
// Everything else is dropped. The list is what the common test toolchains need
// to find themselves and their caches (PATH, HOME, the Go, Cargo, Rustup, Node,
// Python and JVM roots) and to behave like a terminal-less CI run; it holds no
// credential. A repository whose tests need more says so in `verify.env_allow`.
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

var (
	envNameRE    = regexp.MustCompile(`^[A-Za-z_][A-Za-z0-9_]*$`)
	secretNameRE = regexp.MustCompile(`(?i)(KEY|SECRET|TOKEN|PASSW|CREDENTIAL|AUTH|PRIVATE|COOKIE)`)
	blockListRE  = regexp.MustCompile(`(?m)^\s*env_allow:\s*(#.*)?$`)
)

// EnvPolicy is the environment a verification command runs with: the base
// allowlist plus the repository's `verify.env_allow` additions that were
// admitted. Only names are ever stored, never values.
type EnvPolicy struct {
	// Extra holds the config-supplied names that were admitted, sorted.
	Extra []string
	// Refused holds config-supplied names that were not admitted: not a legal
	// variable name, or shaped like a credential. config.yaml is part of the
	// tree an agent edits and the run being judged is that agent's, so it
	// cannot be what hands a credential to the command that judges it.
	Refused []string
	// Err is set when the config could not be read as an allowlist. The policy
	// then carries the base allowlist alone: unreadable config narrows access
	// and says so, it never widens it.
	Err string
}

// LoadEnvPolicy reads `verify.env_allow` from root's .devcouncil/config.yaml.
// A missing file is the base allowlist with no error. The value is a scalar or
// an inline list (`[A, B]` or `A,B`); a block list is an error, because the
// config reader does not carry lists and silently reading one as empty would
// hide a setting the operator wrote.
func LoadEnvPolicy(root string) EnvPolicy {
	path := filepath.Join(root, ".devcouncil", "config.yaml")
	data, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			return EnvPolicy{}
		}
		return EnvPolicy{Err: fmt.Sprintf("%s: %v", path, err)}
	}
	values, err := flags.ParseConfig(strings.NewReader(string(data)))
	if err != nil {
		return EnvPolicy{Err: fmt.Sprintf("%s: %v", path, err)}
	}
	raw, present := values[envAllowKey]
	if !present {
		if blockListRE.Match(data) {
			return EnvPolicy{Err: fmt.Sprintf("%s: verify.env_allow must be written inline, as [NAME, NAME]; block lists are not read", path)}
		}
		return EnvPolicy{}
	}
	var p EnvPolicy
	seen := map[string]struct{}{}
	raw = strings.TrimSpace(strings.TrimSuffix(strings.TrimPrefix(strings.TrimSpace(raw), "["), "]"))
	for _, name := range strings.FieldsFunc(raw, func(r rune) bool { return r == ',' || r == ' ' || r == '\t' }) {
		name = strings.Trim(name, `"'`)
		if name == "" {
			continue
		}
		if _, dup := seen[name]; dup {
			continue
		}
		seen[name] = struct{}{}
		if !envNameRE.MatchString(name) || secretNameRE.MatchString(name) {
			p.Refused = append(p.Refused, name)
			continue
		}
		p.Extra = append(p.Extra, name)
	}
	sort.Strings(p.Extra)
	sort.Strings(p.Refused)
	return p
}

// allowed reports whether name may reach the command.
func (p EnvPolicy) allowed(name string) bool {
	if _, ok := baseEnvAllow[name]; ok {
		return true
	}
	for _, prefix := range baseEnvAllowPrefixes {
		if strings.HasPrefix(name, prefix) {
			return true
		}
	}
	for _, extra := range p.Extra {
		if extra == name {
			return true
		}
	}
	return false
}

// Environ filters environ (KEY=VALUE entries, as os.Environ returns them) down
// to what the policy allows. It always returns a non-nil slice: a nil Cmd.Env
// means "inherit everything", which is exactly what this exists to prevent.
func (p EnvPolicy) Environ(environ []string) []string {
	out := make([]string, 0, len(environ))
	for _, kv := range environ {
		name, _, ok := strings.Cut(kv, "=")
		if ok && p.allowed(name) {
			out = append(out, kv)
		}
	}
	return out
}
