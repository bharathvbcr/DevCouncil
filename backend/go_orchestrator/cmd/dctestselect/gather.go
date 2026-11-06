package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/devmap"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

// Command is one process to run, in the repository-relative directory Dir.
type Command struct {
	Display string   `json:"cmd"`
	Dir     string   `json:"dir"`
	Argv    []string `json:"argv"`
	// GoJSON marks `go test -json`, whose stdout is an event stream.
	GoJSON bool `json:"go_json,omitempty"`
	// Log names the file the command's output is kept in, for the audit.
	Log string `json:"log,omitempty"`
}

func newCommand(dir string, argv ...string) Command {
	return Command{Display: strings.Join(argv, " "), Dir: dir, Argv: argv}
}

const (
	// maxCaptured bounds what one query subprocess may hand back. `go list`
	// over this module and `cargo metadata` are a few MiB at most.
	maxCaptured = 64 << 20
	maxStderr   = 1 << 20
)

// runProcess is the one place this program starts a child. The deadline covers
// the fork as well as the process, the whole process group dies with it, and
// the caller supplies the writers (a capped buffer, or a log).
func runProcess(ctx context.Context, timeout time.Duration, dir string, argv []string, stdout, stderr io.Writer) error {
	if len(argv) == 0 {
		return errors.New("empty command")
	}
	ctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()
	cmd := exec.CommandContext(ctx, argv[0], argv[1:]...)
	proc.ConfigureGroup(cmd)
	cmd.Dir = dir
	cmd.Stdout = stdout
	cmd.Stderr = stderr
	cmd.WaitDelay = 2 * time.Second
	err, timedOut := proc.RunBounded(ctx, cmd.Run)
	if timedOut {
		return fmt.Errorf("%s did not return within %s", argv[0], timeout)
	}
	if err != nil && errors.Is(ctx.Err(), context.DeadlineExceeded) {
		return fmt.Errorf("%s was stopped at its %s deadline: %w", argv[0], timeout, err)
	}
	return err
}

// Runner answers one question with a child's bounded stdout. It exists so a
// test can make cargo, go or git fail, which a real binary cannot be made to
// do on demand.
type Runner interface {
	Output(ctx context.Context, dir string, argv ...string) ([]byte, error)
}

type execRunner struct{ timeout time.Duration }

func (r execRunner) Output(ctx context.Context, dir string, argv ...string) ([]byte, error) {
	out := &proc.CappedBuffer{Limit: maxCaptured}
	errs := &proc.CappedBuffer{Limit: maxStderr}
	if err := runProcess(ctx, r.timeout, dir, argv, out, errs); err != nil {
		return nil, fmt.Errorf("%s failed: %w%s", strings.Join(argv, " "), err, tail(errs.String()))
	}
	if out.Overflowed() {
		return nil, fmt.Errorf("%s produced more than %d bytes; a partial answer is not a short one", strings.Join(argv, " "), maxCaptured)
	}
	return out.Bytes(), nil
}

func tail(stderr string) string {
	stderr = strings.TrimSpace(stderr)
	if stderr == "" {
		return ""
	}
	const keep = 400
	if len(stderr) > keep {
		stderr = "..." + stderr[len(stderr)-keep:]
	}
	return " (" + strings.ReplaceAll(stderr, "\n", " | ") + ")"
}

// Options are the flags of `plan`.
type Options struct {
	Repo     string // absolute, symlinks resolved
	Devmap   string
	Base     string
	Event    string
	RefName  string
	Before   string
	BaseRef  string
	Trigger  string
	Excludes []string
	RustDir  string
	GoDir    string
}

var (
	zeroSHA   = regexp.MustCompile(`^0+$`)
	refToken  = regexp.MustCompile(`^[A-Za-z0-9._/@~^-]+$`)
	shaSyntax = regexp.MustCompile(`^[0-9a-fA-F]{7,64}$`)
)

// resolveRev turns a revision into a full commit id, refusing anything git
// could read as an option.
func resolveRev(ctx context.Context, run Runner, repo, rev string) (string, error) {
	if rev == "" || strings.HasPrefix(rev, "-") || !refToken.MatchString(rev) {
		return "", fmt.Errorf("%q is not a usable revision", rev)
	}
	out, err := run.Output(ctx, repo, "git", "rev-parse", "--verify", "--quiet", rev+"^{commit}")
	if err != nil {
		return "", err
	}
	sha := strings.TrimSpace(string(out))
	if !fullSHAre.MatchString(sha) {
		return "", fmt.Errorf("git rev-parse %s answered %q", rev, sha)
	}
	return sha, nil
}

var fullSHAre = regexp.MustCompile(`^[0-9a-f]{40,64}$`)

// resolveBase picks the revision the change is measured from.
//
//   - an explicit --base wins;
//   - a push to the default branch measures from the commit before it, when
//     that commit is present and not the all-zero id of a new ref;
//   - everything else (a pull request, a push to another branch, a manual run)
//     measures from the merge base with origin/<default branch>.
//
// Any failure is returned, and the caller widens the selection to everything:
// a base that could not be resolved is not an empty change.
func resolveBase(ctx context.Context, run Runner, o Options) (base, head string, err error) {
	head, err = resolveRev(ctx, run, o.Repo, "HEAD")
	if err != nil {
		return "", "", fmt.Errorf("resolve HEAD: %w", err)
	}
	if o.Base != "" {
		base, err = resolveRev(ctx, run, o.Repo, o.Base)
		return base, head, err
	}
	if o.Event == "push" && o.RefName == o.BaseRef && o.Before != "" && !zeroSHA.MatchString(o.Before) {
		if !shaSyntax.MatchString(o.Before) {
			return "", head, fmt.Errorf("the push's before id %q is not a commit id", o.Before)
		}
		base, err = resolveRev(ctx, run, o.Repo, o.Before)
		if err != nil {
			return "", head, fmt.Errorf("the commit before this push is not in the clone (a force push?): %w", err)
		}
		return base, head, nil
	}
	remote := "origin/" + o.BaseRef
	if _, err := resolveRev(ctx, run, o.Repo, remote); err != nil {
		return "", head, fmt.Errorf("resolve %s: %w", remote, err)
	}
	out, err := run.Output(ctx, o.Repo, "git", "merge-base", remote, head)
	if err != nil {
		return "", head, fmt.Errorf("merge-base %s HEAD: %w", remote, err)
	}
	base = strings.TrimSpace(string(out))
	if !fullSHAre.MatchString(base) {
		return "", head, fmt.Errorf("git merge-base answered %q", base)
	}
	return base, head, nil
}

// relativeTo returns abs as a slash path under root, resolving symlinks on
// both sides so a /var vs /private/var spelling is not an escape.
func relativeTo(root, abs string) (string, error) {
	resolved, err := filepath.EvalSymlinks(abs)
	if err != nil {
		return "", err
	}
	rel, err := filepath.Rel(root, resolved)
	if err != nil {
		return "", err
	}
	rel = filepath.ToSlash(rel)
	if rel == ".." || strings.HasPrefix(rel, "../") || filepath.IsAbs(rel) {
		return "", fmt.Errorf("%s is outside the repository %s", abs, root)
	}
	return rel, nil
}

// gitChanged lists the paths between base and head, renames split into a
// delete and an add so the old name is not lost.
func gitChanged(ctx context.Context, run Runner, repo, base, head string) ([]string, error) {
	out, err := run.Output(ctx, repo, "git", "diff", "--name-only", "-z", "--no-renames", base, head)
	if err != nil {
		return nil, err
	}
	var paths []string
	for _, p := range bytes.Split(out, []byte{0}) {
		if len(p) > 0 {
			paths = append(paths, string(p))
		}
	}
	return paths, nil
}

type metadataWire struct {
	Packages []struct {
		Name         string `json:"name"`
		ID           string `json:"id"`
		ManifestPath string `json:"manifest_path"`
		Targets      []struct {
			Name             string   `json:"name"`
			Kind             []string `json:"kind"`
			SrcPath          string   `json:"src_path"`
			RequiredFeatures []string `json:"required-features"`
			Test             *bool    `json:"test"`
		} `json:"targets"`
	} `json:"packages"`
	WorkspaceMembers []string `json:"workspace_members"`
}

// loadRust reads the cargo workspace from `cargo metadata`, which is the
// authority for targets, their sources and their required features.
func loadRust(ctx context.Context, run Runner, repo, rustDir string) (*RustWorkspace, error) {
	out, err := run.Output(ctx, filepath.Join(repo, rustDir), "cargo", "metadata", "--no-deps", "--format-version", "1", "--locked")
	if err != nil {
		return nil, err
	}
	return decodeMetadata(out, repo)
}

func decodeMetadata(raw []byte, repo string) (*RustWorkspace, error) {
	var w metadataWire
	if err := json.Unmarshal(raw, &w); err != nil {
		return nil, fmt.Errorf("cargo metadata returned unparseable output: %w", err)
	}
	if len(w.WorkspaceMembers) == 0 || len(w.Packages) == 0 {
		return nil, errors.New("cargo metadata names no workspace members")
	}
	member := map[string]bool{}
	for _, id := range w.WorkspaceMembers {
		member[id] = true
	}
	ws := &RustWorkspace{}
	for _, p := range w.Packages {
		if !member[p.ID] {
			continue
		}
		if p.Name == "" || p.ManifestPath == "" {
			return nil, fmt.Errorf("cargo metadata package %q has no manifest path", p.Name)
		}
		dir, err := relativeTo(repo, filepath.Dir(p.ManifestPath))
		if err != nil {
			return nil, fmt.Errorf("package %s: %w", p.Name, err)
		}
		pkg := RustPackage{Name: p.Name, Dir: dir}
		for _, t := range p.Targets {
			if t.Name == "" || len(t.Kind) == 0 || t.SrcPath == "" || t.Test == nil {
				return nil, fmt.Errorf("cargo metadata target %q of %s lacks name, kind, src_path or test", t.Name, p.Name)
			}
			src, err := relativeTo(repo, t.SrcPath)
			if err != nil {
				return nil, fmt.Errorf("target %s of %s: %w", t.Name, p.Name, err)
			}
			pkg.Targets = append(pkg.Targets, RustTarget{Name: t.Name, Kinds: t.Kind, Src: src, RequiredFeatures: t.RequiredFeatures, Test: *t.Test})
		}
		ws.Packages = append(ws.Packages, pkg)
	}
	if len(ws.Packages) == 0 {
		return nil, errors.New("no cargo package is a workspace member")
	}
	return ws, nil
}

type goListWire struct {
	ImportPath   string   `json:"ImportPath"`
	Dir          string   `json:"Dir"`
	GoFiles      []string `json:"GoFiles"`
	TestGoFiles  []string `json:"TestGoFiles"`
	XTestGoFiles []string `json:"XTestGoFiles"`
	Imports      []string `json:"Imports"`
	TestImports  []string `json:"TestImports"`
	XTestImports []string `json:"XTestImports"`
	Module       *struct {
		Path string `json:"Path"`
	} `json:"Module"`
	Error *struct {
		Err string `json:"Err"`
	} `json:"Error"`
	DepsErrors []struct {
		Err string `json:"Err"`
	} `json:"DepsErrors"`
}

// loadGo reads the module's packages. Any package `go list` reports an error
// for fails the whole read: a package that cannot be listed cannot be mapped,
// and quietly omitting it would hide its tests from the selection.
func loadGo(ctx context.Context, run Runner, repo, goDir string) (*GoWorkspace, error) {
	out, err := run.Output(ctx, filepath.Join(repo, goDir), "go", "list", "-json", "./...")
	if err != nil {
		return nil, err
	}
	return decodeGoList(out, repo)
}

func decodeGoList(raw []byte, repo string) (*GoWorkspace, error) {
	ws := &GoWorkspace{}
	dec := json.NewDecoder(bytes.NewReader(raw))
	for {
		var p goListWire
		if err := dec.Decode(&p); errors.Is(err, io.EOF) {
			break
		} else if err != nil {
			return nil, fmt.Errorf("go list returned unparseable output: %w", err)
		}
		if p.Error != nil || len(p.DepsErrors) > 0 {
			return nil, fmt.Errorf("go list reports an error for %s", p.ImportPath)
		}
		if p.ImportPath == "" || p.Dir == "" || p.Module == nil || p.Module.Path == "" {
			return nil, fmt.Errorf("go list package %q lacks ImportPath, Dir or Module", p.ImportPath)
		}
		if ws.Module == "" {
			ws.Module = p.Module.Path
		} else if ws.Module != p.Module.Path {
			return nil, fmt.Errorf("go list spans modules %s and %s", ws.Module, p.Module.Path)
		}
		dir, err := relativeTo(repo, p.Dir)
		if err != nil {
			return nil, fmt.Errorf("package %s: %w", p.ImportPath, err)
		}
		pkg := GoPackage{ImportPath: p.ImportPath, Dir: dir}
		for _, f := range p.GoFiles {
			pkg.GoFiles = append(pkg.GoFiles, dir+"/"+f)
		}
		for _, f := range append(append([]string{}, p.TestGoFiles...), p.XTestGoFiles...) {
			pkg.TestFiles = append(pkg.TestFiles, dir+"/"+f)
		}
		pkg.Imports = append(append(append([]string{}, p.Imports...), p.TestImports...), p.XTestImports...)
		ws.Packages = append(ws.Packages, pkg)
	}
	if len(ws.Packages) == 0 {
		return nil, errors.New("go list returned no packages")
	}
	return ws, nil
}

// gather asks every question the plan depends on. It never returns early: each
// answer carries its own error, and BuildPlan turns any of them into a full
// run, so a failure of one source is not decided here by whichever came first.
func gather(ctx context.Context, run Runner, o Options, readFile func(string) ([]byte, error)) Inputs {
	in := Inputs{Trigger: o.Trigger, Excludes: o.Excludes, RustDir: o.RustDir, GoDir: o.GoDir, ReadFile: readFile}
	in.Base, in.Head, in.BaseErr = resolveBase(ctx, run, o)
	if in.BaseErr == nil {
		in.GitChanged, in.GitErr = gitChanged(ctx, run, o.Repo, in.Base, in.Head)
		client := devmap.New(o.Devmap, o.Repo)
		client.Timeout = 5 * time.Minute
		in.Blast, in.BlastErr = client.Blast(ctx, in.Base)
	}
	in.Rust, in.RustErr = loadRust(ctx, run, o.Repo, o.RustDir)
	in.Go, in.GoErr = loadGo(ctx, run, o.Repo, o.GoDir)
	return in
}
