// Command dcmap is the Go port of DevCouncil's `dev map` command surface.
//
// It is named dcmap rather than devmap because devmap is the Rust kernel this
// CLI drives, and two binaries of that name on one PATH would make which one
// runs a property of shell ordering.
package main

import (
	"context"
	"fmt"
	"os"
	"os/signal"
	"syscall"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/internal/mapcli"
)

func main() {
	// Cancellation propagates to the kernel child through exec.CommandContext,
	// so a Ctrl-C during a long build stops the build rather than orphaning it.
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	env := &mapcli.Env{Stderr: os.Stderr, Root: mapcli.DefaultRoot()}
	rest, err := mapcli.ParseGlobals(env, os.Args[1:])
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
	os.Exit(mapcli.Run(ctx, env, os.Stdout, rest))
}
