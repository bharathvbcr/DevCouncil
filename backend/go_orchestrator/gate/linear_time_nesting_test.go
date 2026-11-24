package gate

import (
	"strings"
	"testing"
	"time"
)

// Deciding a command is linear in its length, or refused past a depth cap. The
// audit measured ~4x per doubling on nested `$((` and `($(`: a 125 KB command
// took 21 s to refuse. The assertion is on the ratio between a 32 KB and a
// 128 KB command (4x the length) in process CPU time, not wall time, so a
// loaded runner does not move it. The allowed ratio (10) sits
// between linear (4x) and quadratic (16x) with headroom for timer and GC noise
// on the small run (a loaded host measured 6.6 and 7.9 on a 15 ms run against
// the old limit of 6); each size takes the best of five runs.
func TestNestedArithmeticAndSubstitutionDecideInLinearTime(t *testing.T) {
	shapes := map[string]func(n int) string{
		"nested-arithmetic": func(n int) string {
			return "echo " + strings.Repeat("$((", n) + "1" + strings.Repeat("))", n)
		},
		"arithmetic-in-substitution": func(n int) string {
			return "echo " + strings.Repeat("($(", n) + "1" + strings.Repeat("))", n)
		},
		"alternating": func(n int) string {
			return "echo " + strings.Repeat("$(($(", n) + "1" + strings.Repeat(")))", n)
		},
	}
	decide := func(command string) time.Duration {
		best := time.Duration(1<<63 - 1)
		for i := 0; i < 5; i++ {
			g := newGate(t, nil)
			start := processCPU()
			if _, err := g.EvaluateCommand(command, boundsTask()); err != nil {
				t.Fatal(err)
			}
			if d := processCPU() - start; d < best {
				best = d
			}
		}
		return best
	}
	for name, shape := range shapes {
		// Size by bytes: one level of the shape costs len(shape(2))-len(shape(1)).
		perLevel := len(shape(2)) - len(shape(1))
		small := shape((32 << 10) / perLevel)
		large := shape((120 << 10) / perLevel)
		a, b := decide(small), decide(large)
		ratio := float64(b) / float64(a)
		t.Logf("%s: %d bytes %v, %d bytes %v, ratio %.1f", name, len(small), a, len(large), b, ratio)
		if ratio > 10 {
			t.Errorf("%s: deciding %d bytes took %v and %d bytes took %v (ratio %.1f for 4x the input); "+
				"cost is super-linear in an input the caller controls", name, len(small), a, len(large), b, ratio)
		}
		// The ratio is what proves linearity; this ceiling only catches a
		// uniformly slow decision. It sits under the audited 21 s regression and
		// well over what a shared CI runner takes (alternating measured 3.8 s
		// there, 1 s per 32 KB), so a slow runner cannot trip it.
		if b > 15*time.Second {
			t.Errorf("%s: %d bytes took %v to decide", name, len(large), b)
		}
	}
}
