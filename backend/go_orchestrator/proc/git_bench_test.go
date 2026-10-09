package proc

import (
	"io"
	"testing"
	"time"
)

// BenchmarkGitReadsAB measures what reading the repository's config once
// per operation saves, on the read sequences get_diff and verify's git state
// make. Every read but rev-parse, rev-list and log first lists the
// repository's filter config, because no flag disables a filter driver; made
// as one-off GitCommand calls, get_diff paid that listing four times.
//
// Sides: each sequence as one-off reads (a config listing per read), and the
// same sequence through one GitSession (one listing, re-read only when the
// repository's config changes). Rounds are interleaved and rotate which side
// goes first, so drift in clock speed or cache state lands on every side;
// each side reports its fastest pass (min of N). Run it once:
//
//	go test -run '^$' -bench GitReadsAB -benchtime 1x ./proc
func BenchmarkGitReadsAB(b *testing.B) {
	const rounds, passes = 9, 3
	root := gitRepo(b)
	sequences := map[string][][]string{
		"get_diff": {
			{"diff"},
			{"diff", "--numstat"},
			{"diff", "--name-status", "-z"},
			{"ls-files", "--others", "--exclude-standard", "-z"},
		},
		"verify": {
			{"diff", "HEAD", "--name-only", "-z"},
			{"ls-files", "--others", "--exclude-standard", "-z"},
			{"diff", "HEAD"},
		},
	}
	run := func(session func() *GitSession, reads [][]string) func() {
		return func() {
			git := session()
			for _, args := range reads {
				if git == nil {
					cmd, err := GitCommand(b.Context(), root, args...)
					if err != nil {
						b.Fatal(err)
					}
					cmd.Stdout = io.Discard
					if err := cmd.Run(); err != nil {
						b.Fatal(err)
					}
					continue
				}
				cmd, err := git.Command(b.Context(), args...)
				if err != nil {
					b.Fatal(err)
				}
				cmd.Stdout = io.Discard
				if err := cmd.Run(); err != nil {
					b.Fatal(err)
				}
			}
		}
	}
	oneOff := func() *GitSession { return nil }
	perOperation := func() *GitSession {
		git, err := NewGitSession(root)
		if err != nil {
			b.Fatal(err)
		}
		return git
	}
	type side struct {
		name string
		run  func()
		best time.Duration
	}
	var sides []side
	for _, name := range []string{"get_diff", "verify"} {
		sides = append(sides,
			side{name: name + "-one-off", run: run(oneOff, sequences[name])},
			side{name: name + "-session", run: run(perOperation, sequences[name])},
		)
	}
	for i := range sides {
		sides[i].best = time.Duration(1<<63 - 1)
		sides[i].run() // warm the version cache and the page cache alike
	}
	b.ResetTimer()
	for r := 0; r < rounds; r++ {
		for k := range sides {
			s := &sides[(r+k)%len(sides)]
			for j := 0; j < passes; j++ {
				start := time.Now()
				s.run()
				if d := time.Since(start); d < s.best {
					s.best = d
				}
			}
		}
	}
	b.StopTimer()
	ms := map[string]float64{}
	for _, s := range sides {
		ms[s.name] = float64(s.best.Microseconds()) / 1000
		b.ReportMetric(ms[s.name], s.name+"-ms")
	}
	b.Logf("%d interleaved rounds x %d passes, min of %d per side", rounds, passes, rounds*passes)
	for _, name := range []string{"get_diff", "verify"} {
		b.Logf("%s (%d reads): one-off %.1f ms, session %.1f ms (%.2fx)", name, len(sequences[name]),
			ms[name+"-one-off"], ms[name+"-session"], ms[name+"-one-off"]/ms[name+"-session"])
	}
}
