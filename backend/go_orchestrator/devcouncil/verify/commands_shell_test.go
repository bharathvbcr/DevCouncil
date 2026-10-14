package verify

import (
	"slices"
	"strings"
	"testing"
)

// The verification shell is chosen per GOOS. These run on every host because
// the choice is a pure function of the GOOS and the environment it is handed:
// a Windows-only test would be read by a CI that never runs it as a pass.

func shellEnvOf(vars map[string]string) func(string) string {
	return func(name string) string { return vars[name] }
}

func TestVerifyShellIsPOSIXShOffWindows(t *testing.T) {
	for _, goos := range []string{"linux", "darwin", "freebsd", "openbsd"} {
		shell, args, err := verifyShell(goos, shellEnvOf(nil), "go test ./...")
		if err != nil {
			t.Fatalf("%s: verifyShell refused an ordinary command: %v", goos, err)
		}
		if shell != "/bin/sh" || !slices.Equal(args, []string{"-c", "go test ./..."}) {
			t.Fatalf("%s: got %q %q, want /bin/sh -c <command>", goos, shell, args)
		}
	}
}

func TestVerifyShellOnWindowsIsCmdFromComSpec(t *testing.T) {
	for _, comspec := range []string{
		`C:\Windows\system32\cmd.exe`,
		`c:/windows/system32/cmd.exe`,
		`C:\Program Files\Shell\cmd.exe`,
		`\\server\share\cmd.exe`,
	} {
		shell, args, err := verifyShell("windows", shellEnvOf(map[string]string{
			"ComSpec":    comspec,
			"SystemRoot": `D:\Elsewhere`,
		}), "go test ./...")
		if err != nil {
			t.Fatalf("ComSpec %q: refused: %v", comspec, err)
		}
		if shell != comspec {
			t.Fatalf("ComSpec %q: shell = %q, want ComSpec itself", comspec, shell)
		}
		// /d skips the AutoRun registry hook, so what runs is the command and
		// nothing the host's registry adds; /s /c is the quoting rule the
		// command line below is built for.
		if !slices.Equal(args, []string{"/d", "/s", "/c", "go test ./..."}) {
			t.Fatalf("ComSpec %q: args = %q, want /d /s /c <command>", comspec, args)
		}
	}
}

func TestVerifyShellOnWindowsFallsBackToSystemRoot(t *testing.T) {
	shell, _, err := verifyShell("windows", shellEnvOf(map[string]string{
		"SystemRoot": `C:\Windows`,
	}), "echo ok")
	if err != nil {
		t.Fatalf("refused with SystemRoot set: %v", err)
	}
	if shell != `C:\Windows\System32\cmd.exe` {
		t.Fatalf("shell = %q, want SystemRoot's cmd.exe", shell)
	}
}

// Each refusal fails closed with a reason that names it. A relative shell is
// the dangerous one: exec resolves a name with a separator against cmd.Dir,
// which is the repository under verification, so `.\cmd.exe` would run a file
// the repository supplied.
func TestVerifyShellOnWindowsRefusesWhatItCannotRunSafely(t *testing.T) {
	for _, tc := range []struct {
		name    string
		env     map[string]string
		command string
		want    string
	}{
		{"no ComSpec and no SystemRoot", nil, "echo ok", "neither ComSpec nor SystemRoot"},
		{"bare name", map[string]string{"ComSpec": "cmd.exe"}, "echo ok", "not an absolute path"},
		{"dot relative", map[string]string{"ComSpec": `.\cmd.exe`}, "echo ok", "not an absolute path"},
		{"drive relative", map[string]string{"ComSpec": `C:cmd.exe`}, "echo ok", "not an absolute path"},
		{"rooted without drive", map[string]string{"ComSpec": `\Windows\cmd.exe`}, "echo ok", "not an absolute path"},
		{"relative SystemRoot", map[string]string{"SystemRoot": `Windows`}, "echo ok", "not an absolute path"},
		{"quote in ComSpec", map[string]string{"ComSpec": `C:\a"b\cmd.exe`}, "echo ok", "quote"},
		{"newline in command", map[string]string{"ComSpec": `C:\Windows\system32\cmd.exe`}, "echo a\necho b", "line break"},
		{"carriage return in command", map[string]string{"ComSpec": `C:\Windows\system32\cmd.exe`}, "echo a\recho b", "line break"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			_, _, err := verifyShell("windows", shellEnvOf(tc.env), tc.command)
			if err == nil {
				t.Fatalf("verifyShell accepted %v / %q, want a refusal", tc.env, tc.command)
			}
			if !strings.Contains(err.Error(), tc.want) {
				t.Fatalf("got %q, want it to mention %q", err, tc.want)
			}
		})
	}
}

// cmd.exe does not parse the backslash-escaping Go uses to build a Windows
// command line, so the line is written whole: with /s, cmd strips the first
// and last quote and runs everything between them as typed.
func TestWindowsShellCommandLineIsCmdSQuoting(t *testing.T) {
	got := windowsShellCommandLine(`C:\Program Files\cmd.exe`, `go test ./... && echo "done"`)
	want := `"C:\Program Files\cmd.exe" /d /s /c "go test ./... && echo "done""`
	if got != want {
		t.Fatalf("got  %s\nwant %s", got, want)
	}
}
