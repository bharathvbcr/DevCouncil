module github.com/bharathvbcr/DevCouncil/backend/go_orchestrator

go 1.26.6

// The toolchain CI and releases build with. `go` above is the language floor
// consumers need; govulncheck found os.Root path-handling advisories in
// go1.26.6 (fixed in 1.26.9) that the shipped binaries would carry.
toolchain go1.26.9

require github.com/bharathvbcr/gusset v0.0.0

replace github.com/bharathvbcr/gusset => ../../../gusset
