package devmap

import (
	"context"
	"errors"
	"slices"
	"strings"
	"testing"
)

func TestPredatesAgentsStdin(t *testing.T) {
	err := predatesFlag(errors.New("exit status 2"), []byte("error: unexpected argument '--agents-stdin' found\n"))
	if !strings.Contains(err.Error(), "predates --agents-stdin") {
		t.Fatal(err)
	}
}

// Integrate passes the command to write as `--binary` only when one is set;
// unset, the argv is what it was before the field existed, and devmap names
// itself.
func TestIntegrateSendsBinaryOnlyWhenSet(t *testing.T) {
	const written = "/opt/devcouncil-test/bin/devmap"
	for _, tc := range []struct {
		command string
		want    []string
	}{
		{"", nil},
		{written, []string{"--binary", written}},
	} {
		c, argv := recording(t, map[string]string{"integrate": `{"check_ok":true}`})
		if _, err := c.Integrate(context.Background(), "claude", InstallCheck, nil, tc.command); err != nil {
			t.Fatal(err)
		}
		call := find(t, invocations(t, argv), "integrate")
		want := append([]string{"--json", "integrate", "claude", "--project-root", c.Root}, tc.want...)
		want = append(want, "--check")
		if !slices.Equal(call, want) {
			t.Errorf("command %q: argv %q, want %q", tc.command, call, want)
		}
	}
}
