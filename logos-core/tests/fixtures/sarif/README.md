# SARIF fixtures — one per registered scanner

Inputs for the security ingest tests (`logos-core/src/history/security/tests.rs`).
There is one report per registered scanner. Every report targets the same small
fixture project, which the tests rebuild in a temporary git repository:
`app/db.py`, `app/views.py`, `app/settings.py`, `app/config.py`,
`requirements.txt`, `Dockerfile`, `cmd/server/main.go`, `infra/main.tf`.

| File | Scanner | Version | Provenance |
|------|---------|---------|------------|
| `trivy.sarif` | Trivy | 0.74.0 | **Captured.** `trivy fs --scanners vuln,secret,misconfig --format sarif` was run offline: cached DB, `--skip-db-update --offline-scan`, under a sandbox profile that denies network. The only edit is `originalUriBaseIds.ROOTPATH`, rewritten to `file:///home/ci/project/` so no local path is committed. |
| `semgrep.sarif` | Semgrep OSS | 1.85.0 | Reconstructed from the scanner's SARIF writer. |
| `gitleaks.sarif` | gitleaks | v8.18.4 | Reconstructed from the scanner's SARIF writer. |
| `osv-scanner.sarif` | osv-scanner | 1.9.1 | Reconstructed from the scanner's SARIF writer. |
| `bandit.sarif` | Bandit | 1.7.10 | Reconstructed from the scanner's SARIF writer. |
| `gosec.sarif` | gosec | 2.21.4 | Reconstructed from the scanner's SARIF writer. |
| `checkov.sarif` | Checkov | 3.2.255 | Reconstructed from the scanner's SARIF writer. |
| `codeql.sarif` | CodeQL | 2.19.3 | Reconstructed from the scanner's SARIF writer. |
| `dependency-check.sarif` | dependency-check | 10.0.4 | Reconstructed from the scanner's SARIF writer. |

"Reconstructed" means the file reproduces the shape that scanner version writes:
its field names, its nesting, where it puts rules (CodeQL: `tool.extensions`),
CWEs (tags, `external/cwe/…`, gosec's taxonomy relationships) and identity
(`fingerprints`, `partialFingerprints`), its URI style (relative, `%SRCROOT%`,
absolute `file://`), and its snippets. Those scanners are not installed here, and
the test rules forbid installing one or letting one reach the network. When the
scanner registry runs a real scanner on a machine that has it, replace the file
with that scanner's real output.

## The planted secret

`lgs_pl4nted_S3cr3t_q7Vx9Rz2Kw` is planted in the places real scanners leak one:

- the Gitleaks snippet, plus a Gitleaks `partialFingerprints.commitMessage`;
- the Bandit `B105` message (Bandit prints the password literal) and its snippets;
- the gosec `G101` snippet.

The redaction test asserts that the value occurs in these fixtures, then that it
occurs in no `history.db` row and in no byte of the store file. The string has
no real provider's token format, so it never trips a secret-scanning push
protection.
