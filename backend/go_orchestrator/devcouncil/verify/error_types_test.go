package verify_test

import (
	"context"
	"errors"
	"path/filepath"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

func TestVerifyTaskErrorsKeepTheirBytesAndTypes(t *testing.T) {
	client := store.New(testsupport.DCStore(t), filepath.Join(t.TempDir(), "state.sqlite"))
	if _, err := client.ActiveLeases(context.Background()); err != nil {
		t.Fatal(err)
	}
	_, _, err := verify.VerifyTask(context.Background(), t.TempDir(), client, "NO-SUCH", verify.GateState{Mode: "enforce"}, "", "")
	if err == nil || err.Error() != "task NO-SUCH not found" {
		t.Fatalf("err = %v, want the unchanged bytes \"task NO-SUCH not found\"", err)
	}
	if !errors.Is(err, verify.ErrTaskNotFound) {
		t.Fatalf("errors.Is(err, ErrTaskNotFound) = false for %T", err)
	}
	_, _, err = verify.VerifyTask(context.Background(), t.TempDir(), nil, "X", verify.GateState{}, "", "")
	if err == nil || err.Error() != "store unavailable" || !errors.Is(err, verify.ErrStoreUnavailable) {
		t.Fatalf("nil store err = %v", err)
	}
}
