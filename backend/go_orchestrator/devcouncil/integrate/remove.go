package integrate

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"slices"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/devmap"
)

// An apply leaves a receipt at the repository root, and `--remove` follows it.
//
// The receipt is `.devcouncil-integrate-<host>.json`, beside the skills
// receipt and for the same reason: a file DevCouncil writes into someone's
// repository has to be findable by what wrote it. It lives at the root rather
// than under `.devcouncil/` because the existence of that directory moves
// DevMap's state directory, and an install must not do that as a side effect.
//
// It records the paths an apply wrote, accumulated across applies: a later
// apply that finds everything current must not forget what the first one
// created, or the undo would leave it behind.

// ErrNoReceipt is `--remove` with nothing recorded to follow. It is a refusal:
// undoing without a record would mean deciding by guesswork which entries are
// ours.
var ErrNoReceipt = errors.New("no apply receipt for this host in this repository")

const receiptSchema = 1

func receiptRel(host string) string { return ".devcouncil-integrate-" + host + ".json" }

type appliedReceipt struct {
	Schema int               `json:"schema"`
	Host   string            `json:"host"`
	Wrote  []string          `json:"wrote"`
	Backup map[string]string `json:"backups,omitempty"`
}

func readAppliedReceipt(repo *os.Root, host string) (*appliedReceipt, error) {
	raw, _, err := readHookFile(repo, receiptRel(host))
	if errors.Is(err, fs.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, fmt.Errorf("%s: %w", receiptRel(host), err)
	}
	var prior appliedReceipt
	if err := json.Unmarshal(raw, &prior); err != nil || prior.Schema != receiptSchema || prior.Host != host {
		return nil, fmt.Errorf("%s: not a receipt this command wrote; move it aside to record a new one", receiptRel(host))
	}
	return &prior, nil
}

// recordReceipt extends the host's receipt with what an apply wrote.
func recordReceipt(repo *os.Root, host string, receipt *Receipt) error {
	prior, err := readAppliedReceipt(repo, host)
	if err != nil {
		return err
	}
	next := appliedReceipt{Schema: receiptSchema, Host: host, Backup: map[string]string{}}
	if prior != nil {
		next.Wrote = prior.Wrote
		for path, backup := range prior.Backup {
			next.Backup[path] = backup
		}
	}
	for path, state := range receipt.Files {
		if state == "wrote" && !slices.Contains(next.Wrote, path) {
			next.Wrote = append(next.Wrote, path)
		}
	}
	for path, backup := range receipt.Backups {
		next.Backup[path] = backup
	}
	slices.Sort(next.Wrote)
	encoded, err := json.MarshalIndent(next, "", "  ")
	if err != nil {
		return err
	}
	return writeRooted(repo, receiptRel(host), append(encoded, '\n'), 0o644)
}

// RemoveOptions scopes one host's undo.
type RemoveOptions struct {
	Root      string
	Host      string
	Mode      Mode
	DevmapBin string
	SelfBin   string
}

// RemoveReceipt records what a remove did, or under check/dry-run what it
// would.
type RemoveReceipt struct {
	Host    string            `json:"host"`
	Mode    string            `json:"mode"`
	Files   map[string]string `json:"files"` // path -> present|would_remove|removed
	Backups map[string]string `json:"backups,omitempty"`
	Notes   []string          `json:"notes,omitempty"`
}

// Present lists the paths a check found still carrying something to remove.
func (r *RemoveReceipt) Present() []string {
	var paths []string
	for path, state := range r.Files {
		if state == "present" {
			paths = append(paths, path)
		}
	}
	slices.Sort(paths)
	return paths
}

// Remove undoes this host's apply: it takes DevCouncil's and DevMap's server
// entries out of exactly the documents the apply recorded, deletes DevCouncil's
// Cursor rule (keeping a copy of any edits), and then the receipt. Every other
// byte of the user's documents stays. Guides, skills and hook files are not
// removed — the guides are shared by every host and the skill receipt names
// the rest — and the receipt says so.
func Remove(opts RemoveOptions) (*RemoveReceipt, error) {
	root, err := filepath.Abs(opts.Root)
	if err != nil {
		return nil, err
	}
	host := strings.ToLower(opts.Host)
	if err := checkHost(host); err != nil {
		return nil, err
	}
	mode := opts.Mode
	if mode == "" {
		mode = ModeCheck
	}
	installMode, err := mode.install()
	if err != nil {
		return nil, err
	}
	repo, err := os.OpenRoot(root)
	if err != nil {
		return nil, err
	}
	defer func() { _ = repo.Close() }()
	applied, err := readAppliedReceipt(repo, host)
	if err != nil {
		return nil, err
	}
	if applied == nil {
		return nil, fmt.Errorf("%w (%s): `integrate %s --apply` writes it, and --remove only takes back what it records",
			ErrNoReceipt, receiptRel(host), host)
	}
	selfBin := opts.SelfBin
	if selfBin == "" {
		if selfBin, err = os.Executable(); err != nil {
			return nil, fmt.Errorf("cannot name this devcouncil binary: %w", err)
		}
	}
	runnable, err := devmap.ResolveOutside(opts.DevmapBin, root)
	if err != nil {
		return nil, fmt.Errorf("%w; the host documents are DevMap's to edit, so nothing was removed", err)
	}

	only := make([]string, 0, len(applied.Wrote))
	for _, path := range applied.Wrote {
		if !filepath.IsAbs(path) {
			path = filepath.Join(root, path)
		}
		only = append(only, path)
	}
	report, err := devmap.New(runnable, root).RemoveServers(context.Background(), host, installMode,
		[]devmap.Server{devcouncilServer(selfBin, root)}, only)
	if err != nil {
		return nil, err
	}
	receipt := &RemoveReceipt{Host: host, Mode: string(mode), Files: map[string]string{}}
	state := map[Mode]string{ModeCheck: "present", ModeDryRun: "would_remove", ModeApply: "removed"}[mode]
	for _, doc := range report.Removed {
		if len(doc.Servers) > 0 {
			receipt.Files[devmap.RelativeTo(root, doc.Path)] = state
		}
		if doc.Note != "" {
			receipt.Notes = append(receipt.Notes, doc.Note)
		}
	}
	receipt.Notes = append(receipt.Notes, report.Notes...)

	if host == "cursor" && slices.Contains(applied.Wrote, cursorRuleRel) {
		if err := removeCursorRule(repo, mode, state, receipt); err != nil {
			return receipt, err
		}
	}
	if mode == ModeApply {
		if err := repo.Remove(receiptRel(host)); err != nil {
			return receipt, fmt.Errorf("%s: %w", receiptRel(host), err)
		}
	}
	return receipt, nil
}

// removeCursorRule deletes the rule when it is one this command wrote. An
// edited copy is saved first: recognising a file as ours is not permission to
// lose what its owner added to it.
func removeCursorRule(repo *os.Root, mode Mode, state string, receipt *RemoveReceipt) error {
	existing, _, err := readHookFile(repo, cursorRuleRel)
	if errors.Is(err, fs.ErrNotExist) {
		return nil
	}
	if err != nil {
		return fmt.Errorf("%s: %w", cursorRuleRel, err)
	}
	if !isDevCouncilCursorRule(existing) {
		receipt.Notes = append(receipt.Notes, cursorRuleRel+": not a rule this command wrote; left in place")
		return nil
	}
	receipt.Files[cursorRuleRel] = state
	if mode != ModeApply {
		return nil
	}
	if string(existing) != cursorRule {
		backup, err := backupBeside(repo, cursorRuleRel, existing)
		if err != nil {
			return err
		}
		if receipt.Backups == nil {
			receipt.Backups = map[string]string{}
		}
		receipt.Backups[cursorRuleRel] = backup
	}
	if err := repo.Remove(cursorRuleRel); err != nil {
		return fmt.Errorf("%s: %w", cursorRuleRel, err)
	}
	return nil
}
