package dcgrep

import "testing"

func TestAnEmptyWriteOnAFullBufferIsNotAnOverflow(t *testing.T) {
	b := &cappedBuffer{limit: 4}
	_, _ = b.Write([]byte("abcd"))
	_, _ = b.Write(nil)
	if b.overflow {
		t.Fatal("an empty write on a full buffer dropped nothing, but overflow was recorded")
	}
}
