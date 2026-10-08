package devmap

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

// Host integration and skill installation belong to `devmap`.
//
// DevCouncil used to carry a Go copy of both: the per-host MCP document table
// for its own `devcouncil` server, and a second skill installer writing the
// same `.devcouncil-skills.json` receipt and lock as `devmap skills install`.
// Two writers of one receipt and two tables of one set of host documents were
// kept in step only by tests that parsed the Rust source. Both now have one
// owner: DevCouncil hands its server and its embedded skill library to the
// calls below, over stdin, and DevMap writes them through its own table and
// its own installer.

// installBudget bounds one integrate or skill install. A cold install is the
// slow case; anything past this is a hang, not work.
const installBudget = 5 * time.Minute

// InstallMode is how an installing command treats the tree.
type InstallMode string

const (
	InstallApply  InstallMode = "apply"
	InstallCheck  InstallMode = "check"
	InstallDryRun InstallMode = "dry-run"
)

// Server is one MCP server for `devmap integrate --servers-stdin` to register
// in a host's project document, in that host's own spelling.
type Server struct {
	Name    string            `json:"name"`
	Command string            `json:"command"`
	Args    []string          `json:"args,omitempty"`
	Env     map[string]string `json:"env,omitempty"`
	Cwd     string            `json:"cwd,omitempty"`
}

// Asset is one host file an install examined.
type Asset struct {
	Path    string `json:"path"`
	Changed bool   `json:"changed"`
	Note    string `json:"note"`
}

// Guide is one agent guide an install examined.
type Guide struct {
	Path        string `json:"path"`
	Disposition string `json:"disposition"`
}

// IntegrateReport is what `devmap --json integrate` reports.
type IntegrateReport struct {
	Host            string   `json:"host"`
	Guides          []Guide  `json:"guides"`
	SkillsWritten   []string `json:"skills_written"`
	SkillsDiffering []string `json:"skills_differing"`
	GlobalMCP       []Asset  `json:"global_mcp"`
	ProjectMCP      []Asset  `json:"project_mcp"`
	Hooks           []Asset  `json:"hooks"`
	Servers         []Asset  `json:"servers"`
	Notes           []string `json:"notes"`
	CheckOK         bool     `json:"check_ok"`
}

// SkillsReport is what `devmap --json skills install` reports.
type SkillsReport struct {
	Written   []string `json:"written"`
	Differing []string `json:"differing"`
	Receipt   string   `json:"receipt"`
	CheckOK   bool     `json:"check_ok"`
}

// Skill is one skill of a library handed to `skills install --library-stdin`.
type Skill struct {
	Name    string `json:"name"`
	Content string `json:"content"`
}

// Integrate runs `devmap integrate <host>` for the client's repository and
// registers `servers` beside DevMap's own entry.
//
// Under InstallCheck a tree that differs is an answer, not a failure: the
// report comes back with CheckOK false. Anything else that exits non-zero is
// an error, and so is a check that produced no report — a check that could
// not run must not read as one that ran.
func (c *Client) Integrate(ctx context.Context, host string, mode InstallMode, servers []Server) (*IntegrateReport, error) {
	args := []string{"integrate", host, "--project-root", c.Root}
	var stdin []byte
	if len(servers) > 0 {
		encoded, err := json.Marshal(map[string]any{"servers": servers})
		if err != nil {
			return nil, err
		}
		stdin = encoded
		args = append(args, "--servers-stdin")
	}
	args, err := withMode(args, mode)
	if err != nil {
		return nil, err
	}
	var report IntegrateReport
	if err := c.install(ctx, &report, mode, stdin, args...); err != nil {
		return nil, err
	}
	return &report, nil
}

// InstallSkills installs `library` under each destination through `devmap
// skills install --library-stdin`, the one writer of the skill receipt.
// Destinations empty means DevMap's defaults.
func (c *Client) InstallSkills(ctx context.Context, destinations []string, library []Skill, mode InstallMode) (*SkillsReport, error) {
	if len(library) == 0 {
		return nil, errors.New("no skills to install")
	}
	encoded, err := json.Marshal(map[string]any{"skills": library})
	if err != nil {
		return nil, err
	}
	args := []string{"skills", "install", "--project-root", c.Root, "--library-stdin"}
	for _, dest := range destinations {
		args = append(args, "--destination", dest)
	}
	args, err = withMode(args, mode)
	if err != nil {
		return nil, err
	}
	var report SkillsReport
	if err := c.install(ctx, &report, mode, encoded, args...); err != nil {
		return nil, err
	}
	return &report, nil
}

func withMode(args []string, mode InstallMode) ([]string, error) {
	switch mode {
	case InstallApply:
		return args, nil
	case InstallCheck:
		return append(args, "--check"), nil
	case InstallDryRun:
		return append(args, "--dry-run"), nil
	}
	return nil, fmt.Errorf("unknown install mode %q", mode)
}

// install runs one installing command and decodes its report.
//
// The report is the first JSON document on stdout. A failing `--json` run
// follows it with the CLI's error envelope, so the rest is not parsed.
func (c *Client) install(ctx context.Context, into any, mode InstallMode, stdin []byte, args ...string) error {
	out, err := c.invoke(ctx, installBudget, stdin, args...)
	if err != nil {
		return err
	}
	command := strings.Join(args[:2], " ")
	decodeErr := json.NewDecoder(bytes.NewReader(out.stdout)).Decode(into)
	if out.runErr != nil {
		if mode == InstallCheck && decodeErr == nil && reportsDrift(into) {
			return nil
		}
		return predatesFlag(out.failure(command), out.stderr.text)
	}
	if decodeErr != nil {
		return fmt.Errorf("devmap %s returned unparseable output: %w", command, decodeErr)
	}
	return nil
}

// reportsDrift is true for a decoded check report that says the tree differs:
// the one non-zero exit that is an answer.
func reportsDrift(report any) bool {
	switch r := report.(type) {
	case *IntegrateReport:
		return !r.CheckOK
	case *SkillsReport:
		return !r.CheckOK
	}
	return false
}

// predatesFlag names the remedy when an installed devmap is older than the
// contract: clap's refusal of an unknown flag says nothing about versions.
func predatesFlag(err error, stderr []byte) error {
	for _, flag := range []string{"--servers-stdin", "--library-stdin"} {
		if bytes.Contains(stderr, []byte("unexpected argument '"+flag+"'")) {
			return fmt.Errorf("%w; this devmap predates %s — install the devmap built from this release", err, flag)
		}
	}
	return err
}

// ResolveOutside names the devmap an installing command runs.
//
// An explicit path — the caller's flag, else DEVMAP_BIN — is used or refused,
// never replaced by a PATH lookup. Otherwise PATH is searched, refusing any
// candidate whose canonical location is inside root: an install writes host
// configuration, and running a binary the repository supplied to do it is
// running repository content.
func ResolveOutside(explicit, root string) (string, error) {
	if explicit == "" {
		explicit = strings.TrimSpace(os.Getenv("DEVMAP_BIN"))
	}
	if explicit != "" {
		info, err := os.Stat(explicit)
		if err != nil {
			return "", fmt.Errorf("devmap binary %s: %w (an explicit binary is used or refused, never replaced)", explicit, err)
		}
		if info.IsDir() {
			return "", fmt.Errorf("devmap binary %s is a directory (an explicit binary is used or refused, never replaced)", explicit)
		}
		return explicit, nil
	}
	found, err := proc.LookPathOutside("devmap", root)
	if err != nil {
		return "", fmt.Errorf("no devmap outside the repository was found on PATH (%w); pass --devmap-bin or set DEVMAP_BIN", err)
	}
	return found, nil
}

// RelativeTo names a path DevMap reported relative to root when it lies
// inside it, whichever spelling of root it used: DevMap canonicalises, so on
// macOS `/var/…` arrives as `/private/var/…`. A path outside — a user-level
// config — is returned unchanged.
func RelativeTo(root, p string) string {
	roots := []string{root}
	if resolved, err := filepath.EvalSymlinks(root); err == nil && resolved != root {
		roots = append(roots, resolved)
	}
	for _, base := range roots {
		if rel, err := filepath.Rel(base, p); err == nil && rel != ".." && !strings.HasPrefix(rel, "../") {
			return filepath.ToSlash(rel)
		}
	}
	return p
}
