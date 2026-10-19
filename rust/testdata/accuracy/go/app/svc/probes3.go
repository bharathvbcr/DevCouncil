package svc

import "encoding/json"

// Q6: a parameter written `params json.RawMessage` in one function must not
// qualify a same-named, bare-typed local in another. RawMessage here is the
// in-repo decoy: the qualified parameter never binds to it, and the bare local
// always does.
type RawMessage struct{}

func budgetOf(params json.RawMessage) int { return len(params) }

func bareRawUser() int {
	var params RawMessage
	_ = params
	return 0
}

// Probes3 keeps every probe reachable.
func Probes3() int {
	return budgetOf(nil) + bareRawUser()
}
