//go:build !windows

package safefile

import (
	"errors"
	"os"
)

// syncDirectory makes a rename or create in dir durable.
func syncDirectory(dir string) error {
	d, err := os.Open(dir)
	if err != nil {
		return err
	}
	return errors.Join(d.Sync(), d.Close())
}
