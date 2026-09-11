package verify

type Priority int

func (p Priority) valid() bool { return p >= 0 }

type Worker struct {
	Priority Priority
}

func (w Worker) Check() bool {
	return w.Priority.valid()
}
