package svc

import (
	"bytes"

	"example.com/app/store"
)

// Q1: a local whose value comes from a *method* call (not a package func).
type factory struct{}

func (factory) Make() *store.Store { return store.NewStore() }

func methodReturnCaller() int {
	var f factory
	s := f.Make()
	return s.ElemGet24()
}

// Q2: a function-local type is used only inside its function: not dead.
func localTypeUser() int {
	type item struct{ n int }
	items := []item{{n: 1}}
	return len(items)
}

// Q2b: the same local type, later shadowed by a range variable of the same name.
func shadowedLocalTypeUser() int {
	type entry struct{ n int }
	entries := make([]entry, 0, 1)
	entries = append(entries, entry{n: 1})
	total := 0
	for _, entry := range entries {
		total += entry.n
	}
	return total
}

// Q2c: a second function in the same file declares a local type with the same name.
func twinLocalTypeUser() int {
	type entry struct{ n int }
	entries := make([]entry, 0, 1)
	entries = append(entries, entry{n: 2})
	total := 0
	for _, entry := range entries {
		total += entry.n
	}
	return total
}

// Q3: a bare call inside a method of the same name calls the package function, not the method.
func Rebuild(n int) int { return n }

// Holder has a Rebuild method that calls the package-level Rebuild.
type Holder struct{}

// Rebuild on Holder must not get a self-edge; package Rebuild must get the edge.
func (Holder) Rebuild() int { return Rebuild(1) }

// Q4: a method named like a builtin calls the builtin.
type cache struct{ m map[string]int }

func (c *cache) delete(k string) { delete(c.m, k) }

// Q5: a field typed by an external package's type named like an in-repo type.
// bytes.Buffer.Reset must not bind to any in-repo Buffer.Reset.
type Buffer struct{}

// Reset is the in-repo decoy that must NOT receive the edge.
func (Buffer) Reset() {}

type writer struct{ buf *bytes.Buffer }

func (w *writer) clear() { w.buf.Reset() }

// Probes2 keeps every probe reachable.
func Probes2() {
	_ = methodReturnCaller()
	_ = localTypeUser()
	_ = shadowedLocalTypeUser()
	_ = twinLocalTypeUser()
	_ = Holder{}.Rebuild()
	c := &cache{m: map[string]int{}}
	c.delete("k")
	w := &writer{buf: &bytes.Buffer{}}
	w.clear()
}
