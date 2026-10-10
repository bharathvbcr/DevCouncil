package svc

import "example.com/app/store"

// P1: a typed parameter of a concrete type from another package.
func ParamCaller(s *store.Store) string { return s.PtrGet03() }

type counter struct{ n int }

func (c *counter) inc() { c.n++ }

// P2: a same-package `var c counter` declaration types the receiver.
func varDeclCaller() int {
	var c counter
	c.inc()
	return c.n
}

// P3: go and defer on typed method calls.
func goDeferMethods() {
	s := store.NewStore()
	defer s.Close29()
	go s.MethodValue05()
}

// P4: a multi-value constructor types its first result.
func multiReturnCaller() int {
	s, err := store.OpenStore()
	if err != nil {
		return 0
	}
	return s.ElemGet24()
}

type runner interface{ run() int }

type fast struct{}

func (fast) run() int { return 1 }

// unusedM is an unexported method nobody calls: should be dead (P7).
func (fast) unusedM() int { return 2 }

// P5: unexported interface dispatch; fast.run must not be dead.
func useRunner(r runner) int { return r.run() }

// deadPriv is an unexported function nobody calls: should be dead (P6).
func deadPriv() int { return 0 }

func takesFn(f func() int) int { return f() }

// P10: a method value passed as a callback argument.
func callbackCaller() int {
	s := store.NewStore()
	return takesFn(s.MethodValue05)
}

// Probes is the single entry that keeps every probe reachable.
func Probes() {
	_ = ParamCaller(nil)
	_ = varDeclCaller()
	goDeferMethods()
	_ = multiReturnCaller()
	_ = useRunner(fast{})
	_ = callbackCaller()
}
