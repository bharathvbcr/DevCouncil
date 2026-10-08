package integrate

import (
	"context"
	"crypto/rand"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path"
	"path/filepath"
	"slices"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/devmap"
)

// Mode is apply / check / dry-run.
type Mode string

const (
	ModeApply  Mode = "apply"
	ModeCheck  Mode = "check"
	ModeDryRun Mode = "dry-run"
)

// Host integration is owned by `devmap integrate`.
//
// The host list, each host's project document and its shape, DevMap's own
// entry, the `devcouncil` server entry, guides and skills are all written by
// the Rust integrator (`rust/devmap-cli/src/integrate.rs`). This package used
// to keep a second host list and a second per-host document table for the
// `devcouncil` entry, pinned to the Rust ones by tests that parsed the Rust
// source. It now passes that entry over `--servers-stdin` and writes only the
// one file that is DevCouncil's own: the Cursor rule.
//
// What stays here is what only DevCouncil knows: which names it retired and
// why, and what its own server runs.

// Names this command used to accept, and what to do instead.
//
// Kept as data rather than deleted outright: a user who has run
// `integrate gemini` before deserves to be told where it went, not that their
// spelling is invalid. Removing a host from *installation* also says nothing
// about *cleanup* — `Uninstall` still knows `.gemini/settings.json`, because a
// registration already on disk must stay removable after the adapter that
// wrote it is gone.
var retiredHosts = map[string]string{
	"gemini": "Gemini CLI was replaced upstream by Antigravity; " +
		"use `integrate antigravity`. Existing `.gemini/settings.json` hooks are " +
		"still removed by `dev hook disable --client gemini`.",
	"aider": "Aider exposes no MCP server, so there is nothing for `integrate` " +
		"to write. Run Aider against the repository directly; it reaches the " +
		"index through the `devmap` CLI rather than MCP.",
}

// Options for Integrate.
type Options struct {
	Root      string
	Host      string
	Mode      Mode
	DevmapBin string // explicit devmap; used or refused, never replaced
	SelfBin   string // path to this devcouncil binary
}

// Receipt records what was written or would be written.
type Receipt struct {
	HookEntries map[string]int    `json:"hook_entries,omitempty"` // managed entries found before cleanup
	Backups     map[string]string `json:"backups,omitempty"`      // original relative path -> recoverable backup
	Host        string            `json:"host"`
	Mode        string            `json:"mode"`
	Files       map[string]string `json:"files"` // path -> wrote|unchanged|would_write|drift|not_ours
	Spawned     []string          `json:"spawned,omitempty"`
	Notes       []string          `json:"notes,omitempty"`
}

// cursorRule is deliberately split in two registers.
//
// Code navigation stays directive: an agent that guesses where a symbol lives
// reads files it did not need and still answers from the wrong one, so DevMap
// remains the instruction. The DevCouncil *task loop* is advisory, because
// nothing enforces it — host hooks are retired and the gates are off — and a
// rule that tells an agent it must check out a lease that no gate requires
// produces exactly one behaviour: an agent that stops to ask for a lease before
// running `ls`.
const cursorRule = `---
description: DevCouncil navigation, and the task loop when you want it
alwaysApply: true
---

# DevCouncil

Navigate with DevMap before reading or grepping: ` + "`devmap_explore`" + `, ` + "`devmap_search`" + `, ` + "`devmap_ask_evidence`" + ` (a behaviour whose name you do not know: files, source and tests in one answer), ` + "`devmap_impact`" + `, ` + "`devmap_trace`" + `, ` + "`devmap_neighbors`" + `, ` + "`devmap_dead_symbols`" + `, ` + "`devmap_affected_tests`" + ` — or the matching ` + "`devmap`" + ` CLI commands. Run impact analysis before editing a symbol, and read ` + "`truncated`" + ` / ` + "`total`" + ` on every envelope before treating a list as complete. When DevMap cannot answer, record a gap in ` + "`.devcouncil/codeintel/sessions/gaps.jsonl`" + ` rather than switching to another index.

` + "`.devcouncil/repo_map.json`" + ` is the file-level index (subsystems, entry_points, critical_files).

## Product stack

DevCouncil is **components and modules** (` + "`devmap`" + `, ` + "`dcstore`" + `, ` + "`dcverify`" + `, ` + "`dcgrep`" + `, and this host). **Manvi** wraps them into a coding-agent harness. Host apps such as **GitPulse** use Manvi for policy, workbench and agent hosting, and DevCouncil components for code intelligence and related analysis. Take only the modules an app needs; update them independently.

## Tasks and gates are opt-in

DevCouncil host hooks are retired, so
**interactive Shell and Write need no task lease**. Do not claim "Shell is gated", and do not check out a task in order to
run a command. Use the ` + "`devcouncil_*`" + ` MCP tools when you actually want task state,
scope or verification — and prefer them over guessing it. A host that uses MCP policy
must honor its results. The ` + "`--write-gate`" + ` flag has been removed rather than
kept as a refusal, and ` + "`integrations.<host>.write_gate`" + ` is no longer read: both
named a pre-tool-use write gate that only the retired hooks ever installed. A copy
left in an existing ` + "`config.yaml`" + ` is inert.

Engineering skills live under ` + "`.cursor/skills/`" + ` and ` + "`.claude/skills/`" + ` (` + "`devcouncil skills scaffold`" + `).
`

// Run configures one host.
//
// `devmap integrate <host> --servers-stdin` validates the host, writes every
// host document — DevMap's entry and DevCouncil's — and reports per file. A
// host it refuses is refused before anything is written, here included. No
// devmap to run is a refusal too: the documents are DevMap's to write, so a
// receipt without it would describe an installation that did not happen.
func Run(opts Options) (*Receipt, error) {
	root, err := filepath.Abs(opts.Root)
	if err != nil {
		return nil, err
	}
	host := strings.ToLower(opts.Host)
	if err := checkHost(host); err != nil {
		return nil, err
	}
	mode := opts.Mode
	if mode == "" {
		mode = ModeCheck
	}
	installMode, err := mode.install()
	if err != nil {
		return nil, err
	}
	selfBin := opts.SelfBin
	if selfBin == "" {
		if selfBin, err = os.Executable(); err != nil {
			return nil, fmt.Errorf("cannot name this devcouncil binary for the MCP entry: %w", err)
		}
	}
	// PATH discovery refuses any candidate inside the repository: running
	// repository content is what that refusal prevents. An explicit binary
	// stays an operator's choice, used or refused.
	runnable, err := devmap.ResolveOutside(opts.DevmapBin, root)
	if err != nil {
		return nil, fmt.Errorf("%w; host documents, guides and skills are written by "+
			"`devmap integrate`, so nothing was configured", err)
	}

	report, err := devmap.New(runnable, root).Integrate(context.Background(), host, installMode,
		[]devmap.Server{devcouncilServer(selfBin, root)})
	if err != nil {
		// No receipt: DevMap reported nothing this command could vouch for,
		// and an empty one reads as a result. Its refusal names the reason —
		// an unknown host, for one, with the list of the ones it takes.
		return nil, err
	}
	receipt := &Receipt{Host: host, Mode: string(mode), Files: map[string]string{}}
	receipt.Spawned = append(receipt.Spawned, fmt.Sprintf("devmap integrate %s --servers-stdin (%s)", host, mode))
	foldReport(receipt, root, mode, report)

	if host == "cursor" {
		// Every host config is repository content, and a tracked symlink in
		// `.cursor/` would otherwise aim the rule's read or write outside the
		// checkout. It goes through this root and the package's no-follow
		// helpers, the containment `integrate uninstall` has always used.
		repo, err := os.OpenRoot(root)
		if err != nil {
			return receipt, err
		}
		defer func() { _ = repo.Close() }()
		if err := writeCursorRule(repo, mode, receipt); err != nil {
			return receipt, err
		}
	}
	return receipt, nil
}

// devcouncilServer is DevCouncil's own MCP entry: this binary by absolute
// path, so a host runs the build that configured it rather than whichever
// wins on PATH, scoped to the repository it was configured for.
func devcouncilServer(selfBin, root string) devmap.Server {
	return devmap.Server{
		Name:    "devcouncil",
		Command: selfBin,
		Args:    []string{"mcp"},
		Env:     map[string]string{"DEVCOUNCIL_PROJECT_ROOT": root},
		// Written only for hosts that honour it; DevMap's table decides.
		Cwd: root,
	}
}

func (m Mode) install() (devmap.InstallMode, error) {
	switch m {
	case ModeApply:
		return devmap.InstallApply, nil
	case ModeCheck:
		return devmap.InstallCheck, nil
	case ModeDryRun:
		return devmap.InstallDryRun, nil
	}
	return "", fmt.Errorf("unknown mode %q", m)
}

// foldReport records DevMap's per-file outcomes in the receipt, in this
// command's vocabulary, with paths inside the repository made relative.
func foldReport(receipt *Receipt, root string, mode Mode, report *devmap.IntegrateReport) {
	action := func(changed bool) string {
		if !changed {
			return "unchanged"
		}
		switch mode {
		case ModeApply:
			return "wrote"
		case ModeDryRun:
			return "would_write"
		}
		return "drift"
	}
	rel := func(p string) string { return devmap.RelativeTo(root, p) }
	for _, guide := range report.Guides {
		switch guide.Disposition {
		case "not_ours":
			receipt.Files[rel(guide.Path)] = "not_ours"
		case "unchanged":
			receipt.Files[rel(guide.Path)] = "unchanged"
		default:
			receipt.Files[rel(guide.Path)] = action(true)
		}
	}
	for _, written := range report.SkillsWritten {
		receipt.Files[rel(written)] = action(true)
	}
	for _, differing := range report.SkillsDiffering {
		receipt.Files[rel(differing)] = action(true)
	}
	for _, group := range [][]devmap.Asset{report.GlobalMCP, report.ProjectMCP, report.Hooks, report.Servers} {
		for _, asset := range group {
			key := rel(asset.Path)
			// One file can carry two entries (DevMap's and ours); a change to
			// either is a change to the file.
			if receipt.Files[key] == "" || asset.Changed {
				receipt.Files[key] = action(asset.Changed)
			}
		}
	}
	receipt.Notes = append(receipt.Notes, report.Notes...)
}

// checkHost refuses an empty or retired name with its explanation. Every other
// name goes to `devmap integrate`, which owns the list and refuses the rest
// with it.
func checkHost(host string) error {
	if host == "" {
		return errors.New("no host given; `devmap integrate --help` lists the hosts")
	}
	if reason, retired := retiredHosts[host]; retired {
		return fmt.Errorf("host %q is no longer configured here: %s", host, reason)
	}
	return nil
}

const cursorRuleRel = ".cursor/rules/devcouncil.mdc"

// writeCursorRule publishes DevCouncil's Cursor rule.
//
// The file is ours outright, so it is replaced rather than merged — but only
// when the copy on disk is one this command wrote. A user's rule that happens
// to carry the name is refused, not overwritten.
func writeCursorRule(repo *os.Root, mode Mode, receipt *Receipt) error {
	return planWrite(repo, cursorRuleRel, []byte(cursorRule), mode, receipt, replaceOwned(isDevCouncilCursorRule))
}

// isDevCouncilCursorRule recognises every wording of the rule this command has
// written: frontmatter whose description begins "DevCouncil", and a
// `# DevCouncil` heading in the body. Both the Python installer's
// "DevCouncil task loop and navigation" and the current wording carry them.
func isDevCouncilCursorRule(existing []byte) bool {
	text := string(existing)
	rest, ok := strings.CutPrefix(text, "---\n")
	if !ok {
		return false
	}
	front, body, ok := strings.Cut(rest, "\n---\n")
	if !ok {
		return false
	}
	described := false
	for _, line := range strings.Split(front, "\n") {
		if value, found := strings.CutPrefix(line, "description:"); found {
			described = strings.HasPrefix(strings.TrimSpace(value), "DevCouncil")
		}
	}
	return described && slices.Contains(strings.Split(body, "\n"), "# DevCouncil")
}

// replaceOwned is the merge for a file this command owns outright: the
// generated document replaces what is there, provided `owns` recognises the
// existing bytes as ours.
func replaceOwned(owns func(existing []byte) bool) mergeFunc {
	return func(existing, generated []byte) ([]byte, error) {
		if !owns(existing) {
			return nil, errors.New("this command did not write the existing file; " +
				"refuse to replace it — move it aside to let integrate write its own")
		}
		return generated, nil
	}
}

// mergeFunc folds the generated document into what is already on disk.
//
// A nil mergeFunc means the file may only be created: planWrite refuses to
// replace an existing file without one. A file this command owns outright
// passes replaceOwned, which says how to recognise it.
type mergeFunc func(existing, generated []byte) ([]byte, error)

// planWrite inspects and rewrites one host config named *relative to the
// repository root*.
//
// `rel` is never joined onto an absolute path here. It is resolved inside
// `repo` by readHookFile and writeRooted, which refuse a symlink at any
// component and bound the read at maxHostConfigBytes. Both are the helpers
// `integrate uninstall` uses: repository content chooses these filenames, so
// the read must not follow a link out of the checkout (which would copy an
// outside file's contents into the merged result) and the write must not
// land outside it.
func planWrite(repo *os.Root, rel string, content []byte, mode Mode, receipt *Receipt, merge mergeFunc) error {
	existing, _, err := readHookFile(repo, rel)
	exists := err == nil
	if err != nil && !errors.Is(err, fs.ErrNotExist) {
		return fmt.Errorf("%s: %w", rel, err)
	}
	// Resolve what would actually land, once, so check and apply cannot
	// disagree about whether this file needs writing.
	want := content
	if exists && merge == nil && string(existing) != string(content) {
		// Fail closed: a file already on disk is someone's until a merge says
		// otherwise. Replacing it outright is how `.codex/config.toml` lost
		// every table in it.
		return fmt.Errorf("%s: exists and this command has no merge for it; refuse to replace a file it does not own", rel)
	}
	if merge != nil && exists {
		merged, mergeErr := merge(existing, content)
		if mergeErr != nil {
			return fmt.Errorf("%s: %w", rel, mergeErr)
		}
		want = merged
	}
	switch mode {
	case ModeCheck:
		if !exists {
			receipt.Files[rel] = "missing"
			return nil
		}
		// Compared against the merged result, not the generated document: a
		// file that already holds our entry beside three of someone else's is
		// current, and reporting it as drift would demand a rewrite that
		// changes nothing.
		if string(existing) == string(want) {
			receipt.Files[rel] = "unchanged"
		} else {
			receipt.Files[rel] = "drift"
		}
		return nil
	case ModeDryRun:
		receipt.Files[rel] = "would_write"
		return nil
	case ModeApply:
		if dir := path.Dir(rel); dir != "." {
			if err := repo.MkdirAll(dir, 0o755); err != nil {
				return fmt.Errorf("%s: %w", rel, err)
			}
		}
		if err := writeRooted(repo, rel, want, 0o644); err != nil {
			return fmt.Errorf("%s: %w", rel, err)
		}
		receipt.Files[rel] = "wrote"
		return nil
	}
	return fmt.Errorf("unknown mode %q", mode)
}

// writeRooted publishes `content` at `rel` inside `repo` atomically, through
// the same exclusive no-follow create the uninstall path uses. The rename is
// root-relative, so neither the temporary nor the final name can be redirected
// by a link planted between the check and the write.
func writeRooted(repo *os.Root, rel string, content []byte, perm os.FileMode) error {
	tmp := rel + ".devcouncil-tmp-" + rand.Text()
	if err := writeHookExclusive(repo, tmp, content, perm); err != nil {
		return err
	}
	if err := repo.Rename(tmp, rel); err != nil {
		return errors.Join(err, repo.Remove(tmp))
	}
	return nil
}
