---
name: security
title: Application Security / Secure Coding Intake
description: Before writing security-sensitive code or assessing a vulnerability, retrieve current guidance for the relevant class (injection, authn/z, crypto, deserialization), the project's existing controls, and the right scanning/test commands — like a senior application-security engineer briefing themselves.
triggers:
  keywords: [security, vulnerability, "secure coding", hardening, "security audit", pentest, "penetration test", owasp, xss, csrf, ssrf, "sql injection", sqli, rce, deserialization, "path traversal", cryptography, encryption, "threat model", sast, dast, cve, sandbox, "least privilege", "input validation"]
  globs: [".semgrep.yml", ".semgrep.yaml", "bandit.yaml", ".bandit", "*.nuclei.yaml", "trivy.yaml", ".snyk"]
---

# Application Security / Secure Coding Intake

Do this **before** writing security-sensitive code or judging a vulnerability. Don't rely on
training data — attack techniques and recommended mitigations evolve, and a plausible-looking
fix can be incomplete or introduce a new hole. Confirm against current guidance (OWASP, the
framework's security docs, the relevant CVE/advisory) and the project's existing controls.

## Establish current state first

1. **Trust boundaries & data flow** — where untrusted input enters and where it reaches a sink
   (DB, shell, filesystem, deserializer, template, HTTP). Map the path this change touches.
2. **Vulnerability class & correct mitigation** — identify the class precisely and use the
   *current* canonical defense: parameterized queries (not escaping) for SQLi; context-aware
   output encoding for XSS; allow-lists + canonicalization for path/SSRF; safe deserializers;
   constant-time comparison for secrets. Avoid blacklist/regex "sanitizers."
3. **AuthN / AuthZ** — every new entry point authenticates and authorizes (object-level too —
   no IDOR); sessions/tokens follow the project's scheme; deny by default.
4. **Secrets & crypto** — secrets from a vault/env, never committed or logged; use vetted
   libraries and current algorithms/parameters (no home-rolled crypto, no MD5/SHA1 for security).
5. **Dependencies & config** — check for known-vulnerable dependencies and insecure defaults
   (CORS, headers, TLS, file permissions). Note anything in scope.

## Build & CLI tools

- Static/secret scanning: `semgrep`, `bandit` (Python), `gosec`, `npm audit`/`pip-audit`,
  `gitleaks`/`trufflehog`, `trivy`/`grype` for images and deps.
- Dynamic/dependency: the project's DAST/`snyk`/`nuclei` flow where present.
- Add or update a test that *proves* the vulnerability is closed (a failing-then-passing case),
  not just that the happy path still works.

## What to record before coding

- The vulnerability class, the trust boundary, and the exact sink involved.
- The current canonical mitigation chosen (and why a weaker one was rejected).
- The authz/secret/crypto requirements the change must satisfy.
- The scan/test commands and the regression test that prove the issue is fixed.

Stay surgical: fix the specific weakness without unrelated refactors, and never weaken an
existing control as a side effect (see core-engineering).
