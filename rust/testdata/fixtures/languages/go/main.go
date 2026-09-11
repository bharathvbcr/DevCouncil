package sample

import "fmt"

type Service struct{}
func (Service) Run() { fmt.Println("run") }
