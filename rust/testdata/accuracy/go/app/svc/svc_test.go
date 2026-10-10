package svc

import "testing"

// TestCaller01Table is a table test (C21) reaching Caller01 from t.Run.
func TestCaller01Table(t *testing.T) {
	tests := []struct {
		name string
		want int
	}{{"one", 1}}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := Caller01(); got != tt.want {
				t.Fatalf("got %d", got)
			}
		})
	}
}
