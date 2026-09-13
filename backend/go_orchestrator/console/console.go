// Package console owns human CLI presentation and checked output. It is not
// used by MCP or hook protocols. Optional diagnostics never hold up a result.
package console

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"strings"
	"sync"
	"sync/atomic"
	"time"
	"unicode"
	"unicode/utf8"
)

type Policy struct {
	Title, Activity, Progress string
	JSON, Protocol            bool
}

type event struct {
	text  string
	clear bool
	ack   chan struct{}
}

type Session struct {
	humanMu                   sync.Mutex
	humanPending              []byte
	out                       io.Writer
	policy                    Policy
	started                   time.Time
	live, human, color, ascii bool
	width                     func() int
	events                    chan event
	stop                      chan struct{}
	done                      chan struct{}
	quiet                     atomic.Bool
	ended                     atomic.Bool
	wrote                     atomic.Bool
	machine                   atomic.Bool
	outputFailed              atomic.Bool
	lost                      atomic.Uint64
	diagnosticsTotal          atomic.Uint64
	clearOnce                 sync.Once
	context                   context.Context
	diagnosticsMu             sync.Mutex
	diagnostics               []string
}

var active atomic.Pointer[Session]

// Run is the process entry scope. Direct handler tests can still inject os.Stdout
// without a global stream swap or a second CLI implementation.
func Run(policy Policy, ctx context.Context, run func() int) int {
	if policy.Protocol {
		return run()
	}
	s := New(os.Stdout, os.Stderr, policy, ctx)
	if !active.CompareAndSwap(nil, s) {
		s.Finish(1)
		return 1
	}
	defer active.Store(nil)
	return s.Finish(run())
}

func New(out, diagnostic io.Writer, policy Policy, ctx context.Context) *Session {
	s := &Session{out: out, policy: policy, context: ctx, started: time.Now(),
		events: make(chan event, 32), stop: make(chan struct{}), done: make(chan struct{}),
		color: os.Getenv("NO_COLOR") == "", ascii: asciiLocale(), width: func() int { return 80 }}
	s.machine.Store(policy.JSON)
	file, terminal := diagnostic.(*os.File)
	if terminal {
		_, terminal = terminalWidth(file)
	}
	enabled := policy.Progress == "always" || (policy.Progress != "never" && terminal && !policy.JSON)
	s.live = enabled && terminal && os.Getenv("TERM") != "dumb"
	// A live sink must have an independently opened nonblocking description.
	// If the OS cannot supply one, fall back to plain progress.
	var closeSink func()
	if s.live {
		sink, err := openTerminal(file)
		if err != nil {
			s.live = false
		} else {
			diagnostic = sink
			closeSink = func() {
				if sink.Close() != nil {
					s.lost.Add(1)
				}
			}
			s.width = func() int {
				width, ok := terminalWidth(file)
				if !ok {
					return 80
				}
				return width
			}
		}
	}
	if stdout, ok := out.(*os.File); ok {
		_, tty := terminalWidth(stdout)
		s.human = s.live && tty && !policy.JSON
	}
	go s.render(diagnostic, enabled, closeSink)
	return s
}

func Context() context.Context {
	if s := active.Load(); s != nil {
		return s.context
	}
	return context.Background()
}
func Stdout() io.Writer {
	if s := active.Load(); s != nil {
		return outputWriter{s}
	}
	return os.Stdout
}
func Stderr() io.Writer {
	if s := active.Load(); s != nil {
		return diagnosticWriter{s}
	}
	return os.Stderr
}
func ChildOutput() io.Writer {
	if s := active.Load(); s != nil && (s.policy.JSON || s.human) {
		return Stderr()
	}
	return Stdout()
}
func Printf(format string, args ...any) (int, error) { return fmt.Fprintf(Stdout(), format, args...) }
func Println(args ...any) (int, error)               { return fmt.Fprintln(Stdout(), args...) }
func Print(args ...any) (int, error)                 { return fmt.Fprint(Stdout(), args...) }
func Errorf(format string, args ...any) (int, error) { return fmt.Fprintf(Stderr(), format, args...) }
func Errorln(args ...any) (int, error)               { return fmt.Fprintln(Stderr(), args...) }
func Error(args ...any) (int, error)                 { return fmt.Fprint(Stderr(), args...) }
func JSON(value any) error {
	// JSON itself is never decorated, including commands whose default format
	// is a machine receipt even without an explicit --json flag.
	var out io.Writer = os.Stdout
	if s := active.Load(); s != nil {
		s.clear()
		s.machine.Store(true)
		out = rawWriter{s}
	}
	encoder := json.NewEncoder(out)
	encoder.SetIndent("", "  ")
	return encoder.Encode(value)
}

type rawWriter struct{ s *Session }

func (w rawWriter) Write(p []byte) (int, error) {
	w.s.wrote.Store(true)
	n, err := w.s.out.Write(p)
	if err != nil || n != len(p) {
		w.s.outputFailed.Store(true)
		if err == nil {
			err = io.ErrShortWrite
		}
	}
	return n, err
}

type outputWriter struct{ s *Session }

func (w outputWriter) Write(p []byte) (int, error) {
	w.s.clear()
	if !w.s.human || w.s.policy.JSON {
		return (rawWriter{w.s}).Write(p)
	}
	w.s.humanMu.Lock()
	defer w.s.humanMu.Unlock()
	// Child process writes may split a UTF-8 character anywhere. Carry at most
	// three bytes between writes and keep all temporary chunks bounded.
	pending := w.s.humanPending
	w.s.humanPending = nil
	for start := 0; start < len(p); {
		end := min(start+4096, len(p))
		chunk := make([]byte, 0, len(pending)+end-start)
		chunk = append(chunk, pending...)
		chunk = append(chunk, p[start:end]...)
		valid := len(chunk)
		// Locate the start of the final code point, including a lone lead byte.
		tail := len(chunk) - 1
		for tail > 0 && !utf8.RuneStart(chunk[tail]) {
			tail--
		}
		if !utf8.FullRune(chunk[tail:]) {
			valid = tail
		} else {
			valid = len(chunk)
		}
		text := safe(string(chunk[:valid]), w.s.ascii)
		if _, err := (rawWriter{w.s}).Write([]byte(text)); err != nil {
			return start, err
		}
		pending = append(pending[:0], chunk[valid:]...)
		start = end
	}
	w.s.humanPending = pending

	return len(p), nil
}

type diagnosticWriter struct{ s *Session }

func (w diagnosticWriter) Write(p []byte) (int, error) {
	w.s.diagnosticsTotal.Add(1)
	end := min(len(p), 4096)
	if end < len(p) {
		for end > 0 && !utf8.RuneStart(p[end]) {
			end--
		}
	}
	text := string(p[:end])
	if len(p) > 4096 {
		text += " [diagnostic truncated]"
		w.s.lost.Add(1)
	}

	w.s.diagnosticsMu.Lock()
	if len(w.s.diagnostics) < 64 {
		w.s.diagnostics = append(w.s.diagnostics, text)
	}
	w.s.diagnosticsMu.Unlock()
	select {
	case w.s.events <- event{text: text}:
	default:
		w.s.lost.Add(1)
	}
	return len(p), nil
}

func (s *Session) clear() {
	s.clearOnce.Do(func() {
		s.quiet.Store(true)
		ack := make(chan struct{})
		select {
		case s.events <- event{clear: true, ack: ack}:
		default:
			s.lost.Add(1)
			return
		}
		select {
		case <-ack:
		case <-s.done:
		case <-time.After(100 * time.Millisecond):
			s.lost.Add(1)
		}
	})
}

func (s *Session) Finish(code int) int {
	if s.ended.Swap(true) {
		if s.outputFailed.Load() {
			return 1
		}
		return code
	}
	s.clear()
	close(s.stop)
	select {
	case <-s.done:
	case <-time.After(100 * time.Millisecond):
		s.lost.Add(1)
	}
	s.humanMu.Lock()
	if len(s.humanPending) > 0 {
		for _, b := range s.humanPending {
			if _, err := (rawWriter{s}).Write([]byte(fmt.Sprintf("\\x%02x", b))); err != nil {
				code = 1
				break
			}
		}
		s.humanPending = nil
	}
	s.humanMu.Unlock()
	if s.context.Err() != nil && code == 0 {
		code = 1
	}
	if s.policy.JSON && !s.wrote.Load() {
		s.diagnosticsMu.Lock()
		notes := append([]string(nil), s.diagnostics...)
		s.diagnosticsMu.Unlock()
		payload := struct {
			OK          bool     `json:"ok"`
			Diagnostics []string `json:"diagnostics"`
			Dropped     uint64   `json:"dropped_output_updates"`
			Total       uint64   `json:"diagnostics_total"`
			Omitted     uint64   `json:"diagnostics_omitted"`
		}{code == 0, notes, s.lost.Load(), s.diagnosticsTotal.Load(), s.diagnosticsTotal.Load() - uint64(len(notes))}
		data, err := json.Marshal(payload)
		if err != nil {
			return 1
		}
		if _, err := (rawWriter{s}).Write(append(data, '\n')); err != nil {
			return 1
		}
	} else if s.human && !s.machine.Load() {
		status := "Finished"
		if code != 0 {
			status = "Stopped"
		}
		text := fmt.Sprintf("\n  %s / %s / %s in %s\n", "devcouncil", s.policy.Title, status, time.Since(s.started).Round(time.Millisecond))
		if s.color {
			style := "32"
			if code != 0 {
				style = "33"
			}
			text = "\x1b[" + style + "m" + text + "\x1b[0m"
		}
		if _, err := (rawWriter{s}).Write([]byte(text)); err != nil {
			return 1
		}
		if s.lost.Load() != 0 {
			_, _ = (rawWriter{s}).Write([]byte(fmt.Sprintf("  Optional output incomplete: %d update(s) dropped\n", s.lost.Load())))
		}
	}
	if code != 0 && s.lost.Load() != 0 && !s.machine.Load() {
		s.diagnosticsMu.Lock()
		notes := append([]string(nil), s.diagnostics...)
		s.diagnosticsMu.Unlock()
		for _, note := range notes {
			if _, err := (rawWriter{s}).Write([]byte(safe(note, s.ascii))); err != nil {
				return 1
			}
		}
	}
	if s.outputFailed.Load() {
		return 1
	}
	return code
}

func (s *Session) render(out io.Writer, enabled bool, closeSink func()) {
	defer close(s.done)
	if closeSink != nil {
		defer closeSink()
	}
	ticker := time.NewTicker(80 * time.Millisecond)
	defer ticker.Stop()
	painted := false
	tick := 0
	write := func(text string) {
		if n, err := io.WriteString(out, text); err != nil || n != len(text) {
			s.lost.Add(1)
		}
	}
	clear := func() {
		if painted {
			write("\r\x1b[0m\x1b[2K")
			painted = false
		}
	}
	draw := func(first bool) {
		if !s.live || (!first && s.quiet.Load()) {
			return
		}
		write("\r\x1b[2K" + frame(s.policy.Activity, tick, s.width(), s.color, s.ascii, time.Since(s.started)))
		painted = true
		tick++
	}
	if enabled {
		if s.live {
			// clear() waits for the first frame then its clear acknowledgement.
			// A fast command must not suppress the frame by setting quiet first.
			select {
			case <-s.stop:
				return
			default:
			}
			draw(true)
		} else {
			write("[devcouncil] " + safe(s.policy.Activity, s.ascii) + "\n")
		}
	}
	for {
		select {
		case e := <-s.events:
			clear()
			if e.text != "" {
				write(safe(e.text, s.ascii))
				if !strings.HasSuffix(e.text, "\n") {
					write("\n")
				}
			}
			if e.ack != nil {
				close(e.ack)
			}
		case <-ticker.C:
			draw(false)
		case <-s.stop:
			clear()
			for {
				select {
				case e := <-s.events:
					if e.text != "" {
						write(safe(e.text, s.ascii))
						if !strings.HasSuffix(e.text, "\n") {
							write("\n")
						}
					}
					if e.ack != nil {
						close(e.ack)
					}
				default:
					return
				}
			}
		}
	}
}

func frame(activity string, tick, width int, color, ascii bool, elapsed time.Duration) string {
	spinner := []rune("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
	if ascii {
		spinner = []rune("|/-\\")
	}
	nodes := make([]string, 5)
	for i := range nodes {
		nodes[i] = "·"
		if ascii {
			nodes[i] = "."
		}
		if i == (tick/2)%5 {
			nodes[i] = "◉"
			if ascii {
				nodes[i] = "*"
			}
		}
	}
	trail := strings.Join(nodes, "─")
	if ascii {
		trail = strings.Join(nodes, "-")
	}
	prefix := fmt.Sprintf("%c devcouncil ", spinner[tick%len(spinner)])
	if width >= 80 {
		prefix += trail + "  "
	}
	text := prefix + activity + "  " + elapsed.Round(time.Millisecond).String()
	text = fit(strings.ReplaceAll(safe(text, ascii), "\n", "\\n"), width-1)
	if color {
		return "\x1b[36m" + text + "\x1b[0m"
	}
	return text
}

func safe(text string, ascii bool) string {
	var out strings.Builder
	for len(text) > 0 {
		c, n := utf8.DecodeRuneInString(text)
		if c == utf8.RuneError && n == 1 {
			fmt.Fprintf(&out, "\\x%02x", text[0])
			text = text[1:]
			continue
		}
		text = text[n:]
		if (unicode.IsControl(c) && c != '\n') || c == '\u061c' || c == '\u200e' || c == '\u200f' || (c >= '\u202a' && c <= '\u202e') || (c >= '\u2066' && c <= '\u2069') || (ascii && c > 127) {
			fmt.Fprintf(&out, "\\u{%x}", c)
		} else {
			out.WriteRune(c)
		}
	}
	return out.String()
}
func fit(text string, width int) string {
	var out strings.Builder
	used := 0
	for _, c := range text {
		cells := 1
		if c > 127 {
			cells = 2
		}
		if used+cells > width {
			break
		}
		out.WriteRune(c)
		used += cells
	}
	return out.String()
}
func asciiLocale() bool {
	for _, key := range []string{"LC_ALL", "LC_CTYPE", "LANG"} {
		if value := strings.ToLower(os.Getenv(key)); value != "" {
			return !strings.Contains(value, "utf-8") && !strings.Contains(value, "utf8")
		}
	}
	return false
}
