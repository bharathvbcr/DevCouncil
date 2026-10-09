package gen

//go:generate stringer -type=Color

// Color is the enum stringer would generate String() for (C23).
type Color int

// Use23 calls the generated String method and the generated helper.
func Use23(c Color) string { return c.String() + GeneratedHelper23() }
