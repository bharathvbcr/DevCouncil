package policy

import "testing"

func TestWrapperSpecsAreCommandWrappers(t *testing.T) {
	for name := range wrapperSpecs {
		if !commandWrappers[name] {
			t.Errorf("wrapperSpecs[%q] is not in commandWrappers, the one wrapper list", name)
		}
	}
}

func TestPeelWrapperFindsWrappedCommand(t *testing.T) {
	for line, want := range map[string]string{
		"env -i cat .env": "cat", "env -u HOME cat x": "cat", "env FOO=1 cat x": "cat", "sudo -u root cat x": "cat",
		"sudo -E cat x": "cat", "nice -n 5 cat x": "cat", "nice -5 cat x": "cat", "nice -n5 cat x": "cat",
		"timeout 5 cat x": "cat", "timeout -s KILL 5 cat x": "cat", "timeout --signal=KILL 5 cat x": "cat",
		"stdbuf -o0 cat x": "cat", "stdbuf -o 0 cat x": "cat", "ionice -c3 cat x": "cat", "env -i sudo -u r nice -n 1 cat x": "cat",
		"env -- cat x": "cat", "command -p cat x": "cat",
	} {
		words := shellWords(line)
		at, unknown := peelCommandPair(words)
		if unknown != "" || at < 0 || words[at].text != want {
			t.Errorf("%q: got at=%d unknown=%q, want command %q", line, at, unknown, want)
		}
	}
	for line, want := range map[string]string{"env --frobnicate cat x": "--frobnicate", "nice -z cat x": "-z", "nohup -x cat x": "-x"} {
		words := shellWords(line)
		if at, unknown := peelCommandPair(words); unknown != want || at < 0 || words[at].text != "cat" {
			t.Errorf("%q: got at=%d unknown=%q, want unknown option %q and the guessed command cat", line, at, unknown, want)
		}
	}
	for line, want := range map[string]string{"env -C .git rm x": "-C", "sudo -D d ls": "-D", "sudo --chroot=/ ls": "--chroot"} {
		if p := peelCommand(shellWords(line)); p.chdir != want || p.at < 0 {
			t.Errorf("%q: got chdir=%q at=%d, want chdir %q", line, p.chdir, p.at, want)
		}
	}
	if p := peelCommand(shellWords("time -a -o out.txt true")); len(p.writes) != 1 || p.writes[0] != "out.txt" {
		t.Errorf("time -o: got writes %v", p.writes)
	}
	if p := peelCommand(shellWords("sudo -e a b")); len(p.writes) != 2 || p.at != -1 {
		t.Errorf("sudo -e: got writes %v at=%d, want both operands as writes", p.writes, p.at)
	}
	if p := peelCommand(shellWords("env -S 'cat .env' x")); p.reparse != "cat .env" || len(p.reparseRest) != 1 {
		t.Errorf("env -S: got reparse %q rest %d", p.reparse, len(p.reparseRest))
	}
	for _, line := range []string{"env", "sudo", "command -v cat", "env FOO=1"} {
		if at, unknown := peelCommandPair(shellWords(line)); at != -1 || unknown != "" {
			t.Errorf("%q: got at=%d unknown=%q, want nothing run", line, at, unknown)
		}
	}
}

func peelCommandPair(words []shellWord) (int, string) {
	p := peelCommand(words)
	return p.at, p.unknown
}
