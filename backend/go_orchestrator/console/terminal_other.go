//go:build !darwin && !linux

package console

import (
	"fmt"
	"os"
)

func terminalWidth(_ *os.File) (int, bool) { return 80, false }
func openTerminal(_ *os.File) (*os.File, error) {
	return nil, fmt.Errorf("animated terminal output unavailable on this platform")
}
