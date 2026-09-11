package probe

import "example.com/helper"

type Widget struct {
	Name string
}

func (w *Widget) Render() string {
	return helper.Helper(w.Name)
}

func Main() {
	w := &Widget{}
	w.Render()
}
