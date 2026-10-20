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
| `gitleaks.sarif` | gitleaks | v8.18.4 | Reconstructed from the scanner's SARIF writer, with the secret in its snippet as a run without `--redact` writes it; its commit is in no test repository. The recorded reports are in `gitleaks-8.18.4/` (below). |
| `osv-scanner.sarif` | osv-scanner | 1.9.1 | Reconstructed from the scanner's SARIF writer, without the `level` and `ruleIndex` 1.9.x writes on each result: it stands for a result with no level of its own. |
| `osv-scanner-1.9.2.sarif` | osv-scanner | 1.9.2 | **Upstream's own test output**, verbatim: `TestPrintSARIFReport/#00` in `internal/output/__snapshots__/sarif_test.snap` at tag `v1.9.2`. Every result reads `warning`; no rule carries a score. |
| `osv-scanner-2.7.0.sarif` | osv-scanner | 2.7.0 | **Upstream's own test output**, verbatim: the same test case at tag `v2.7.0`. The rule whose advisory has a CVSS score carries it as `security-severity` (`7.5`); the other carries none. `partialFingerprints` reads `[line-hash]`, as upstream's snapshot normalises it. |
| `bandit.sarif` | Bandit | 1.7.10 | Reconstructed from the scanner's SARIF writer. |
| `gosec.sarif` | gosec | 2.21.4 | Reconstructed from the scanner's SARIF writer. |
| `checkov.sarif` | Checkov | 3.2.255 | Reconstructed from the scanner's SARIF writer. |
| `codeql.sarif` | CodeQL | 2.19.3 | Reconstructed from the scanner's SARIF writer. |
| `dependency-check.sarif` | dependency-check | 10.0.4 | Reconstructed from the scanner's SARIF writer. |

The two versioned OSV-Scanner reports pin S-768: the same scan before and after
osv-scanner 2.0.0 began writing `security-severity`, read without running the
scanner (it sends the dependency list to the OSV API).

"Reconstructed" means the file reproduces the shape that scanner version writes:
its field names, its nesting, where it puts rules (CodeQL: `tool.extensions`),
CWEs (tags, `external/cwe/…`, gosec's taxonomy relationships) and identity
(`fingerprints`, `partialFingerprints`), its URI style (relative, `%SRCROOT%`,
absolute `file://`), and its snippets. Those scanners are not installed here, and
the test rules forbid installing one or letting one reach the network. When the
scanner registry runs a real scanner on a machine that has it, replace the file
with that scanner's real output.

## Gitleaks 8.18.4, recorded — `gitleaks-8.18.4/`

**Captured** from the real Gitleaks 8.18.4 with the registry's own arguments
(`detect --redact …`), by `gitleaks-8.18.4/record.sh`, over a repository the
script builds with a fixed identity and fixed dates. The identity tests
(`logos-core/src/history/security/identity_tests.rs`) rebuild it commit for
commit, so its commits get the SHAs the reports name, and assert that they do.

| File | Mode | At |
|------|------|----|
| `history.sarif` | full scan, git history (`--source .`) | C3 |
| `scoped-c1.sarif` | scoped run (`--no-git --source tests/fixtures`) | C1 |
| `scoped-c3.sarif` | scoped run | C3 |

One secret is added in C1, edited on its line in C2 and moved down ten lines
in C3, so history mode reports it three times; a second one is in a file C2
deletes. The reports hold no secret (`--redact`: the snippet reads `REDACTED`),
and they show the column quirk the identity is built around: a line that is
not the first of the text Gitleaks matched reads one column late (`2..42`
against `1..41`). Re-record with
`record.sh <gitleaks-8.18.4-binary> <empty-scratch-dir>`; no test runs it.

`neighbours/` (`record-neighbours.sh`, same conventions) holds two secrets whose
match touches a byte that is not a delimiter: `MY_API_KEY` (Gitleaks' generic
rule starts the match at the keyword, so `_` stands before it) and
`"…".strip()` (a `.` after it). Each is written in C1 on a later line (the late
reading) and edited alone in C2 or C3 (its hunk's first line, the reading as
given): `history.sarif` (full scan at C3) and `scoped.sarif` (`--no-git
--source conf` at C3).

## The planted secret

`lgs_pl4nted_S3cr3t_q7Vx9Rz2Kw` is planted in the places real scanners leak one:

- the Gitleaks snippet, plus a Gitleaks `partialFingerprints.commitMessage`;
- the Bandit `B105` message (Bandit prints the password literal) and its snippets;
- the gosec `G101` snippet.

The redaction test asserts that the value occurs in these fixtures, then that it
occurs in no `history.db` row and in no byte of the store file. The string has
no real provider's token format, so it never trips a secret-scanning push
protection.
