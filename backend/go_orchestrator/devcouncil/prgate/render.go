package prgate

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/url"
	"sort"
	"strconv"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/dc/redact"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/gitrev"
)

// RenderJSON renders the deterministic `devcouncil.pr_gate/v1` JSON report.
func RenderJSON(r Report) ([]byte, error) {
	b, err := json.MarshalIndent(r, "", "  ")
	if err != nil {
		return nil, err
	}
	return append(b, '\n'), nil
}

// sanitizeCodeSpanInner turns an untrusted repository path into a single-line
// string safe to wrap inside a single-backtick Markdown code span:
//   - invalid/escaping paths (absolute, `..`, NUL, backslash) become `[invalid-path]`;
//   - control chars (`\r`, `\n`, `\t`, etc.) become `\xNN`;
//   - backticks become `\x60` so nothing can break out of the code span;
//   - `<` and `>` become `\x3c` and `\x3e`, and `::` becomes `\x3a\x3a`;
//   - pipes (`|`) are escaped as `\|` when tableCell is true so GFM tables
//     cannot split on a pipe inside a code span.
//
// Inside a backtick code span, GitHub never expands `@mentions`, `www.evil.tld`
// autolinks, HTML entities, or Markdown emphasis.
func sanitizeCodeSpanInner(p string, tableCell bool) string {
	if p != "[withheld-secret-path]" {
		if err := gitrev.CheckRepoPath(p); err != nil {
			return "[invalid-path]"
		}
	}
	var b strings.Builder
	b.Grow(len(p))
	for i := 0; i < len(p); i++ {
		c := p[i]
		switch {
		case c < 0x20 || c == 0x7f:
			fmt.Fprintf(&b, "\\x%02x", c)
		case c == '`':
			b.WriteString("\\x60")
		case c == '<':
			b.WriteString("\\x3c")
		case c == '>':
			b.WriteString("\\x3e")
		case c == '|' && tableCell:
			b.WriteString("\\|")
		case c == ':' && i+1 < len(p) && p[i+1] == ':':
			b.WriteString("\\x3a\\x3a")
			i++
		default:
			b.WriteByte(c)
		}
	}
	return b.String()
}

// EscapeMarkdownPath renders an untrusted repository path inside a Markdown
// code span (escaping backticks and control chars; pipes are escaped with `\|`
// for table safety).
func EscapeMarkdownPath(p string) string {
	if p == "" {
		return "-"
	}
	return "`" + sanitizeCodeSpanInner(p, true) + "`"
}

// EscapeMarkdownLocation renders an optional path and line number inside a
// single Markdown code span (e.g. “ `pkg/a.go:42` “).
func EscapeMarkdownLocation(p string, line int, tableCell bool) string {
	if p == "" {
		return "-"
	}
	inner := sanitizeCodeSpanInner(p, tableCell)
	if line > 0 {
		inner = inner + ":" + strconv.Itoa(line)
	}
	return "`" + inner + "`"
}

// RenderMarkdown renders the privacy-safe PR comment & GitHub Job Summary.
// Always starts with StickyMarker, contains NO timestamps, NO symbol names,
// NO owner emails, NO raw Gap.Description/Evidence/RecommendedFix, and is
// guaranteed to stay strictly under 65,536 bytes.
func RenderMarkdown(r Report) string {
	var b strings.Builder
	b.WriteString(StickyMarker)
	b.WriteString("\n### DevCouncil PR Gate: ")
	b.WriteString(outcomeHeadline(r.Outcome))
	b.WriteString("\n\n")

	shortHead := shortSHA(r.HeadSHA)
	if r.Outcome == OutcomeSkipped {
		b.WriteString("Gate mode is `off`; verification was skipped and no commit status is posted.\n")
		return enforceCommentCap(b.String())
	}

	fmt.Fprintf(&b, "- **Outcome:** `%s` (`%s`)\n", safeWord(r.Outcome), StatusContext)
	if shortHead != "" {
		fmt.Fprintf(&b, "- **Commit:** `%s`\n", shortHead)
	}
	fmt.Fprintf(&b, "- **Mode:** `%s`\n", safeWord(r.Mode))
	if r.ErrorCode != "" {
		fmt.Fprintf(&b, "- **Error code:** `%s`\n", safeWord(r.ErrorCode))
	}
	fmt.Fprintf(&b, "- **Coverage:** %s\n", safeCoverageLabel(r.Coverage))
	if len(r.GatesRun) > 0 {
		safeGates := make([]string, 0, len(r.GatesRun))
		for _, g := range r.GatesRun {
			safeGates = append(safeGates, "`"+safeWord(g)+"`")
		}
		fmt.Fprintf(&b, "- **Gates run:** %s\n", strings.Join(safeGates, ", "))
	}

	b.WriteString("\n| Metric | Count |\n| --- | ---: |\n")
	fmt.Fprintf(&b, "| Changed files | %d |\n", r.Summary.ChangedFiles)
	fmt.Fprintf(&b, "| Blocking findings | %d |\n", r.Summary.BlockingFindings)
	fmt.Fprintf(&b, "| Advisory findings | %d |\n", r.Summary.AdvisoryFindings)
	fmt.Fprintf(&b, "| Author `allow-stub` markers | %d |\n", r.Summary.StubAllowed)
	fmt.Fprintf(&b, "| Degraded checks | %d |\n", r.Summary.Degraded)
	fmt.Fprintf(&b, "| Sensitive config/workflow paths | %d |\n", r.Summary.SensitivePaths)
	fmt.Fprintf(&b, "| Affected test files (DevMap) | %d |\n", r.Summary.AffectedTests)

	if len(r.Findings) > 0 {
		b.WriteString("\n#### Findings\n\n")
		b.WriteString("| Severity | Blocking | Gate | Location | Summary |\n| --- | --- | --- | --- | --- |\n")
		limit := len(r.Findings)
		if limit > MaxMarkdownRows {
			limit = MaxMarkdownRows
		}
		for i := 0; i < limit; i++ {
			f := r.Findings[i]
			loc := EscapeMarkdownLocation(f.Path, f.Line, true)
			blocking := "no"
			if f.Blocking {
				blocking = "yes"
			}
			fmt.Fprintf(&b, "| `%s` | %s | `%s` | %s | %s |\n",
				safeWord(f.Severity), blocking, safeWord(f.GapType), loc, FixedGapMessage(f.GapType))
		}
		if len(r.Findings) > limit {
			fmt.Fprintf(&b, "\n_... and %d more finding(s) omitted._\n", len(r.Findings)-limit)
		}
	}

	if len(r.StubAllowed) > 0 {
		b.WriteString("\n#### Author `allow-stub` Markers\n\n")
		limit := len(r.StubAllowed)
		if limit > MaxMarkdownRows {
			limit = MaxMarkdownRows
		}
		for i := 0; i < limit; i++ {
			s := r.StubAllowed[i]
			fmt.Fprintf(&b, "- %s\n", EscapeMarkdownLocation(s.Path, s.Line, false))
		}
		if len(r.StubAllowed) > limit {
			fmt.Fprintf(&b, "- _... and %d more marker(s) omitted._\n", len(r.StubAllowed)-limit)
		}
	}

	if len(r.Degraded) > 0 {
		b.WriteString("\n#### Degraded Checks\n\n")
		limit := len(r.Degraded)
		if limit > MaxMarkdownRows {
			limit = MaxMarkdownRows
		}
		for i := 0; i < limit; i++ {
			d := r.Degraded[i]
			if d.Path != "" {
				fmt.Fprintf(&b, "- `%s` (%s): %s\n", safeWord(d.Code), EscapeMarkdownPath(d.Path), d.Message)
			} else {
				fmt.Fprintf(&b, "- `%s`: %s\n", safeWord(d.Code), d.Message)
			}
		}
		if len(r.Degraded) > limit {
			fmt.Fprintf(&b, "- _... and %d more degraded item(s) omitted._\n", len(r.Degraded)-limit)
		}
	}

	if len(r.Notices) > 0 {
		b.WriteString("\n#### Notices\n\n")
		limit := len(r.Notices)
		if limit > MaxMarkdownRows {
			limit = MaxMarkdownRows
		}
		for i := 0; i < limit; i++ {
			n := r.Notices[i]
			if n.Path != "" {
				fmt.Fprintf(&b, "- `%s` (%s): %s\n", safeWord(n.Code), EscapeMarkdownPath(n.Path), n.Message)
			} else {
				fmt.Fprintf(&b, "- `%s`: %s\n", safeWord(n.Code), n.Message)
			}
		}
		if len(r.Notices) > limit {
			fmt.Fprintf(&b, "- _... and %d more notice(s) omitted._\n", len(r.Notices)-limit)
		}
	}

	return enforceCommentCap(b.String())
}

func outcomeHeadline(outcome string) string {
	switch outcome {
	case OutcomeSuccess:
		return "Passed"
	case OutcomeFailure:
		return "Blocked"
	case OutcomeError:
		return "Error / Degraded"
	case OutcomeSkipped:
		return "Skipped"
	default:
		return "Error"
	}
}

func safeCoverageLabel(c CoverageSummary) string {
	if c.Measured {
		return "measured"
	}
	return "not measured"
}

func shortSHA(sha string) string {
	if !gitrev.IsFullSHA(sha) {
		return ""
	}
	if len(sha) > 12 {
		return sha[:12]
	}
	return sha
}

func enforceCommentCap(s string) string {
	if len(s) <= MaxCommentBytes {
		return s
	}
	const footer = "\n\n_(Report truncated to fit GitHub comment size limit.)_\n"
	limit := MaxCommentBytes - len(footer)
	if limit < 0 {
		limit = 0
	}
	cut := limit
	if nl := strings.LastIndexByte(s[:cut], '\n'); nl > 0 {
		cut = nl
	}
	return s[:cut] + footer
}

// RenderAnnotations emits GitHub Actions workflow annotations (`::error`,
// `::warning`, `::notice`) using gitrev.AnnotateAt, capped at MaxAnnotations (50).
func RenderAnnotations(r Report) []string {
	if r.Outcome == OutcomeSkipped {
		return []string{}
	}
	var all []string

	if r.ErrorCode != "" && r.ErrorCode != "degraded" {
		all = append(all, gitrev.AnnotateAt("error", "", 0, "devcouncil/pr-gate",
			fmt.Sprintf("PR gate error (%s).", safeWord(r.ErrorCode))))
	}

	for _, f := range r.Findings {
		level := "warning"
		if f.Blocking {
			level = "error"
		}
		file := safeAnnotationPath(f.Path)
		all = append(all, gitrev.AnnotateAt(level, file, f.Line, "devcouncil/"+safeWord(f.GapType), FixedGapMessage(f.GapType)))
	}

	for _, d := range r.Degraded {
		file := safeAnnotationPath(d.Path)
		all = append(all, gitrev.AnnotateAt("error", file, 0, "devcouncil/"+safeWord(d.Code), d.Message))
	}

	for _, n := range r.Notices {
		file := safeAnnotationPath(n.Path)
		all = append(all, gitrev.AnnotateAt("notice", file, 0, "devcouncil/"+safeWord(n.Code), n.Message))
	}

	if len(all) <= MaxAnnotations {
		return all
	}
	omitted := len(all) - (MaxAnnotations - 1)
	capped := append([]string(nil), all[:MaxAnnotations-1]...)
	capped = append(capped, gitrev.Annotate("notice", fmt.Sprintf("%d additional annotation(s) omitted (cap %d).", omitted, MaxAnnotations)))
	return capped
}

func safeAnnotationPath(p string) string {
	if p == "" || p == "[withheld-secret-path]" {
		return ""
	}
	if err := gitrev.CheckRepoPath(p); err != nil {
		return ""
	}
	return p
}

// SARIF 2.1.0 minimal schema structs.
type sarifLog struct {
	Schema  string     `json:"$schema"`
	Version string     `json:"version"`
	Runs    []sarifRun `json:"runs"`
}

type sarifRun struct {
	Tool    sarifTool     `json:"tool"`
	Results []sarifResult `json:"results"`
}

type sarifTool struct {
	Driver sarifDriver `json:"driver"`
}

type sarifDriver struct {
	Name  string      `json:"name"`
	Rules []sarifRule `json:"rules"`
}

type sarifRule struct {
	ID               string       `json:"id"`
	ShortDescription sarifMessage `json:"shortDescription"`
}

type sarifResult struct {
	RuleID    string          `json:"ruleId"`
	Level     string          `json:"level"`
	Message   sarifMessage    `json:"message"`
	Locations []sarifLocation `json:"locations,omitempty"`
}

type sarifMessage struct {
	Text string `json:"text"`
}

type sarifLocation struct {
	PhysicalLocation sarifPhysicalLocation `json:"physicalLocation"`
}

type sarifPhysicalLocation struct {
	ArtifactLocation sarifArtifactLocation `json:"artifactLocation"`
	Region           *sarifRegion          `json:"region,omitempty"`
}

type sarifArtifactLocation struct {
	URI string `json:"uri"`
}

type sarifRegion struct {
	StartLine int `json:"startLine"`
}

// RenderSARIF renders a deterministic SARIF 2.1.0 document containing findings
// and degraded checks using ONLY fixed per-gate messages and URI-encoded relative paths.
func RenderSARIF(r Report) ([]byte, error) {
	rulesMap := map[string]sarifRule{}
	results := make([]sarifResult, 0, len(r.Findings)+len(r.Degraded))

	addRule := func(id, desc string) {
		if _, exists := rulesMap[id]; !exists {
			rulesMap[id] = sarifRule{
				ID:               id,
				ShortDescription: sarifMessage{Text: desc},
			}
		}
	}

	if r.ErrorCode != "" && r.ErrorCode != "degraded" {
		ruleID := "error/" + safeWord(r.ErrorCode)
		msg := fmt.Sprintf("PR gate could not complete verification (%s).", safeWord(r.ErrorCode))
		addRule(ruleID, msg)
		results = append(results, sarifResult{
			RuleID:  ruleID,
			Level:   "error",
			Message: sarifMessage{Text: msg},
		})
	}

	for _, f := range r.Findings {
		ruleID := "gate/" + safeWord(f.GapType)
		msg := FixedGapMessage(f.GapType)
		addRule(ruleID, msg)
		level := "warning"
		if f.Blocking {
			level = "error"
		} else if f.Severity == "low" {
			level = "note"
		}
		res := sarifResult{
			RuleID:  ruleID,
			Level:   level,
			Message: sarifMessage{Text: msg},
		}
		if uri := sarifURI(f.Path); uri != "" {
			loc := sarifPhysicalLocation{ArtifactLocation: sarifArtifactLocation{URI: uri}}
			if f.Line > 0 {
				loc.Region = &sarifRegion{StartLine: f.Line}
			}
			res.Locations = []sarifLocation{{PhysicalLocation: loc}}
		}
		results = append(results, res)
	}

	for _, d := range r.Degraded {
		ruleID := "degraded/" + safeWord(d.Code)
		addRule(ruleID, d.Message)
		res := sarifResult{
			RuleID:  ruleID,
			Level:   "error",
			Message: sarifMessage{Text: d.Message},
		}
		if uri := sarifURI(d.Path); uri != "" {
			res.Locations = []sarifLocation{{
				PhysicalLocation: sarifPhysicalLocation{
					ArtifactLocation: sarifArtifactLocation{URI: uri},
				},
			}}
		}
		results = append(results, res)
	}

	rules := make([]sarifRule, 0, len(rulesMap))
	for _, rule := range rulesMap {
		rules = append(rules, rule)
	}
	sort.Slice(rules, func(i, j int) bool { return rules[i].ID < rules[j].ID })

	doc := sarifLog{
		Schema:  "https://json.schemastore.org/sarif-2.1.0.json",
		Version: "2.1.0",
		Runs: []sarifRun{{
			Tool: sarifTool{
				Driver: sarifDriver{
					Name:  "devcouncil-pr-gate",
					Rules: rules,
				},
			},
			Results: results,
		}},
	}
	b, err := json.MarshalIndent(doc, "", "  ")
	if err != nil {
		return nil, err
	}
	return append(b, '\n'), nil
}

// sarifURI encodes each segment of a validated repo-relative path for SARIF
// artifactLocation.uri. Invalid or withheld paths return "".
func sarifURI(p string) string {
	if p == "" || p == "[withheld-secret-path]" {
		return ""
	}
	if err := gitrev.CheckRepoPath(p); err != nil {
		return ""
	}
	parts := strings.Split(p, "/")
	for i, part := range parts {
		parts[i] = url.PathEscape(part)
	}
	return strings.Join(parts, "/")
}

// RenderedOutputs holds all egress-scrubbed PR Gate render surfaces.
type RenderedOutputs struct {
	JSON        []byte
	Markdown    string
	SARIF       []byte
	Annotations []string
}

// ScrubMarkdown runs fail-closed egress scrubbing on rendered Markdown while
// preserving the sticky comment marker and GitHub's comment size cap.
func ScrubMarkdown(ctx context.Context, md string, s *redact.Scrubber) (string, error) {
	scrubbed, err := s.Scrub(ctx, md)
	if err != nil {
		return "", fmt.Errorf("scrub markdown: %w", err)
	}
	scrubbed = enforceCommentCap(scrubbed)
	if !strings.HasPrefix(scrubbed, StickyMarker+"\n") && scrubbed != StickyMarker {
		return "", errors.New("scrub markdown: sticky comment marker was lost after redaction")
	}
	return scrubbed, nil
}

// ScrubSARIF runs fail-closed egress scrubbing on a rendered SARIF 2.1.0 payload
// and verifies the scrubbed output remains valid JSON.
func ScrubSARIF(ctx context.Context, sarifBytes []byte, s *redact.Scrubber) ([]byte, error) {
	scrubbed, err := s.Scrub(ctx, string(sarifBytes))
	if err != nil {
		return nil, fmt.Errorf("scrub sarif: %w", err)
	}
	out := []byte(scrubbed)
	if !json.Valid(out) {
		return nil, errors.New("scrub sarif: redacted output is not valid JSON")
	}
	return out, nil
}

// ScrubJSON runs fail-closed egress scrubbing on a rendered JSON report and
// verifies the scrubbed output remains valid JSON.
func ScrubJSON(ctx context.Context, jsonBytes []byte, s *redact.Scrubber) ([]byte, error) {
	scrubbed, err := s.Scrub(ctx, string(jsonBytes))
	if err != nil {
		return nil, fmt.Errorf("scrub json: %w", err)
	}
	out := []byte(scrubbed)
	if !json.Valid(out) {
		return nil, errors.New("scrub json: redacted output is not valid JSON")
	}
	return out, nil
}

// ScrubAnnotations runs fail-closed egress scrubbing on each GitHub Actions
// workflow annotation line and verifies no line breaks were introduced.
func ScrubAnnotations(ctx context.Context, anns []string, s *redact.Scrubber) ([]string, error) {
	if len(anns) == 0 {
		return []string{}, nil
	}
	out := make([]string, len(anns))
	for i, a := range anns {
		scrubbed, err := s.Scrub(ctx, a)
		if err != nil {
			return nil, fmt.Errorf("scrub annotation %d: %w", i, err)
		}
		if strings.ContainsAny(scrubbed, "\r\n") {
			return nil, fmt.Errorf("scrub annotation %d: redacted annotation contains line break", i)
		}
		out[i] = scrubbed
	}
	return out, nil
}

// RenderScrubbed renders JSON, Markdown, SARIF, and GitHub workflow annotations
// and scrubs every output surface fail-closed through s (or the pure-Go
// fallback scrubber when s is nil).
func RenderScrubbed(ctx context.Context, r Report, s *redact.Scrubber) (RenderedOutputs, error) {
	jsonBytes, err := RenderJSON(r)
	if err != nil {
		return RenderedOutputs{}, fmt.Errorf("render json: %w", err)
	}
	md := RenderMarkdown(r)
	sarifBytes, err := RenderSARIF(r)
	if err != nil {
		return RenderedOutputs{}, fmt.Errorf("render sarif: %w", err)
	}
	anns := RenderAnnotations(r)

	scrubbedJSON, err := ScrubJSON(ctx, jsonBytes, s)
	if err != nil {
		return RenderedOutputs{}, err
	}
	scrubbedMD, err := ScrubMarkdown(ctx, md, s)
	if err != nil {
		return RenderedOutputs{}, err
	}
	scrubbedSARIF, err := ScrubSARIF(ctx, sarifBytes, s)
	if err != nil {
		return RenderedOutputs{}, err
	}
	scrubbedAnns, err := ScrubAnnotations(ctx, anns, s)
	if err != nil {
		return RenderedOutputs{}, err
	}

	return RenderedOutputs{
		JSON:        scrubbedJSON,
		Markdown:    scrubbedMD,
		SARIF:       scrubbedSARIF,
		Annotations: scrubbedAnns,
	}, nil
}
