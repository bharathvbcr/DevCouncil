package gate

import (
	"strings"
	"testing"
	"time"
)

// Deciding a command is linear in its length, or refused past a depth cap. The
// audit measured ~4x per doubling on nested `$((` and `($(`: a 125 KB command
// took 21 s to refuse. The assertion is on the ratio between a 32 KB and a
// 120 KB command (3.75x the length), measured in process CPU time so a loaded
// runner does not move it. Linear is 3.75, n^1.5 is 7.3 and quadratic 14, so
// the limit of 6 separates linear from n^1.5. A single decision of the small
// command is ~15 ms, where timer and GC granularity is tens of percent, so each
// sample is a batch of decisions that spans at least minSampleCPU, reported per
// call, and each size takes the best of three batches.
const minSampleCPU = 250 * time.Millisecond

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
	// batch decides the command reps times on fresh gates (built outside the
	// timed span) and returns the CPU time per decision.
	batch := func(command string, reps int) time.Duration {
		gates := make([]*Gate, reps)
		for i := range gates {
			gates[i] = newGate(t, nil)
		}
		start := processCPU()
		for _, g := range gates {
			if _, err := g.EvaluateCommand(command, boundsTask()); err != nil {
				t.Fatal(err)
			}
		}
		return (processCPU() - start) / time.Duration(reps)
	}
	decide := func(command string) time.Duration {
		// Size the batch from one decision so a sample spans >= minSampleCPU.
		reps := int(minSampleCPU/max(batch(command, 1), time.Microsecond)) + 1
		reps = min(reps, 500)
		best := time.Duration(1<<63 - 1)
		for i := 0; i < 3; i++ {
			best = min(best, batch(command, reps))
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
			t.Errorf("%s: deciding %d bytes took %v and %d bytes took %v (ratio %.1f for 3.75x the input); "+
				"cost is super-linear in an input the caller controls", name, len(small), a, len(large), b, ratio)
		}
		// The ratio is what proves linearity; this ceiling (CPU time per
		// decision) only catches a uniformly slow decision. It sits under the
		// audited 21 s regression and well over what a shared CI runner takes
		// (alternating measured 3.8 s there, 1 s per 32 KB).
		if b > 15*time.Second {
			t.Errorf("%s: %d bytes took %v of CPU to decide", name, len(large), b)
		}
	}
}
