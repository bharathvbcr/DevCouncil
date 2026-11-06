package flags

import (
	"fmt"
	"os"
	"runtime"
)

// NonRegularConfigError is a config path that is not a regular file.
// Open is not called for the path that was lstat'd, so a FIFO cannot block
// the load. A Windows named-pipe path is refused before lstat: both lstat
// and Open reach CreateFile, which waits while a PIPE_WAIT pipe has no free
// instance. Kind is a word a caller can match: symlink, named pipe,
// directory, socket, device, or non-regular file.
type NonRegularConfigError struct {
	Path string
	Kind string
}

func (e *NonRegularConfigError) Error() string {
	return fmt.Sprintf("%s: refusing to open a %s", e.Path, e.Kind)
}

// danglingOpenError is the error from opening a symlink whose target is gone.
// It does not unwrap. os.IsNotExist follows Unwrap, and a dangling link must
// not be reported as a path that was never created.
type danglingOpenError struct {
	err error
}

func (e *danglingOpenError) Error() string {
	if e == nil || e.err == nil {
		return "dangling symlink"
	}
	return e.err.Error()
}

// lstatPath, statPath and openFollowPath are the calls OpenRegular makes.
// Tests replace them to show a symlink that was dangling at stat and live
// at open. Production uses the os and syscall functions.
var (
	lstatPath      = os.Lstat
	statPath       = os.Stat
	openFollowPath = openFollow
)

// classifyBeforeOpen is unset on unix. A FIFO is a filesystem node, and
// lstat names it without opening. A non-unix init points it at
// windowsNonRegularKind, because that name is not a file lstat can classify.
var classifyBeforeOpen = func(string) (string, bool) { return "", false }

// openSymlinkTarget is false on the non-unix build. Following a symlink
// there can open a named-pipe target and block. Unix follows only a symlink
// that stat said was dangling, and that open is O_NONBLOCK.
var openSymlinkTarget = true

func init() {
	// Referenced from this file so a unix build still compiles the classifier.
	// Only a windows process installs it here. open_regular_other.go installs
	// it for every non-unix build, including windows.
	if runtime.GOOS == "windows" {
		classifyBeforeOpen = windowsNonRegularKind
		openSymlinkTarget = false
	}
}

// OpenRegular opens path only when it is a regular file.
//
// A missing path returns the lstat error, which is os.IsNotExist. A live
// symlink is refused before Open. A dangling symlink is opened so the error
// is the open's own, wrapped so it does not unwrap to os.ErrNotExist. If that
// open succeeds, the link was live by the time it was followed: the file is
// closed and the path is refused as a symlink, so its contents are not
// parsed. The non-unix build refuses every symlink before that follow, so a
// link whose target is a pipe is not opened. A FIFO, directory, socket, or
// other non-regular file is refused before Open.
func OpenRegular(path string) (*os.File, error) {
	if kind, refuse := classifyBeforeOpen(path); refuse {
		return nil, &NonRegularConfigError{Path: path, Kind: kind}
	}
	info, err := lstatPath(path)
	if err != nil {
		return nil, err
	}
	if info.Mode()&os.ModeSymlink != 0 {
		if !openSymlinkTarget {
			return nil, &NonRegularConfigError{Path: path, Kind: "symlink"}
		}
		if _, statErr := statPath(path); statErr != nil {
			if os.IsNotExist(statErr) {
				f, openErr := openFollowPath(path)
				if openErr != nil {
					return nil, &danglingOpenError{err: openErr}
				}
				// Stat said the target was gone. A successful open followed a
				// target that appeared in between, which is a live symlink.
				_ = f.Close()
				return nil, &NonRegularConfigError{Path: path, Kind: "symlink"}
			}
			return nil, statErr
		}
		return nil, &NonRegularConfigError{Path: path, Kind: "symlink"}
	}
	if !info.Mode().IsRegular() {
		return nil, &NonRegularConfigError{Path: path, Kind: fileKind(info.Mode())}
	}
	f, err := openNoFollow(path)
	if err != nil {
		return nil, err
	}
	return rejectUnlessRegular(path, f)
}

func rejectUnlessRegular(path string, f *os.File) (*os.File, error) {
	st, err := f.Stat()
	if err != nil {
		_ = f.Close()
		return nil, err
	}
	if !st.Mode().IsRegular() {
		kind := fileKind(st.Mode())
		_ = f.Close()
		return nil, &NonRegularConfigError{Path: path, Kind: kind}
	}
	return f, nil
}

func fileKind(mode os.FileMode) string {
	switch {
	case mode&os.ModeNamedPipe != 0:
		return "named pipe"
	case mode&os.ModeSocket != 0:
		return "socket"
	case mode&os.ModeDevice != 0:
		return "device"
	case mode.IsDir():
		return "directory"
	case mode&os.ModeSymlink != 0:
		return "symlink"
	default:
		return "non-regular file"
	}
}
