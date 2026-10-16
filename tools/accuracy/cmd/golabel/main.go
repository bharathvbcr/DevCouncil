// Command golabel writes the Go half of a DevMap accuracy labels file: what
// go/types says every in-repo function and method use in a corpus binds to.
//
// It is the oracle the corpus record is scored against
// (rust/devmap-analyze/tests/real_corpus_accuracy.rs reads the output as
// <checkout>/.devmap-accuracy/labels.json). The labels name private paths and
// symbols, so for a private corpus they are written beside it, never into this
// repository.
//
//	golabel -root <checkout> [-dead <dead.json>] [-o <labels.json>]
//
// -dead merges hand-labelled dead rows (a JSON array of
// {"language","id","truly_dead"}) verbatim: whether a symbol is dead is a
// judgement go/types cannot make (external interfaces, reflection, cgo), and
// the scorer requires a label for every confident-tier dead row.
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"os"
)

func main() {
	os.Exit(run(os.Args[1:], os.Stdout, os.Stderr))
}

// run is main without the process: exit 0 on success, 1 when the corpus could
// not be labelled, 2 on a usage error.
func run(args []string, stdout, stderr io.Writer) int {
	fs := flag.NewFlagSet("golabel", flag.ContinueOnError)
	fs.SetOutput(stderr)
	root := fs.String("root", "", "corpus checkout to label (required)")
	deadPath := fs.String("dead", "", "JSON array of hand-labelled dead rows to merge")
	out := fs.String("o", "", "write the labels here instead of stdout")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	if *root == "" || fs.NArg() != 0 {
		fmt.Fprintln(stderr, "usage: golabel -root <checkout> [-dead <dead.json>] [-o <labels.json>]")
		return 2
	}
	var dead []DeadLabel
	if *deadPath != "" {
		raw, err := os.ReadFile(*deadPath)
		if err != nil {
			fmt.Fprintf(stderr, "golabel: %v\n", err)
			return 1
		}
		if err := json.Unmarshal(raw, &dead); err != nil {
			fmt.Fprintf(stderr, "golabel: %s: %v\n", *deadPath, err)
			return 1
		}
	}
	labels, err := Label(*root)
	if err != nil {
		fmt.Fprintf(stderr, "golabel: %v\n", err)
		return 1
	}
	labels.Dead = dead
	encoded, err := labels.Encode()
	if err != nil {
		fmt.Fprintf(stderr, "golabel: %v\n", err)
		return 1
	}
	if *out == "" {
		if _, err := stdout.Write(encoded); err != nil {
			fmt.Fprintf(stderr, "golabel: %v\n", err)
			return 1
		}
		return 0
	}
	if err := os.WriteFile(*out, encoded, 0o644); err != nil {
		fmt.Fprintf(stderr, "golabel: %v\n", err)
		return 1
	}
	return 0
}
