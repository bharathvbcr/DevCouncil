# Security Policy

## Supported versions

Security fixes are made on `main` and released in the latest published release.
Older releases are not patched; upgrade to the latest release.

| Version | Supported |
|---|---|
| Latest [release](https://github.com/bharathvbcr/DevCouncil/releases) | Yes |
| Anything older | No |

## Reporting a vulnerability

**Do not open a public issue, discussion or pull request for a vulnerability.**

Report it privately through GitHub's private vulnerability reporting:
<https://github.com/bharathvbcr/DevCouncil/security/advisories/new>

Include what you can of:

- the affected component (`devmap`, `dcstore`, `dcverify`, `dcgrep`, the Go host
  `devcouncil`, or the npm launcher) and its version (`--version`);
- the steps or input that trigger the problem, and the impact you expect;
- whether you have a proof of concept, and whether you have shared the report
  with anyone else.

## What to expect

- **Acknowledgement:** within 5 business days of the report.
- **Assessment:** we will confirm or dispute the report and tell you what we
  found, and keep you informed while a fix is prepared.
- **Coordinated disclosure:** please give us a reasonable window to ship a fix
  before publishing details. We aim to fix and disclose within 90 days of the
  report, sooner for actively exploited issues, and will agree a date with you
  when a fix needs longer. Fixes ship with a GitHub security advisory that
  credits the reporter unless you ask us not to.

This is a single-maintainer project, so these are targets and not a contractual
service level.

## Scope

DevCouncil analyses repositories and verifies task evidence locally. Its
execution and input boundaries (which executables it runs, filesystem
protections, resource limits) are documented in
[docs/SECURITY_BOUNDARIES.md](docs/SECURITY_BOUNDARIES.md) and
[docs/security.md](docs/security.md). A report that shows one of those
documented boundaries failing is in scope.

Not vulnerabilities: bugs without a security impact (open an
[issue](https://github.com/bharathvbcr/DevCouncil/issues/new/choose)), and
findings in third-party dependencies that DevCouncil does not reach (report
those upstream; Dependabot tracks the ones we do ship).
