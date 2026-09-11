package dc

import "testing"

// Phase 7 (2026-09-10) retired DevCouncil's Python requirement / acceptance
// models (src/devcouncil/domain and the pydantic producers this file used to
// drive). The three tests that lived here — serialise-via-Python, defaults
// agreement, and enum-set parity — could not run once those packages were
// deleted: a checkout still has a .venv, so the locator found an interpreter
// and then failed on import, which is a gate failure rather than a skip.
//
// Wire-shape coverage for this package is owned by requirement_test.go, which
// asserts decode, defaults, and enum validity against the Go types themselves.
// Reintroduce a cross-plane interop check only when a second producer of the
// same JSON exists; until then, a Python-driven suite would be examining a
// retired surface.
func TestPythonRequirementInteropRetiredWithPhase7(t *testing.T) {
	// Explicit retirement marker so the suite records why the Python-driven
	// checks are gone rather than leaving a silent absence.
}
