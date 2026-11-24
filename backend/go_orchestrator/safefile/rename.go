package safefile

import (
	"os"
	"time"
)

// renameBudget bounds how long a publish waits for a transient refusal to pass.
const renameBudget = 2 * time.Second

// renameWithRetry renames old to new, repeating the call while transient says
// its error is one that passes by itself, for at most budget.
//
// The commit point of WriteAtomic is a rename over the destination. Windows
// refuses that rename (access denied, or a sharing violation) for as long as
// anyone has the destination open without delete sharing, and Go's own readers
// (os.ReadFile, os.Open) do exactly that, as does a concurrent publisher
// mid-rename. The refusal ends when they close, in microseconds, so the
// publish waits rather than fails; this is what the Go toolchain does for its
// own replaced files. Anywhere transient is always false the first error is
// returned at once.
//
// A refusal that is not transient, or one still standing when the budget ends,
// is returned as the error of the last attempt: a destination that really is
// read-only fails, only after the wait, and never as success.
func renameWithRetry(rename func(old, new string) error, transient func(error) bool, old, new string, budget time.Duration) error {
	deadline := time.Now().Add(budget)
	delay := time.Millisecond
	for {
		err := rename(old, new)
		if err == nil || !transient(err) || !time.Now().Add(delay).Before(deadline) {
			return err
		}
		time.Sleep(delay)
		if delay < 50*time.Millisecond {
			delay *= 2
		}
	}
}

// replaceFile is the publish rename: os.Rename, retried through the refusals
// transientRename names.
func replaceFile(old, new string) error {
	return renameWithRetry(os.Rename, transientRename, old, new, renameBudget)
}
