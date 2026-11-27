# Contributing to DevCouncil

Thank you for your interest in contributing to DevCouncil!

## Development Workflow

- Product code is the Rust workspace under `rust/` and the Go orchestrator under
  `backend/go_orchestrator/`.
- Start by checking [AGENTS.md](AGENTS.md) and the
  [documentation index](docs/README.md).
- Run the local checks before opening a pull request:
  - Rust workspace: `cargo test --manifest-path rust/Cargo.toml`
  - Go orchestrator: `go -C backend/go_orchestrator test ./...`
  - Repository scripts & contracts: `npm run test:scripts` or `npm run ci:local`

## Licensing & Developer Certificate of Origin (DCO 1.1)

DevCouncil is dual-licensed under the [MIT License](LICENSE-MIT) or the
[Apache License, Version 2.0](LICENSE-APACHE) (`MIT OR Apache-2.0`), at your option.

### Inbound = Outbound Contribution Terms

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this work by you, as defined in the Apache-2.0 license, shall
be dual-licensed as above (`MIT OR Apache-2.0`), without any additional terms
or conditions.

### Developer Certificate of Origin (DCO 1.1)

We use the [Developer Certificate of Origin 1.1](DCO.txt)
([developercertificate.org](https://developercertificate.org/)) rather than a
separate Contributor License Agreement (CLA). Every commit must carry a
`Signed-off-by:` trailer using your real name and email address:

```bash
git commit -s -m "feat: your change"
```

To add a sign-off to existing commits on your branch:

```bash
git commit --amend -s --no-edit          # single commit
git rebase --signoff main                # multiple commits
```
