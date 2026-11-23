# Governance

DevCouncil is an open-source project under the [Apache License 2.0](LICENSE).
This file says who decides what and how changes reach `main` and a release.

## Roles

- **Lead maintainer:** [@bharathvbcr](https://github.com/bharathvbcr). Owns the
  roadmap, has the final say on every decision, is the code owner for the whole
  repository ([`.github/CODEOWNERS`](.github/CODEOWNERS)), and is the only person
  who creates release tags and GitHub Releases.
- **Collaborators:** people the lead maintainer has given write access. They
  review and merge pull requests and triage issues, under the same rules as
  everyone else.
- **Contributors:** anyone who opens an issue, a discussion or a pull request.
  See [CONTRIBUTING.md](CONTRIBUTING.md).

Everyone is bound by the [Code of Conduct](CODE_OF_CONDUCT.md).

## How decisions are made

Most decisions happen in a pull request or an issue: discuss, then the
maintainers decide. The lead maintainer seeks consensus among the people who
have taken part, and decides when there is none. Larger changes (a new
component, a change to a documented contract or schema, a new dependency, a
retired command) should start as a [Discussion](https://github.com/bharathvbcr/DevCouncil/discussions)
or an issue before code is written, so effort is not spent on a change that
will not be accepted. New third-party dependencies need explicit approval from
the lead maintainer.

Security reports are handled privately and are not decided in public; see
[SECURITY.md](SECURITY.md).

## Becoming a maintainer

Collaborator access is by invitation from the lead maintainer. It follows a
sustained record of good changes and reviews in this repository: work that
meets the bar in [CONTRIBUTING.md](CONTRIBUTING.md), reviews that catch real
problems, and conduct consistent with the Code of Conduct. Nobody can ask for
it as of right. Access is removed on request, on inactivity, or for a Code of
Conduct violation.

## Branches and merging

`main` is the only long-lived branch, and it is protected:

- Contributors change it through pull requests only. A pull request needs one
  approving review from a code owner, all conversations resolved, and the
  required status checks passing.
- Force pushes and deletion of `main` are blocked.
- Release tags (`v*`) are protected: they cannot be moved or deleted once
  published.

The branch protection settings live in the repository's GitHub rulesets, which
are the source of truth for the exact list of required checks and for who may
bypass them.

Every commit carries a DCO sign-off (`git commit -s`); see
[CONTRIBUTING.md](CONTRIBUTING.md). Pull requests are merged by a maintainer
once the requirements above are met.

## Releases

Only the lead maintainer cuts a release. The checklist is in
[docs/workflow.md](docs/workflow.md#release-checklist): every workflow green on
the release commit, `npm run ci:local` passing, release notes at
`docs/releases/<tag>.md` naming the schema numbers, then the tag.

Pushing a version tag starts the automated release:

- [`.github/workflows/release.yml`](.github/workflows/release.yml) builds the
  platform archives and installers with cargo-dist; its `create-release` job
  opens the GitHub Release and the build jobs upload to it.
- [`.github/workflows/npm-publish.yml`](.github/workflows/npm-publish.yml)
  publishes the `devcouncil` npm launcher package with provenance.

## Shared schemas and cross-repository coordination

DevCouncil owns the schemas that other projects read: the devmap store,
`dc-store` and workbench schemas (see the [quickstart](docs/quickstart.md)).
[GitPulse](https://github.com/bharathvbcr/GitPulse),
[Manvi](https://github.com/bharathvbcr/Manvi) and Jarvis consume them: GitPulse
vendors copies of the crates, and Manvi and Jarvis pin DevCouncil revisions.

A change to one of these schemas is therefore not complete in this repository
alone. The pull request must say it changes a schema (the pull request template
asks), the new version numbers must be recorded in the release notes, and the
consuming projects must be updated to read the new version before it is
released. After a checkout install, `devcouncil install` runs the schema-parity
check against the consumers and reports any that are stale (see the "Get
started" section of the [README](README.md)).

## Changing this document

Changes to governance go through a pull request like any other change and are
decided by the lead maintainer.
