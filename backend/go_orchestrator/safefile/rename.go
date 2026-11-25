package safefile

import (
	"os"
	"time"
)

// renameBudget bounds how long a publish, or a read that meets one, waits for a
// transient refusal to pass.
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
	return retryWhile(func() error { return rename(old, new) }, transient, budget)
}

// retryWhile repeats op while transient says its error passes by itself, for
// at most budget, and returns the last attempt's error.
func retryWhile(op func() error, transient func(error) bool, budget time.Duration) error {
	deadline := time.Now().Add(budget)
	delay := time.Millisecond
	for {
		err := op()
		if err == nil || !transient(err) {
			return err
		}
		// Give up only once the budget is spent, and never sleep past it. Giving
		// up when the next back-off would cross the deadline ended a 60 ms
		// budget after about 31 ms (1+2+4+8+16), half of the wait unused.
		left := time.Until(deadline)
		if left <= 0 {
			return err
		}
		time.Sleep(min(delay, left))
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

// RetryTransient runs op, repeating it for up to two seconds while it fails
// with a refusal that passes by itself. That is the other side of the rename
// above: on Windows an open that lands while WriteAtomic renames over the file
// is refused (access denied, or a sharing violation) until the rename is done,
// so a reader of a file some writer replaces atomically could otherwise read
// it as missing or unreadable. Only Windows has such refusals; elsewhere the
// first error is returned at once. A refusal still standing at the end of the
// budget is returned, so a file that really is unreadable fails, after the wait.
func RetryTransient(op func() error) error {
	return retryWhile(op, transientRename, renameBudget)
}

// ReadFile is os.ReadFile through RetryTransient: the read for a file a writer
// replaces with WriteAtomic.
func ReadFile(path string) ([]byte, error) {
	var data []byte
	err := RetryTransient(func() error {
		var readErr error
		data, readErr = os.ReadFile(path)
		return readErr
	})
	return data, err
}
