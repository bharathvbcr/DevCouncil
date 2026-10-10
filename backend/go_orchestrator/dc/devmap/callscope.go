package devmap

import "context"

// callKey carries a callMemo in a context.
type callKey struct{}

// callMemo is the status one call read, shared by everything that call runs.
//
// It is scoped to a call and not to the client on purpose. A status answers
// "is the index current with the working tree", and that changes the moment
// the agent writes a file; a verdict held across calls would describe the tree
// as it was when somebody last asked. Within a call nothing the agent does
// runs, so the verdict taken at its start is true for all of it.
type callMemo struct {
	// slot is a one-place semaphore rather than a mutex so a caller waiting
	// for the read in flight can give up when its own context does.
	slot   chan struct{}
	client *Client
	status *Status
}

// WithCall returns a context that scopes status reads to one tool call. A
// context that is already scoped is returned unchanged, so a nested call
// shares its caller's read.
func WithCall(ctx context.Context) context.Context {
	if _, ok := ctx.Value(callKey{}).(*callMemo); ok {
		return ctx
	}
	return context.WithValue(ctx, callKey{}, &callMemo{slot: make(chan struct{}, 1)})
}
