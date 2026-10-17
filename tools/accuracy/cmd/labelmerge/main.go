// Command labelmerge merges a label store's per-round files into the one file
// the corpus runner reads.
//
// The store holds labels-<lang>[-rN].json per language and round; the runner
// (rust/devmap-analyze/tests/real_corpus_accuracy.rs) reads a single
// <checkout>/.devmap-accuracy/labels.json, or DEVMAP_CORPUS_<NAME>_LABELS.
//
//	labelmerge -dir <store> [-private] [-o <labels.json>]
//
// The output order is fixed. A site two files label differently, a malformed
// file, a dead label with a bad `symbol_hash`, an empty store and an -o that
// would overwrite an input are all refused with an error naming the file.
package main

import (
	"flag"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sort"
)

func main() {
	os.Exit(run(os.Args[1:], os.Stdout, os.Stderr))
}

// run is main without the process: exit 0 on success, 1 when the store could
// not be merged, 2 on a usage error.
func run(args []string, stdout, stderr io.Writer) int {
	fs := flag.NewFlagSet("labelmerge", flag.ContinueOnError)
	fs.SetOutput(stderr)
	dir := fs.String("dir", "", "label store holding labels-<lang>[-rN].json (required)")
	out := fs.String("o", "", "write the merged labels here instead of stdout")
	private := fs.Bool("private", false, "require symbol_hash and category on every dead label (any corpus but DevCouncil's)")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	if *dir == "" || fs.NArg() != 0 {
		fmt.Fprintln(stderr, "usage: labelmerge -dir <store> [-private] [-o <labels.json>]")
		return 2
	}
	if *out != "" {
		if err := refuseInput(*dir, *out); err != nil {
			fmt.Fprintf(stderr, "labelmerge: %v\n", err)
			return 1
		}
	}
	merged, counts, err := Merge(*dir, Options{Private: *private})
	if err != nil {
		fmt.Fprintf(stderr, "labelmerge: %v\n", err)
		return 1
	}
	languages := make([]string, 0, len(counts))
	for l := range counts {
		languages = append(languages, l)
	}
	sort.Strings(languages)
	for _, l := range languages {
		fmt.Fprintf(stderr, "labelmerge: %s: %d entries\n", l, counts[l])
	}
	if *out == "" {
		if _, err := stdout.Write(merged); err != nil {
			fmt.Fprintf(stderr, "labelmerge: %v\n", err)
			return 1
		}
		return 0
	}
	if err := os.WriteFile(*out, merged, 0o644); err != nil {
		fmt.Fprintf(stderr, "labelmerge: %v\n", err)
		return 1
	}
	return 0
}

// refuseInput errors when out is one of the store's files, compared by file
// identity so a symlink or a relative spelling cannot get around it.
func refuseInput(dir, out string) error {
	outInfo, err := os.Stat(out)
	if err != nil {
		if os.IsNotExist(err) {
			return nil
		}
		return err
	}
	files, err := inputFiles(dir)
	if err != nil {
		return err
	}
	for _, f := range files {
		in, err := os.Stat(filepath.Join(dir, f.name))
		if err != nil {
			return err
		}
		if os.SameFile(in, outInfo) {
			return fmt.Errorf("refusing to write over the input %s", filepath.Join(dir, f.name))
		}
	}
	return nil
}
