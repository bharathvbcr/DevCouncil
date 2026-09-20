package main

import (
	"errors"
	"os"
	"strconv"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/console"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/dcgrep"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

// runGrep is the host's inbound to dc/dcgrep. The client package existed with
// no production import; an empty match list is a real negative, so a missing
// binary must be an error here the same way it is inside the client.
func runGrep(args []string) int {
	root := projectRoot()
	req := dcgrep.Request{}
	jsonOut := false
	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--json":
			jsonOut = true
		case "--path":
			i++
			if i >= len(args) {
				console.Errorln("--path needs a directory")
				return 2
			}
			req.Path = args[i]
		case "--max", "--max-results":
			i++
			if i >= len(args) {
				console.Errorln("--max needs a positive integer")
				return 2
			}
			n, err := strconv.Atoi(args[i])
			if err != nil || n < 1 {
				console.Errorf("--max needs a positive integer, got %q\n", args[i])
				return 2
			}
			req.MaxResults = n
		case "--ignore-case":
			req.CaseInsensitive = true
		case "--include-ignored":
			req.IncludeIgnored = true
		case "--project-root":
			i++
			if i >= len(args) {
				console.Errorln("--project-root needs a value")
				return 2
			}
			root = args[i]
		case "-h", "--help":
			console.Errorln("usage: devcouncil grep PATTERN [--json] [--path DIR] [--max N] [--ignore-case] [--include-ignored]")
			return 0
		default:
			if strings.HasPrefix(args[i], "-") {
				console.Errorf("unknown flag: %s\n", args[i])
				return 2
			}
			if req.Pattern == "" {
				req.Pattern = args[i]
			} else {
				console.Errorf("unexpected argument: %s\n", args[i])
				return 2
			}
		}
	}
	if req.Pattern == "" {
		console.Errorln("grep requires PATTERN")
		return 2
	}

	client := grepClient(root)
	if client == nil {
		console.Errorln(grepMissingBinary)
		return 1
	}
	result, err := client.Search(console.Context(), req)
	if err != nil {
		if errors.Is(err, dcgrep.ErrNoBinary) {
			console.Errorln(grepMissingBinary)
			return 1
		}
		console.Errorln(err)
		return 1
	}
	if jsonOut {
		if err := console.JSON(result); err != nil {
			console.Errorln(err)
			return 1
		}
		return 0
	}
	for _, match := range result.Matches {
		line := match.Line
		if match.LineTruncated {
			line += "…"
		}
		console.Printf("%s:%d:%s\n", match.Path, match.LineNumber, line)
	}
	if n := result.Skipped.Total(); n > 0 {
		console.Errorf("dcgrep skipped %d files (too_large=%d binary=%d unreadable=%d unrepresentable_name=%d)\n",
			n, result.Skipped.TooLarge, result.Skipped.Binary, result.Skipped.Unreadable, result.Skipped.UnrepresentableName)
	}
	if result.Truncated {
		console.Errorf("truncated at %d matches\n", result.Limit)
	}
	return 0
}

const grepMissingBinary = "no dcgrep binary; install with `devcouncil install dcgrep`, or set " + dcgrep.BinaryEnv

func grepClient(root string) *dcgrep.Client {
	if override := strings.TrimSpace(os.Getenv(dcgrep.BinaryEnv)); override != "" {
		return dcgrep.New(override, root)
	}
	binary, err := proc.LookPathOutside("dcgrep", root)
	if err != nil {
		return nil
	}
	return dcgrep.New(binary, root)
}
