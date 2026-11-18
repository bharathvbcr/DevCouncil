package main

import "testing"

// The tool command's stdout is the MCP answer text; a terminal must not get a
// framed or wrapped rendering of it.
func TestToolOutputIsNeverRenderedForAPerson(t *testing.T) {
	for _, argv := range [][]string{
		{"tool", "call", "devcouncil_next_task"},
		{"tool", "list"},
	} {
		_, policy, err := presentationArgs(argv)
		if err != nil {
			t.Fatal(err)
		}
		if !policy.JSON {
			t.Errorf("%v: presentation policy is not JSON, so a terminal would frame the answer", argv)
		}
	}
}
