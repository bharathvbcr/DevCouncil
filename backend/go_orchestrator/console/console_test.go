package console

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"strings"
	"sync"
	"testing"
	"time"
	"unicode/utf8"
)

type stalledWriter struct {
	entered, release chan struct{}
	once             sync.Once
}

func (w *stalledWriter) Write(p []byte) (int, error) {
	w.once.Do(func() { close(w.entered) })
	<-w.release
	return len(p), nil
}

type shortWriter struct{}

func (shortWriter) Write(p []byte) (int, error) { return 0, nil }

func TestBlockedDiagnosticsAreBoundedAndReported(t *testing.T) {
	var out bytes.Buffer
	stalled := &stalledWriter{entered: make(chan struct{}), release: make(chan struct{})}
	s := New(&out, stalled, Policy{JSON: true, Progress: "always", Activity: "Working"}, context.Background())
	defer close(stalled.release)
	select {
	case <-stalled.entered:
	case <-time.After(time.Second):
		t.Fatal("worker did not start")
	}
	start := time.Now()
	for i := 0; i < 10000; i++ {
		_, _ = (diagnosticWriter{s}).Write([]byte(strings.Repeat("x", 5000)))
	}
	if code := s.Finish(1); code != 1 {
		t.Fatalf("exit %d", code)
	}
	if time.Since(start) > 2*time.Second {
		t.Fatal("optional output blocked the result")
	}
	var receipt struct {
		OK          bool
		Diagnostics []string
		Dropped     uint64 `json:"dropped_output_updates"`
	}
	if err := json.Unmarshal(out.Bytes(), &receipt); err != nil {
		t.Fatal(err)
	}
	if receipt.OK || receipt.Dropped == 0 || len(receipt.Diagnostics) != 64 {
		t.Fatalf("incomplete disclosure: %+v", receipt)
	}
	for _, note := range receipt.Diagnostics {
		if len(note) > 4200 {
			t.Fatal("unbounded diagnostic")
		}
	}
}

func TestCheckedResultAndDiagnosticShortWrites(t *testing.T) {
	s := New(shortWriter{}, io.Discard, Policy{}, context.Background())
	if _, err := (outputWriter{s}).Write([]byte("result")); err != io.ErrShortWrite {
		t.Fatalf("%v", err)
	}
	if s.Finish(0) == 0 {
		t.Fatal("failed output reported success")
	}
	var out bytes.Buffer
	s = New(&out, shortWriter{}, Policy{JSON: true, Progress: "always"}, context.Background())
	if s.Finish(0) != 0 {
		t.Fatal("optional write failed command")
	}
	if s.lost.Load() == 0 {
		t.Fatal("diagnostic short write was not disclosed")
	}
}

func TestHumanOutputPreservesUnicodeAcrossBufferBoundaries(t *testing.T) {
	var out bytes.Buffer
	s := New(&out, io.Discard, Policy{}, context.Background())
	s.clear()
	close(s.stop)
	<-s.done
	s.human = true
	input := strings.Repeat("x", 4095) + "🙂日本語" + strings.Repeat("y", 5000)
	if _, err := (outputWriter{s}).Write([]byte(input)); err != nil {
		t.Fatal(err)
	}
	if !utf8.Valid(out.Bytes()) || out.String() != input {
		t.Fatal("a buffer boundary corrupted UTF-8")
	}
}

func TestFramesFitAllWidthsAndEscapeControls(t *testing.T) {
	for width := 0; width <= 200; width++ {
		for tick := 0; tick < 100; tick++ {
			value := frame("working\x1b[31m\r\n日本語", tick, width, false, false, time.Second)
			if strings.ContainsAny(value, "\x1b\r\n") {
				t.Fatalf("terminal control in frame: %q", value)
			}
			cells := 0
			for _, r := range value {
				cells++
				if r > 127 {
					cells++
				}
			}
			if cells > max(0, width-1) {
				t.Fatalf("width %d: %q", width, value)
			}
		}
	}
}

func TestHumanOutputPreservesUnicodeAcrossWrites(t *testing.T) {
	var out bytes.Buffer
	s := New(&out, io.Discard, Policy{}, context.Background())
	s.clear()
	close(s.stop)
	<-s.done
	s.human = true
	input := []byte("🙂日本語")
	for _, b := range input {
		if _, err := (outputWriter{s}).Write([]byte{b}); err != nil {
			t.Fatal(err)
		}
	}
	if out.String() != string(input) {
		t.Fatalf("split writes corrupted Unicode: %q", out.String())
	}
}

func TestInteractiveInstallerLogsLeaveTheLoaderActive(t *testing.T) {
	var output, diagnostics bytes.Buffer
	s := New(&output, &diagnostics, Policy{Activity: "Preparing tools"}, context.Background())
	s.human = true
	if !active.CompareAndSwap(nil, s) {
		t.Fatal("another output scope is active")
	}
	defer active.Store(nil)
	if _, err := ChildOutput().Write([]byte("installer work\n")); err != nil {
		t.Fatal(err)
	}
	if s.quiet.Load() {
		t.Error("installer logs stopped the loader before installation completed")
	}
	if s.Finish(0) != 0 {
		t.Fatal("installer log delivery failed command")
	}
	if strings.Contains(output.String(), "installer work") || !strings.Contains(diagnostics.String(), "installer work") {
		t.Fatal("interactive installer logs did not use the diagnostic stream")
	}
}
