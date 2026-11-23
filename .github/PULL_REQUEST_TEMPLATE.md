## What this changes

<!-- One or two sentences. What behaviour is different after this PR? -->

Closes #

## Why

<!-- The problem being solved. If this fixes a bug, what was the root cause? -->

## Verification

**Required: every box must be genuinely checked, not assumed.**

- [ ] `npm run ci:local` passes locally.
      <!-- If you ran the pieces separately, or a step reported NOT RUN, say so
           below rather than ticking this. -->
- [ ] For Rust changes: `cargo fmt --all -- --check` and the clippy and test
      commands from `.github/workflows/rust.yml` pass (run from `rust/`).
- [ ] I ran the changed behaviour and saw it work. Evidence (commands and
      output) is below.

**If this is a bug fix:**

- [ ] I added a test that **fails against the unfixed code** and passes now.

**Schema impact** (the DevMap store, extraction or code-graph schemas, the
`dc-store` or workbench schema, or a contract under `backend/contracts/`):

- [ ] This change does not alter a schema or contract.
- [ ] This change alters one. The new version numbers are named below, and the
      consumers (GitPulse, Manvi, Jarvis) are updated or have a linked follow-up.
      See the Governance section on shared schemas.

**Documentation and dependencies:**

- [ ] User-visible behaviour is reflected in the README or `docs/`.
- [ ] No new dependency, or it is named below with its justification and has
      been agreed with the maintainer.

**Sign-off:**

- [ ] Every commit is signed off (`git commit -s`), certifying the
      [Developer Certificate of Origin](https://developercertificate.org/).

## Test evidence

<!-- Commands you ran and the relevant output. -->

## Anything reviewers should look at closely?

<!-- Trade-offs you made, alternatives you rejected, parts you are unsure about.
     "Nothing" is a valid answer. -->
