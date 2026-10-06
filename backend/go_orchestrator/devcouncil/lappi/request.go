package lappi

import (
	"encoding/base64"
	"encoding/json"
	"errors"
)

// Slot types this client can read an answer for.
const (
	SlotChoice = "choice"
	SlotSpan   = "span"
)

// The code.defect_class request, pinned by contract §4 and rendered into the
// prompt verbatim: a renamed task, question or slot serves the model a prompt
// it never saw.
const (
	DefectClassTask     = "code.defect_class"
	DefectClassQuestion = "What kind of change is this diff, and which lines does it touch?"
	DefectClassSlot     = "defect_class"
	DefectSpanSlot      = "defect_span"
	// DefectClassClean is the option that is not a defect.
	DefectClassClean = "clean"
)

// DefectClassOptions are the defect_class choices, in the trained order.
var DefectClassOptions = []string{"stub", "logic", "cosmetic", "clean"}

// App is DevCouncil's name in example ids and caller records.
const App = "devcouncil"

// SlotSpec is one slot of a request.
type SlotSpec struct {
	Name    string   `json:"name"`
	Type    string   `json:"type"`
	Options []string `json:"options,omitempty"`
}

// requestMetadata encodes as {} — the contract's `metadata: {}`.
type requestMetadata struct{}

// Request is one request line (schema-api.md "Request").
type Request struct {
	SchemaVersion int             `json:"schema_version"`
	Task          string          `json:"task"`
	ContextB64    string          `json:"context_b64"`
	ContextLen    int             `json:"context_len"`
	Question      string          `json:"question"`
	Slots         []SlotSpec      `json:"slots"`
	Route         string          `json:"route"`
	ExampleID     string          `json:"example_id"`
	Metadata      requestMetadata `json:"metadata"`
}

// DefectClassSlots returns fresh copies of the two trained slots.
func DefectClassSlots() []SlotSpec {
	return []SlotSpec{
		{Name: DefectClassSlot, Type: SlotChoice, Options: append([]string(nil), DefectClassOptions...)},
		{Name: DefectSpanSlot, Type: SlotSpan},
	}
}

// NewDefectClassRequest builds the trained code.defect_class request for one
// file's context (`file: <path>`, a blank line, its hunks). context_b64 is
// standard, padded base64 of the context's bytes and context_len is their
// count; the bytes are never re-encoded through a string.
func NewDefectClassRequest(context []byte, recordID string) Request {
	return Request{
		SchemaVersion: 1,
		Task:          DefectClassTask,
		ContextB64:    base64.StdEncoding.EncodeToString(context),
		ContextLen:    len(context),
		Question:      DefectClassQuestion,
		Slots:         DefectClassSlots(),
		Route:         "generic",
		ExampleID:     App + ":" + DefectClassTask + ":" + recordID,
	}
}

// Encode returns the request as one JSON line, without its newline.
func (r Request) Encode() ([]byte, error) {
	if r.ContextLen < 0 {
		return nil, errors.New("lappi: negative context_len")
	}
	for _, s := range r.Slots {
		if s.Type != SlotChoice && s.Type != SlotSpan {
			return nil, errors.New("lappi: slot " + s.Name + " has a type this client cannot read: " + s.Type)
		}
	}
	return json.Marshal(r)
}
