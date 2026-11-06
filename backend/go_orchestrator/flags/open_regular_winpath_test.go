package flags

import (
	"errors"
	"math/rand"
	"runtime"
	"strings"
	"testing"
)

func TestWindowsNonRegularKindTable(t *testing.T) {
	cases := []struct {
		path string
		kind string
		ok   bool
	}{
		{`\\.\pipe\foo`, "named pipe", true},
		{`//./pipe/foo`, "named pipe", true},
		{`\\.\PIPE\Foo`, "named pipe", true},
		{`\\.\pipe`, "named pipe", true},
		{`\\.\pipe\`, "named pipe", true},
		{`\\.\pipe\帮助`, "named pipe", true},
		{`\\.\pipe.\foo`, "named pipe", true},
		{`\\.\pipe \foo`, "named pipe", true},
		{`\\.\pipe:stream`, "named pipe", true},
		{`\\.\pipe\foo\..\..\C$\windows`, "named pipe", true},
		{`\\?\pipe\foo`, "named pipe", true},
		{`//?/pipe/foo`, "named pipe", true},
		{`\\server\pipe`, "named pipe", true},
		{`\\server\pipe\`, "named pipe", true},
		{`\\server\pipe:stream`, "named pipe", true},
		{`\\.\pipe.`, "named pipe", true},
		{`\\server\pipe\foo`, "named pipe", true},
		{`//server/pipe/foo`, "named pipe", true},
		{`\\server\PIPE\foo`, "named pipe", true},
		{`\\server\pipe.\foo`, "named pipe", true},
		{`\\?\UNC\server\pipe\foo`, "named pipe", true},
		{`//?/unc/server/pipe/foo`, "named pipe", true},
		{`\\.\UNC\server\pipe\foo`, "named pipe", true},
		{`\\.\GLOBALROOT\Device\NamedPipe\foo`, "named pipe", true},
		{`\\?\GLOBALROOT\Device\NamedPipe\foo`, "named pipe", true},
		{`\Device\NamedPipe\foo`, "named pipe", true},
		{`\\.\mailslot\foo`, "non-regular file", true},
		{`\\?\mailslot\foo`, "non-regular file", true},
		{`\\server\mailslot\foo`, "non-regular file", true},
		{`\\.\COM1`, "non-regular file", true},
		{`\\.\NUL`, "non-regular file", true},
		{`\\.\PhysicalDrive0`, "non-regular file", true},
		{`\\.\pipeline\x`, "non-regular file", true},
		{`\\.\GLOBALROOT\Device\Mailslot\foo`, "non-regular file", true},
		{`\\.\C:\ok.yaml`, "", false},
		{`\\.\c:\ok.yaml`, "", false},
		{`\\.\C:`, "", false},
		{`//./C:/ok.yaml`, "", false},
		{`\\?\C:\ok.yaml`, "", false},
		{`\\?\UNC\server\share\ok.yaml`, "", false},
		{`\\.\UNC\server\share\ok.yaml`, "", false},
		{`\\server\share\config.yaml`, "", false},
		{`\\server\pipeline\x`, "", false},
		{`C:\device\namedpipe\cfg.yaml`, "", false},
		{`C:\pipe\config.yaml`, "", false},
		{`config.yaml`, "", false},
		{`\\.\pipe`, "named pipe", true},
		{"", "", false},
		{`\\.\C:foo`, "non-regular file", true},
	}
	for _, tc := range cases {
		t.Run(tc.path, func(t *testing.T) {
			kind, ok := windowsNonRegularKind(tc.path)
			if ok != tc.ok || kind != tc.kind {
				t.Fatalf("windowsNonRegularKind(%q) = %q, %v; want %q, %v", tc.path, kind, ok, tc.kind, tc.ok)
			}
		})
	}
}

func TestWindowsNonRegularKindIsStableUnderHostileSuffixes(t *testing.T) {
	rng := rand.New(rand.NewSource(1))
	const alphabet = "abcABC._-帮助 /:\\"
	for i := 0; i < 200; i++ {
		suffix := randomPathSuffix(rng, alphabet, 1+rng.Intn(64))
		pipe := `\\.\pipe\` + suffix
		kind, ok := windowsNonRegularKind(pipe)
		if !ok || kind != "named pipe" {
			t.Fatalf("pipe suffix %q => %q %v", suffix, kind, ok)
		}
		drive := `\\.\C:\` + suffix
		if _, refused := windowsNonRegularKind(drive); refused {
			t.Fatalf("drive path refused: %q", drive)
		}
		ordinary := `C:\work\` + suffix
		if _, refused := windowsNonRegularKind(ordinary); refused {
			t.Fatalf("ordinary path refused: %q", ordinary)
		}
	}
	long := `\\.\pipe\` + strings.Repeat("a", 40000)
	kind, ok := windowsNonRegularKind(long)
	if !ok || kind != "named pipe" {
		t.Fatalf("long pipe path => %q %v", kind, ok)
	}
	nul := "\\\\.\\pipe\\\x00name"
	kind, ok = windowsNonRegularKind(nul)
	if !ok || kind != "named pipe" {
		t.Fatalf("nul pipe path => %q %v", kind, ok)
	}
}

func FuzzWindowsNonRegularKind(f *testing.F) {
	for _, seed := range []string{
		`\\.\pipe\a`,
		`//./pipe/a`,
		`\\?\pipe\a`,
		`\\srv\pipe\a`,
		`\\?\UNC\srv\pipe\a`,
		`\\.\C:\a`,
		`C:\pipe\a`,
		`\\srv\share\a`,
		``,
		`\\.\mailslot\a`,
		`\\.\GLOBALROOT\Device\NamedPipe\a`,
		"\\\\.\\pipe\\\x00",
		`\\.\pipe\帮助`,
	} {
		f.Add(seed)
	}
	f.Fuzz(func(t *testing.T, path string) {
		kind, ok := windowsNonRegularKind(path)
		kindAgain, okAgain := windowsNonRegularKind(path)
		if kind != kindAgain || ok != okAgain {
			t.Fatalf("unstable for %q", path)
		}
		if !ok {
			if kind != "" {
				t.Fatalf("kind %q with ok false", kind)
			}
			return
		}
		if kind != "named pipe" && kind != "non-regular file" {
			t.Fatalf("kind %q for %q", kind, path)
		}
	})
}

func TestUnixBuildDoesNotInstallThePipeClassifier(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("windows installs the classifier")
	}
	_, err := OpenRegular(`\\.\pipe\not-a-unix-fifo`)
	var kind *NonRegularConfigError
	if errors.As(err, &kind) {
		t.Fatalf("unix OpenRegular refused a pipe-shaped name as %s", kind.Kind)
	}
}

func randomPathSuffix(rng *rand.Rand, alphabet string, n int) string {
	buf := make([]byte, n)
	for i := range buf {
		buf[i] = alphabet[rng.Intn(len(alphabet))]
	}
	return string(buf)
}
