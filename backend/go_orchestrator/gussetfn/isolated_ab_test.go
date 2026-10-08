//go:build unix

package gussetfn

import (
	"context"
	"sort"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/fnmatch"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
)

// BenchmarkIsolatedBatchAB is one write decision's questions — the secret
// and protected lists folded, two planned-file patterns — asked the way a
// policy gate asks them in production: once per agent tool call, after the
// process has been idle. Each side sleeps before every sample, sides
// alternate, and each reports min, p50 and p90 of 300 samples. The engine
// side is the best engine path there is: every list prepared, all four
// questions in one crossing. Run it once:
//
//	go test -run '^$' -bench IsolatedBatchAB -benchtime 1x ./gussetfn
func BenchmarkIsolatedBatchAB(b *testing.B) {
	t := b
	ctx := context.Background()
	name := "backend/go_orchestrator/policy/file.go"
	qs := []Query{{Patterns: policy.SecretPathPatterns, Fold: true}, {Patterns: policy.ProtectedWritePatterns, Fold: true}, {Patterns: []string{"docs/*"}}, {Patterns: []string{"backend/*"}}}
	if _, err := MatchBatch(ctx, name, qs); err != nil {
		t.Fatal(err)
	}
	goSide := func() {
		fnmatch.MatchAnyFold(policy.SecretPathPatterns, name)
		fnmatch.MatchAnyFold(policy.ProtectedWritePatterns, name)
		fnmatch.MatchAny([]string{"docs/*"}, name)
		fnmatch.MatchAny([]string{"backend/*"}, name)
	}
	for _, gap := range []time.Duration{time.Millisecond, 10 * time.Millisecond} {
		var g, e []time.Duration
		for i := 0; i < 300; i++ {
			time.Sleep(gap)
			s := time.Now()
			goSide()
			g = append(g, time.Since(s))
			time.Sleep(gap)
			s = time.Now()
			MatchBatch(ctx, name, qs)
			e = append(e, time.Since(s))
		}
		sort.Slice(g, func(i, j int) bool { return g[i] < g[j] })
		sort.Slice(e, func(i, j int) bool { return e[i] < e[j] })
		t.Logf("gap %v: go min %v p50 %v p90 %v | engine batched min %v p50 %v p90 %v", gap, g[0], g[150], g[270], e[0], e[150], e[270])
		t.ReportMetric(float64(g[150].Nanoseconds()), "go-p50-ns-"+gap.String())
		t.ReportMetric(float64(e[150].Nanoseconds()), "engine-p50-ns-"+gap.String())
	}
}
