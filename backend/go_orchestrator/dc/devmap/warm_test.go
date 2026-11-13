package devmap

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"syscall"
	"testing"
	"time"
)

func TestWarmMCPHelperProcess(t *testing.T) {
	if os.Getenv("GO_WANT_WARM_MCP_HELPER") != "1" {
		return
	}
	mode := os.Getenv("WARM_HELPER_MODE")
	countFile := os.Getenv("WARM_HELPER_COUNT_FILE")
	repoRoot := os.Getenv("WARM_HELPER_ROOT")

	// Strip test flags: look at arguments after "--"
	var args []string
	for i, a := range os.Args {
		if a == "--" {
			args = os.Args[i+1:]
			break
		}
	}

	appendLine := func(tag string) int {
		if countFile == "" {
			return 1
		}
		f, err := os.OpenFile(countFile, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o644)
		if err == nil {
			_, _ = fmt.Fprintf(f, "%s:%d\n", tag, os.Getpid())
			_ = f.Close()
		}
		raw, _ := os.ReadFile(countFile)
		n := 0
		for _, line := range strings.Split(strings.TrimSpace(string(raw)), "\n") {
			if strings.HasPrefix(line, tag+":") {
				n++
			}
		}
		return n
	}

	if len(args) >= 2 && args[len(args)-2] == "manifest" && args[len(args)-1] == "--help" {
		appendLine("probe")
		_, _ = os.Stdout.WriteString("Usage: devmap manifest --output <FILE> --graph-output <FILE>\n")
		os.Exit(0)
	}

	isMCP := len(args) > 0 && args[len(args)-1] == "mcp"
	if !isMCP {
		appendLine("cli")
		if len(args) >= 2 && args[1] == "status" {
			_, _ = fmt.Fprintf(os.Stdout, `{"db_path":%q,"generation_id":3,"node_count":42,"edge_count":19,"is_fresh":true,"host_contract_version":1,"schema_version":19,"expected_schema_version":19,"schema_relation":"current","reader_ready":true,"query_ready":true,"capabilities":{"explore":true,"impact":true,"trace":true,"affected":true}}`+"\n", filepath.Join(repoRoot, "db.sqlite"))
			os.Exit(0)
		}
		if len(args) >= 2 && args[1] == "search" {
			_, _ = os.Stdout.WriteString(`{"items":[{"file_path":"pkga/a.go","symbol_name":"Alpha","kind":"function","score":1.0,"source_span":"1..3"}],"hidden":0}` + "\n")
			os.Exit(0)
		}
		os.Exit(2)
	}

	mcpSpawnNum := appendLine("mcp")
	if mode == "no-mcp" {
		_, _ = os.Stderr.WriteString("error: unrecognized subcommand 'mcp'\n")
		os.Exit(2)
	}

	scanner := bufio.NewScanner(os.Stdin)
	scanner.Buffer(make([]byte, 64<<10), 4<<20)
	var writeMu sync.Mutex
	writeJSON := func(v any) {
		b, _ := json.Marshal(v)
		b = append(b, '\n')
		writeMu.Lock()
		_, _ = os.Stdout.Write(b)
		writeMu.Unlock()
	}

	for scanner.Scan() {
		var req map[string]any
		if err := json.Unmarshal(scanner.Bytes(), &req); err != nil {
			continue
		}
		method, _ := req["method"].(string)
		id, hasID := req["id"]
		if !hasID {
			continue
		}
		switch method {
		case "initialize":
			writeJSON(map[string]any{
				"jsonrpc": "2.0",
				"id":      id,
				"result": map[string]any{
					"protocolVersion": "2025-11-25",
					"capabilities":    map[string]any{"tools": map[string]any{}},
					"serverInfo":      map[string]any{"name": "devmap", "version": "0.1.0"},
				},
			})
			// Also emit an unsolicited notification to prove readLoop ignores non-id frames.
			writeJSON(map[string]any{
				"jsonrpc": "2.0",
				"method":  "notifications/tools/list_changed",
			})

		case "tools/call":
			if mode == "hang-first" && mcpSpawnNum == 1 {
				// Alive, reading nothing, answering nothing: a hung server.
				time.Sleep(time.Hour)
			}
			if mode == "die-on-call" {
				_, _ = os.Stderr.WriteString("boom\n")
				os.Exit(3)
			}
			if mode == "binary-replaced-once" && mcpSpawnNum == 1 {
				writeJSON(map[string]any{
					"jsonrpc": "2.0",
					"id":      id,
					"result": map[string]any{
						"isError": true,
						"content": []map[string]any{
							{"type": "text", "text": "binary_replaced: devmap executable changed on disk"},
						},
					},
				})
				continue
			}

			params, _ := req["params"].(map[string]any)
			toolName, _ := params["name"].(string)
			toolArgs, _ := params["arguments"].(map[string]any)

			reportedRoot := repoRoot
			if mode == "wrong-root" {
				reportedRoot = filepath.Join(filepath.Dir(repoRoot), "other-checkout")
			}
			repoAttr := map[string]any{
				"root":          reportedRoot,
				"store":         filepath.Join(reportedRoot, ".devcouncil/codeintel/devmap.sqlite"),
				"resolved_from": "repo_path",
			}

			var structured map[string]any
			switch toolName {
			case "devmap_status":
				structured = map[string]any{
					"generation_id":     3,
					"node_count":        42,
					"edge_count":        19,
					"pending_count":     0,
					"quarantined_count": 0,
					"is_fresh":          true,
					"degraded_reason":   nil,
					"coverage_gaps":     map[string]any{},
					"repository":        repoAttr,
				}
			case "devmap_dependencies":
				target, _ := toolArgs["target"].(string)
				structured = map[string]any{
					"items": []map[string]any{
						{
							"source_file":   target,
							"source_symbol": "Caller",
							"target_file":   "pkga/a.go",
							"target_symbol": "Alpha",
							"edge_kind":     "calls",
							"confidence":    0.95,
						},
					},
					"shown":      1,
					"hidden":     0,
					"total":      1,
					"truncated":  false,
					"repository": repoAttr,
				}
			case "devmap_search":
				structured = map[string]any{
					"items": []map[string]any{
						{
							"file_path":   "pkga/a.go",
							"symbol_name": "Alpha",
							"kind":        "function",
							"score":       1.0,
							"source_span": "1..4",
						},
					},
					"shown":      1,
					"hidden":     0,
					"total":      1,
					"truncated":  false,
					"repository": repoAttr,
				}
			case "devmap_dead_symbols":
				structured = map[string]any{
					"items": []map[string]any{
						{
							"file_path":        "pkga/a.go",
							"symbol_name":      "NeverCalled",
							"confidence":       0.9,
							"is_exempt":        false,
							"exemption_reason": nil,
						},
					},
					"shown":      1,
					"hidden":     0,
					"total":      1,
					"truncated":  false,
					"repository": repoAttr,
				}
			case "devmap_explore":
				query, _ := toolArgs["query"].(string)
				structured = map[string]any{
					"query": query,
					"limit": 20,
					"definitions": map[string]any{
						"items": []map[string]any{
							{"symbol_name": query, "file_path": "pkga/a.go"},
						},
						"resolution":  "Available",
						"shown":       1,
						"total":       1,
						"hidden":      0,
						"truncated":   false,
						"tokens_used": 12,
					},
					"blast_radius": map[string]any{
						"layers": map[string]any{
							"items":       []any{},
							"resolution":  "Available",
							"shown":       0,
							"total":       0,
							"hidden":      0,
							"truncated":   false,
							"tokens_used": 0,
						},
					},
					"budget":     map[string]any{},
					"repository": repoAttr,
				}
			case "devmap_impact":
				target, _ := toolArgs["target"].(string)
				structured = map[string]any{
					"items": []map[string]any{
						{
							"source_file":   "pkgb/b.go",
							"source_symbol": "Caller",
							"target_file":   "pkga/a.go",
							"target_symbol": target,
							"edge_kind":     "calls",
							"confidence":    1.0,
						},
					},
					"resolution":  "Available",
					"shown":       1,
					"total":       1,
					"hidden":      0,
					"truncated":   false,
					"tokens_used": 10,
					"repository":  repoAttr,
				}
			case "devmap_trace":
				from, _ := toolArgs["from"].(string)
				to, _ := toolArgs["to"].(string)
				structured = map[string]any{
					"items": []map[string]any{
						{
							"source_file":   "pkgb/b.go",
							"source_symbol": from,
							"target_file":   "pkga/a.go",
							"target_symbol": to,
							"edge_kind":     "calls",
							"confidence":    1.0,
						},
					},
					"resolution":  "Available",
					"shown":       1,
					"total":       1,
					"hidden":      0,
					"truncated":   false,
					"tokens_used": 10,
					"repository":  repoAttr,
				}
			case "devmap_affected_tests":
				targets, _ := toolArgs["targets"].([]any)
				structured = map[string]any{
					"targets": targets,
					"tests": map[string]any{
						"items": []map[string]any{
							{"file_path": "pkga/a_test.go"},
						},
						"resolution":  "Available",
						"shown":       1,
						"total":       1,
						"hidden":      0,
						"truncated":   false,
						"tokens_used": 8,
					},
					"blast_radius": map[string]any{
						"layers": map[string]any{
							"items":       []any{},
							"resolution":  "Available",
							"shown":       0,
							"total":       0,
							"hidden":      0,
							"truncated":   false,
							"tokens_used": 0,
						},
					},
					"repository": repoAttr,
				}
			case "devmap_neighbors":
				targets, _ := toolArgs["targets"].([]any)
				first := "Alpha"
				if len(targets) > 0 {
					if s, ok := targets[0].(string); ok {
						first = s
					}
				}
				structured = map[string]any{
					"neighbors": []map[string]any{
						{
							"target": first,
							"callers": map[string]any{
								"items": []map[string]any{
									{
										"source_file":   "pkgb/b.go",
										"source_symbol": "Beta",
										"target_file":   "pkga/a.go",
										"target_symbol": first,
										"edge_kind":     "calls",
										"confidence":    1.0,
									},
								},
								"hidden": 0,
							},
							"callees": map[string]any{
								"items":  []any{},
								"hidden": 0,
							},
						},
					},
					"repository": repoAttr,
				}
			}

			writeJSON(map[string]any{
				"jsonrpc": "2.0",
				"id":      id,
				"result": map[string]any{
					"isError":           false,
					"content":           []map[string]any{{"type": "text", "text": "{}"}},
					"structuredContent": structured,
				},
			})
		}
	}
	os.Exit(0)
}

func writeWarmHelperWrapper(t *testing.T, mode, root, countFile string) string {
	t.Helper()
	wrapper := filepath.Join(t.TempDir(), "fake-devmap")
	script := fmt.Sprintf(
		"#!/bin/sh\nexport GO_WANT_WARM_MCP_HELPER=1\nexport WARM_HELPER_MODE=%q\nexport WARM_HELPER_ROOT=%q\nexport WARM_HELPER_COUNT_FILE=%q\nexec %q -test.run=^TestWarmMCPHelperProcess$ -- \"$@\"\n",
		mode, root, countFile, os.Args[0],
	)
	if err := os.WriteFile(wrapper, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	return wrapper
}

func countSpawns(t *testing.T, countFile string) map[string]int {
	t.Helper()
	raw, err := os.ReadFile(countFile)
	if err != nil {
		t.Fatal(err)
	}
	counts := map[string]int{}
	for _, line := range strings.Split(strings.TrimSpace(string(raw)), "\n") {
		if line == "" {
			continue
		}
		tag, _, ok := strings.Cut(line, ":")
		if ok {
			counts[tag]++
		}
	}
	return counts
}

func TestWarmSessionSingleProcess25ConcurrentDeps(t *testing.T) {
	root := t.TempDir()
	countFile := filepath.Join(t.TempDir(), "spawns.log")
	bin := writeWarmHelperWrapper(t, "ok", root, countFile)

	base := New(bin, root)
	ctx := context.Background()
	if err := base.Probe(ctx); err != nil {
		t.Fatalf("probe: %v", err)
	}

	session := base.WarmSession()
	defer func() {
		if err := session.Close(); err != nil {
			t.Fatalf("close: %v", err)
		}
	}()

	callCtx := WithCall(ctx)
	st, err := session.Available(callCtx)
	if err != nil {
		t.Fatalf("available: %v", err)
	}
	if st.GenerationID != 3 || st.DBPath == "" {
		t.Fatalf("unexpected warm status: %+v", st)
	}

	const files = 25
	var wg sync.WaitGroup
	errs := make(chan error, files)
	for i := range files {
		wg.Add(1)
		go func(idx int) {
			defer wg.Done()
			target := fmt.Sprintf("pkg/file_%02d.go", idx)
			res, err := session.Deps(callCtx, target)
			if err != nil {
				errs <- err
				return
			}
			if len(res.Items) != 1 || res.Items[0].SourceFile != target {
				errs <- fmt.Errorf("unexpected deps for %s: %+v", target, res.Items)
			}
		}(i)
	}
	wg.Wait()
	close(errs)
	for err := range errs {
		t.Fatal(err)
	}

	if found, err := session.Search(callCtx, "Alpha"); err != nil || len(found.Items) != 1 {
		t.Fatalf("warm search = %+v, err = %v", found, err)
	}
	if dead, err := session.Dead(callCtx); err != nil || len(dead.Items) != 1 {
		t.Fatalf("warm dead = %+v, err = %v", dead, err)
	}

	counts := countSpawns(t, countFile)
	if counts["probe"] != 1 {
		t.Fatalf("expected 1 probe process, got %v", counts)
	}
	if counts["mcp"] != 1 {
		t.Fatalf("expected 1 warm mcp process across Available + 25 Deps + Search + Dead, got %v", counts)
	}
	if counts["cli"] != 0 {
		t.Fatalf("expected 0 CLI fallback processes, got %v", counts)
	}
}

func TestWarmMCPAttributionMismatchFailsClosed(t *testing.T) {
	root := t.TempDir()
	countFile := filepath.Join(t.TempDir(), "spawns.log")
	bin := writeWarmHelperWrapper(t, "wrong-root", root, countFile)

	c := NewWarm(bin, root)
	defer c.Close()

	_, err := c.Search(context.Background(), "Alpha")
	if err == nil || !strings.Contains(err.Error(), "repository.root mismatch") {
		t.Fatalf("expected fail-closed repository.root mismatch error, got %v", err)
	}
}

func TestWarmMCPBinaryReplacedRestartsOnceAndSucceeds(t *testing.T) {
	root := t.TempDir()
	countFile := filepath.Join(t.TempDir(), "spawns.log")
	bin := writeWarmHelperWrapper(t, "binary-replaced-once", root, countFile)

	c := NewWarm(bin, root)
	defer c.Close()

	found, err := c.Search(WithCall(context.Background()), "Alpha")
	if err != nil {
		t.Fatalf("expected transparent recovery after binary_replaced, got: %v", err)
	}
	if len(found.Items) != 1 || found.Items[0].Name != "Alpha" {
		t.Fatalf("unexpected search items after restart: %+v", found.Items)
	}
	counts := countSpawns(t, countFile)
	if counts["mcp"] != 2 {
		t.Fatalf("expected 2 mcp spawns (initial + 1 restart), got %v", counts)
	}
}

func TestWarmMCPFallsBackCleanlyWhenBinaryLacksMCP(t *testing.T) {
	root := t.TempDir()
	countFile := filepath.Join(t.TempDir(), "spawns.log")
	bin := writeWarmHelperWrapper(t, "no-mcp", root, countFile)

	c := NewWarm(bin, root)
	defer c.Close()

	found, err := c.Search(WithCall(context.Background()), "Alpha")
	if err != nil {
		t.Fatalf("expected fallback to CLI when mcp is unsupported, got: %v", err)
	}
	if len(found.Items) != 1 || found.Items[0].Name != "Alpha" {
		t.Fatalf("unexpected CLI fallback search result: %+v", found.Items)
	}
}

func TestWarmMCPLiveContractMatchesCLI(t *testing.T) {
	bin := devmapBinary(t)
	root := fixtureRepo(t)

	cold := New(bin, root)
	cold.Timeout = 60 * time.Second
	ctx := context.Background()
	if _, err := cold.Build(ctx, 2*time.Minute); err != nil {
		t.Fatalf("build: %v", err)
	}

	warm := NewWarm(bin, root)
	warm.Timeout = 60 * time.Second
	defer warm.Close()

	st, err := warm.Available(WithCall(ctx))
	if err != nil {
		t.Fatalf("warm available: %v", err)
	}
	if st.GenerationID <= 0 || st.NodeCount <= 0 || st.DBPath == "" {
		t.Fatalf("unexpected warm status: %+v", st)
	}

	coldSearch, err := cold.Search(WithCall(ctx), "Alpha")
	if err != nil {
		t.Fatalf("cold search: %v", err)
	}
	warmSearch, err := warm.Search(WithCall(ctx), "Alpha")
	if err != nil {
		t.Fatalf("warm search: %v", err)
	}
	if len(warmSearch.Items) != len(coldSearch.Items) || len(warmSearch.Items) == 0 {
		t.Fatalf("warm vs cold search mismatch: warm=%+v cold=%+v", warmSearch.Items, coldSearch.Items)
	}

	coldDeps, err := cold.Deps(WithCall(ctx), "pkgb/b.go")
	if err != nil {
		t.Fatalf("cold deps: %v", err)
	}
	warmDeps, err := warm.Deps(WithCall(ctx), "pkgb/b.go")
	if err != nil {
		t.Fatalf("warm deps: %v", err)
	}
	if len(warmDeps.Items) != len(coldDeps.Items) || len(warmDeps.Items) == 0 {
		t.Fatalf("warm vs cold deps mismatch: warm=%+v cold=%+v", warmDeps.Items, coldDeps.Items)
	}

	// Advanced() must still populate CLI-only status readiness fields on a NewWarm client.
	adv, err := warm.Advanced(ctx, AdvancedQuery{
		Kind:          QueryExplore,
		Query:         "Alpha",
		Depth:         2,
		MinConfidence: 0,
	})
	if err != nil {
		t.Fatalf("warm Advanced(explore): %v", err)
	}
	if err := ValidateAdvancedResult(QueryExplore, adv); err != nil {
		t.Fatalf("ValidateAdvancedResult: %v", err)
	}
}

// mcpPids lists the pids of the helper `mcp` processes in spawn order.
func mcpPids(t *testing.T, countFile string) []int {
	t.Helper()
	raw, err := os.ReadFile(countFile)
	if err != nil {
		t.Fatal(err)
	}
	var pids []int
	for _, line := range strings.Split(strings.TrimSpace(string(raw)), "\n") {
		tag, pid, ok := strings.Cut(line, ":")
		if !ok || tag != "mcp" {
			continue
		}
		var n int
		if _, err := fmt.Sscanf(pid, "%d", &n); err == nil {
			pids = append(pids, n)
		}
	}
	return pids
}

func waitPidGone(t *testing.T, pid int, within time.Duration) bool {
	t.Helper()
	deadline := time.Now().Add(within)
	for time.Now().Before(deadline) {
		if err := syscall.Kill(pid, 0); err == syscall.ESRCH {
			return true
		}
		time.Sleep(20 * time.Millisecond)
	}
	return syscall.Kill(pid, 0) == syscall.ESRCH
}

func TestWarmMCPHungServerIsRestarted(t *testing.T) {
	root := t.TempDir()
	countFile := filepath.Join(t.TempDir(), "spawns.log")
	bin := writeWarmHelperWrapper(t, "hang-first", root, countFile)

	c := NewWarm(bin, root)
	c.Timeout = 750 * time.Millisecond
	defer c.Close()

	_, err := c.Search(WithCall(context.Background()), "Alpha")
	if err == nil || !strings.Contains(err.Error(), "deadline exceeded") {
		t.Fatalf("call 1 must return a deadline error, got %v", err)
	}
	c.Timeout = 10 * time.Second
	found, err := c.Search(WithCall(context.Background()), "Alpha")
	if err != nil {
		t.Fatalf("call 2 must succeed on a fresh process, got %v", err)
	}
	if len(found.Items) != 1 {
		t.Fatalf("unexpected items: %+v", found.Items)
	}
	pids := mcpPids(t, countFile)
	if len(pids) != 2 {
		t.Fatalf("expected a second mcp process, got pids %v", pids)
	}
	if !waitPidGone(t, pids[0], 3*time.Second) {
		t.Fatalf("hung server pid %d is still alive", pids[0])
	}
}

func TestWarmMCPOwnerContextCancelKillsProcess(t *testing.T) {
	root := t.TempDir()
	countFile := filepath.Join(t.TempDir(), "spawns.log")
	bin := writeWarmHelperWrapper(t, "ok", root, countFile)

	owner, cancel := context.WithCancel(context.Background())
	defer cancel()
	c := New(bin, root)
	c.EnableWarmMCPContext(owner)
	defer c.Close()

	if _, err := c.Search(WithCall(context.Background()), "Alpha"); err != nil {
		t.Fatalf("search: %v", err)
	}
	pids := mcpPids(t, countFile)
	if len(pids) != 1 {
		t.Fatalf("expected one mcp process, got %v", pids)
	}
	cancel()
	if !waitPidGone(t, pids[0], 2*time.Second) {
		t.Fatalf("mcp pid %d survived cancellation of its owner context", pids[0])
	}
}

func TestWarmMCPSessionDeathCarriesStderr(t *testing.T) {
	root := t.TempDir()
	countFile := filepath.Join(t.TempDir(), "spawns.log")
	bin := writeWarmHelperWrapper(t, "die-on-call", root, countFile)

	c := NewWarm(bin, root)
	defer c.Close()

	_, err := c.Search(WithCall(context.Background()), "Alpha")
	if err == nil {
		t.Fatal("expected an error from a server that exits mid-call")
	}
	if !strings.Contains(err.Error(), "boom") || !strings.Contains(err.Error(), "exit status 3") {
		t.Fatalf("error must carry the exit status and the stderr tail, got %v", err)
	}
}

func TestWarmMCPAdvancedQueriesUseWarmSession(t *testing.T) {
	root := t.TempDir()
	countFile := filepath.Join(t.TempDir(), "spawns.log")
	bin := writeWarmHelperWrapper(t, "ok", root, countFile)

	c := NewWarm(bin, root)
	defer c.Close()

	ctx := context.Background()
	for _, q := range []AdvancedQuery{
		{Kind: QueryExplore, Query: "Alpha", Depth: 2, Budget: 1500, MinConfidence: 0.25},
		{Kind: QueryImpact, Query: "Alpha", Depth: 3, Budget: 1500, MinRung: "high"},
		{Kind: QueryTrace, Query: "Beta", To: "Alpha", Depth: 3, Budget: 1500, MinRung: "deterministic"},
		{Kind: QueryAffected, Targets: []string{"pkga/a.go", "Alpha"}, Depth: 2, Budget: 1500, MinConfidence: 0.5},
	} {
		res, err := c.Advanced(ctx, q)
		if err != nil {
			t.Fatalf("warm Advanced(%s) failed: %v", q.Kind, err)
		}
		if err := ValidateAdvancedResult(q.Kind, res); err != nil {
			t.Fatalf("ValidateAdvancedResult(%s): %v", q.Kind, err)
		}
	}

	neighbors, err := c.Neighbors(WithCall(ctx), 2, "Alpha")
	if err != nil {
		t.Fatalf("warm Neighbors failed: %v", err)
	}
	if len(neighbors.Items) != 1 || neighbors.Items[0].Target != "Alpha" || len(neighbors.Items[0].Callers.Items) != 1 {
		t.Fatalf("unexpected warm Neighbors: %+v", neighbors.Items)
	}

	deps, err := c.Deps(WithCall(ctx), "pkgb/b.go")
	if err != nil || len(deps.Items) != 1 {
		t.Fatalf("warm Deps = %+v, err = %v", deps, err)
	}

	counts := countSpawns(t, countFile)
	if counts["mcp"] != 1 {
		t.Fatalf("expected exactly 1 warm mcp process across Explore+Impact+Trace+Affected+Neighbors+Deps, got %v", counts)
	}
	// Each Advanced() performs 2 CLI-only status checks (before & after snapshot qualification);
	// Neighbors + Deps under WithCall use warm MCP status, so cli spawns must equal 8 (never query fallbacks).
	if counts["cli"] != 8 {
		t.Fatalf("expected 8 CLI status spawns and 0 query CLI fallbacks, got %v", counts)
	}
}
