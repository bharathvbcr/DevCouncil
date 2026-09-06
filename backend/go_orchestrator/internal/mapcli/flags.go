package mapcli

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// ParseGlobals splits leading global flags off the argument list and returns
// the remainder, which begins with the subcommand.
//
// Hand-rolled rather than `flag`: the standard parser stops at the first
// non-flag argument, which is what we want, but it also owns `-h`, prints to
// its own writer and exits the process on a parse error. This CLI must keep
// every diagnostic on stderr under --json, so it cannot delegate that decision
// to a package that may print elsewhere.
//
// Parsing stops at the subcommand, so flags after it belong to the subcommand
// and are forwarded untouched — that is what lets a delegated command receive
// the kernel's own flags without this CLI having to know them.
func ParseGlobals(env *Env, args []string) ([]string, error) {
	i := 0
	for i < len(args) {
		arg := args[i]
		switch {
		case arg == "--":
			// An explicit end of global flags: everything after is the
			// subcommand and its arguments, even if it looks like a flag.
			return args[i+1:], nil
		case arg == "--json":
			env.JSON = true
			i++
		case arg == "--root":
			if i+1 >= len(args) {
				return nil, fmt.Errorf("--root needs a directory")
			}
			if err := setRoot(env, args[i+1]); err != nil {
				return nil, err
			}
			i += 2
		case strings.HasPrefix(arg, "--root="):
			if err := setRoot(env, strings.TrimPrefix(arg, "--root=")); err != nil {
				return nil, err
			}
			i++
		default:
			// Not a global flag: the subcommand starts here. Unknown flags are
			// deliberately not rejected — they may be the subcommand's.
			return args[i:], nil
		}
	}
	return nil, nil
}

func setRoot(env *Env, dir string) error {
	if dir == "" {
		return fmt.Errorf("--root needs a directory")
	}
	abs, err := filepath.Abs(dir)
	if err != nil {
		return fmt.Errorf("--root %q: %w", dir, err)
	}
	info, err := os.Stat(abs)
	if err != nil {
		return fmt.Errorf("--root %q: %w", dir, err)
	}
	if !info.IsDir() {
		return fmt.Errorf("--root %q is not a directory", dir)
	}
	env.Root = abs
	return nil
}

// DefaultRoot is the working directory, resolved. A relative root would be
// re-resolved against the child's directory when the kernel is exec'd, which is
// the kind of difference that only shows up on someone else's machine.
func DefaultRoot() string {
	wd, err := os.Getwd()
	if err != nil {
		return "."
	}
	abs, err := filepath.Abs(wd)
	if err != nil {
		return wd
	}
	return abs
}
