package proc

import (
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
)

func TestCappedBufferHoldsTheCapAndReportsFullWrites(t *testing.T) {
	b := &CappedBuffer{Limit: 10}

	if n, err := b.Write([]byte("12345")); n != 5 || err != nil {
		t.Fatalf("a write under the cap must pass through: n=%d err=%v", n, err)
	}
	if b.Overflowed() {
		t.Fatal("nothing was dropped yet")
	}
	// An empty write at any point is not an overflow.
	if n, err := b.Write(nil); n != 0 || err != nil || b.Overflowed() {
		t.Fatalf("an empty write: n=%d err=%v overflow=%v", n, err, b.Overflowed())
	}
	// Straddles the cap: five bytes fit, four do not, and the caller is still
	// told all nine were written.
	if n, err := b.Write([]byte("6789abcde")); n != 9 || err != nil {
		t.Fatalf("a straddling write must report complete: n=%d err=%v", n, err)
	}
	if !b.Overflowed() {
		t.Fatal("dropping bytes must be recorded")
	}
	if got := b.String(); got != "123456789a" {
		t.Fatalf("kept %q, want the first ten bytes", got)
	}
	if n, err := b.Write([]byte("more")); n != 4 || err != nil || b.Len() != 10 {
		t.Fatalf("a write past the cap: n=%d err=%v kept=%d", n, err, b.Len())
	}
}

func TestCappedBufferAtExactlyTheLimitIsNotAnOverflow(t *testing.T) {
	b := &CappedBuffer{Limit: 4}
	_, _ = b.Write([]byte("abcd"))
	if b.Overflowed() {
		t.Fatal("filling the buffer exactly drops nothing")
	}
	_, _ = b.Write(nil)
	if b.Overflowed() {
		t.Fatal("an empty write on a full buffer drops nothing")
	}
	_, _ = b.Write([]byte("e"))
	if !b.Overflowed() {
		t.Fatal("one byte past the limit is dropped and must be recorded")
	}
}

// proc.CappedBuffer is the one output cap. A local copy is how the empty-write
// overflow defect reached four packages with two different semantics, so a
// second writer type that tracks an overflow/truncation flag is refused here.
func TestNoPackageKeepsItsOwnCappedBuffer(t *testing.T) {
	root, err := filepath.Abs("../..")
	if err != nil {
		t.Fatal(err)
	}
	local := regexp.MustCompile(`(?m)^type (cappedBuffer|capped|limitWriter|limitedWriter)\s+struct`)
	var offenders []string
	err = filepath.WalkDir(root, func(path string, d os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() || !strings.HasSuffix(path, ".go") || strings.HasSuffix(path, "_test.go") {
			return nil
		}
		body, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		if m := local.FindString(string(body)); m != "" {
			rel, _ := filepath.Rel(root, path)
			offenders = append(offenders, rel+": "+m)
		}
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	if len(offenders) > 0 {
		t.Fatalf("use proc.CappedBuffer instead of a local copy:\n  %s", strings.Join(offenders, "\n  "))
	}
}
