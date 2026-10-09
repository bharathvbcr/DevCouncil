// Command dcrelease is the release identity gate (package
// devcouncil/release). The release workflows run it before any build:
//
//	go -C backend/go_orchestrator run ./cmd/dcrelease [--tag vX.Y.Z] [--artifacts DIR] [--root DIR]
//
// A relative --artifacts is resolved against the checkout root, because
// `go -C` has already moved the working directory into the module.
//
// --tag-at-head adds the one check CI structurally cannot make: that the
// tag, if it already exists, names HEAD (see release.TagNamesHead). ci:local
// passes it before a push.
//
// Exit codes: 0 identity holds · 1 mismatch · 2 the check could not run.
// Only a built binary carries all three: `go run` reports any non-zero exit
// as 1 (printing "exit status 2"). The workflows only need pass/fail and use
// `go run`; ci:local builds the binary so the distinction survives.
package main

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/release"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

const usage = "Usage: go -C backend/go_orchestrator run ./cmd/dcrelease [--tag vX.Y.Z [--tag-at-head]] [--artifacts DIR] [--root DIR]"

type args struct {
	tag, artifacts, root string
	tagAtHead, help      bool
}

func parseArgs(argv []string) (args, error) {
	var out args
	value := func(i *int, flag string) (string, error) {
		*i++
		if *i >= len(argv) || argv[*i] == "" {
			return "", fmt.Errorf("%s needs a value", flag)
		}
		return argv[*i], nil
	}
	for i := 0; i < len(argv); i++ {
		var err error
		switch arg := argv[i]; arg {
		case "-h", "--help":
			out.help = true
		case "--tag":
			out.tag, err = value(&i, arg)
		case "--artifacts":
			out.artifacts, err = value(&i, arg)
		case "--root":
			out.root, err = value(&i, arg)
		case "--tag-at-head":
			out.tagAtHead = true
		default:
			err = fmt.Errorf("unknown argument: %s", arg)
		}
		if err != nil {
			return args{}, err
		}
	}
	if out.tagAtHead && out.tag == "" {
		return args{}, errors.New("--tag-at-head needs --tag")
	}
	// `go -C` has already moved the working directory into the module, so a
	// relative root names a directory the caller did not mean.
	if out.root != "" && !filepath.IsAbs(out.root) {
		return args{}, fmt.Errorf("--root must be an absolute path, got %q (under go -C the working directory is the Go module)", out.root)
	}
	return out, nil
}

// revParse resolves rev to a commit sha, or "" when no such rev exists. A git
// that cannot run is an error, never an empty answer.
func revParse(root, rev string) (string, error) {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "git", "-C", root, "rev-parse", "-q", "--verify", rev+"^{commit}")
	proc.ConfigureGroup(cmd)
	cmd.WaitDelay = 2 * time.Second
	var out bytes.Buffer
	cmd.Stdout = &out
	err, timedOut := proc.RunBounded(ctx, cmd.Run)
	if timedOut {
		return "", fmt.Errorf("git rev-parse %s timed out after 30s", rev)
	}
	sha := strings.TrimSpace(out.String())
	var exit *exec.ExitError
	if err != nil && !(errors.As(err, &exit) && exit.ExitCode() == 1 && sha == "") {
		return "", fmt.Errorf("git rev-parse %s: %w", rev, err)
	}
	return sha, nil
}

func run(argv []string, stdout, stderr io.Writer) int {
	a, err := parseArgs(argv)
	if err != nil {
		fmt.Fprintf(stderr, "dcrelease: %v\n%s\n", err, usage)
		return 2
	}
	if a.help {
		fmt.Fprintln(stdout, usage)
		return 0
	}
	root := a.root
	if root == "" {
		wd, err := os.Getwd()
		if err == nil {
			root, err = release.FindRoot(wd)
		}
		if err != nil {
			fmt.Fprintf(stderr, "dcrelease: %v\n", err)
			return 2
		}
	}
	if a.tagAtHead {
		head, err := revParse(root, "HEAD")
		if err == nil && head == "" {
			err = errors.New("could not resolve HEAD")
		}
		var tagSha string
		if err == nil {
			tagSha, err = revParse(root, "refs/tags/"+a.tag)
		}
		if err != nil {
			fmt.Fprintf(stderr, "dcrelease: %v\n", err)
			return 2
		}
		if reason := release.TagNamesHead(a.tag, tagSha, head); reason != "" {
			fmt.Fprintf(stderr, "FAIL: %s\n", reason)
			return 1
		}
		fmt.Fprintf(stdout, "OK: %s would publish the notes at HEAD\n", a.tag)
	}
	artifacts := a.artifacts
	if artifacts != "" && !filepath.IsAbs(artifacts) {
		artifacts = filepath.Join(root, artifacts)
	}
	result := release.Inspect(release.DefaultSources(root), release.Options{Tag: a.tag, ArtifactsDir: artifacts})
	if len(result.Errors) > 0 {
		for _, e := range result.Errors {
			fmt.Fprintf(stderr, "FAIL: %s\n", e)
		}
		return 1
	}
	fmt.Fprintf(stdout, "OK: product version %s is consistent\n", result.Version)
	return 0
}

func main() {
	os.Exit(run(os.Args[1:], os.Stdout, os.Stderr))
}
