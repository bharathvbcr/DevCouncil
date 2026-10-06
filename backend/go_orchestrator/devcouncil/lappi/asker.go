package lappi

import (
	"context"
	"errors"
	"io"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/version"
)

// The settings (contract §6), both off by default.
const (
	// AskEnv = 1 sends one code.defect_class request per changed file.
	AskEnv = "DEVCOUNCIL_LAPPI_ASK"
	// CollectEnv = 1 writes one caller record per changed file.
	CollectEnv = "DEVCOUNCIL_LAPPI_COLLECT"
	// CollectOverrideEnv = 0 forces recording off whatever CollectEnv says.
	CollectOverrideEnv = "LAPPI_COLLECT"
)

// The bounds on one verify run.
const (
	// MaxFilesPerRun is how many diff sections a run asks about or records.
	MaxFilesPerRun = 16
	// RunBudget is the wall time all of a run's asks share, sequentially.
	RunBudget = 10 * time.Second
)

// Asker is one verify run's view of Lappi: an optional client (asking) and an
// optional writer (recording). Recording is independent of asking: with only
// the writer, every file is recorded as not_asked.
type Asker struct {
	client *Client
	writer *Writer
	budget time.Duration
	now    func() time.Time
	newID  func() (string, error)
}

// NewAsker combines a client and a writer; either may be nil, not both.
func NewAsker(client *Client, writer *Writer) (*Asker, error) {
	if client == nil && writer == nil {
		return nil, errors.New("lappi: an asker with neither a client nor a writer does nothing")
	}
	return &Asker{client: client, writer: writer, budget: RunBudget, now: time.Now, newID: NewRecordID}, nil
}

// FromEnvironment reads the settings. It returns nil, nil when both are off,
// which is the default and leaves a verify run exactly as it was.
func FromEnvironment(getenv func(string) string, logTo io.Writer) (*Asker, error) {
	ask := getenv(AskEnv) == "1"
	collect := getenv(CollectEnv) == "1" && getenv(CollectOverrideEnv) != "0"
	if !ask && !collect {
		return nil, nil
	}
	var client *Client
	if ask {
		socket, err := SocketPath(getenv)
		if err != nil {
			return nil, err
		}
		if client, err = NewClient(socket, MaxExchangeTimeout); err != nil {
			return nil, err
		}
	}
	var writer *Writer
	if collect {
		root, err := DefaultStoreRoot(getenv)
		if err != nil {
			return nil, err
		}
		if writer, err = NewWriter(root, logTo); err != nil {
			return nil, err
		}
	}
	return NewAsker(client, writer)
}

// Asking reports whether this asker sends requests.
func (a *Asker) Asking() bool { return a != nil && a.client != nil }

// Writer is the record writer, nil when recording is off.
func (a *Asker) Writer() *Writer {
	if a == nil {
		return nil
	}
	return a.writer
}

// FileDecision is one file's exchange.
type FileDecision struct {
	File     FileDiff
	RecordID string
	Result   Result
}

// RunOutcome is one run's account: every file it considered, and how many
// diff sections there were in all, so a capped run never reads as complete.
type RunOutcome struct {
	Decisions     []FileDecision
	TotalSections int
	// RecordsWritten and RecordsDropped count this run's caller records.
	RecordsWritten int
	RecordsDropped int
}

// Run asks about, and records, at most MaxFilesPerRun files of diff, one at a
// time, within RunBudget and ctx. choice is the verify run's own decision,
// fixed before Lappi was asked; nothing here can change it.
func (a *Asker) Run(ctx context.Context, diff string, choice AppChoice) RunOutcome {
	files := SplitDiff([]byte(diff))
	out := RunOutcome{TotalSections: len(files)}
	if len(files) > MaxFilesPerRun {
		files = files[:MaxFilesPerRun]
	}
	deadline := a.now().Add(a.budget)
	for _, file := range files {
		id, err := a.newID()
		if err != nil {
			// Without an id there is no example_id to send and no record to
			// write; the file is neither asked about nor recorded.
			out.RecordsDropped++
			continue
		}
		dec := FileDecision{File: file, RecordID: id, Result: a.ask(ctx, file, id, deadline)}
		out.Decisions = append(out.Decisions, dec)
		if a.writer == nil {
			continue
		}
		line, err := DecisionLine(id, version.Version, a.now(), Facts{
			Language: file.Language, Hunks: file.Hunks, LinesAdded: file.Added, LinesDeleted: file.Deleted,
		}, choice, dec.Result)
		if err == nil {
			err = a.writer.Append(line)
		}
		if err != nil {
			out.RecordsDropped++
			continue
		}
		out.RecordsWritten++
	}
	return out
}

func (a *Asker) ask(ctx context.Context, file FileDiff, id string, deadline time.Time) Result {
	switch {
	case file.Skip != "":
		return notAsked(file.Skip)
	case a.client == nil:
		return notAsked(SkipAskingOff)
	case ctx.Err() != nil:
		return notAsked(SkipCancelled)
	}
	remaining := deadline.Sub(a.now())
	if remaining <= 0 {
		return notAsked(SkipBudgetSpent)
	}
	askCtx, cancel := context.WithTimeout(ctx, remaining)
	defer cancel()
	return a.client.Ask(askCtx, NewDefectClassRequest(file.Context, id))
}
