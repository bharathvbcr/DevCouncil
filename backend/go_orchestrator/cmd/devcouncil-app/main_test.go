package main

import (
	"crypto/rand"
	"crypto/rsa"
	"crypto/x509"
	"encoding/json"
	"encoding/pem"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
)

func TestBuildServerFailsClosedWithoutWebhookSecret(t *testing.T) {
	t.Setenv("DEVCOUNCIL_WEBHOOK_SECRET", "")
	_, err := buildServerFromEnv(appConfig{
		appID:          "123",
		privateKeyFile: "/nonexistent.pem",
	})
	if err == nil {
		t.Fatalf("expected fail-closed error when DEVCOUNCIL_WEBHOOK_SECRET is unset")
	}
}

func TestBuildServerLoadsKeyAndServesHealthz(t *testing.T) {
	key, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		t.Fatalf("generate rsa key: %v", err)
	}
	pemBytes := pem.EncodeToMemory(&pem.Block{
		Type:  "RSA PRIVATE KEY",
		Bytes: x509.MarshalPKCS1PrivateKey(key),
	})
	keyPath := filepath.Join(t.TempDir(), "app.pem")
	if err := os.WriteFile(keyPath, pemBytes, 0o600); err != nil {
		t.Fatalf("write key: %v", err)
	}
	t.Setenv("DEVCOUNCIL_WEBHOOK_SECRET", "whsec_cmd_test")

	srv, err := buildServerFromEnv(appConfig{
		appID:          "999",
		privateKeyFile: keyPath,
		relayURL:       "https://relay.example.com",
	})
	if err != nil {
		t.Fatalf("buildServerFromEnv: %v", err)
	}
	defer srv.Shutdown(t.Context())

	req := httptest.NewRequest(http.MethodGet, "/healthz", nil)
	rec := httptest.NewRecorder()
	srv.Handler().ServeHTTP(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200 on /healthz, got %d", rec.Code)
	}
	var payload map[string]any
	if err := json.Unmarshal(rec.Body.Bytes(), &payload); err != nil {
		t.Fatalf("decode healthz: %v", err)
	}
	if payload["ok"] != true || payload["webhook_configured"] != true {
		t.Fatalf("unexpected healthz payload: %+v", payload)
	}
}
