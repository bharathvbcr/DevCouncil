//go:build !unix

package flags

import "os"

func openNoFollow(path string) (*os.File, error) {
	return os.Open(path)
}

func openFollow(path string) (*os.File, error) {
	return os.Open(path)
}
