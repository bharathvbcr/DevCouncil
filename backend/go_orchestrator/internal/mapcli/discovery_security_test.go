package mapcli

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"testing"
)

func TestDiscoveryNeverProbesRepositoryExecutablesImplicitly(t *testing.T) {
	for _, source := range []string{"local-build", "path", "path-alias", "relative-path"} {
		t.Run(source, func(t *testing.T) {
			root := t.TempDir()
			local := filepath.Join(root, "rust", "target-lane", "release", "devmap")
			if err := os.MkdirAll(filepath.Dir(local), 0o755); err != nil {
				t.Fatal(err)
			}
			body, err := os.ReadFile(fakeKernel(t, kernelStatusOK))
			if err != nil {
				t.Fatal(err)
			}
			body = append([]byte("#!/bin/sh\necho probed >> \"$DEVMAP_PROBE_MARKER\"\n"), body...)
			if err := os.WriteFile(local, body, 0o755); err != nil {
				t.Fatal(err)
			}
			marker := filepath.Join(t.TempDir(), "probe-marker")
			t.Setenv("DEVMAP_PROBE_MARKER", marker)
			t.Setenv("DEVMAP_BINARY", "")
			trusted := fakeKernel(t, kernelStatusOK)
			switch source {
			case "local-build":
				t.Setenv("PATH", filepath.Dir(trusted))
			case "path":
				t.Setenv("PATH", filepath.Dir(local))
			case "path-alias":
				aliasDir := t.TempDir()
				if err := os.Symlink(local, filepath.Join(aliasDir, "devmap")); err != nil {
					t.Fatal(err)
				}
				t.Setenv("PATH", aliasDir)
			case "relative-path":
				t.Chdir(root)
				t.Setenv("PATH", "rust/target-lane/release")
			}
			got, err := discoverBinary(context.Background(), root)
			if _, statErr := os.Stat(marker); !errors.Is(statErr, os.ErrNotExist) {
				t.Fatalf("implicit discovery executed repository content (%s): %v", source, statErr)
			}
			if source == "local-build" {
				if err != nil || got != trusted {
					t.Fatalf("trusted PATH control: got %q, %v; want %q", got, err, trusted)
				}
			} else if !errors.Is(err, ErrNoBinary) {
				t.Fatalf("repository PATH candidate must be refused: got %q, %v", got, err)
			}
			// Existing explicit selection authorizes a local development build.
			t.Setenv("DEVMAP_BINARY", local)
			if got, err := discoverBinary(context.Background(), root); err != nil || got != local {
				t.Fatalf("explicit local selection: got %q, %v", got, err)
			}
			if _, err := os.Stat(marker); err != nil {
				t.Fatalf("explicit local control did not run: %v", err)
			}
		})
	}
}
