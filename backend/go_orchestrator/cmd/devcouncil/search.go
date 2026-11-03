// Command devcouncil search routes queries across devmap and dcgrep.
package main

import (
	"context"
	"errors"
	"os/exec"
	"regexp"
	"strconv"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/console"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/dcgrep"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/devmap"
)

// UnifiedMatch represents a single search match across symbol, ast, or text engines.
type UnifiedMatch struct {
	Path       string  `json:"path"`
	LineNumber int     `json:"line_number,omitempty"`
	Line       string  `json:"line,omitempty"`
	SymbolName string  `json:"symbol_name,omitempty"`
	Kind       string  `json:"kind,omitempty"`
	Score      float64 `json:"score,omitempty"`
}

// UnifiedSearchResult represents the canonical search response envelope.
type UnifiedSearchResult struct {
	OK          bool           `json:"ok"`
	Query       string         `json:"query"`
	RoutingType string         `json:"routing_type"`
	EngineUsed  string         `json:"engine_used"`
	Total       int            `json:"total"`
	Shown       int            `json:"shown"`
	Truncated   bool           `json:"truncated"`
	Matches     []UnifiedMatch `json:"matches"`
	Error       string         `json:"error,omitempty"`
}

var identRegex = regexp.MustCompile(`^[A-Za-z_][A-Za-z0-9_]*$`)
var regexChars = regexp.MustCompile(`[.*+?^${}()|[\]\\]`)

// runSearch routes incoming searches to dcgrep or devmap according to type and heuristics.
func runSearch(args []string) int {
	query := ""
	searchType := ""
	searchPath := ""
	maxResults := 50
	root := ""
	jsonOut := false

	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--json":
			jsonOut = true
		case "--type":
			i++
			if i >= len(args) {
				console.Errorln("--type requires an argument (symbol|structural|ast|literal|regex)")
				return 2
			}
			searchType = strings.ToLower(args[i])
		case "--path":
			i++
			if i >= len(args) {
				console.Errorln("--path requires a directory")
				return 2
			}
			searchPath = args[i]
		case "--max", "--max-results":
			i++
			if i >= len(args) {
				console.Errorln("--max requires a positive integer")
				return 2
			}
			n, err := strconv.Atoi(args[i])
			if err != nil || n < 1 {
				console.Errorf("--max requires a positive integer, got %q\n", args[i])
				return 2
			}
			maxResults = n
		case "--project-root":
			i++
			r, ok := projectRootFlag(args, i)
			if !ok {
				return 2
			}
			root = r
		case "-h", "--help":
			console.Errorln("usage: devcouncil search QUERY [--type symbol|structural|literal|regex] [--path DIR] [--max N] [--json]")
			return 0
		default:
			if strings.HasPrefix(args[i], "-") {
				console.Errorf("unknown flag: %s\n", args[i])
				return 2
			}
			if query == "" {
				query = args[i]
			} else {
				console.Errorf("unexpected argument: %s\n", args[i])
				return 2
			}
		}
	}

	if query == "" {
		console.Errorln("search requires QUERY")
		return 2
	}

	resolvedRoot, err := resolveRoot(root)
	if err != nil {
		console.Errorf("search root: %v\n", err)
		return 2
	}

	// Determine routing type
	routing := searchType
	if routing == "" {
		if regexChars.MatchString(query) {
			routing = "regex"
		} else if identRegex.MatchString(query) {
			routing = "symbol"
		} else {
			routing = "literal"
		}
	}

	ctx := console.Context()

	switch routing {
	case "symbol":
		// Route to devmap search
		res, devmapErr := routeDevmapSymbol(ctx, resolvedRoot, query, maxResults)
		if devmapErr == nil && len(res.Matches) > 0 {
			return outputSearchResult(res, jsonOut)
		}
		// If explicit symbol was requested, report devmap error/empty
		if searchType == "symbol" {
			if devmapErr != nil {
				console.Errorf("devmap symbol search: %v\n", devmapErr)
				return 1
			}
			return outputSearchResult(res, jsonOut)
		}
		// In auto mode, fallback to literal search
		routing = "literal"
		fallthrough

	case "literal", "regex":
		isRegex := (routing == "regex")
		res, grepErr := routeDcgreg(ctx, resolvedRoot, query, searchPath, maxResults, isRegex)
		if grepErr != nil {
			if errors.Is(grepErr, exec.ErrNotFound) || errors.Is(grepErr, dcgrep.ErrNoBinary) {
				console.Errorln(grepMissingBinary)
			} else {
				console.Errorf("dcgrep search: %v\n", grepErr)
			}
			return 1
		}
		res.RoutingType = routing
		return outputSearchResult(res, jsonOut)

	case "structural", "ast":
		res, astErr := routeDevmapAst(ctx, resolvedRoot, query, maxResults)
		if astErr != nil {
			console.Errorf("devmap structural search: %v\n", astErr)
			return 1
		}
		return outputSearchResult(res, jsonOut)

	default:
		console.Errorf("unsupported search type: %q (expected symbol, structural, literal, regex)\n", routing)
		return 2
	}
}

func routeDevmapSymbol(ctx context.Context, root, query string, maxResults int) (UnifiedSearchResult, error) {
	bin, err := resolveDevmap(root)
	if err != nil {
		return UnifiedSearchResult{}, err
	}
	client := devmap.New(bin, root)
	client.Budget = maxResults

	rawRes, err := client.Search(ctx, query)
	if err != nil {
		return UnifiedSearchResult{}, err
	}

	matches := make([]UnifiedMatch, 0, len(rawRes.Items))
	for _, sym := range rawRes.Items {
		matches = append(matches, UnifiedMatch{
			Path:       sym.FilePath,
			SymbolName: sym.Name,
			Kind:       sym.Kind,
			Score:      sym.Score,
			Line:       sym.SourceSpan,
		})
	}

	return UnifiedSearchResult{
		OK:          true,
		Query:       query,
		RoutingType: "symbol",
		EngineUsed:  "devmap",
		Total:       len(matches) + rawRes.Hidden,
		Shown:       len(matches),
		Truncated:   rawRes.Hidden > 0,
		Matches:     matches,
	}, nil
}

func routeDevmapAst(ctx context.Context, root, query string, maxResults int) (UnifiedSearchResult, error) {
	bin, err := resolveDevmap(root)
	if err != nil {
		return UnifiedSearchResult{}, err
	}
	client := devmap.New(bin, root)
	client.Budget = maxResults

	// Run structural query using devmap client
	rawRes, err := client.Search(ctx, query)
	if err != nil {
		return UnifiedSearchResult{}, err
	}

	matches := make([]UnifiedMatch, 0, len(rawRes.Items))
	for _, sym := range rawRes.Items {
		matches = append(matches, UnifiedMatch{
			Path:       sym.FilePath,
			SymbolName: sym.Name,
			Kind:       sym.Kind,
			Score:      sym.Score,
			Line:       sym.SourceSpan,
		})
	}

	return UnifiedSearchResult{
		OK:          true,
		Query:       query,
		RoutingType: "structural",
		EngineUsed:  "devmap",
		Total:       len(matches) + rawRes.Hidden,
		Shown:       len(matches),
		Truncated:   rawRes.Hidden > 0,
		Matches:     matches,
	}, nil
}

func routeDcgreg(ctx context.Context, root, query, searchPath string, maxResults int, isRegex bool) (UnifiedSearchResult, error) {
	client, err := grepClient(root)
	if err != nil {
		return UnifiedSearchResult{}, err
	}

	pattern := query
	if !isRegex {
		pattern = regexp.QuoteMeta(query)
	}

	req := dcgrep.Request{
		Pattern:    pattern,
		Path:       searchPath,
		MaxResults: maxResults,
	}

	rawRes, err := client.Search(ctx, req)
	if err != nil {
		return UnifiedSearchResult{}, err
	}

	matches := make([]UnifiedMatch, 0, len(rawRes.Matches))
	for _, m := range rawRes.Matches {
		matches = append(matches, UnifiedMatch{
			Path:       m.Path,
			LineNumber: m.LineNumber,
			Line:       m.Line,
		})
	}

	engine := "dcgrep"
	routingType := "literal"
	if isRegex {
		routingType = "regex"
	}

	return UnifiedSearchResult{
		OK:          true,
		Query:       query,
		RoutingType: routingType,
		EngineUsed:  engine,
		Total:       len(matches),
		Shown:       len(matches),
		Truncated:   rawRes.Truncated,
		Matches:     matches,
	}, nil
}

func outputSearchResult(res UnifiedSearchResult, jsonOut bool) int {
	if jsonOut {
		if err := console.JSON(res); err != nil {
			console.Errorln(err)
			return 1
		}
		return 0
	}

	if len(res.Matches) == 0 {
		console.Printf("No matches found for %q (engine: %s, type: %s)\n", res.Query, res.EngineUsed, res.RoutingType)
		return 0
	}

	for _, m := range res.Matches {
		if m.SymbolName != "" {
			console.Printf("%s: [symbol:%s] %s (%s)\n", m.Path, m.Kind, m.SymbolName, m.Line)
		} else {
			console.Printf("%s:%d: %s\n", m.Path, m.LineNumber, m.Line)
		}
	}

	if res.Truncated {
		console.Errorf("truncated at %d matches\n", res.Shown)
	}
	return 0
}
