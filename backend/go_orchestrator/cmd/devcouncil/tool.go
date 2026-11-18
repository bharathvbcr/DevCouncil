package main

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"os"
	"os/signal"
	"strings"
	"syscall"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/console"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/mcp"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/gate"
)

const toolHelp = `devcouncil tool — call the devcouncil_* MCP tools from a shell

The tools the MCP server serves, run in this process through the same
registry, with the same arguments and the same answer text. Use it when a
devcouncil_* MCP call errors or times out, and say in your report that you did.

Usage:
  devcouncil tool list [--json]
  devcouncil tool call NAME [--args JSON|-] [--project-root DIR]

--args is the tool's MCP arguments as one JSON object; "-" reads it from stdin.
Omit it for a tool that takes none. Example:
  devcouncil tool call devcouncil_checkout_task --args '{"task_id":"T-1","client_id":"me"}'

Exit status: 0 the tool answered ok; 1 it refused (ok:false); 2 the call was
malformed (unknown tool, bad JSON, an undeclared or missing argument); 3 it
could not run here (no task store, a store, gate or verification failure, a
timeout).

Leases are the task store's, not a server's: one taken here is the one MCP
sees. Reuse the client_id your MCP session used and checkout hands that lease
back instead of refusing it.
`

// maxToolArgsBytes bounds --args, from the flag or stdin. It matches the MCP
// server's frame cap, so the shell accepts no call the server would refuse
// for size.
const maxToolArgsBytes = 4 * 1024 * 1024

func runTool(args []string) int {
	if len(args) < 1 {
		console.Error(toolHelp)
		return devcouncil.ToolExitUsage
	}
	switch args[0] {
	case "-h", "--help", "help":
		console.Error(toolHelp)
		return 0
	case "list":
		return runToolList(args[1:])
	case "call":
		return runToolCall(args[1:])
	default:
		console.Errorf("unknown tool subcommand: %s\n", args[0])
		console.Error(toolHelp)
		return devcouncil.ToolExitUsage
	}
}

func runToolList(args []string) int {
	jsonOut := false
	for _, arg := range args {
		switch arg {
		case "--json":
			jsonOut = true
		case "-h", "--help":
			console.Error(toolHelp)
			return 0
		default:
			console.Errorf("tool list: unknown argument %s\n", arg)
			return devcouncil.ToolExitUsage
		}
	}
	specs := devcouncil.ServedToolSpecs()
	if jsonOut {
		listed, err := devcouncil.ToolsListResult(specs)
		if err != nil {
			console.Errorf("tool list: %v\n", err)
			return 1
		}
		if err := console.JSON(listed); err != nil {
			return 1
		}
		return 0
	}
	for _, spec := range specs {
		mode := "writes"
		if spec.Behaviour.ReadOnly {
			mode = "read-only"
		}
		console.Printf("%s (%s)\n    %s\n", spec.Name, mode, spec.Description)
	}
	return 0
}

func runToolCall(args []string) int {
	var name, rawArgs, rootFlag string
	argsGiven := false
	for i := 0; i < len(args); i++ {
		switch arg := args[i]; {
		case arg == "-h" || arg == "--help":
			console.Error(toolHelp)
			return 0
		case arg == "--args":
			i++
			if i >= len(args) {
				console.Errorln("tool call: --args needs a JSON object, or - to read one from stdin")
				return devcouncil.ToolExitUsage
			}
			if argsGiven {
				console.Errorln("tool call: --args given twice")
				return devcouncil.ToolExitUsage
			}
			rawArgs, argsGiven = args[i], true
		case arg == "--project-root":
			i++
			root, ok := projectRootFlag(args, i)
			if !ok {
				return devcouncil.ToolExitUsage
			}
			rootFlag = root
		case strings.HasPrefix(arg, "-"):
			console.Errorf("tool call: unknown flag %s\n", arg)
			return devcouncil.ToolExitUsage
		case name == "":
			name = arg
		default:
			console.Errorf("tool call: unexpected argument %s; pass the tool's arguments as --args JSON\n", arg)
			return devcouncil.ToolExitUsage
		}
	}
	if name == "" {
		console.Errorln("tool call: name the tool; `devcouncil tool list` names them")
		return devcouncil.ToolExitUsage
	}
	if !servedTool(name) {
		console.Errorf("tool call: %s is not a tool this host serves; `devcouncil tool list` names them\n", name)
		return devcouncil.ToolExitUsage
	}
	toolArgs, err := parseToolArgs(rawArgs, argsGiven, os.Stdin)
	if err != nil {
		console.Errorf("tool call: %v\n", err)
		return devcouncil.ToolExitUsage
	}
	root, err := resolveRoot(rootFlag)
	if err != nil {
		console.Errorf("tool call: %v\n", err)
		return devcouncil.ToolExitUsage
	}

	answer := callToolInProcess(root, name, toolArgs)
	if _, err := console.Println(answer.Text); err != nil {
		return 1
	}
	// Said every time, on stderr where it cannot corrupt the answer: a fallback
	// nobody reports is how a broken MCP server stays broken.
	console.Errorf("devcouncil tool: %s answered from the shell, not over MCP; say so in your report\n", name)
	return answer.ExitStatus()
}

// callToolInProcess is runMCP's registry, asked once. The write gate is built
// only for the tool that consults it: building it reads the whole code graph,
// which every other tool would pay for and never use (runVerify skips it for
// the same reason).
func callToolInProcess(root, name string, toolArgs map[string]any) devcouncil.ToolAnswer {
	var g *gate.Gate
	if name == "devcouncil_policy_check_write" {
		built, err := openGate(root)
		if err != nil {
			// As in runMCP: the tool then answers not_initialized rather than a
			// verdict it could not reach.
			console.Errorf("devcouncil tool: write gate unavailable: %v\n", err)
		} else {
			g = built
		}
	}
	client := openStore(root)
	if client != nil {
		defer client.Close()
	}
	reg := devcouncil.NewRegistry(root, client, g)

	budget := mcp.ToolCallBudget(name)
	parent, stop := signal.NotifyContext(console.Context(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	ctx, cancel := context.WithTimeout(parent, budget)
	defer cancel()
	answer := devcouncil.AnswerTool(reg.Call(ctx, name, toolArgs))
	if errors.Is(ctx.Err(), context.DeadlineExceeded) && answer.ExitStatus() != devcouncil.ToolExitOK {
		// What a handler returns after its context expired is a cancellation,
		// not the tool's answer; the MCP server answers it as a timeout too.
		return devcouncil.TimeoutAnswer(name, budget.String(), mcp.CallBudgetEnv)
	}
	return answer
}

func servedTool(name string) bool {
	for _, served := range devcouncil.ServedToolNames() {
		if served == name {
			return true
		}
	}
	return false
}

// parseToolArgs reads the arguments the way the MCP server does: one JSON
// object decoded into map[string]any (numbers as float64), absent or null
// meaning none. Anything that is not an object is the caller's mistake, as
// a non-object "arguments" is the server's -32602.
func parseToolArgs(raw string, given bool, stdin io.Reader) (map[string]any, error) {
	if !given {
		return map[string]any{}, nil
	}
	data := []byte(raw)
	if raw == "-" {
		read, err := io.ReadAll(io.LimitReader(stdin, maxToolArgsBytes+1))
		if err != nil {
			return nil, errors.New("--args -: reading stdin: " + err.Error())
		}
		data = read
	}
	if len(data) > maxToolArgsBytes {
		return nil, errors.New("--args is larger than the MCP frame cap")
	}
	var value any
	if err := json.Unmarshal(data, &value); err != nil {
		return nil, errors.New("--args is not JSON: " + err.Error())
	}
	switch v := value.(type) {
	case nil:
		return map[string]any{}, nil
	case map[string]any:
		return v, nil
	default:
		return nil, errors.New("--args must be a JSON object of the tool's arguments")
	}
}
