package probe

import "example.com/helper"

type Reader interface {
	Read() string
}

type Widget struct {
	Reader
	Name string
}

func (w *Widget) Render() string {
	return helper.Helper(w.Name)
}

func Main() {
	w := &Widget{}
	w.Render()
}
