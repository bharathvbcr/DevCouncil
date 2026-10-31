package policy

import (
	"os"
	"path/filepath"
	"testing"
)

// Brute-forces single-rune substitutions of every prompt-input name against the
// filesystem under the test: any variant the OS resolves to the existing file
// is an alias; the text rule (IsPromptInput, no filesystem) must recognise it,
// because for a name that does not exist yet there is no on-disk spelling to
// fall back on.
func TestAdvFilesystemAliasOrbitIsCoveredByText(t *testing.T) {
	root, err := filepath.EvalSymlinks(t.TempDir())
	advMust(t, err)
	names := append(append([]string{}, PromptInputPatterns...), "skills")
	maxRune := rune(0x1FFFF)
	if testing.Short() {
		maxRune = 0x3000
	}
	for _, name := range names {
		advWrite(t, filepath.Join(root, name), "x")
		runes := []rune(name)
		for i := range runes {
			orig := runes[i]
			for r := rune(0x80); r <= maxRune; r++ {
				runes[i] = r
				alt := string(runes)
				if _, err := os.Lstat(filepath.Join(root, alt)); err != nil {
					continue
				}
				found := name
				if name == "skills" {
					found = ".claude/skills/" + alt
				} else {
					found = alt
				}
				if name == "skills" {
					continue
				}
				if !IsPromptInput(found) {
					t.Errorf("BROKEN: %q (U+%04X at %d) resolves to %q on this filesystem but IsPromptInput is false", alt, r, i, name)
				} else {
					t.Logf("alias covered: %q resolves to %q", alt, name)
				}
			}
			runes[i] = orig
		}
	}
}
