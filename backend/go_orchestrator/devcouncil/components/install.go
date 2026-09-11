package components

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

const HelpText = `devcouncil install — install DevCouncil components

Usage:
  devcouncil install [names…] [options]
  devcouncil install --list [--json]
  devcouncil uninstall [names…] [--all] [--yes] [--prefix DIR] [--dry-run]
  devcouncil disable NAME [--prefix DIR]
  devcouncil enable NAME [--prefix DIR]

Names are component IDs or presets. Empty install selects preset "all".

Presets:
  all         host + analysis (default)
  analysis    devmap dcstore dcverify dcgrep
  codeintel   devmap
  devmap      devmap only (standalone code-intelligence CLI)
  host        Go host (devcouncil + dev symlink)

Components:
  host        Go orchestrator (devcouncil / dev)
  devmap      code-intelligence graph
  dcstore     task and lease store
  dcverify    deterministic verifier
  dcgrep      ignore-aware search

Options:
  --list              print the catalog and exit
  --json              machine-readable list/status
  --prefix DIR        install root (default: $PREFIX or ~/.local)
  --dry-run           print commands; do not build or copy
  --yes               skip uninstall confirmation
  --all               uninstall every catalog component (required when no names)
  -h, --help          this help

Bootstrap (no host binary yet):
  bash scripts/install.sh --help
  bash scripts/install.sh --only=devmap
  bash scripts/install-components.sh --help

There is no uv / Python install path. Components are native Go and Rust
binaries. cargo install --git and the scripts above are the supported
first-time routes; this command is for once the host is on PATH.

Examples:
  devcouncil install devmap
  devcouncil install analysis
  devcouncil install
  devcouncil uninstall dcgrep --yes
  devcouncil uninstall --all --yes
  devcouncil disable hooks          # same as: integrate uninstall --target hooks
  devcouncil disable devmap
`

// Runner is the process seam tests inject.
type Runner interface {
	Run(name string, args []string, dir string, env []string) error
}

// ExecRunner shells out. Tests replace it.
type ExecRunner struct{}

func (ExecRunner) Run(name string, args []string, dir string, env []string) error {
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Minute)
	defer cancel()
	cmd := exec.CommandContext(ctx, name, args...)
	proc.ConfigureGroup(cmd)
	cmd.Dir = dir
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	cmd.WaitDelay = 2 * time.Second
	if len(env) > 0 {
		cmd.Env = append(os.Environ(), env...)
	}
	runErr, timedOut := proc.RunBounded(ctx, cmd.Run)
	if timedOut {
		return fmt.Errorf("%s timed out after 15m", name)
	}
	return runErr
}

// Options for Install / Uninstall / Disable.
type Options struct {
	Prefix     string
	DryRun     bool
	Yes        bool
	SourceRoot string
	Runner     Runner
}

func (o Options) runner() Runner {
	if o.Runner != nil {
		return o.Runner
	}
	return ExecRunner{}
}

func (o Options) prefix() string {
	if strings.TrimSpace(o.Prefix) != "" {
		return o.Prefix
	}
	return DefaultPrefix()
}

func (o Options) source() string {
	if o.SourceRoot != "" {
		if looksLikeCheckout(o.SourceRoot) {
			return o.SourceRoot
		}
		return ""
	}
	return FindSourceRoot()
}

// ListJSON is `install --list --json`.
func ListJSON() ([]byte, error) {
	type row struct {
		Component
		PresetOf []string `json:"presets,omitempty"`
	}
	rows := make([]row, 0, len(Catalog))
	for _, c := range Catalog {
		var presets []string
		for name, ids := range Presets {
			for _, id := range ids {
				if id == c.ID {
					presets = append(presets, name)
					break
				}
			}
		}
		rows = append(rows, row{Component: c, PresetOf: presets})
	}
	return json.MarshalIndent(map[string]any{
		"components": rows,
		"presets":    Presets,
	}, "", "  ")
}

// PlannedCommands is what --dry-run prints.
func PlannedCommands(cs []Component, o Options) []string {
	src := o.source()
	prefix := o.prefix()
	var cmds []string
	if IncludesHost(cs) {
		if src != "" {
			out := filepath.Join(prefix, "bin", hostBinaryName())
			cmds = append(cmds, fmt.Sprintf("go -C %s build -o %s ./cmd/devcouncil",
				shellQuote(filepath.Join(src, "backend", "go_orchestrator")), shellQuote(out)))
			cmds = append(cmds, fmt.Sprintf("ln -sf %s %s", hostBinaryName(), shellQuote(filepath.Join(prefix, "bin", "dev"))))
		} else {
			cmds = append(cmds, "GOBIN="+shellQuote(filepath.Join(prefix, "bin"))+" go install "+HostGoInstall)
		}
	}
	rust := RustIDs(cs)
	if len(rust) == 0 {
		return cmds
	}
	if src != "" {
		script := filepath.Join(src, "scripts", "install-components.sh")
		cmds = append(cmds, fmt.Sprintf("PREFIX=%s bash %s %s", shellQuote(prefix), shellQuote(script), strings.Join(rust, " ")))
		return cmds
	}
	for _, id := range rust {
		name, args, err := cargoInstallArgs(id, prefix)
		if err != nil {
			cmds = append(cmds, err.Error())
			continue
		}
		cmds = append(cmds, name+" "+joinQuoted(args))
	}
	return cmds
}

func cargoInstallArgs(id, prefix string) (string, []string, error) {
	args := []string{"install", "--git", PublicGit, "--locked", "--force", "--root", prefix}
	switch id {
	case "devmap":
		args = append(args, "devmap-cli")
	case "dcstore":
		args = append(args, "--bin", "dcstore", "dc-store")
	case "dcverify":
		args = append(args, "--bin", "dcverify", "dc-verify")
	case "dcgrep":
		args = append(args, "--bin", "dcgrep", "dc-grep")
	default:
		return "", nil, fmt.Errorf("no remote install command for %s", id)
	}
	return "cargo", args, nil
}

func shellQuote(s string) string {
	if s == "" {
		return "''"
	}
	if !strings.ContainsAny(s, " \t\n'\"\\$`") {
		return s
	}
	return "'" + strings.ReplaceAll(s, "'", `'\''`) + "'"
}

func joinQuoted(args []string) string {
	parts := make([]string, len(args))
	for i, a := range args {
		parts[i] = shellQuote(a)
	}
	return strings.Join(parts, " ")
}

func hostBinaryName() string {
	if runtime.GOOS == "windows" {
		return "devcouncil.exe"
	}
	return "devcouncil"
}

func rustBinaryName(id string) string {
	if runtime.GOOS == "windows" {
		return id + ".exe"
	}
	return id
}

// Install builds and copies the selected components.
func Install(cs []Component, o Options) error {
	if o.DryRun {
		for _, c := range PlannedCommands(cs, o) {
			fmt.Println(c)
		}
		return nil
	}
	prefix := o.prefix()
	bindir := filepath.Join(prefix, "bin")
	if err := os.MkdirAll(bindir, 0o755); err != nil {
		return err
	}
	src := o.source()
	run := o.runner()
	st := LoadState(prefix)

	var firstErr error
	if IncludesHost(cs) {
		if err := installHost(src, prefix, run); err != nil {
			return err
		}
		st.Record("host", filepath.Join(bindir, hostBinaryName()))
	}

	rust := RustIDs(cs)
	if len(rust) > 0 {
		if err := installRust(src, prefix, rust, run); err != nil {
			firstErr = err
		} else {
			for _, id := range rust {
				st.Record(id, filepath.Join(bindir, rustBinaryName(id)))
				st.SetDisabled(id, false)
			}
		}
	}
	if err := SaveState(st); err != nil {
		if firstErr != nil {
			return fmt.Errorf("%v; also save receipt: %w", firstErr, err)
		}
		return err
	}
	return firstErr
}

func installHost(src, prefix string, run Runner) error {
	bindir := filepath.Join(prefix, "bin")
	out := filepath.Join(bindir, hostBinaryName())
	if src != "" {
		mod := filepath.Join(src, "backend", "go_orchestrator")
		if err := run.Run("go", []string{"build", "-o", out, "./cmd/devcouncil"}, mod, nil); err != nil {
			return fmt.Errorf("build host: %w", err)
		}
	} else {
		if err := run.Run("go", []string{"install", HostGoInstall}, "",
			[]string{"GOBIN=" + bindir}); err != nil {
			return fmt.Errorf("go install host: %w", err)
		}
	}
	return installDevLink(bindir)
}

func installDevLink(bindir string) error {
	if runtime.GOOS == "windows" {
		src := filepath.Join(bindir, "devcouncil.exe")
		dest := filepath.Join(bindir, "dev.exe")
		if err := copyFile(src, dest); err != nil {
			return fmt.Errorf("install dev.exe: %w", err)
		}
		return nil
	}
	dest := filepath.Join(bindir, "dev")
	kind, err := classifyDevLink(dest)
	if err != nil {
		return err
	}
	if kind == devLinkOurs {
		if err := os.Remove(dest); err != nil {
			return err
		}
	}
	return os.Symlink("devcouncil", dest)
}

type devLinkKind int

const (
	devLinkAbsent devLinkKind = iota
	devLinkOurs
	devLinkForeign
)

func classifyDevLink(dest string) (devLinkKind, error) {
	info, err := os.Lstat(dest)
	if err != nil {
		if os.IsNotExist(err) {
			return devLinkAbsent, nil
		}
		return 0, err
	}
	if info.IsDir() && info.Mode()&os.ModeSymlink == 0 {
		return devLinkForeign, fmt.Errorf("refusing to replace %s (it is a directory)", dest)
	}
	if info.Mode()&os.ModeSymlink != 0 {
		target, err := os.Readlink(dest)
		if err != nil {
			return 0, err
		}
		base := filepath.Base(target)
		if base != "devcouncil" && base != "devcouncil.exe" {
			return devLinkForeign, fmt.Errorf("refusing to replace %s (symlink to %s, not devcouncil)", dest, target)
		}
		return devLinkOurs, nil
	}
	return devLinkForeign, fmt.Errorf("refusing to replace %s (not a symlink to our binary)", dest)
}

func installRust(src, prefix string, ids []string, run Runner) error {
	if src != "" {
		script := filepath.Join(src, "scripts", "install-components.sh")
		if _, err := os.Stat(script); err == nil {
			args := append([]string{script}, ids...)
			env := []string{"PREFIX=" + prefix}
			if err := run.Run("bash", args, src, env); err != nil {
				return fmt.Errorf("install-components.sh: %w", err)
			}
			return nil
		}
	}
	for _, id := range ids {
		name, args, err := cargoInstallArgs(id, prefix)
		if err != nil {
			return err
		}
		if err := run.Run(name, args, "", nil); err != nil {
			return fmt.Errorf("%s: %w", id, err)
		}
	}
	return nil
}

func copyFile(src, dest string) error {
	data, err := os.ReadFile(src)
	if err != nil {
		return err
	}
	return os.WriteFile(dest, data, 0o755)
}

// Uninstall removes binaries we have a receipt for, or that live in $PREFIX/bin
// under a catalog name. It never cargo-uninstalls a foreign toolchain copy.
func Uninstall(cs []Component, o Options) error {
	prefix := o.prefix()
	st := LoadState(prefix)
	bindir := filepath.Join(prefix, "bin")
	for _, c := range cs {
		rec, have := st.Installed[c.ID]
		candidates := []string{}
		if have {
			candidates = append(candidates, rec.Binary)
		}
		candidates = append(candidates, filepath.Join(bindir, rustBinaryName(c.Binary)))
		if c.ID == "host" {
			candidates = append(candidates, filepath.Join(bindir, hostBinaryName()))
		}
		removed := false
		for _, p := range unique(candidates) {
			did, err := removeOwned(prefix, p, o.DryRun)
			if err != nil {
				return err
			}
			if did {
				removed = true
			}
		}
		if c.ID == "host" {
			dev := filepath.Join(bindir, "dev")
			if runtime.GOOS == "windows" {
				dev = filepath.Join(bindir, "dev.exe")
				if _, err := removeOwned(prefix, dev, o.DryRun); err != nil {
					return err
				}
			} else {
				kind, err := classifyDevLink(dev)
				if err != nil {
					fmt.Fprintf(os.Stderr, "note: left %s in place: %v\n", dev, err)
				} else if kind == devLinkOurs {
					if _, err := removeOwned(prefix, dev, o.DryRun); err != nil {
						return err
					}
				}
			}
		}
		if removed || have {
			st.Forget(c.ID)
		}
	}
	if o.DryRun {
		return nil
	}
	return SaveState(st)
}

func pathInside(prefix, candidate string) bool {
	if candidate == "" || prefix == "" {
		return false
	}
	root := filepath.Clean(prefix)
	target := filepath.Clean(candidate)
	if filepath.VolumeName(root) != filepath.VolumeName(target) {
		return false
	}
	rel, err := filepath.Rel(root, target)
	if err != nil {
		return false
	}
	if rel == "." || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
		return false
	}
	return true
}

func removeOwned(prefix, path string, dry bool) (bool, error) {
	if !pathInside(prefix, path) {
		return false, nil
	}
	info, err := os.Lstat(path)
	if err != nil {
		if os.IsNotExist(err) {
			return false, nil
		}
		return false, err
	}
	if info.IsDir() {
		fmt.Fprintf(os.Stderr, "note: left directory %s in place\n", path)
		return false, nil
	}
	if dry {
		fmt.Println("rm", path)
		return true, nil
	}
	if err := os.Remove(path); err != nil && !os.IsNotExist(err) {
		return false, fmt.Errorf("remove %s: %w", path, err)
	}
	return true, nil
}

func unique(in []string) []string {
	seen := map[string]bool{}
	var out []string
	for _, s := range in {
		if s == "" || seen[s] {
			continue
		}
		seen[s] = true
		out = append(out, s)
	}
	return out
}

// Disable keeps binaries on disk and records them as skipped.
func catalogID(id string) (string, error) {
	id = strings.TrimSpace(strings.ToLower(id))
	if id == "" {
		return "", fmt.Errorf("unknown component %q", id)
	}
	if _, ok := Lookup(id); !ok {
		return "", fmt.Errorf("unknown component %q", id)
	}
	return id, nil
}

func Disable(id string, o Options) error {
	id, err := catalogID(id)
	if err != nil {
		return err
	}
	if o.DryRun {
		fmt.Println("disable", id)
		return nil
	}
	st := LoadState(o.prefix())
	st.SetDisabled(id, true)
	return SaveState(st)
}

// Enable clears a disable mark.
func Enable(id string, o Options) error {
	id, err := catalogID(id)
	if err != nil {
		return err
	}
	if o.DryRun {
		fmt.Println("enable", id)
		return nil
	}
	st := LoadState(o.prefix())
	st.SetDisabled(id, false)
	return SaveState(st)
}
