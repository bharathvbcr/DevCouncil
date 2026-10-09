package api

import "net/http"

func handlePapers(w http.ResponseWriter, r *http.Request) { w.Write([]byte("[]")) }
func handleNotes(w http.ResponseWriter, r *http.Request)  { w.WriteHeader(201) }

func Register(mux *http.ServeMux) {
	mux.HandleFunc("GET /api/papers", handlePapers)
	mux.HandleFunc("POST /api/notes", handleNotes)
}
