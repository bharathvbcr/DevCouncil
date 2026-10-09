package store

// Number is a constraint interface (C10).
type Number interface {
	~int | ~float64
}

// Sum10 is a generic function called with inferred type args (C10a).
func Sum10[T Number](xs []T) T {
	var s T
	for _, x := range xs {
		s += x
	}
	return s
}

// Zero10 must be instantiated explicitly because T is only in the result (C10b, one type arg).
func Zero10[T any]() T {
	var z T
	return z
}

// Pair10 takes two explicit type args (C10c).
func Pair10[K comparable, V any](k K, v V) map[K]V { return map[K]V{k: v} }

// Box is a generic type (C10d).
type Box[T any] struct{ v T }

// NewBox10 constructs a Box; called with an explicit type arg (C10e).
func NewBox10[T any](v T) *Box[T] { return &Box[T]{v: v} }

// Get10 is a method on a generic type (C10d).
func (b *Box[T]) Get10() T { return b.v }
