package devmap

import (
	"errors"
	"strings"
	"testing"
)

func TestPredatesAgentsStdin(t *testing.T) {
	err := predatesFlag(errors.New("exit status 2"), []byte("error: unexpected argument '--agents-stdin' found\n"))
	if !strings.Contains(err.Error(), "predates --agents-stdin") {
		t.Fatal(err)
	}
}
