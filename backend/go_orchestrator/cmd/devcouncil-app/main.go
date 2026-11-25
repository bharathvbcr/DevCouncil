// Command devcouncil-app runs the hosted DevCouncil PR Gate GitHub App webhook
// server (pure Go stdlib, zero external dependencies).
//
// Secrets are read strictly from environment variables or mode-restricted key
// files (never from CLI argv):
//   - DEVCOUNCIL_WEBHOOK_SECRET:        GitHub App webhook HMAC-SHA256 secret
//   - GITPULSE_PR_GATE_APP_SECRET:      shared HMAC secret for gitpulse-relay /v1/pr-gate/app-authorize
//   - DEVCOUNCIL_APP_PRIVATE_KEY_FILE:  path to PEM RSA private key (or --private-key-file)
package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"net/http"
	"os"
	"os/signal"
	"strings"
	"syscall"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/dc/redact"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/prgateapp"
)

type appConfig struct {
	listenAddr         string
	appID              string
	privateKeyFile     string
	githubAPIBase      string
	relayURL           string
	devcouncilBin      string
	devmapBin          string
	dcverifyBin        string
	dcredactBin        string
	buildIndex         bool
	gateTimeout        time.Duration
	workers            int
	queueCapacity      int
	rateLimitPerMinute int
}

func buildServerFromEnv(cfg appConfig) (*prgateapp.Server, error) {
	webhookSecret := strings.TrimSpace(os.Getenv("DEVCOUNCIL_WEBHOOK_SECRET"))
	if webhookSecret == "" {
		return nil, errors.New("DEVCOUNCIL_WEBHOOK_SECRET environment variable must be set (fail-closed)")
	}
	if strings.TrimSpace(cfg.appID) == "" {
		return nil, errors.New("--app-id (or DEVCOUNCIL_APP_ID) is required")
	}
	keyFile := strings.TrimSpace(cfg.privateKeyFile)
	if keyFile == "" {
		return nil, errors.New("--private-key-file (or DEVCOUNCIL_APP_PRIVATE_KEY_FILE) is required")
	}
	pemBytes, err := os.ReadFile(keyFile)
	if err != nil {
		return nil, fmt.Errorf("read GitHub App private key file: %w", err)
	}
	privKey, err := prgateapp.ParseRSAPrivateKeyPEM(pemBytes)
	if err != nil {
		return nil, err
	}

	ghClient := &prgateapp.HTTPGitHubClient{
		AppID:      cfg.appID,
		PrivateKey: privKey,
		APIBase:    cfg.githubAPIBase,
	}

	var entChecker prgateapp.EntitlementChecker
	if strings.TrimSpace(cfg.relayURL) != "" {
		relaySecret := strings.TrimSpace(os.Getenv("GITPULSE_PR_GATE_APP_SECRET"))
		if relaySecret == "" {
			relaySecret = strings.TrimSpace(os.Getenv("DEVCOUNCIL_RELAY_TOKEN"))
		}
		entChecker = &prgateapp.RelayEntitlementClient{
			BaseURL:      strings.TrimSpace(cfg.relayURL),
			ServiceToken: relaySecret,
		}
	}

	var scrubber *redact.Scrubber
	if bin := strings.TrimSpace(cfg.dcredactBin); bin != "" {
		info, err := os.Stat(bin)
		if err != nil || info.IsDir() {
			return nil, fmt.Errorf("resolve --dcredact-bin %q: executable not found", bin)
		}
		scrubber = &redact.Scrubber{Client: redact.New(bin), RequireBinary: true}
	}

	runner := &prgateapp.CLIRunner{
		DevCouncilBin: cfg.devcouncilBin,
		DevMapBin:     cfg.devmapBin,
		DCVerifyBin:   cfg.dcverifyBin,
		DCRedactBin:   cfg.dcredactBin,
		BuildIndex:    cfg.buildIndex,
		Timeout:       cfg.gateTimeout,
	}

	return prgateapp.NewServer(prgateapp.ServerConfig{
		WebhookSecret:      webhookSecret,
		GitHub:             ghClient,
		Entitlements:       entChecker,
		Runner:             runner,
		Scrubber:           scrubber,
		WorkerCount:        cfg.workers,
		QueueCapacity:      cfg.queueCapacity,
		JobTimeout:         cfg.gateTimeout,
		RateLimitPerMinute: cfg.rateLimitPerMinute,
	}), nil
}

func run(args []string) int {
	fs := flag.NewFlagSet("devcouncil-app", flag.ContinueOnError)
	cfg := appConfig{}
	fs.StringVar(&cfg.listenAddr, "listen", envOr("DEVCOUNCIL_APP_LISTEN", "127.0.0.1:8080"), "HTTP listen address")
	fs.StringVar(&cfg.appID, "app-id", os.Getenv("DEVCOUNCIL_APP_ID"), "GitHub App ID")
	fs.StringVar(&cfg.privateKeyFile, "private-key-file", os.Getenv("DEVCOUNCIL_APP_PRIVATE_KEY_FILE"), "Path to GitHub App RSA private key PEM file")
	fs.StringVar(&cfg.githubAPIBase, "github-api-base", envOr("DEVCOUNCIL_GITHUB_API_BASE", prgateapp.DefaultGitHubAPIBase), "GitHub REST API base URL")
	fs.StringVar(&cfg.relayURL, "relay-url", os.Getenv("DEVCOUNCIL_RELAY_URL"), "GitPulse Relay base URL for private-repo entitlement enforcement")
	fs.StringVar(&cfg.devcouncilBin, "devcouncil-bin", envOr("DEVCOUNCIL_BIN", "devcouncil"), "Path to pinned devcouncil binary")
	fs.StringVar(&cfg.devmapBin, "devmap-bin", os.Getenv("DEVMAP_BIN"), "Path to pinned devmap binary")
	fs.StringVar(&cfg.dcverifyBin, "dcverify-bin", os.Getenv("DCVERIFY_BIN"), "Path to pinned dcverify binary")
	fs.StringVar(&cfg.dcredactBin, "dcredact-bin", os.Getenv("DCREDACT_BIN"), "Optional path to pinned dcredact binary")
	fs.BoolVar(&cfg.buildIndex, "build-index", true, "Run devmap build before blast")
	fs.DurationVar(&cfg.gateTimeout, "timeout", 5*time.Minute, "Per-PR background verification timeout")
	fs.IntVar(&cfg.workers, "workers", prgateapp.DefaultWorkerCount, "Bounded concurrent PR Gate worker count")
	fs.IntVar(&cfg.queueCapacity, "queue-capacity", prgateapp.DefaultQueueCapacity, "Bounded pending webhook queue capacity")
	fs.IntVar(&cfg.rateLimitPerMinute, "rate-limit-per-minute", prgateapp.DefaultRateLimitPerMinute, "Per-installation/repo webhook rate limit per minute")

	if err := fs.Parse(args); err != nil {
		if errors.Is(err, flag.ErrHelp) {
			return 0
		}
		return 64
	}

	srv, err := buildServerFromEnv(cfg)
	if err != nil {
		fmt.Fprintf(os.Stderr, "devcouncil-app: %v\n", err)
		return 2
	}

	// Webhook handlers return 202 Accepted immediately; short HTTP write timeout
	// protects against slowloris while background workers run up to gateTimeout.
	httpSrv := &http.Server{
		Addr:              cfg.listenAddr,
		Handler:           srv.Handler(),
		ReadHeaderTimeout: 10 * time.Second,
		ReadTimeout:       15 * time.Second,
		WriteTimeout:      15 * time.Second,
		IdleTimeout:       60 * time.Second,
	}

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	errCh := make(chan error, 1)
	go func() {
		fmt.Fprintf(os.Stderr, "devcouncil-app: listening on %s\n", cfg.listenAddr)
		if err := httpSrv.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
			errCh <- err
		}
		close(errCh)
	}()

	select {
	case <-ctx.Done():
		shutCtx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()
		_ = httpSrv.Shutdown(shutCtx)
		_ = srv.Shutdown(shutCtx)
		return 0
	case err := <-errCh:
		_ = srv.Shutdown(context.Background())
		if err != nil {
			fmt.Fprintf(os.Stderr, "devcouncil-app: server error: %v\n", err)
			return 1
		}
		return 0
	}
}

func envOr(key, fallback string) string {
	if v := strings.TrimSpace(os.Getenv(key)); v != "" {
		return v
	}
	return fallback
}

func main() {
	os.Exit(run(os.Args[1:]))
}
