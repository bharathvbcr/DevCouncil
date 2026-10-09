package main

// How the host finds dcstore and devmap. The repository under analysis is
// input: a binary it carries, or one a PATH entry resolves to inside it, must
// not become the program that owns its task store or answers its code graph.

import (
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

// plantExecutable writes a shell stand-in named name into dir.
func plantExecutable(t *testing.T, dir, name string) string {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("the planted binaries here are shell scripts")
	}
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(dir, name)
	// #nosec G306 -- an executable stand-in; the execute bit is the point.
	if err := os.WriteFile(path, []byte("#!/bin/sh\nexit 0\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	return path
}

// storeRepo returns a repository root with a state database, so openStore
// gets as far as resolving the binary.
func storeRepo(t *testing.T) string {
	t.Helper()
	root := t.TempDir()
	if err := os.MkdirAll(filepath.Join(root, ".devcouncil"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, ".devcouncil", "state.sqlite"), nil, 0o644); err != nil {
		t.Fatal(err)
	}
	return root
}

func canonical(t *testing.T, path string) string {
	t.Helper()
	got, err := filepath.EvalSymlinks(path)
	if err != nil {
		t.Fatal(err)
	}
	return got
}

// A dcstore inside the repository is refused, and the refusal is said out
// loud: "state is unavailable" alone would be false for a repository that
// has a database and a binary that was turned away.
func TestOpenStoreRefusesRepositoryLocalDcstore(t *testing.T) {
	root := storeRepo(t)
	plantExecutable(t, filepath.Join(root, "bin"), "dcstore")
	t.Setenv("PATH", filepath.Join(root, "bin"))
	t.Setenv("HOME", t.TempDir())

	stderr, restore := swapStderr(t)
	client := openStore(root)
	restore()
	if client != nil {
		t.Fatalf("openStore ran %q, which is inside the repository under analysis", client.Binary)
	}
	if !strings.Contains(stderr.String(), "inside the repository") {
		t.Fatalf("the refusal was silent: stderr = %q", stderr.String())
	}
}

// Without a dcstore anywhere the client is nil rather than the bare name,
// which exec would resolve on PATH again with none of the checks above.
func TestOpenStoreWithoutDcstoreIsNilNotABareName(t *testing.T) {
	root := storeRepo(t)
	t.Setenv("PATH", t.TempDir())
	t.Setenv("HOME", t.TempDir())

	stderr, restore := swapStderr(t)
	client := openStore(root)
	restore()
	if client != nil {
		t.Fatalf("openStore invented a binary: %q", client.Binary)
	}
	if !strings.Contains(stderr.String(), "dcstore") {
		t.Fatalf("stderr does not name the missing binary: %q", stderr.String())
	}
}

// The positive control: an installation outside the repository is used, by
// its canonical path.
func TestOpenStoreUsesDcstoreOutsideRepository(t *testing.T) {
	root := storeRepo(t)
	installed := plantExecutable(t, t.TempDir(), "dcstore")
	t.Setenv("PATH", filepath.Dir(installed))
	t.Setenv("HOME", t.TempDir())

	client := openStore(root)
	if client == nil {
		t.Fatal("openStore found nothing for a dcstore outside the repository")
	}
	if client.Binary != canonical(t, installed) {
		t.Fatalf("resolved %q, want %q", client.Binary, canonical(t, installed))
	}
}

// ~/.local/bin is where `devcouncil install` puts components, so it is
// searched when PATH has no candidate at all.
func TestOpenStoreFallsBackToLocalBin(t *testing.T) {
	root := storeRepo(t)
	home := t.TempDir()
	installed := plantExecutable(t, filepath.Join(home, ".local", "bin"), "dcstore")
	t.Setenv("PATH", t.TempDir())
	t.Setenv("HOME", home)

	client := openStore(root)
	if client == nil {
		t.Fatal("openStore did not search ~/.local/bin")
	}
	if client.Binary != canonical(t, installed) {
		t.Fatalf("resolved %q, want %q", client.Binary, canonical(t, installed))
	}
}

// `dev map` resolves against the directory it runs in. A devmap the
// repository carries is not run.
func TestRepositoryLocalDevmapIsNotRun(t *testing.T) {
	_, argvFile := fakeDevmap(t)
	t.Setenv("DEVMAP_BIN", "")
	root := t.TempDir()
	planted := filepath.Join(root, "bin", "devmap")
	if err := os.MkdirAll(filepath.Dir(planted), 0o755); err != nil {
		t.Fatal(err)
	}
	script := "#!/bin/sh\n: > \"$DEVMAP_ARGV_FILE\"\n"
	// #nosec G306 -- an executable stand-in; the execute bit is the point.
	if err := os.WriteFile(planted, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", filepath.Dir(planted))
	t.Chdir(root)

	stderr, restore := swapStderr(t)
	code := dispatch([]string{"map", "status"})
	restore()
	if code != 1 {
		t.Fatalf("exit %d, want 1", code)
	}
	if _, err := os.Stat(argvFile); !os.IsNotExist(err) {
		t.Fatalf("the repository's devmap ran (argv file exists, err=%v)", err)
	}
	if !strings.Contains(stderr.String(), "inside the repository") {
		t.Fatalf("stderr = %q, want the refusal named", stderr.String())
	}
}

// `dev map --root R` asks about R, so R is the repository a devmap must be
// outside of — not the directory the command happens to run in. From $HOME,
// using the working directory would refuse ~/.local/bin/devmap as
// repository content.
func TestDevmapRootFlagChoosesTheRepository(t *testing.T) {
	for _, spelling := range []string{"separate", "equals"} {
		t.Run(spelling, func(t *testing.T) {
			if runtime.GOOS == "windows" {
				t.Skip("the fake devmap is a shell script")
			}
			home := t.TempDir()
			repo := t.TempDir()
			argvFile := filepath.Join(t.TempDir(), "argv")
			script := "#!/bin/sh\n: > \"$DEVMAP_ARGV_FILE\"\nfor a in \"$@\"; do printf '%s\\n' \"$a\" >> \"$DEVMAP_ARGV_FILE\"; done\n"
			if err := os.MkdirAll(filepath.Join(home, ".local", "bin"), 0o755); err != nil {
				t.Fatal(err)
			}
			// #nosec G306 -- an executable stand-in; the execute bit is the point.
			if err := os.WriteFile(filepath.Join(home, ".local", "bin", "devmap"), []byte(script), 0o755); err != nil {
				t.Fatal(err)
			}
			t.Setenv("DEVMAP_ARGV_FILE", argvFile)
			t.Setenv("DEVMAP_BIN", "")
			t.Setenv("HOME", home)
			t.Setenv("PATH", t.TempDir())
			t.Chdir(home)

			args := []string{"map", "--root", repo, "status"}
			if spelling == "equals" {
				args = []string{"map", "--root=" + repo, "status"}
			}
			stderr, restore := swapStderr(t)
			code := dispatch(args)
			restore()
			if code != 0 {
				t.Fatalf("exit %d, stderr = %q", code, stderr.String())
			}
			if _, err := os.Stat(argvFile); err != nil {
				t.Fatalf("devmap did not run: %v", err)
			}
		})
	}
}
