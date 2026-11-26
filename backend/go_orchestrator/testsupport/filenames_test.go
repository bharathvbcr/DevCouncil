package testsupport

import "testing"

func TestWindowsNameProblemNamesOnlyWhatWindowsRefuses(t *testing.T) {
	for _, c := range []struct {
		name    string
		invalid bool
	}{
		{"plain.txt", false},
		{"dir with space/file name.txt", false},
		{" leading.txt", false},
		{"-dash.txt", false},
		{"dollar$name.txt", false},
		{"quote'single.txt", false},
		{"é.txt", false},
		{"a/../b.txt", false},
		{"./b.txt", false},
		{".hidden", false},
		{"tab\tname.txt", true},
		{"odd\nname.rs", true},
		{`odd"name.rs`, true},
		{"up../x.rs", true},
		{"trailing.txt ", true},
		{"dir./file", true},
		{"a:b", true},
		{"what?", true},
		{"star*", true},
		{"pipe|", true},
		{"lt<gt>", true},
	} {
		if got := windowsNameProblem(c.name) != ""; got != c.invalid {
			t.Errorf("windowsNameProblem(%q) flagged = %v, want %v", c.name, got, c.invalid)
		}
	}
}
