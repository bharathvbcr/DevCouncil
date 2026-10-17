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
	if err := writeAtomic(*out, merged); err != nil {
		fmt.Fprintf(stderr, "labelmerge: %v\n", err)
		return 1
	}
	return 0
}

// writeAtomic puts data at path through a temporary file in the same
// directory, synced and renamed over it: a reader sees the old file or the
// whole new one, never a truncated labels.json.
func writeAtomic(path string, data []byte) (err error) {
	tmp, err := os.CreateTemp(filepath.Dir(path), ".labelmerge-*.tmp")
	if err != nil {
		return err
	}
	defer func() {
		if err != nil {
			_ = tmp.Close()
			_ = os.Remove(tmp.Name())
		}
	}()
	if _, err = tmp.Write(data); err != nil {
		return err
	}
	if err = tmp.Chmod(0o644); err != nil {
		return err
	}
	if err = tmp.Sync(); err != nil {
		return err
	}
	if err = tmp.Close(); err != nil {
		return err
	}
	return os.Rename(tmp.Name(), path)
}

// refuseInput errors when out could land in the store or on one of its
// files: a symlink (whose target may be a name the store will read), a file in
// the store directory (the next merge would read it back as an input), or a
// hard link to an input, compared by file identity.
func refuseInput(dir, out string) error {
	if info, err := os.Lstat(out); err == nil && info.Mode()&os.ModeSymlink != 0 {
		return fmt.Errorf("refusing -o %s: it is a symbolic link", out)
	}
	outInfo, err := os.Stat(out)
	switch {
	case err == nil:
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
	case !os.IsNotExist(err):
		return err
	}
	if dirInfo, err := os.Stat(dir); err == nil {
		if outDir, err := os.Stat(filepath.Dir(out)); err == nil && os.SameFile(dirInfo, outDir) {
			return fmt.Errorf("refusing -o %s: it is inside the label store %s", out, dir)
		}
	}
	return nil
}
