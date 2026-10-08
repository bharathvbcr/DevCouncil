package proc

import "testing"

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
