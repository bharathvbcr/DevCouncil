package proc

import (
	"bytes"
	"sync"
)

// CappedBuffer collects a child's output up to Limit bytes and records that it
// stopped, so an overrun is a reported condition rather than an allocation the
// size of whatever the child chose to print.
//
// It caps during the copy rather than checking after it, and it always reports
// a full write. Returning short would make os/exec's copier treat the cap as
// io.ErrShortWrite, close the pipe and hand the child SIGPIPE, which turns a
// command that ran fine into a failure.
//
// It lives beside RunBounded for the same reason ConfigureGroup does: bounding
// a child's output is the same lesson at the same seam. It is the only output
// cap: TestNoPackageKeepsItsOwnCappedBuffer refuses a local copy.
//
// It is safe for concurrent use. exec.Cmd copies a child's output on its own
// goroutines, and RunBounded may return on a deadline while the child is still
// writing, so a reader can overlap a writer; every method takes the lock and
// Bytes returns a copy, so what a reader holds is not written under it.
type CappedBuffer struct {
	Limit    int
	mu       sync.Mutex
	buf      bytes.Buffer
	overflow bool
}

// Write keeps at most Limit bytes in total and never fails.
func (b *CappedBuffer) Write(p []byte) (int, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	if remaining := b.Limit - b.buf.Len(); remaining > 0 {
		if len(p) > remaining {
			b.buf.Write(p[:remaining])
			b.overflow = true
		} else {
			b.buf.Write(p)
		}
	} else if len(p) > 0 {
		b.overflow = true
	}
	return len(p), nil
}

// Bytes returns what was kept.
func (b *CappedBuffer) Bytes() []byte {
	b.mu.Lock()
	defer b.mu.Unlock()
	return bytes.Clone(b.buf.Bytes())
}

// String returns what was kept.
func (b *CappedBuffer) String() string {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.buf.String()
}

// Len reports how many bytes were kept.
func (b *CappedBuffer) Len() int {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.buf.Len()
}

// Overflowed reports whether any byte was dropped.
func (b *CappedBuffer) Overflowed() bool {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.overflow
}
