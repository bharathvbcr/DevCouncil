package flags

import "strings"

// windowsNonRegularKind reports a Win32 path that must not be handed to
// CreateFile. A named pipe lives in a device namespace, not on a volume:
// GetFileAttributesEx often fails it, os.Lstat then calls CreateFile, and
// CreateFile waits while a PIPE_WAIT pipe has no free instance. A drive path
// (\\.\C:\...) and an ordinary UNC share are not special and return false.
func windowsNonRegularKind(path string) (string, bool) {
	if path == "" {
		return "", false
	}
	folded := strings.ToLower(strings.ReplaceAll(path, "/", `\`))
	if kind, ok := ntDeviceKind(folded); ok {
		return kind, true
	}
	if strings.HasPrefix(folded, `\\?\`) {
		return extendedKind(folded[4:])
	}
	if strings.HasPrefix(folded, `\\.\`) {
		return deviceKind(folded[4:])
	}
	if strings.HasPrefix(folded, `\\`) {
		return uncKind(folded[2:])
	}
	return "", false
}

func ntDeviceKind(path string) (string, bool) {
	switch {
	case strings.HasPrefix(path, `\\.\globalroot\device\namedpipe`),
		strings.HasPrefix(path, `\\?\globalroot\device\namedpipe`),
		strings.HasPrefix(path, `\??\globalroot\device\namedpipe`),
		strings.HasPrefix(path, `\device\namedpipe`):
		return "named pipe", true
	case strings.HasPrefix(path, `\\.\globalroot\device\mailslot`),
		strings.HasPrefix(path, `\\?\globalroot\device\mailslot`),
		strings.HasPrefix(path, `\??\globalroot\device\mailslot`),
		strings.HasPrefix(path, `\device\mailslot`):
		return "non-regular file", true
	default:
		return "", false
	}
}

func extendedKind(rest string) (string, bool) {
	if strings.HasPrefix(rest, `unc\`) {
		return uncKind(rest[4:])
	}
	if isPipeName(firstComponent(rest)) {
		return "named pipe", true
	}
	if isMailslotName(firstComponent(rest)) {
		return "non-regular file", true
	}
	return "", false
}

func deviceKind(rest string) (string, bool) {
	if strings.HasPrefix(rest, `unc\`) {
		return uncKind(rest[4:])
	}
	if isDrivePath(rest) {
		return "", false
	}
	if isPipeName(firstComponent(rest)) {
		return "named pipe", true
	}
	// COM1, NUL, PhysicalDrive0, mailslot, and any other \\.\ device.
	// Opening one can block or is not a config file. A drive letter is the
	// only \\.\ form that names a volume path.
	return "non-regular file", true
}

func uncKind(rest string) (string, bool) {
	parts := strings.SplitN(rest, `\`, 3)
	if len(parts) < 2 || parts[0] == "" {
		return "", false
	}
	if isPipeName(parts[1]) {
		return "named pipe", true
	}
	if isMailslotName(parts[1]) {
		return "non-regular file", true
	}
	return "", false
}

func firstComponent(rest string) string {
	head, _, _ := strings.Cut(rest, `\`)
	return head
}

// isPipeName is the pipe namespace after Windows strips trailing dots and
// spaces, including an alternate-data-stream suffix (pipe:stream).
func isPipeName(name string) bool {
	name = strings.TrimRight(name, ". ")
	return name == "pipe" || strings.HasPrefix(name, "pipe:")
}

func isMailslotName(name string) bool {
	name = strings.TrimRight(name, ". ")
	return name == "mailslot" || strings.HasPrefix(name, "mailslot:")
}

func isDrivePath(rest string) bool {
	if len(rest) < 2 || rest[1] != ':' {
		return false
	}
	letter := rest[0]
	if letter < 'a' || letter > 'z' {
		return false
	}
	return len(rest) == 2 || rest[2] == '\\'
}
