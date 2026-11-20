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
	// so in the secretNameRE vocabulary. It is a NAME rule: it cannot promise
	// that no credential gets through, only that these shapes do not. Classes:
	//   - tokens between underscores or at the ends: DSN, PAT, JWT, PASS, PWD,
	//     HOOK, WEBHOOK(S); so PATCH_LEVEL, PATTERN, OUTPUT_PATH, BYPASS_CACHE
	//     and GIT_HOOKS_DIR stay admitted. Run-together PAT is deliberately not
	//     refused (COMPAT_MODE, COMPASS, ...): no token boundary, too many
	//     ordinary names. Run-together DSN (suffix) and WEBHOOK (anywhere) are
	//     refused (SENTRYDSN, SLACKWEBHOOK).
	//   - PGPASS, PASSPHRASE, NETRC, ASKPASS (GIT_ASKPASS names a program the
	//     child runs with credentials), CONNECTION_STRING / CONNSTR.
	//   - a name ending in PROXY (proxy URLs embed user:pass). This also refuses
	//     NO_PROXY and GOPROXY: a module proxy URL can embed credentials, and a
	//     repository that needs either has no per-repo route.
	//   - datastore, broker, mail, directory and package-index URLs or URIs,
	//     whose conventional value embeds user:password@: a stem token, then
	//     _URL or _URI followed by _, a digit or the end (DATABASE_URL_TEST,
	//     DB_URL_RO, DATABASE_URL2), and any *_INDEX_URL (PIP/UV).
	// A plain CI_URL or API_URL (no credential by convention) stays admitted.
	credentialNameRE = regexp.MustCompile(`(?i)((^|_)(DSN|PAT|JWT|PASS|HOOK|WEBHOOKS?)($|_)|DSN$|WEBHOOK|PGPASS|PASSPHRASE|NETRC|ASKPASS|CONNECTION_?STRING|CONNSTR|PROXY$|((^|_)(DATABASE|DB|REDIS|AMQP|MONGO|MONGODB|POSTGRES|POSTGRESQL|PG|JDBC|DATASOURCE|MYSQL|MARIADB|MSSQL|SQLSERVER|SQLITE|COUCHDB|NEO4J|ELASTICSEARCH|OPENSEARCH|CLICKHOUSE|KAFKA|RABBITMQ|NATS|MEMCACHED|SMTP|LDAP|BROKER|CACHE)(_[A-Z0-9]+)*|_INDEX)_(URL|URI)($|_|[0-9]))`)
	pwdTokenRE       = regexp.MustCompile(`(?i)(^|_)PWD($|_)`)
	blockListRE      = regexp.MustCompile(`(?m)^\s*env_allow:\s*(#.*)?$`)
)

// isCredentialName is the one name rule for what a repository's own config may
// not hand to a verification command. It binds LoadEnvPolicy only. There is no
// user-level env_allow source today: proc.ScrubEnv's extras come from
// EnvPolicy.Environ and from code-owned lists, and are not subject to it. A
// PWD token (DB_PWD, MYSQL_PWD) is a password, but the bare shell variable PWD
// is the working directory and stays admitted.
func isCredentialName(name string) bool {
	if secretNameRE.MatchString(name) || credentialNameRE.MatchString(name) {
		return true
	}
	return !strings.EqualFold(name, "PWD") && pwdTokenRE.MatchString(name)
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
