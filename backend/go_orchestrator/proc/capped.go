package proc

import "bytes"

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
// a child's output is the same lesson at the same seam. dc/store, dc/dcgrep,
// dc/dcverify and dc/devmap still carry their own copies; new call sites use
// this one.
type CappedBuffer struct {
	Limit    int
	buf      bytes.Buffer
	overflow bool
}

// Write keeps at most Limit bytes in total and never fails.
func (b *CappedBuffer) Write(p []byte) (int, error) {
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
func (b *CappedBuffer) Bytes() []byte { return b.buf.Bytes() }

// String returns what was kept.
func (b *CappedBuffer) String() string { return b.buf.String() }

// Len reports how many bytes were kept.
func (b *CappedBuffer) Len() int { return b.buf.Len() }

// Overflowed reports whether any byte was dropped.
func (b *CappedBuffer) Overflowed() bool { return b.overflow }
