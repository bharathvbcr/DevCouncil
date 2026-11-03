package main

import (
	"strings"
	"testing"
)

func TestDispatchSearchHelp(t *testing.T) {
	stderr, restore := swapStderr(t)
	code := dispatch([]string{"search", "--help"})
	restore()
	if code != 0 {
		t.Fatalf("exit %d, want 0 stderr=%s", code, stderr.String())
	}
	if strings.Contains(stderr.String(), "unknown command") {
		t.Fatalf("search was rejected as unknown: %s", stderr.String())
	}
}

func TestDispatchSearchWithoutQueryExits2(t *testing.T) {
	stderr, restore := swapStderr(t)
	code := dispatch([]string{"search"})
	restore()
	if code != 2 {
		t.Fatalf("exit %d, want 2 stderr=%s", code, stderr.String())
	}
	if !strings.Contains(stderr.String(), "QUERY") {
		t.Fatalf("stderr=%q", stderr.String())
	}
}

func TestDispatchSearchInvalidTypeExits2(t *testing.T) {
	stderr, restore := swapStderr(t)
	code := dispatch([]string{"search", "foo", "--type", "invalid_type"})
	restore()
	if code != 2 {
		t.Fatalf("exit %d, want 2 stderr=%s", code, stderr.String())
	}
	if !strings.Contains(stderr.String(), "unsupported search type") {
		t.Fatalf("stderr=%q", stderr.String())
	}
}

func TestSearchRoutingHeuristics(t *testing.T) {
	// Identifiers route to symbol
	if !identRegex.MatchString("ParseConfig") {
		t.Fatalf("ParseConfig should be recognized as identifier")
	}
	if !identRegex.MatchString("my_var_123") {
		t.Fatalf("my_var_123 should be recognized as identifier")
	}

	// Regex characters route to regex
	if !regexChars.MatchString("foo.*bar") {
		t.Fatalf("foo.*bar should be recognized as regex")
	}
	if !regexChars.MatchString("[0-9]+") {
		t.Fatalf("[0-9]+ should be recognized as regex")
	}
	if regexChars.MatchString("plainword") {
		t.Fatalf("plainword should not have regex chars")
	}
}

func TestUnifiedSearchResultStructure(t *testing.T) {
	res := UnifiedSearchResult{
		OK:          true,
		Query:       "testQuery",
		RoutingType: "symbol",
		EngineUsed:  "devmap",
		Total:       1,
		Shown:       1,
		Matches: []UnifiedMatch{
			{
				Path:       "src/lib.rs",
				SymbolName: "testQuery",
				Kind:       "Function",
				Score:      0.95,
				Line:       "10:1-10:20",
			},
		},
	}
	if !res.OK || res.Total != 1 || res.Matches[0].SymbolName != "testQuery" {
		t.Fatalf("unexpected UnifiedSearchResult payload: %+v", res)
	}
}
