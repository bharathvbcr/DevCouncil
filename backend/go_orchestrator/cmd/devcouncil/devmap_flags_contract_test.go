package main

import (
	"os/exec"
	"regexp"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// optionLine matches a clap option header in `--help`: `  -d, --db <DB>`,
// `      --json`.
var optionLine = regexp.MustCompile(`(?m)^\s+(?:-\w, )?(--[a-z][a-z-]*)( <[A-Z_]+>)?\s*$`)

// The flag tables in main.go let `devcouncil map` peel devmap's global flags
// off argv before it decides what the subcommand is. They are a copy, so they
// are pinned to the producer: every global the real devmap advertises must be
// in the table with the right arity, and the table may hold nothing devmap does
// not advertise. A flag added to devmap without a line here fails this test
// rather than being mistaken for a subcommand at runtime.
func TestDevmapGlobalFlagTablesMatchTheRealBinary(t *testing.T) {
	bin := testsupport.Devmap(t)
	out, err := exec.Command(bin, "--help").Output()
	if err != nil {
		t.Fatalf("devmap --help: %v", err)
	}
	advertised := map[string]bool{} // name -> takes a value
	for _, m := range optionLine.FindAllStringSubmatch(string(out), -1) {
		advertised[m[1]] = m[2] != ""
	}
	if len(advertised) < 4 {
		t.Fatalf("parsed %d options from devmap --help; the help format has moved:\n%s", len(advertised), out)
	}
	for name, takesValue := range advertised {
		_, isBool := devmapBoolFlags[name]
		_, isValue := devmapValueFlags[name]
		switch {
		case takesValue && !isValue:
			t.Errorf("devmap advertises %s <value> but devmapValueFlags lacks it (bool table has it: %v)", name, isBool)
		case !takesValue && !isBool:
			t.Errorf("devmap advertises %s but devmapBoolFlags lacks it (value table has it: %v)", name, isValue)
		}
	}
	for table, flags := range map[string]map[string]struct{}{"devmapBoolFlags": devmapBoolFlags, "devmapValueFlags": devmapValueFlags} {
		for name := range flags {
			if _, ok := advertised[name]; !ok && !shortAlias(name) {
				t.Errorf("%s holds %s, which devmap no longer advertises", table, name)
			}
		}
	}
}

// shortAlias is a one-letter alias (-d, -v, -h, -V): the help lists the long
// name, and the alias rides on it.
func shortAlias(name string) bool { return len(name) == 2 && name[0] == '-' && name[1] != '-' }
