//go:build !unix

package flags

import "os"

func init() {
	classifyBeforeOpen = windowsNonRegularKind
	openSymlinkTarget = false
}

func openNoFollow(path string) (*os.File, error) {
	if kind, refuse := windowsNonRegularKind(path); refuse {
		return nil, &NonRegularConfigError{Path: path, Kind: kind}
	}
	return os.Open(path)
}

func openFollow(path string) (*os.File, error) {
	if kind, refuse := windowsNonRegularKind(path); refuse {
		return nil, &NonRegularConfigError{Path: path, Kind: kind}
	}
	return os.Open(path)
}
