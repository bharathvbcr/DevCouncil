# DevCouncil & DevMap Privacy and Telemetry Notice

**DevCouncil (`devmap`, `devcouncil`, `dcstore`, `dcverify`, `dcgrep`, `dcredact`) is local-first by default and collects zero product telemetry, zero analytics, and zero crash reports.**

---

## 1. Zero Product Telemetry

- DevCouncil binaries contain **no** analytics SDKs, crash reporters, device fingerprint collectors, or background update/phone-home beacons.
- Index stores (`.devmap/`, `.devcouncil/`) live inside the local repository or user-specified store directory and are never sent to DevCouncil maintainers.

---

## 2. Network Egress & Data Flows

| Path | Default State | Destination | Data Transmitted & Protection |
| --- | --- | --- | --- |
| **Core CLI & MCP (`devmap`, `devcouncil`, `dcstore`, `dcverify`, `dcgrep`, `dcredact`)** | Always local | None (local filesystem & stdio/Unix IPC) | None |
| **Remote Cache (`devmap cache push` / `pull`)** | **Off** unless explicitly invoked | Operator-configured S3 / R2 / object store | Requires `DEVMAP_CACHE_ENCRYPTION_KEY`; wraps compressed tarballs in `DMC1` (`ChaCha20-Poly1305` + `HKDF-SHA256`) ciphertext, fails closed if missing/tampered, and strips source literals by default (`--include-literals` required to opt in) |
| **Optional Model-Assisted Verify (Lappi)** | **Off** unless explicitly configured | Operator-configured model endpoint | Applies `policy.WithholdSecretDiff` (secret-pattern paths withheld) and `dc-redact` credential stripping before model inspection |

---

## 3. Verification & Security References

- [Security Policy](SECURITY.md)
- [Security and Execution Boundaries](docs/security.md)
- [Repository Input and Verification Boundaries](docs/SECURITY_BOUNDARIES.md)
- [Third-Party Notices](NOTICE) · [License](LICENSE) (`MIT OR Apache-2.0`)
