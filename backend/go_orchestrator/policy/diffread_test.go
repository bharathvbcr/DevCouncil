package policy

import (
	"strings"
	"testing"
)

const ordinarySection = "diff --git a/src/a.go b/src/a.go\nindex 1..2 100644\n--- a/src/a.go\n+++ b/src/a.go\n@@ -1 +1 @@\n-a\n+b\n"

func TestWithholdSecretDiffKeepsOrdinarySectionsByteForByte(t *testing.T) {
	in := ordinarySection + "diff --git a/img.png b/img.png\nBinary files a/img.png and b/img.png differ\n"
	got, withheld := WithholdSecretDiff(in)
	if got != in || withheld != nil {
		t.Fatalf("an ordinary diff changed: %q withheld=%v", got, withheld)
	}
}

func TestWithholdSecretDiffRemovesEverySpellingOfASecretSection(t *testing.T) {
	for name, section := range map[string]string{
		"default prefixes": "diff --git a/.env b/.env\n--- a/.env\n+++ b/.env\n@@ -1 +1 @@\n-A=1\n+A=hunter2\n",
		// diff.noprefix and diff.mnemonicPrefix are a repository's to set.
		"no prefix":       "diff --git .npmrc .npmrc\n--- .npmrc\n+++ .npmrc\n@@ -1 +1 @@\n-x\n+_authToken=hunter2\n",
		"mnemonic prefix": "diff --git i/secrets/x w/secrets/x\n--- i/secrets/x\n+++ w/secrets/x\n@@ -1 +1 @@\n-x\n+hunter2\n",
		"quoted name":     "diff --git \"a/secrets/caf\\303\\251\" \"b/secrets/caf\\303\\251\"\n--- \"a/secrets/caf\\303\\251\"\n+++ \"b/secrets/caf\\303\\251\"\n@@ -1 +1 @@\n-x\n+hunter2\n",
		"spaced name":     "diff --git a/sub dir/x.key b/sub dir/x.key\n--- a/sub dir/x.key\t\n+++ b/sub dir/x.key\t\n@@ -1 +1 @@\n-x\n+hunter2\n",
		"new file":        "diff --git a/credentials/k b/credentials/k\nnew file mode 100644\n--- /dev/null\n+++ b/credentials/k\n@@ -0,0 +1 @@\n+hunter2\n",
		"deleted file":    "diff --git a/id_rsa b/id_rsa\ndeleted file mode 100644\n--- a/id_rsa\n+++ /dev/null\n@@ -1 +0,0 @@\n-hunter2\n",
		"rename out":      "diff --git a/secrets/p b/config/p\nsimilarity index 80%\nrename from secrets/p\nrename to config/p\n--- a/secrets/p\n+++ b/config/p\n@@ -1 +1 @@\n-x\n+hunter2\n",
		"case folded":     "diff --git a/.ENV b/.ENV\n--- a/.ENV\n+++ b/.ENV\n@@ -1 +1 @@\n-x\n+hunter2\n",
		"combined":        "diff --cc .env\nindex 1,2..3\n--- a/.env\n+++ b/.env\n@@@ -1,1 -1,1 +1,1 @@@\n- x\n -y\n++hunter2\n",
	} {
		t.Run(name, func(t *testing.T) {
			got, withheld := WithholdSecretDiff(ordinarySection + section + ordinarySection)
			if strings.Contains(got, "hunter2") {
				t.Fatalf("secret section survived:\n%s", got)
			}
			if got != ordinarySection+ordinarySection {
				t.Fatalf("the ordinary sections around it changed:\n%s", got)
			}
			if len(withheld) == 0 {
				t.Fatal("nothing was reported withheld")
			}
		})
	}
}

// A section with hunks whose names cannot be read has content of unknown
// provenance, and that is withheld rather than guessed about.
func TestWithholdSecretDiffFailsClosedOnAnUnreadableHeader(t *testing.T) {
	for name, section := range map[string]string{
		"bad quoting":   "diff --git \"a/x \"b/x\n--- \"a/x\n+++ \"b/x\n@@ -1 +1 @@\n-x\n+hunter2\n",
		"no name lines": "diff --git a/x b/x\n@@ -1 +1 @@\n-x\n+hunter2\n",
	} {
		t.Run(name, func(t *testing.T) {
			got, _ := WithholdSecretDiff(section)
			if strings.Contains(got, "hunter2") {
				t.Fatalf("unreadable section survived:\n%s", got)
			}
		})
	}
}

func TestWithholdSecretDiffReportsNamesWithoutTheirPrefix(t *testing.T) {
	section := "diff --git a/secrets/p b/config/p\nrename from secrets/p\nrename to config/p\n--- a/secrets/p\n+++ b/config/p\n@@ -1 +1 @@\n-x\n+y\n"
	_, withheld := WithholdSecretDiff(section)
	want := []string{"secrets/p", "config/p", "a/secrets/p", "b/config/p"}
	got := map[string]bool{}
	for _, w := range withheld {
		got[w] = true
	}
	for _, w := range want {
		if !got[w] {
			t.Errorf("withheld %v does not name %q", withheld, w)
		}
	}
	if len(got) != len(withheld) {
		t.Errorf("withheld names repeat: %v", withheld)
	}
}

func TestSecretPathSpellingAgreesWithTheReadRung(t *testing.T) {
	g := FileGate{Root: "/repo", HardRules: true}
	for _, p := range []string{
		".env", "sub/.env", "secrets/x", "credentials/a/b", "x.pem", "a/b.KEY", "id_ed25519",
		".npmrc", "a/.pypirc", ".netrc", "src/calc.go", "README.md", "secrets.go", "envs/.env.example",
	} {
		if rung, spelled := g.EvaluateRead(p, nil).Blocked(), SecretPathSpelling(p); rung != spelled {
			t.Errorf("%q: read rung blocked=%v, SecretPathSpelling=%v", p, rung, spelled)
		}
	}
}

// Output the filter does not recognize is withheld, never copied: a
// repository's color.diff=always once hid every header from it.
func TestWithholdSecretDiffFailsClosedOnUnrecognizedOutput(t *testing.T) {
	colored := "\x1b[1mdiff --git a/.env b/.env\x1b[m\n\x1b[1m--- a/.env\x1b[m\n\x1b[1m+++ b/.env\x1b[m\n" +
		"\x1b[36m@@ -1 +1 @@\x1b[m\n\x1b[31m-x\x1b[m\n\x1b[32m+hunter2\x1b[m\n"
	got, withheld := WithholdSecretDiff(colored + ordinarySection)
	if strings.Contains(got, "hunter2") {
		t.Fatalf("unrecognized output survived:\n%q", got)
	}
	if got != ordinarySection {
		t.Fatalf("the recognized section after it changed: %q", got)
	}
	if len(withheld) != 1 || withheld[0] != UnattributedDiffText {
		t.Fatalf("the removal was not reported: %v", withheld)
	}
}
