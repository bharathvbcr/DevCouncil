//go:build unix

package gussetfn

import (
	"context"
	"encoding/binary"
	"errors"
	"fmt"
	"hash/maphash"
	"math"
	"slices"
	"sync"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/fnmatch"
	"github.com/bharathvbcr/gusset"
)

// Opcodes for prepared pattern lists (OPCODE_PREPARE and
// OPCODE_MATCH_PREPARED in the umbrella).
const (
	opcodePrepare       uint32 = 2
	opcodeMatchPrepared uint32 = 3
)

// registryFull is the umbrella's REGISTRY_FULL: the list was not prepared,
// and is asked with a match-any frame instead.
const registryFull uint32 = math.MaxUint32

// maxPrepared bounds the lists remembered here, matching MAX_SETS in the
// umbrella. Past it a list is still answered by the engine, one match-any
// frame per question.
const maxPrepared = 4096

// Query is one MatchBatch question: does the name match any of Patterns,
// case-folded when Fold is set — fnmatch.MatchAnyFold rather than
// fnmatch.MatchAny.
type Query struct {
	Patterns []string
	Fold     bool
}

// list is a pattern list as the engine holds it.
//
// Go settles what fnmatch decides without walking, once, when the list is
// first seen: a pattern past fnmatch's cap makes a case-folded list a match
// for every name and drops out of a case-sensitive one. What is left is
// folded (when Fold) and normalized, and prepared on the engine under id.
type list struct {
	patterns []string // the caller's list, compared on every lookup
	fold     bool
	always   bool // answers true for every name
	empty    bool // answers false for every name
	prepared bool // id names it on the engine; false sends kept per question
	id       uint32
	kept     []string
}

var lists struct {
	sync.Mutex
	seed   maphash.Seed
	byHash map[uint64][]*list
	n      int
}

func init() {
	lists.seed = maphash.MakeSeed()
	lists.byHash = make(map[uint64][]*list)
}

func listHash(patterns []string, fold bool) uint64 {
	var h maphash.Hash
	h.SetSeed(lists.seed)
	if fold {
		h.WriteByte(1)
	} else {
		h.WriteByte(0)
	}
	var n [4]byte
	for _, p := range patterns {
		binary.LittleEndian.PutUint32(n[:], uint32(len(p)))
		h.Write(n[:])
		h.WriteString(p)
	}
	return h.Sum64()
}

// lookup returns patterns as the engine holds them, preparing it on first
// sight. The answer is keyed on content, never on the slice's identity: a
// list rebuilt per call, or a backing array reused for another list, finds
// what its patterns are, not what a pointer used to hold.
func lookup(ctx context.Context, patterns []string, fold bool) (*list, error) {
	key := listHash(patterns, fold)
	lists.Lock()
	for _, l := range lists.byHash[key] {
		if l.fold == fold && slices.Equal(l.patterns, patterns) {
			lists.Unlock()
			return l, nil
		}
	}
	lists.Unlock()

	l := &list{patterns: slices.Clone(patterns), fold: fold}
	for _, p := range patterns {
		if fnmatch.Oversized(p) {
			if fold {
				l.always = true
				break
			}
			continue
		}
		if fold {
			p = fnmatch.Fold(p)
		}
		l.kept = append(l.kept, normalize(p))
	}
	l.empty = !l.always && len(l.kept) == 0
	if !l.always && !l.empty {
		if frame, ok := encodePrepare(l.kept); ok {
			out, err := call(gusset.ContextWithOpcode(ctx, opcodePrepare), frame)
			if err != nil {
				return nil, err
			}
			if len(out) != 4 {
				return nil, fmt.Errorf("gusset: prepare returned %d bytes, want 4", len(out))
			}
			if id := binary.LittleEndian.Uint32(out); id != registryFull {
				l.id, l.prepared = id, true
			}
		}
	}

	lists.Lock()
	defer lists.Unlock()
	for _, other := range lists.byHash[key] {
		if other.fold == fold && slices.Equal(other.patterns, patterns) {
			return other, nil
		}
	}
	if lists.n < maxPrepared {
		lists.byHash[key] = append(lists.byHash[key], l)
		lists.n++
	}
	return l, nil
}

// encodePrepare builds a prepare frame for kept, or reports that it does not
// fit one: a list past the umbrella's pattern count or the frame bound is
// asked with match-any frames instead, which split it.
func encodePrepare(kept []string) ([]byte, bool) {
	if len(kept) > maxPatterns {
		return nil, false
	}
	size := 4
	for _, p := range kept {
		size += 4 + len(p)
	}
	if size > maxFrameBytes {
		return nil, false
	}
	buf := make([]byte, 0, size)
	buf = binary.LittleEndian.AppendUint32(buf, uint32(len(kept)))
	for _, p := range kept {
		buf = appendField(buf, p)
	}
	return buf, true
}

// MatchBatch answers several questions about one name, each exactly as
// fnmatch.MatchAny or fnmatch.MatchAnyFold answers it, or an error.
//
// Each pattern list is prepared on the engine once per process — folded,
// normalized and compiled there — and every prepared question in the batch
// crosses together, in one call per maxPatterns of them. A list the engine
// could not hold is asked with its own match-any frames.
func MatchBatch(ctx context.Context, name string, qs []Query) ([]bool, error) {
	if ctx == nil {
		return nil, errors.New("gusset: nil context")
	}
	if err := ctx.Err(); err != nil {
		// Refused on a dead context even when the answer needs no crossing.
		return nil, err
	}
	out := make([]bool, len(qs))
	if fnmatch.Oversized(name) {
		for i, q := range qs {
			out[i] = len(q.Patterns) > 0 && q.Fold
		}
		return out, nil
	}
	type pending struct {
		at   int
		list *list
	}
	var batch []pending
	for i, q := range qs {
		if len(q.Patterns) == 0 {
			continue
		}
		l, err := lookup(ctx, q.Patterns, q.Fold)
		if err != nil {
			return nil, err
		}
		switch {
		case l.always:
			out[i] = true
		case l.empty:
		case l.prepared:
			batch = append(batch, pending{at: i, list: l})
		default:
			ok, err := askKept(ctx, l.kept, foldedName(name, l.fold), l.fold)
			if err != nil {
				return nil, err
			}
			out[i] = ok
		}
	}
	if len(batch) == 0 {
		return out, nil
	}
	raw, folded := normalize(name), normalize(fnmatch.Fold(name))
	actx := gusset.ContextWithOpcode(ctx, opcodeMatchPrepared)
	for start := 0; start < len(batch); start += maxPatterns {
		part := batch[start:min(start+maxPatterns, len(batch))]
		frame := make([]byte, 0, 12+len(raw)+len(folded)+5*len(part))
		frame = appendField(frame, raw)
		frame = appendField(frame, folded)
		frame = binary.LittleEndian.AppendUint32(frame, uint32(len(part)))
		for _, p := range part {
			frame = binary.LittleEndian.AppendUint32(frame, p.list.id)
			if p.list.fold {
				frame = append(frame, 1)
			} else {
				frame = append(frame, 0)
			}
		}
		answers, err := call(actx, frame)
		if err != nil {
			return nil, err
		}
		if len(answers) != len(part) {
			return nil, fmt.Errorf("gusset: engine returned %d answers for %d lists", len(answers), len(part))
		}
		for j, p := range part {
			a, err := answer(answers[j : j+1])
			if err != nil {
				return nil, err
			}
			out[p.at] = a == match || (a == undecided && p.list.fold)
		}
	}
	return out, nil
}

func foldedName(name string, fold bool) string {
	if fold {
		name = fnmatch.Fold(name)
	}
	return normalize(name)
}
