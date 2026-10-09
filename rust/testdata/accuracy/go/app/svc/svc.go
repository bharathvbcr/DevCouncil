package svc

import (
	"errors"
	"sync"

	"example.com/app/internal/secret"
	"example.com/app/store"
	"example.com/app/util"
	u "example.com/app/util"
	"example.com/libmod"
)

// Repo is the dependency-injected interface (C15).
type Repo interface {
	Find15(id string) string
}

// SQLRepo implements Repo implicitly (C15).
type SQLRepo struct{}

// Find15 is SQLRepo's implementation of Repo.Find15.
func (SQLRepo) Find15(id string) string { return id }

// Service is wired by constructor injection (C15) and embeds a Logger interface (C09) and a sync.Mutex.
type Service struct {
	store.Logger
	sync.Mutex
	repo  Repo
	st    *store.Store
	cache map[string]*store.Store
}

// NewService is the DI constructor (C15).
func NewService(repo Repo, lg store.Logger) *Service {
	return &Service{repo: repo, Logger: lg, st: store.NewStore(), cache: map[string]*store.Store{}}
}

// helper11 is called only from inside a goroutine func literal (C11).
func helper11() {}

// worker12 is started with `go worker12(...)` (C12).
func worker12(n int) {}

// cleanup12 is only called via defer (C12).
func cleanup12() {}

// Caller01 calls a package-qualified function.
func Caller01() int { return util.Target01() }

// Caller28 calls through an aliased import.
func Caller28() int { return u.Target28() }

// Caller19 calls into an internal/ package.
func Caller19() int { return secret.Target19() }

// W2 wiring: the internal FileSink is handed out as a store.Logger.
func SinkLogger() store.Logger { return secret.FileSink{} }

// Caller20 calls into the other module.
func Caller20() int {
	var v libmod.LibType20
	return libmod.LibTarget20() + v.LibMethod20()
}

// Caller03 calls a pointer method on an addressable value.
func Caller03() string {
	var s store.Store
	return s.PtrGet03()
}

// Caller04 calls a value method through a pointer.
func Caller04() string {
	s := &store.Store{}
	return s.ValGet04()
}

// Caller05 takes and invokes a method value.
func Caller05() int {
	s := store.NewStore()
	f := s.MethodValue05
	return f()
}

// Caller06 uses a method expression.
func Caller06() int {
	s := store.NewStore()
	return (*store.Store).MethodExpr06(s)
}

// Caller07 calls through an interface (dynamic dispatch to FileReader/MemReader).
func Caller07(r store.Reader) int { return r.Read07() }

// Caller07b assigns concrete types to the interface.
func Caller07b() int {
	var r store.Reader = &store.FileReader{}
	return Caller07(r) + Caller07(store.MemReader{})
}

// Caller08 calls promoted methods through embedding.
func Caller08() int {
	s := store.NewStore()
	return s.Promoted08() + s.PtrPromoted08()
}

// Caller09 calls a method promoted from an embedded interface field.
func (sv *Service) Caller09() {
	sv.Log09("x")
	sv.Lock()
	defer sv.Unlock()
}

// Caller10 calls generics in every instantiation shape.
func Caller10() int {
	a := store.Sum10([]int{1, 2})
	b := store.Zero10[int]()
	c := store.Pair10[string, int]("k", 1)
	bx := store.NewBox10[int](3)
	return a + b + len(c) + bx.Get10()
}

// Caller11 starts a goroutine whose literal calls helper11.
func Caller11() {
	var wg sync.WaitGroup
	wg.Add(1)
	go func() {
		defer wg.Done()
		helper11()
	}()
	wg.Wait()
}

// Caller12 uses go and defer with named functions.
func Caller12() {
	defer cleanup12()
	go worker12(1)
}

// Caller15 calls the injected dependency through the struct field.
func (sv *Service) Caller15() string { return sv.repo.Find15("id") + sv.st.PtrGet03() }

// Caller16 calls a method on a range-loop variable.
func Caller16(xs []*store.Store) int {
	n := 0
	for _, x := range xs {
		n += x.RangeGet16()
	}
	return n
}

// Caller17 calls methods on type-switch bound variables.
func Caller17(v any) int {
	switch s := v.(type) {
	case *store.Circle:
		return s.Area17()
	case store.Square:
		return s.Side17()
	}
	return 0
}

// Caller18 uses errors.As then calls a method on the target.
func Caller18(err error) string {
	var pe *store.PathErr
	if errors.As(err, &pe) {
		return pe.Detail18()
	}
	if errors.Is(err, store.ErrSentinel) {
		return "s"
	}
	return ""
}

// Caller24 calls methods on map and slice elements.
func (sv *Service) Caller24(xs []*store.Store) int {
	return sv.cache["k"].ElemGet24() + xs[0].ElemGet24()
}

// Caller25 calls on the constructor's result held in a local.
func Caller25() int {
	s := store.NewStore()
	return s.Ctor25()
}

// Caller26 calls on a chained constructor result.
func Caller26() int { return store.NewStore().Ctor25() }

// Caller29 must bind Close29 to *Store only (not Other).
func Caller29() error {
	s := store.NewStore()
	return s.Close29()
}
