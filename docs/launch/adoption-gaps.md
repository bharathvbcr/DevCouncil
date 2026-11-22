# DevMap and DevCouncil: adoption gaps and launch plan

Written 2026-11-22. It covers two audit passes and a docs lookup (six Sonnet agents), a Fable strategy
review, and the cards filed from them. Every claim carries a label:

- **verified**: checked against a file, command output or a GitHub API answer that day.
- **vendor**: from the competitor's own site.
- **inferred**: reasoning, not yet checked.

The GitPulse half of this plan, including the cloud conflict-warning work, is in GitPulse's
`docs/launch/ADOPTION_GAPS.md`.

## Why two small devtools got into YC

| | Graphify Labs (YC S26) | Herdr (YC F26) | DevCouncil | GitPulse |
|---|---|---|---|---|
| Stars | ~150k (vendor) | ~25k, $6M seed (vendor) | 4 (verified) | 18 (verified) |
| Install | `uv tool install graphifyy`, then `/graphify .` | brew / curl | clone + Rust + Go 1.27.4 + `../gusset` (verified) | ad-hoc-signed DMG plus `xattr` (verified) |
| Pitch | one sentence | one sentence | starts with architecture (verified) | concept-heavy (verified) |

Both are solo founders who rode a wave: Karpathy's "LLM Wiki" post, and running many agents in
parallel. Both monetized after the traction came. Graphify sells Pro at $15/mo and Teams at
$29/seat, and offers hosted PR review (vendor). Herdr's only stated plan is features that connect
multiple machines (vendor). Graphify's headline 71.5× token cut was measured on 52 files and six
questions, which is where reproducible benchmarks can win.

**Conclusion:** the gap is distribution, not engine quality.

## Wedge

Fable's recommendation, **decided by the owner on 2026-11-22**: lead with one capability. The capability is the collision radar for
parallel agents, with DevMap as its engine: *"your agents are about to overwrite each other; one
command shows where."* DevMap itself leads this repository's README. Its pitch is *"tell your coding
agent what an edit will break before it makes it,"* not "a better code graph". "Better than Graphify"
is the weakest pitch available.

### What happens to the other modules

No module is dropped or deprecated. DevMap leads because it installs prebuilt and answers a
question in seconds. The README's "The rest of DevCouncil" table names every module (devcouncil,
dcstore, dcverify, dcgrep, dcredact). docs/architecture.md keeps the full ownership table.
Today only `devmap-cli` is dist-able. dc-store, dc-verify, dc-grep and dc-redact-cli set
`[package.metadata.dist] dist = false`, and the Go host links the Rust engine through cgo, so it
cannot go through cargo-dist as it is. The host's prebuilt archives are ft-1f7825c8 (host only). The
analysis binaries' prebuilt releases are card ft-3f6a1a26. Until those ship, the
"From source" cells are accurate.

## Gaps, ranked, and where each one went

| # | Gap | Status | Card |
|---|---|---|---|
| 1 | No install without building from source. Release CI already built 4 platform archives, but nothing pointed at them, and the npm package is only a launcher. | **Wired on `feat/launch-install-readme`**: cargo-dist shell, powershell and homebrew installers; the release gate requires each asset. Goes live at v0.2.5 after the owner steps below. | ft-1e3c8f0a |
| 2 | README leads with architecture; no one-sentence hook. | **Rewritten on the branch.** DevMap-first, three install lines, a five-command tour, an agent-question table. Architecture moved to docs/architecture.md. | ft-1e3c8f0a |
| 3 | Surface size: about 45 devmap subcommands, about 20 dev families, 7 binaries with 3 names each. | Open | ft-fbe43919 |
| 4 | About 25 report files at the repository root read as a private workspace. | Open (needs two-signal proof before moving) | ft-fbe43919 |
| 5 | No demo recording. | Open | ft-fbe43919 |
| 6 | First run: an unknown symbol exits 0 with no suggestion; `affected` buries the tests; doubled paths; a 480-character warning on every command. | Open (unknown-symbol case verified on 0.2.5) | ft-f437cc4b |
| 7 | devcouncil.vbcr.dev and gitpulse.vbcr.dev serve identical portfolio HTML, so links unfurl as the author, not the product. | Open (verified) | ft-07d78669 |
| 8 | Verification and the task loop are advertised but not adoptable: gate off by default, host builds only from a checkout, sandbox not implemented. | Proposed to stay out of the launch story (owner decision). The host install is ft-1f7825c8 and ft-5dd0f6f0. | existing |

## Second audit (cold reader, 2026-11-22)

A Sonnet agent followed only the new README on a `--depth 1` clone of BurntSushi/ripgrep (144
files) with the v0.2.3 binary:

- `build --manifest` took 2.6 s; each query took 0.4-1.0 s including process start (verified).
  The README now says "under a second", not "milliseconds".
- v0.2.3 listed two live functions as dead at 0.90 confidence. **On 0.2.5 this does not reproduce**:
  all 139 candidates are at most 0.40, and neither function appears (verified). Shipping v0.2.5 fixes
  the worst first impression.
- `impact parse_config` (the README's placeholder) answers `unavailable` and exits 0, still on 0.2.5
  (verified). The README tour now starts with `devmap search`, and ft-f437cc4b adds did-you-mean.
- `integrate --dry-run` would register global MCP entries as well as repository files. The README
  now says so.

Marketing improvements from the same pass, in order. The cards above carry them.

1. Give each product a real, prerendered page with its own og tags.
2. Fix the first-run path.
3. Make a 15 s demo GIF: build, then `preview` an unsaved deletion, then an agent calls `devmap_impact`.
4. Lead with "Index in 3 seconds; your agent knows what each edit breaks", plus `devmap integrate claude`.
5. Keep a comparison page that states the losses, already in docs/devmap/comparison.md.
6. Post "Show HN: DevMap, a local code graph that tells your agent what an edit will break", then
   r/ClaudeAI, r/rust and the MCP directories.
7. Collect three named before/after stories before the post.
8. For GitPulse: "Herdr runs your agents. GitPulse knows what they're changing."

## Release checklist for the installers (owner steps, not done)

These are outward-facing, so they were not done in the session that wired the installers:

1. Create the `bharathvbcr/homebrew-tap` repository (public, with a README).
2. Add the `HOMEBREW_TAP_TOKEN` secret to this repository. It should be a fine-grained token limited
   to the tap repository, with contents: write.
3. Write `docs/releases/v0.2.5.md`. Merge `feat/launch-install-readme` after a `git merge-tree`
   check against the unmerged land-* branches, which also touch release.yml and quickstart.md. Then tag
   `v0.2.5`.
4. Run every README install line verbatim on clean macOS, Linux and Windows machines. Until then, do
   not merge: the curl, irm and brew lines 404 before the first release that carries installers.

Security impact of the new `publish-homebrew-formula` job: it checks out the tap with
`HOMEBREW_TAP_TOKEN` and `persist-credentials: true` so that it can push the formula. It holds
`contents: read` on this repository, runs only after `host` succeeds, and is skipped for
prereleases. A token scoped to the tap repository bounds what a leak could reach to that one
repository.
