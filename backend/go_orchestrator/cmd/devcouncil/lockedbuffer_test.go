package main

import (
	"bytes"
	"strings"
	"sync"
)

// lockedBuffer is an io.Writer the server goroutine writes while the test
// reads. It has no build constraint because both the unix-only tool tests and
// the portable mcp CLI tests use it.
type lockedBuffer struct {
	mu  sync.Mutex
	buf bytes.Buffer
}

func (b *lockedBuffer) Write(p []byte) (int, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.buf.Write(p)
}

func (b *lockedBuffer) lines() []string {
	b.mu.Lock()
	defer b.mu.Unlock()
	return strings.Split(b.buf.String(), "\n")
}
