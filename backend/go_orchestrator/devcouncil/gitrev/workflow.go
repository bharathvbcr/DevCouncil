package gitrev

import (
	"strconv"
	"strings"
)

var (
	dataEscaper     = strings.NewReplacer("%", "%25", "\r", "%0D", "\n", "%0A")
	propertyEscaper = strings.NewReplacer("%", "%25", "\r", "%0D", "\n", "%0A", ":", "%3A", ",", "%2C")
)

// EscapeData escapes a GitHub Actions workflow command message the way the
// runner decodes it: `%`, CR and LF, so untrusted values cannot end the command
// and inject another.
func EscapeData(s string) string {
	return dataEscaper.Replace(s)
}

// EscapeProperty escapes a GitHub Actions workflow command property value
// (e.g. `file=`, `title=`): `%`, CR, LF, `:` and `,`, so untrusted file names
// cannot break out of a property or inject additional properties or commands.
func EscapeProperty(s string) string {
	return propertyEscaper.Replace(s)
}

// Annotate renders a GitHub Actions workflow command (`::level::msg`) with an
// escaped message.
func Annotate(level, msg string) string {
	return "::" + sanitizeLevel(level) + "::" + EscapeData(msg)
}

// AnnotateAt renders a GitHub Actions workflow annotation with optional
// escaped `file`, `line`, and `title` properties:
//
//	::error file=...,line=...,title=...::msg
func AnnotateAt(level, file string, line int, title, msg string) string {
	var props []string
	if file != "" {
		props = append(props, "file="+EscapeProperty(file))
	}
	if line > 0 {
		props = append(props, "line="+strconv.Itoa(line))
	}
	if title != "" {
		props = append(props, "title="+EscapeProperty(title))
	}
	lvl := sanitizeLevel(level)
	if len(props) == 0 {
		return "::" + lvl + "::" + EscapeData(msg)
	}
	return "::" + lvl + " " + strings.Join(props, ",") + "::" + EscapeData(msg)
}

// Group renders a GitHub Actions log group start marker with an escaped title.
func Group(title string) string {
	return "::group::" + EscapeData(title)
}

func sanitizeLevel(level string) string {
	switch level {
	case "error", "warning", "notice", "debug":
		return level
	default:
		return "notice"
	}
}
