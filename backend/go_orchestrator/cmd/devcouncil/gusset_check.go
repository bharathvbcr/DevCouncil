package main

import (
	"context"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/console"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/gussetfn"
)

func runGussetCheck() int {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	if err := gussetfn.Check(ctx); err != nil {
		console.Errorf("gusset-check: %v\n", err)
		return 1
	}
	console.Println("gusset-check: ok")
	return 0
}
