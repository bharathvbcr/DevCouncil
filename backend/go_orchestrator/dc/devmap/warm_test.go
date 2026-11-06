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
			_, _ = fmt.Fprintf(os.Stdout, `{"db_path":%q,"generation_id":1,"node_count":5,"edge_count":3,"is_fresh":true}`+"\n", filepath.Join(repoRoot, "db.sqlite"))
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
