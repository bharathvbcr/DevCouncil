package gate

import (
	"strings"
	"testing"
	"time"
)

// Deciding a command is linear in its length, or refused past a depth cap. The
// audit measured ~4x per doubling on nested `$((` and `($(`: a 125 KB command
// took 21 s to refuse. The assertion is on the ratio between a 32 KB and a
// 128 KB command (4x the length), not on wall time, so a loaded machine does
// not move it; the allowed ratio is generous against linear (4x) and far under
// quadratic (16x).
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
		for i := 0; i < 3; i++ {
			g := newGate(t, nil)
			start := time.Now()
			if _, err := g.EvaluateCommand(command, boundsTask()); err != nil {
				t.Fatal(err)
			}
			if d := time.Since(start); d < best {
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
		if ratio > 6 {
			t.Errorf("%s: deciding %d bytes took %v and %d bytes took %v (ratio %.1f for 4x the input); "+
				"cost is super-linear in an input the caller controls", name, len(small), a, len(large), b, ratio)
		}
		if b > 2*time.Second {
			t.Errorf("%s: %d bytes took %v to decide", name, len(large), b)
		}
	}
}
