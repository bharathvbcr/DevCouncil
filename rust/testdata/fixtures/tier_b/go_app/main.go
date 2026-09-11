// Go Audit Blind-spot Syntax Fixture (X2 composite literal as call/ref, X17 block imports)
package main

import (
	"fmt"
	"strings"
)

type Config struct {
	Name  string
	Count int
}

func ProcessConfig(cfg Config) string {
	return fmt.Sprintf("%s:%d", strings.ToUpper(cfg.Name), cfg.Count)
}

func main() {
	cfg := Config{Name: "service", Count: 5} // X2: composite literal reference
	res := ProcessConfig(cfg)
	fmt.Println(res)
}
