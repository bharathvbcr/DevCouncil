package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/exec"
	"os/signal"
	"path/filepath"
	"strings"
	"syscall"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/console"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/dcverify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/devmap"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/dc/redact"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/gitrev"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/prgate"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

func runPRGate(args []string) int {
	fs := flag.NewFlagSet("pr-gate", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	fs.Usage = func() {
		console.Errorf(`Usage:
  devcouncil pr-gate --base SHA [--head SHA] [--repo DIR | --project-root DIR]
                     [--mode enforce|advisory|off]
                     [--devmap BIN] [--dcverify BIN] [--dcredact BIN]
                     [--format json|markdown|sarif|annotations]
                     [--json-out PATH] [--markdown-out PATH] [--sarif-out PATH]
                     [--annotations] [--build-index=true|false]
                     [--coverage PATH] [--timeout 5m]

Exit codes:
  0   passed (or mode=off skipped)
  1   blocked (blocking findings)
  2   error or degraded (could not verify completely; never neutral)
  64  usage error
`)
	}

	var (
		baseFlag        string
		headFlag        string
		repoFlag        string
		projectRootFlag string
		modeFlag        string
		devmapFlag      string
		dcverifyFlag    string
		dcredactFlag    string
		formatFlag      string
		jsonOut         string
		markdownOut     string
		sarifOut        string
		emitAnnotations bool
		buildIndex      bool
		coveragePath    string
		timeout         time.Duration
	)
	fs.StringVar(&baseFlag, "base", "", "base commit SHA (40-hex)")
	fs.StringVar(&headFlag, "head", "", "head commit SHA (40-hex; defaults to checked-out HEAD)")
	fs.StringVar(&repoFlag, "repo", "", "repository root directory")
	fs.StringVar(&projectRootFlag, "project-root", "", "alias for --repo")
	fs.StringVar(&modeFlag, "mode", "enforce", "gate mode: enforce, advisory, or off (never read from PR tree)")
	fs.StringVar(&devmapFlag, "devmap", "", "devmap binary path (must be outside repository)")
	fs.StringVar(&dcverifyFlag, "dcverify", "", "dcverify binary path (must be outside repository)")
	fs.StringVar(&dcredactFlag, "dcredact", "", "optional dcredact binary path for egress scrubbing (must be outside repository)")
	fs.StringVar(&formatFlag, "format", "json", "stdout format: json, markdown, sarif, or annotations")
	fs.StringVar(&jsonOut, "json-out", "", "write devcouncil.pr_gate/v1 JSON report to PATH")
	fs.StringVar(&markdownOut, "markdown-out", "", "write privacy-safe Markdown summary to PATH")
	fs.StringVar(&sarifOut, "sarif-out", "", "write SARIF 2.1.0 report to PATH")
	fs.BoolVar(&emitAnnotations, "annotations", false, "also print GitHub workflow annotations")
	fs.BoolVar(&buildIndex, "build-index", true, "run devmap build before blast")
	fs.StringVar(&coveragePath, "coverage", "", "optional coverage profile path")
	fs.DurationVar(&timeout, "timeout", 5*time.Minute, "timeout for devmap build and blast")

	if err := fs.Parse(args); err != nil {
		if errors.Is(err, flag.ErrHelp) {
			return 0
		}
		return prgate.ExitUsage
	}
	if fs.NArg() > 0 {
		console.Errorf("pr-gate: unexpected positional argument(s): %s\n", strings.Join(fs.Args(), " "))
		return prgate.ExitUsage
	}

	mode, err := verify.NormalizeRangeMode(modeFlag)
	if err != nil {
		console.Errorf("pr-gate: %v\n", err)
		return prgate.ExitUsage
	}

	format := strings.ToLower(strings.TrimSpace(formatFlag))
	switch format {
	case "json", "markdown", "sarif", "annotations":
	default:
		console.Errorf("pr-gate: invalid --format %q (expected json, markdown, sarif, or annotations)\n", formatFlag)
		return prgate.ExitUsage
	}

	if repoFlag != "" && projectRootFlag != "" && repoFlag != projectRootFlag {
		console.Errorf("pr-gate: conflicting --repo and --project-root\n")
		return prgate.ExitUsage
	}
	rawRepo := repoFlag
	if rawRepo == "" {
		rawRepo = projectRootFlag
	}
	var root string
	if rawRepo != "" {
		if strings.Contains(rawRepo, "$") {
			console.Errorf("pr-gate: repository path %q contains an unexpanded shell variable\n", rawRepo)
			return prgate.ExitUsage
		}
		abs, absErr := filepath.Abs(rawRepo)
		if absErr != nil {
			console.Errorf("pr-gate: repository %q: %v\n", rawRepo, absErr)
			return prgate.ExitUsage
		}
		root, err = checkProjectRoot(abs, "--repo")
	} else {
		root, err = projectRoot()
	}
	if err != nil {
		console.Errorf("pr-gate: %v\n", err)
		return prgate.ExitUsage
	}

	ctx, stop := signal.NotifyContext(console.Context(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	// Mode off: emit skipped report without invoking git, devmap, or dcverify,
	// and never post a commit status.
	if mode == "off" {
		rep := prgate.Evaluate(prgate.EvalInput{
			Mode:    "off",
			BaseSHA: baseFlag,
			HeadSHA: headFlag,
		})
		if err := emitPRGateOutputs(ctx, rep, nil, format, emitAnnotations, jsonOut, markdownOut, sarifOut, os.Stdout, os.Stderr); err != nil {
			console.Errorf("pr-gate: %v\n", err)
			return prgate.ExitError
		}
		return rep.ExitCode
	}

	// Validate base and head SHAs against strict 40-hex syntax (never option-like
	// or untrusted branch names).
	if !gitrev.IsSHA40(baseFlag) {
		console.Errorf("pr-gate: --base must be a 40-character lowercase hex commit SHA (got %q)\n", baseFlag)
		return prgate.ExitUsage
	}
	if headFlag != "" && !gitrev.IsSHA40(headFlag) {
		console.Errorf("pr-gate: --head must be a 40-character lowercase hex commit SHA (got %q)\n", headFlag)
		return prgate.ExitUsage
	}

	// Resolve optional dcredact egress scrubber outside repository.
	// If --dcredact (or DCREDACT_BIN) is explicitly passed and missing, inside
	// the repo, or fails during scrubbing, fail closed with ExitError.
	scrubber, dcredactErr := resolvePRGateDCRedact(root, dcredactFlag)
	if dcredactErr != nil {
		console.Errorf("pr-gate: %v\n", dcredactErr)
		return prgate.ExitError
	}

	git, gitSessErr := proc.NewGitSession(root)
	if gitSessErr != nil {
		rep := prgate.Evaluate(prgate.EvalInput{
			Mode:    mode,
			BaseSHA: baseFlag,
			HeadSHA: headFlag,
			Range: verify.RunRange(ctx, verify.RangeInput{
				Root:     root,
				BaseSHA:  baseFlag,
				HeadSHA:  headFlag,
				GateMode: mode,
				GitErr:   gitSessErr,
			}),
		})
		if err := emitPRGateOutputs(ctx, rep, scrubber, format, emitAnnotations, jsonOut, markdownOut, sarifOut, os.Stdout, os.Stderr); err != nil {
			console.Errorf("pr-gate: %v\n", err)
			return prgate.ExitError
		}
		return rep.ExitCode
	}

	actualHead, headErr := gitrev.ResolveRevSession(ctx, git, "HEAD")
	if headErr != nil {
		rep := prgate.Evaluate(prgate.EvalInput{
			Mode:    mode,
			BaseSHA: baseFlag,
			HeadSHA: headFlag,
			Range: verify.RunRange(ctx, verify.RangeInput{
				Root:     root,
				BaseSHA:  baseFlag,
				HeadSHA:  headFlag,
				GateMode: mode,
				GitErr:   fmt.Errorf("resolve HEAD: %w", headErr),
			}),
		})
		if err := emitPRGateOutputs(ctx, rep, scrubber, format, emitAnnotations, jsonOut, markdownOut, sarifOut, os.Stdout, os.Stderr); err != nil {
			console.Errorf("pr-gate: %v\n", err)
			return prgate.ExitError
		}
		return rep.ExitCode
	}

	headSHA := headFlag
	if headSHA == "" {
		headSHA = actualHead
	}
	if actualHead != headSHA {
		rep := prgate.Evaluate(prgate.EvalInput{
			Mode:             mode,
			BaseSHA:          baseFlag,
			HeadSHA:          headSHA,
			CheckoutMismatch: true,
		})
		if err := emitPRGateOutputs(ctx, rep, scrubber, format, emitAnnotations, jsonOut, markdownOut, sarifOut, os.Stdout, os.Stderr); err != nil {
			console.Errorf("pr-gate: %v\n", err)
			return prgate.ExitError
		}
		return rep.ExitCode
	}

	mergeBaseSHA, mbErr := gitrev.MergeBaseSession(ctx, git, baseFlag, headSHA)

	var state verify.RangeState
	var rangeGitErr error
	if mbErr != nil {
		rangeGitErr = fmt.Errorf("merge-base(%s, %s): %w", baseFlag, headSHA, mbErr)
	} else {
		state, rangeGitErr = verify.CollectRangeState(ctx, root, mergeBaseSHA, headSHA)
	}

	// Resolve dcverify strictly outside the repository under analysis.
	rigorClient, dcverifyErr := resolvePRGateDCVerify(root, dcverifyFlag)
	if dcverifyErr != nil {
		// Pass nil client so verify.RunRange records dcverify_missing / error
		// (fail closed, no AllowNoRigor).
		rigorClient = nil
	}

	rangeRes := verify.RunRange(ctx, verify.RangeInput{
		Root:         root,
		BaseSHA:      mergeBaseSHA,
		HeadSHA:      headSHA,
		GateMode:     mode,
		State:        state,
		GitErr:       rangeGitErr,
		Rigor:        rigorClient,
		CoveragePath: coveragePath,
	})
	if dcverifyErr != nil && rangeRes.RigorError {
		rangeRes.RigorErrorDetail = dcverifyErr.Error()
	}

	// Run DevMap build (if requested) + blast on mergeBase..headSHA.
	var blast *devmap.Blast
	var blastErr error
	if rangeGitErr != nil {
		blastErr = rangeGitErr
	} else {
		dmBin, dmErr := resolvePRGateDevmap(root, devmapFlag)
		if dmErr != nil {
			blastErr = dmErr
		} else {
			dm := devmap.New(dmBin, root)
			dm.Timeout = timeout
			if buildIndex {
				if _, buildErr := dm.Build(ctx, timeout); buildErr != nil {
					blastErr = fmt.Errorf("devmap build: %w", buildErr)
				}
			}
			if blastErr == nil {
				blast, blastErr = dm.Blast(ctx, mergeBaseSHA)
			}
		}
	}

	rep := prgate.Evaluate(prgate.EvalInput{
		Mode:         mode,
		BaseSHA:      baseFlag,
		HeadSHA:      headSHA,
		MergeBaseSHA: mergeBaseSHA,
		Range:        rangeRes,
		Blast:        blast,
		BlastErr:     blastErr,
	})

	if err := emitPRGateOutputs(ctx, rep, scrubber, format, emitAnnotations, jsonOut, markdownOut, sarifOut, os.Stdout, os.Stderr); err != nil {
		console.Errorf("pr-gate: %v\n", err)
		return prgate.ExitError
	}
	return rep.ExitCode
}

func resolvePRGateDCRedact(root, flagPath string) (*redact.Scrubber, error) {
	if flagPath != "" {
		client, err := redact.ResolveOutside(root, flagPath)
		if err != nil {
			return nil, err
		}
		return &redact.Scrubber{Client: client, RequireBinary: true}, nil
	}
	if env := proc.EnvOverride(redact.BinaryEnv); env.Path != "" {
		client, err := redact.ResolveOutside(root, "")
		if err != nil {
			return nil, err
		}
		return &redact.Scrubber{Client: client, RequireBinary: true}, nil
	}
	client, err := redact.ResolveOutside(root, "")
	if err == nil {
		return &redact.Scrubber{Client: client, RequireBinary: true}, nil
	}
	if errors.Is(err, exec.ErrNotFound) {
		return &redact.Scrubber{}, nil
	}
	return nil, err
}

func resolvePRGateDevmap(root, flagPath string) (string, error) {
	override := proc.EnvOverride("DEVMAP_BIN")
	if flagPath != "" {
		override = proc.Override{Path: flagPath, Source: "--devmap"}
	}
	bin, err := proc.ResolveComponent("devmap", root, override)
	if err != nil {
		return "", err
	}
	if err := ensureOutsideRepo(root, bin, "devmap"); err != nil {
		return "", err
	}
	return bin, nil
}

func resolvePRGateDCVerify(root, flagPath string) (*dcverify.Client, error) {
	if flagPath != "" {
		bin, err := proc.ResolveComponent("dcverify", root, proc.Override{Path: flagPath, Source: "--dcverify"})
		if err != nil {
			return nil, err
		}
		if err := ensureOutsideRepo(root, bin, "dcverify"); err != nil {
			return nil, err
		}
		return dcverify.New(bin, root), nil
	}
	client := verify.RigorClient(root)
	if client == nil {
		return nil, errors.New("dcverify binary not found outside repository")
	}
	if err := ensureOutsideRepo(root, client.Binary, "dcverify"); err != nil {
		return nil, err
	}
	return client, nil
}

// ensureOutsideRepo refuses any executable located inside the repository under
// analysis, preventing an untrusted PR from supplying its own verifier binary.
func ensureOutsideRepo(root, bin, name string) error {
	resolvedRoot, err := filepath.EvalSymlinks(root)
	if err != nil {
		resolvedRoot = filepath.Clean(root)
	}
	resolvedBin, err := filepath.EvalSymlinks(bin)
	if err != nil {
		resolvedBin = filepath.Clean(bin)
	}
	rel, err := filepath.Rel(resolvedRoot, resolvedBin)
	if err != nil {
		return nil
	}
	if rel == "." || (rel != ".." && !strings.HasPrefix(rel, ".."+string(os.PathSeparator))) {
		return fmt.Errorf("refusing %s binary %q inside repository under analysis", name, bin)
	}
	return nil
}

func emitPRGateOutputs(ctx context.Context, rep prgate.Report, scrubber *redact.Scrubber, format string, emitAnnotations bool, jsonOut, markdownOut, sarifOut string, stdout, stderr io.Writer) error {
	rendered, err := prgate.RenderScrubbed(ctx, rep, scrubber)
	if err != nil {
		return fmt.Errorf("egress render/scrub: %w", err)
	}
	jsonBytes := rendered.JSON
	md := rendered.Markdown
	sarifBytes := rendered.SARIF
	anns := rendered.Annotations

	if jsonOut != "" {
		if err := writeFileAtomic(jsonOut, jsonBytes); err != nil {
			return fmt.Errorf("write --json-out: %w", err)
		}
	}
	if markdownOut != "" {
		if err := writeFileAtomic(markdownOut, []byte(md)); err != nil {
			return fmt.Errorf("write --markdown-out: %w", err)
		}
	}
	if sarifOut != "" {
		if err := writeFileAtomic(sarifOut, sarifBytes); err != nil {
			return fmt.Errorf("write --sarif-out: %w", err)
		}
	}

	switch format {
	case "json":
		if _, err := stdout.Write(jsonBytes); err != nil {
			return err
		}
	case "markdown":
		if _, err := io.WriteString(stdout, md); err != nil {
			return err
		}
	case "sarif":
		if _, err := stdout.Write(sarifBytes); err != nil {
			return err
		}
	case "annotations":
		for _, a := range anns {
			if _, err := fmt.Fprintln(stdout, a); err != nil {
				return err
			}
		}
	}

	if emitAnnotations && format != "annotations" {
		for _, a := range anns {
			if _, err := fmt.Fprintln(stderr, a); err != nil {
				return err
			}
		}
	}
	return nil
}

func writeFileAtomic(path string, data []byte) error {
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	tmp, err := os.CreateTemp(dir, filepath.Base(path)+".tmp*")
	if err != nil {
		return err
	}
	_, werr := tmp.Write(data)
	cerr := tmp.Close()
	if err := errors.Join(werr, cerr); err != nil {
		_ = os.Remove(tmp.Name())
		return err
	}
	if err := os.Rename(tmp.Name(), path); err != nil {
		_ = os.Remove(tmp.Name())
		return err
	}
	return nil
}
