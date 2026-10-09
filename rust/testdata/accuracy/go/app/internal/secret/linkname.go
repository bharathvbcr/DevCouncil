package secret

import _ "unsafe"

// W4: published to another package by go:linkname; it stays exempt.
//
//go:linkname LinkedClock example.com/app/svc.linkedClock
func LinkedClock() int64 { return 0 }
