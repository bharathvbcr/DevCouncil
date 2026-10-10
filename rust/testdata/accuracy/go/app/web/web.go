package web

import (
	"net/http"

	"example.com/app/svc"
)

// Handler holds the service; its methods are registered as method values (C14b).
type Handler struct{ sv *svc.Service }

// handleIndex14 is registered by function value with HandleFunc (C14a).
func handleIndex14(w http.ResponseWriter, r *http.Request) {}

// ServeItems14 is registered by method value h.ServeItems14 (C14b).
func (h *Handler) ServeItems14(w http.ResponseWriter, r *http.Request) {}

// handleWrapped14 is registered wrapped in http.HandlerFunc(...) (C14c).
func handleWrapped14(w http.ResponseWriter, r *http.Request) {}

// inline14 is called from an inline func literal handler (C14d).
func inline14() {}

// Routes registers every handler shape.
func Routes(mux *http.ServeMux, h *Handler) {
	mux.HandleFunc("/", handleIndex14)
	mux.HandleFunc("/items", h.ServeItems14)
	mux.Handle("/wrapped", http.HandlerFunc(handleWrapped14))
	mux.HandleFunc("/inline", func(w http.ResponseWriter, r *http.Request) { inline14() })
}
