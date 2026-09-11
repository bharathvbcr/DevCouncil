# Release and demo verification notes

Attached to GitHub Releases created from `v*.*.*` tags (see `.github/workflows/npm-publish.yml`).

## Verification steps

1. **Fresh npm install** — after this tag's publish + registry smoke:
   ```bash
   npm view devcouncil version
   npm install -g devcouncil@<version>
   devcouncil --help
   ```
2. **Host binary** (from a checkout, no API keys):
   ```bash
   git clone https://github.com/bharathvbcr/DevCouncil.git
   cd DevCouncil
   bash scripts/install.sh
   devcouncil --help
   ```
   The old `bash scripts/build-week-demo.sh` / `dev check --verify` demo was retired with the Python package.
3. **Interactive code graph** — self-contained HTML (not a blank canvas):
   ```bash
   mkdir -p /tmp/devcouncil-demo
   dev graph demo --project-root /tmp/devcouncil-demo --json
   # open /tmp/devcouncil-demo/.devcouncil/graph/demo.html
   ```

Prefer the controlled demo paths above over this repository's historical dogfood dashboard.

## Maintainer release gate

- Tag `vX.Y.Z` runs package verify → npm publish → **registry smoke** → GitHub Release.
- The release is only marked complete after registry smoke succeeds.
- Use `dev report release-health` in CI to separate historical gaps from RC regressions; never treat stale historical blockers as a green release.

See also: [build-week-demo.md](build-week-demo.md).
