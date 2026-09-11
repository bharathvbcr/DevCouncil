package skills

import (
	"embed"
	"io/fs"
)

//go:embed library/*.md
var rawLibrary embed.FS

// Embedded is the domain-skill library shipped inside the binary.
var Embedded = Library{FS: mustLibrary()}

func mustLibrary() fs.FS {
	sub, err := fs.Sub(rawLibrary, "library")
	if err != nil {
		panic("skills: embed library: " + err.Error())
	}
	return sub
}
