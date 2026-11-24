package main

import (
	"bufio"
	"context"
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"sort"
	"strings"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

// mcpProbeBudget bounds one server's whole probe: start, initialize and
// tools/list. The host builds its write gate before it reads stdin, so a slow
// handshake is a finding to time, not a hang to wait out.
const mcpProbeBudget = 30 * time.Second

// mcpProbeMaxLine bounds one reply line. DevMap's tools/list is about 80 KiB.
const mcpProbeMaxLine = 8 * 1024 * 1024

// mcpProbeProtocol is a handshake revision both servers accept.
const mcpProbeProtocol = "2025-06-18"

// mcpProbe is what `doctor --mcp` says about one MCP server.
type mcpProbe struct {
	Server  string   `json:"server"`
	Command []string `json:"command"`
	// Status is ok, failed (it ran and something was wrong) or not_run (it
	// could not be started). not_run is never reported as ok.
	Status       string   `json:"status"`
	Reason       string   `json:"reason,omitempty"`
	StartMS      int64    `json:"start_ms"`
	InitializeMS int64    `json:"initialize_ms"`
	ToolsListMS  int64    `json:"tools_list_ms"`
	Tools        int      `json:"tools"`
	MissingTools []string `json:"missing_tools,omitempty"`
	ExtraTools   []string `json:"extra_tools,omitempty"`
	Stderr       string   `json:"stderr_tail,omitempty"`
}

// mcpServerSpec is one server to probe. want, when set, is the tool list it
// must advertise exactly.
type mcpServerSpec struct {
	name string
	bin  string
	args []string
	env  []string
	want []string
	// notRun is set when the server could not even be located.
	notRun string
}

// mcpServers names the two servers an agent here is given: this host, and
// DevMap. The host is this executable, so the probe asks the binary the
// agent's own `dev` resolves to, not another copy.
func mcpServers(root string) []mcpServerSpec {
	host := mcpServerSpec{
		name: "devcouncil",
		args: []string{"mcp"},
		env:  []string{"DEVCOUNCIL_PROJECT_ROOT=" + root},
		want: devcouncil.ServedToolNames(),
	}
	if self, err := os.Executable(); err != nil {
		host.notRun = "this executable's path is unknown: " + err.Error()
	} else {
		host.bin = self
	}
	devmapSpec := mcpServerSpec{name: "devmap", args: []string{"mcp", "--root", root}}
	if bin, err := resolveDevmap(root); err != nil {
		devmapSpec.notRun = err.Error()
	} else {
		devmapSpec.bin = bin
	}
	return []mcpServerSpec{host, devmapSpec}
}

func probeMCPServers(ctx context.Context, root string) []mcpProbe {
	specs := mcpServers(root)
	out := make([]mcpProbe, 0, len(specs))
	for _, spec := range specs {
		out = append(out, probeMCPServer(ctx, spec, mcpProbeBudget))
	}
	return out
}

// probeMCPServer starts one server, runs initialize and tools/list, and stops
// it. The child is killed by the process handle this function started, under
// budget, and never by name.
func probeMCPServer(parent context.Context, spec mcpServerSpec, budget time.Duration) (report mcpProbe) {
	report = mcpProbe{Server: spec.name, Command: append([]string{spec.bin}, spec.args...), Status: "not_run"}
	if spec.notRun != "" {
		report.Reason = spec.notRun
		return report
	}
	ctx, cancel := context.WithTimeout(parent, budget)
	defer cancel()
	cmd := exec.CommandContext(ctx, spec.bin, spec.args...)
	proc.ConfigureGroup(cmd)
	cmd.Env = append(os.Environ(), spec.env...)
	cmd.WaitDelay = 2 * time.Second
	stderr := &tailBuffer{max: 2048}
	cmd.Stderr = stderr
	stdin, err := cmd.StdinPipe()
	if err != nil {
		report.Reason = err.Error()
		return report
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		report.Reason = err.Error()
		return report
	}
	started := time.Now()
	if err := cmd.Start(); err != nil {
		report.Reason = "start: " + err.Error()
		return report
	}
	report.StartMS = time.Since(started).Milliseconds()
	defer func() {
		_ = stdin.Close()
		cancel()
		_ = cmd.Wait()
		report.Stderr = strings.TrimSpace(stderr.String())
	}()
	report.Status = "failed"
	lines := bufio.NewReaderSize(stdout, 64*1024)

	phase := time.Now()
	if err := sendFrame(stdin, map[string]any{"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": map[string]any{
		"protocolVersion": mcpProbeProtocol, "capabilities": map[string]any{},
		"clientInfo": map[string]any{"name": "devcouncil-doctor", "version": Version},
	}}); err != nil {
		report.Reason = "initialize: " + err.Error()
		return report
	}
	if _, err := readReply(ctx, lines, 1, budget); err != nil {
		report.Reason = "initialize: " + err.Error()
		return report
	}
	report.InitializeMS = time.Since(phase).Milliseconds()

	phase = time.Now()
	if err := sendFrame(stdin, map[string]any{"jsonrpc": "2.0", "method": "notifications/initialized"}); err != nil {
		report.Reason = "initialized: " + err.Error()
		return report
	}
	if err := sendFrame(stdin, map[string]any{"jsonrpc": "2.0", "id": 2, "method": "tools/list"}); err != nil {
		report.Reason = "tools/list: " + err.Error()
		return report
	}
	result, err := readReply(ctx, lines, 2, budget)
	if err != nil {
		report.Reason = "tools/list: " + err.Error()
		return report
	}
	report.ToolsListMS = time.Since(phase).Milliseconds()
	var listed struct {
		Tools []struct {
			Name string `json:"name"`
		} `json:"tools"`
	}
	if err := json.Unmarshal(result, &listed); err != nil {
		report.Reason = "tools/list result: " + err.Error()
		return report
	}
	report.Tools = len(listed.Tools)
	if report.Tools == 0 {
		report.Reason = "tools/list returned no tools"
		return report
	}
	if spec.want != nil {
		got := make([]string, 0, len(listed.Tools))
		for _, t := range listed.Tools {
			got = append(got, t.Name)
		}
		report.MissingTools, report.ExtraTools = setDifference(spec.want, got), setDifference(got, spec.want)
		if len(report.MissingTools) > 0 || len(report.ExtraTools) > 0 {
			report.Reason = "the served tools differ from this binary's list; the server on this path is another build"
			return report
		}
	}
	report.Status = "ok"
	return report
}

// sendFrame writes one JSON-RPC frame. Map keys are sorted: encoding/json/v2
// otherwise emits them in Go's random map order, so the same frame reached the
// server as `"id":2,` or `"id":2}` depending on the run, and a server that
// matches the id by its bytes (the shell fakes in the tests) missed it.
func sendFrame(w io.Writer, frame map[string]any) error {
	data, err := json.Marshal(frame, json.Deterministic(true))
	if err != nil {
		return err
	}
	_, err = w.Write(append(data, '\n'))
	return err
}

// readReply reads lines until the response with the given id, skipping
// notifications. A JSON-RPC error is returned as an error.
func readReply(ctx context.Context, r *bufio.Reader, id int, budget time.Duration) (jsontext.Value, error) {
	for {
		line, err := readBoundedLine(r, mcpProbeMaxLine)
		if err != nil {
			if ctxErr := ctx.Err(); ctxErr != nil {
				return nil, fmt.Errorf("no answer within %s", budget)
			}
			return nil, err
		}
		var msg struct {
			ID     jsontext.Value `json:"id"`
			Result jsontext.Value `json:"result"`
			Error  *struct {
				Code    int    `json:"code"`
				Message string `json:"message"`
			} `json:"error"`
		}
		if json.Unmarshal(line, &msg) != nil || string(msg.ID) != fmt.Sprint(id) {
			continue
		}
		if msg.Error != nil {
			return nil, fmt.Errorf("error %d: %s", msg.Error.Code, msg.Error.Message)
		}
		return msg.Result, nil
	}
}

func readBoundedLine(r *bufio.Reader, limit int) ([]byte, error) {
	var buf []byte
	for {
		chunk, err := r.ReadSlice('\n')
		buf = append(buf, chunk...)
		if len(buf) > limit {
			return nil, fmt.Errorf("a reply line passed %d bytes", limit)
		}
		if err == nil {
			return buf, nil
		}
		if errors.Is(err, bufio.ErrBufferFull) {
			continue
		}
		if errors.Is(err, io.EOF) {
			return nil, errors.New("the server closed its output before answering")
		}
		return nil, err
	}
}

func setDifference(a, b []string) []string {
	in := make(map[string]bool, len(b))
	for _, s := range b {
		in[s] = true
	}
	var out []string
	for _, s := range a {
		if !in[s] {
			out = append(out, s)
		}
	}
	sort.Strings(out)
	return out
}

// tailBuffer keeps the last max bytes written to it.
type tailBuffer struct {
	max  int
	data []byte
}

func (t *tailBuffer) Write(p []byte) (int, error) {
	t.data = append(t.data, p...)
	if over := len(t.data) - t.max; over > 0 {
		t.data = t.data[over:]
	}
	return len(p), nil
}

func (t *tailBuffer) String() string { return string(t.data) }
