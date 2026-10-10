package main

import (
	"fmt"
	"net/http"

	"example.com/app/cgox"
	"example.com/app/gen"
	"example.com/app/svc"
	"example.com/app/web"
)

// initOnly13 is called only from init (C13).
func initOnly13() {}

func init() { initOnly13() }

type nopLogger struct{}

func (nopLogger) Log09(string) {}

func main() {
	s := svc.NewService(svc.SQLRepo{}, nopLogger{})
	mux := http.NewServeMux()
	web.Routes(mux, &web.Handler{})
	fmt.Println(svc.Caller01(), svc.Caller28(), svc.Caller19(), svc.Caller20(), svc.Caller03(), svc.Caller04(),
		svc.Caller05(), svc.Caller06(), svc.Caller07b(), svc.Caller08(), svc.Caller10(), svc.Caller16(nil),
		svc.Caller17(nil), svc.Caller18(nil), svc.Caller25(), svc.Caller26(), svc.Caller29(), s.Caller15(),
		s.Caller24(nil), gen.Use23(0), cgox.Caller22())
	s.Caller09()
	svc.Probes()
	svc.Probes2()
	svc.Caller11()
	svc.Caller12()
}

// W5 (added for the dead-policy card): an exported function in package main
// that nothing calls. Nothing can import package main — a 0.4
// `exported_unreferenced` candidate.
func StampSectionRescoredAt() {}
