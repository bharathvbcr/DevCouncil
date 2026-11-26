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
		at, unknown := argumentCommandWordPeeled(words)
		if unknown != "" || at < 0 || words[at].text != want {
			t.Errorf("%q: got at=%d unknown=%q, want command %q", line, at, unknown, want)
		}
	}
	for line, want := range map[string]string{"env --frobnicate cat x": "--frobnicate", "sudo -e .env": "-e", "nice -z cat x": "-z", "nohup -x cat x": "-x"} {
		if at, unknown := argumentCommandWordPeeled(shellWords(line)); at != -1 || unknown != want {
			t.Errorf("%q: got at=%d unknown=%q, want unpeelable option %q", line, at, unknown, want)
		}
	}
	for _, line := range []string{"env", "sudo", "command -v cat", "env FOO=1"} {
		if at, unknown := argumentCommandWordPeeled(shellWords(line)); at != -1 || unknown != "" {
			t.Errorf("%q: got at=%d unknown=%q, want nothing run", line, at, unknown)
		}
	}
}
