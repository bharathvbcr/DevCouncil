package proc

import (
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
)

// unboundedOutputCall matches the os/exec calls that collect a child's whole
// output into memory, or start one with no context at all:
//
//	cmd.CombinedOutput()  cmd.Output()  exec.Command(...)
//
// None of them is bounded in bytes (the buffer grows with whatever the child
// writes) and exec.Command is not bounded in time. This is how verify's
// runner stayed unbounded while the invariants above counted only cmd.Run()
// and exec.CommandContext(. The bounded forms are proc.CappedBuffer on
// Stdout/Stderr with the call wrapped in proc.RunBounded.
var unboundedOutputCall = regexp.MustCompile(`\.CombinedOutput\(\)|\b\w*[cC]md\w*\.Output\(\)|\bexec\.Command\(`)

// unboundedOutputSites reports the lines of one non-test source file that
// make such a call. Comment lines are skipped.
func unboundedOutputSites(src string) []string {
	var sites []string
	for n, line := range strings.Split(src, "\n") {
		trimmed := strings.TrimSpace(line)
		if strings.HasPrefix(trimmed, "//") {
			continue
		}
		if unboundedOutputCall.MatchString(trimmed) {
			sites = append(sites, trimmed+" (line "+itoa(n+1)+")")
		}
	}
	return sites
}

// unboundedOutputAllowlist names the call sites that are unbounded on purpose,
// each with the reason. A file is listed whole; there is none today.
var unboundedOutputAllowlist = map[string]string{}

func TestNoCallSiteCollectsUnboundedChildOutput(t *testing.T) {
	root, err := filepath.Abs("../..")
	if err != nil {
		t.Fatal(err)
	}
	var offenders []string
	err = filepath.WalkDir(root, func(path string, d os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() || !strings.HasSuffix(path, ".go") || strings.HasSuffix(path, "_test.go") {
			return nil
		}
		rel, _ := filepath.Rel(root, path)
		if _, ok := unboundedOutputAllowlist[filepath.ToSlash(rel)]; ok {
			return nil
		}
		body, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		for _, site := range unboundedOutputSites(string(body)) {
			offenders = append(offenders, rel+": "+site)
		}
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	if len(offenders) > 0 {
		t.Fatalf("these calls collect a child's output without a byte bound or start it "+
			"without a deadline; use proc.CappedBuffer with proc.RunBounded:\n  %s",
			strings.Join(offenders, "\n  "))
	}
}

// The red demonstration: the scan names each planted call, and ignores the
// bounded forms and comments, so the invariant above is not vacuous.
func TestTheUnboundedOutputScanCatchesAPlantedCall(t *testing.T) {
	planted := map[string]string{
		"combined": "out, err := cmd.CombinedOutput()",
		"output":   "out, err := cmd.Output()",
		"named":    "out, err := buildCmd.Output()",
		"bare":     `cmd := exec.Command("cargo", "build")`,
	}
	for name, line := range planted {
		if got := unboundedOutputSites("package x\n" + line + "\n"); len(got) != 1 {
			t.Errorf("%s: %q was not caught (sites=%v)", name, line, got)
		}
	}
	for name, line := range map[string]string{
		"context":   `cmd := exec.CommandContext(ctx, "git")`,
		"run":       "err := cmd.Run()",
		"commented": "// out, err := cmd.CombinedOutput()",
		"other":     "n := report.Output()",
	} {
		if got := unboundedOutputSites("package x\n" + line + "\n"); len(got) != 0 {
			t.Errorf("%s: %q was flagged: %v", name, line, got)
		}
	}
}
