package verify

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/flags"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

// envAllowKey is the config.yaml key that extends the base allowlist.
const envAllowKey = "verify.env_allow"

// A verification command inherits proc's base allowlist (proc.ScrubEnv:
// the toolchain roots and terminal settings, no credential). A repository
// whose tests need more says so in `verify.env_allow`.

var (
	envNameRE    = regexp.MustCompile(`^[A-Za-z_][A-Za-z0-9_]*$`)
	secretNameRE = regexp.MustCompile(`(?i)(KEY|SECRET|TOKEN|PASSW|CREDENTIAL|AUTH|PRIVATE|COOKIE)`)
	// credentialNameRE names variables that carry a credential without saying
	// so in the secretNameRE vocabulary: a DSN or PAT token, a webhook (the URL
	// is the secret), a connection string, or a datastore/broker URL or URI
	// whose conventional value embeds user:password@. Tokens are matched
	// between underscores so PATCH_LEVEL, PATTERN and DB_HOST stay admitted,
	// and a plain CI_URL or API_URL (no credential by convention) stays too.
	credentialNameRE = regexp.MustCompile(`(?i)((^|_)(DSN|PAT|WEBHOOKS?)($|_)|CONNECTION_?STRING|CONNSTR|(^|_)(DATABASE|DB|REDIS|AMQP|MONGO|MONGODB|POSTGRES|POSTGRESQL|MYSQL|BROKER|CACHE)(_[A-Z0-9]+)*_(URL|URI)$)`)
	blockListRE      = regexp.MustCompile(`(?m)^\s*env_allow:\s*(#.*)?$`)
)

// isCredentialName is the one name rule for what a repository's own config may
// not hand to a verification command. It binds LoadEnvPolicy only: a caller
// that passes extras straight to proc.ScrubEnv (the user-level path) is not
// subject to it.
func isCredentialName(name string) bool {
	return secretNameRE.MatchString(name) || credentialNameRE.MatchString(name)
}

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
		if !envNameRE.MatchString(name) || isCredentialName(name) {
			p.Refused = append(p.Refused, name)
			continue
		}
		p.Extra = append(p.Extra, name)
	}
	sort.Strings(p.Extra)
	sort.Strings(p.Refused)
	return p
}

// Environ filters environ (KEY=VALUE entries, as os.Environ returns them) down
// to what the policy allows. It always returns a non-nil slice: a nil Cmd.Env
// means "inherit everything", which is exactly what this exists to prevent.
func (p EnvPolicy) Environ(environ []string) []string {
	return proc.ScrubEnv(environ, p.Extra...)
}
