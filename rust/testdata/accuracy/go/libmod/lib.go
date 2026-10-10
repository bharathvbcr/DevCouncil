// Package libmod is a second module reached through go.work and replace.
package libmod

// LibTarget20 is called from app via the module path (C20 multi-module).
func LibTarget20() int { return 20 }

// LibType20 has a method called across modules (C20b).
type LibType20 struct{}

// LibMethod20 is called as v.LibMethod20() on a libmod.LibType20 receiver.
func (LibType20) LibMethod20() int { return 21 }
