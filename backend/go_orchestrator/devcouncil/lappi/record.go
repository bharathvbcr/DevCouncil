package lappi

import (
	"bytes"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"
)

// Caller records (contract §5): local, off by default, held out by path, and
// never admitted to training or eval. Lappi-decision
// crates/qd-runtime/src/caller_record.rs is the checker these lines are
// written for.
const (
	recordDiscriminant   = "lappi.caller_record"
	recordVersion        = 1
	admissionNotAdmitted = "not_admitted"
	// RedactionPolicy names what this writer records: structured facts only,
	// never diff text, file contents or paths.
	RedactionPolicy = "devcouncil.facts-only"

	// MaxRecordBytes is one line's cap (caller_record.rs MAX_RECORD_BYTES).
	MaxRecordBytes = 64 * 1024
	// MaxDayFileBytes and MaxStoreBytes stop recording once reached (§5
	// "Writing"). The store is this app's directory, <root>/devcouncil.
	MaxDayFileBytes = 32 << 20
	MaxStoreBytes   = 256 << 20

	// StoreRelativeToHome is caller_record.rs STORE_RELATIVE_TO_HOME. Its
	// `heldout` segment is what makes qd-train's rule-3 door refuse the store.
	StoreRelativeToHome = "Library/Application Support/Lappi/heldout/caller-records"
)

// heldOutMarkers are caller_record.rs HELD_OUT_MARKERS.
var heldOutMarkers = [3]string{"heldout", "held_out", "held-out"}

// Errors a record append can return. Every one of them is a dropped record,
// and none of them changes a verify decision.
var (
	ErrRecordTooLarge = errors.New("lappi: caller record over the 64 KiB line cap")
	ErrDayFileFull    = errors.New("lappi: today's caller-record file is at its 32 MiB cap; recording stopped until it is cleared")
	ErrStoreFull      = errors.New("lappi: the caller-record store is at its 256 MiB cap; recording stopped until it is cleared")
)

// NewRecordID returns 32 lowercase hex characters from crypto/rand.
func NewRecordID() (string, error) {
	var b [16]byte
	if _, err := rand.Read(b[:]); err != nil {
		return "", fmt.Errorf("lappi: record id: %w", err)
	}
	return hex.EncodeToString(b[:]), nil
}

// DefaultStoreRoot is $HOME/StoreRelativeToHome. An unset HOME is an error,
// never a relative path that would land the store wherever the process started.
func DefaultStoreRoot(getenv func(string) string) (string, error) {
	home := getenv("HOME")
	if home == "" {
		return "", errors.New("lappi: HOME is unset; caller records have nowhere to go")
	}
	return filepath.Join(home, StoreRelativeToHome), nil
}

// Writer appends caller records for this app under one store root.
type Writer struct {
	dir   string
	now   func() time.Time
	logTo io.Writer

	mu      sync.Mutex
	dropped uint64
	logOnce sync.Once
}

// NewWriter returns a writer for root/devcouncil. root must be absolute and
// carry a held-out segment; anything else is refused before a byte is written.
func NewWriter(root string, logTo io.Writer) (*Writer, error) {
	if !filepath.IsAbs(root) {
		return nil, fmt.Errorf("lappi: caller-record root %q is not absolute", root)
	}
	if !hasHeldOutSegment(root) {
		return nil, fmt.Errorf("lappi: caller-record root %q has no held-out segment %v", root, heldOutMarkers)
	}
	if logTo == nil {
		logTo = io.Discard
	}
	return &Writer{dir: filepath.Join(root, App), now: time.Now, logTo: logTo}, nil
}

func hasHeldOutSegment(path string) bool {
	for _, seg := range strings.Split(filepath.ToSlash(path), "/") {
		for _, m := range heldOutMarkers {
			if strings.EqualFold(seg, m) {
				return true
			}
		}
	}
	return false
}

// Dropped is how many records this writer did not write.
func (w *Writer) Dropped() uint64 {
	w.mu.Lock()
	defer w.mu.Unlock()
	return w.dropped
}

// Append writes one complete line (without its newline) to today's file:
// O_APPEND|O_CREATE, mode 0600, directories 0700, one write(2) of the line and
// its newline, then fsync. A refused or failed append is counted, logged once
// per writer, and returned.
func (w *Writer) Append(line []byte) error {
	w.mu.Lock()
	defer w.mu.Unlock()
	err := w.append(line)
	if err != nil {
		w.dropped++
		w.logOnce.Do(func() {
			// A diagnostic stream that cannot be written has nowhere further
			// to report to; the drop is already counted.
			_, _ = fmt.Fprintf(w.logTo, "devcouncil: lappi caller record not written (further failures are counted, not logged): %v\n", err)
		})
	}
	return err
}

func (w *Writer) append(line []byte) error {
	if len(line) > MaxRecordBytes {
		return ErrRecordTooLarge
	}
	if bytes.IndexByte(line, '\n') >= 0 || len(line) == 0 {
		return errors.New("lappi: a caller record is one non-empty line")
	}
	if err := os.MkdirAll(w.dir, 0o700); err != nil {
		return fmt.Errorf("lappi: caller-record directory: %w", err)
	}
	path := filepath.Join(w.dir, w.now().UTC().Format("2006-01-02")+".jsonl")
	add := int64(len(line) + 1)

	var day int64
	switch info, err := os.Stat(path); {
	case err == nil:
		day = info.Size()
	case errors.Is(err, os.ErrNotExist):
	default:
		return fmt.Errorf("lappi: caller-record file: %w", err)
	}
	if day+add > MaxDayFileBytes {
		return ErrDayFileFull
	}
	store, err := dirBytes(w.dir)
	if err != nil {
		return err
	}
	if store+add > MaxStoreBytes {
		return ErrStoreFull
	}

	f, err := os.OpenFile(path, os.O_WRONLY|os.O_APPEND|os.O_CREATE, 0o600)
	if err != nil {
		return fmt.Errorf("lappi: open caller-record file: %w", err)
	}
	framed := make([]byte, 0, len(line)+1)
	framed = append(framed, line...)
	framed = append(framed, '\n')
	n, werr := f.Write(framed)
	if werr == nil && n != len(framed) {
		werr = io.ErrShortWrite
	}
	if werr == nil {
		werr = f.Sync()
	}
	cerr := f.Close()
	if werr != nil {
		return fmt.Errorf("lappi: write caller record: %w", werr)
	}
	if cerr != nil {
		return fmt.Errorf("lappi: close caller-record file: %w", cerr)
	}
	return nil
}

// dirBytes sums the regular files directly in dir.
func dirBytes(dir string) (int64, error) {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return 0, fmt.Errorf("lappi: caller-record store: %w", err)
	}
	var total int64
	for _, e := range entries {
		if !e.Type().IsRegular() {
			continue
		}
		info, err := e.Info()
		if err != nil {
			return 0, fmt.Errorf("lappi: caller-record store: %w", err)
		}
		total += info.Size()
	}
	return total, nil
}

// Facts are what a defect_class decision was made from: the file's language
// and counts. No path, no diff text.
type Facts struct {
	Language     string `json:"language"`
	Hunks        int    `json:"hunks"`
	LinesAdded   int    `json:"lines_added"`
	LinesDeleted int    `json:"lines_deleted"`
}

// AppChoice is what the verify run decided on its own.
type AppChoice struct {
	Status       string `json:"status"`
	BlockingGaps int    `json:"blocking_gaps"`
}

type lappiBlock struct {
	Asked     bool                  `json:"asked"`
	Task      *string               `json:"task"`
	Reading   string                `json:"reading"`
	Kind      *string               `json:"kind"`
	Backend   *string               `json:"backend"`
	Slots     map[string]SlotAnswer `json:"slots"`
	LatencyMs *int64                `json:"latency_ms"`
}

type redactionBlock struct {
	Policy         string `json:"policy"`
	FieldsRedacted int    `json:"fields_redacted"`
}

type decisionRecord struct {
	Record        string         `json:"record"`
	RecordVersion int            `json:"record_version"`
	Kind          string         `json:"kind"`
	RecordID      string         `json:"record_id"`
	App           string         `json:"app"`
	AppVersion    string         `json:"app_version"`
	DecisionPoint string         `json:"decision_point"`
	CreatedAt     string         `json:"created_at"`
	Admission     string         `json:"admission"`
	Facts         Facts          `json:"facts"`
	AppChoice     AppChoice      `json:"app_choice"`
	Lappi         lappiBlock     `json:"lappi"`
	Redaction     redactionBlock `json:"redaction"`
}

func strp(s string) *string { return &s }

// DecisionLine renders one decision record. It refuses a Result that would
// make an inconsistent lappi block (caller_record.rs check_lappi) rather than
// write one.
func DecisionLine(recordID, appVersion string, createdAt time.Time, facts Facts, choice AppChoice, res Result) ([]byte, error) {
	if len(recordID) != 32 || strings.Trim(recordID, "0123456789abcdef") != "" {
		return nil, fmt.Errorf("lappi: record id %q is not 32 lowercase hex characters", recordID)
	}
	if !res.Reading.Valid() {
		return nil, fmt.Errorf("lappi: %s is not a reading", res.Reading)
	}
	block := lappiBlock{Asked: res.Reading != NotAsked, Reading: res.Reading.String()}
	if block.Asked {
		block.Task = strp(DefectClassTask)
		ms := res.Latency.Milliseconds()
		block.LatencyMs = &ms
	}
	switch res.Reading {
	case ModelAnswered, ModelAbstained:
		if len(res.Slots) == 0 || res.Backend == "" || res.Kind != "" {
			return nil, errors.New("lappi: an answer carries its slots and backend and no kind")
		}
		block.Backend = strp(res.Backend)
		block.Slots = res.Slots
	case RequestRefused, BackendFailed, Unavailable:
		if res.Kind == "" || res.Slots != nil || res.Backend != "" {
			return nil, errors.New("lappi: a refusal, error or unavailability names its kind and carries no answer")
		}
		block.Kind = strp(res.Kind)
	case NotAsked:
		if res.Kind != "" || res.Slots != nil || res.Backend != "" {
			return nil, errors.New("lappi: a request that was not sent carries no reply fields")
		}
	}
	rec := decisionRecord{
		Record:        recordDiscriminant,
		RecordVersion: recordVersion,
		Kind:          "decision",
		RecordID:      recordID,
		App:           App,
		AppVersion:    appVersion,
		DecisionPoint: DefectClassTask,
		CreatedAt:     createdAt.UTC().Format(time.RFC3339),
		Admission:     admissionNotAdmitted,
		Facts:         facts,
		AppChoice:     choice,
		Lappi:         block,
		Redaction:     redactionBlock{Policy: RedactionPolicy, FieldsRedacted: 0},
	}
	line, err := json.Marshal(rec)
	if err != nil {
		return nil, fmt.Errorf("lappi: encode caller record: %w", err)
	}
	if len(line) > MaxRecordBytes {
		return nil, ErrRecordTooLarge
	}
	return line, nil
}
