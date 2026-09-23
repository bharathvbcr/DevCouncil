// Package version is the single Go-side owner of the DevCouncil product
// version.
//
// It is a library rather than a constant in `package main` because `package
// main` is unreachable: `devcouncil/mcp` cannot import the host binary, so the
// MCP server's `serverInfo` — the one field through which every host learns
// which build answered it — was written as its own literal and stopped
// tracking the product at 0.1.0 while releases went to 0.2.3. Nothing could
// have caught that, because there was no shared constant for it to disagree
// with. One importable constant is what makes the disagreement unrepresentable.
//
// `scripts/check-release.mjs` parses this file with
// `/^const Version = "([^"]+)"/m`, so the assignment must stay a quoted string
// literal on one line. It is the Go arm of the release identity check, which
// compares this against package.json, package-lock.json, the Rust workspace
// version and the vX.Y.Z tag before any build minutes are spent.
package version

// Version must match package.json, package-lock.json, rust/Cargo.toml
// [workspace.package], and the release tag.
const Version = "1.3.5"
