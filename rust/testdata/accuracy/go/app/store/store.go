package store

import "errors"

// Base is embedded into Store; its methods are promoted (C08).
type Base struct{}

// Promoted08 is a value-receiver method promoted through embedding (C08).
func (Base) Promoted08() int { return 8 }

// PtrPromoted08 is a pointer-receiver method promoted through embedding (C08).
func (b *Base) PtrPromoted08() int { return 80 }

// Store is the main concrete type.
type Store struct {
	Base
	name string
}

// NewStore is a constructor whose return type types the local (C25).
func NewStore() *Store { return &Store{name: "s"} }

// PtrGet03 has a pointer receiver, called on an addressable value (C03).
func (s *Store) PtrGet03() string { return s.name }

// ValGet04 has a value receiver, called through a pointer (C04).
func (s Store) ValGet04() string { return s.name }

// MethodValue05 is taken as a method value f := s.MethodValue05 (C05).
func (s *Store) MethodValue05() int { return 5 }

// MethodExpr06 is called as a method expression (*Store).MethodExpr06(s) (C06).
func (s *Store) MethodExpr06() int { return 6 }

// Ctor25 is called on the result of NewStore() (C25) and chained (C26).
func (s *Store) Ctor25() int { return 25 }

// ElemGet24 is called on map and slice elements (C24).
func (s *Store) ElemGet24() int { return 24 }

// RangeGet16 is called on a range-loop variable (C16).
func (s *Store) RangeGet16() int { return 16 }

// Close29 shares its name with Other.Close29; only Store's must be bound from a *Store (C29).
func (s *Store) Close29() error { return nil }

// Other is a decoy type with a same-named method (C29 precision).
type Other struct{}

// Close29 on Other must NOT receive the edge from a *Store receiver.
func (Other) Close29() error { return nil }

// Reader is an interface implicitly satisfied by *FileReader and MemReader (C07).
type Reader interface {
	Read07() int
}

// FileReader implicitly implements Reader.
type FileReader struct{}

// Read07 is FileReader's implementation.
func (f *FileReader) Read07() int { return 7 }

// MemReader implicitly implements Reader.
type MemReader struct{}

// Read07 is MemReader's implementation.
func (MemReader) Read07() int { return 70 }

// Logger is embedded as an interface inside Service-like structs (C09).
type Logger interface {
	Log09(msg string)
}

// PathErr is the errors.As target type (C18).
type PathErr struct{ Path string }

func (e *PathErr) Error() string { return e.Path }

// Detail18 is called on the errors.As-bound variable (C18).
func (e *PathErr) Detail18() string { return e.Path }

// ErrSentinel is compared with errors.Is (C18).
var ErrSentinel = errors.New("sentinel")

// Shape types for the type switch (C17).
type Circle struct{}

// Area17 is called on the type-switch-bound variable in case *Circle (C17).
func (c *Circle) Area17() int { return 17 }

// Square is the second type-switch case.
type Square struct{}

// Side17 is called in case Square (C17).
func (Square) Side17() int { return 170 }

// OpenStore returns two values; the first types the local (P4).
func OpenStore() (*Store, error) { return &Store{}, nil }
