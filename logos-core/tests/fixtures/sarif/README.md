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
| `semgrep-low-confidence.sarif` | Semgrep OSS | 1.85.0 | Reconstructed: `semgrep.sarif` with both rules tagged `LOW CONFIDENCE`, the confidence tag Semgrep writes for a rule it rates low (S-817). It stands for a report whose every finding needs review; `semgrep.sarif` itself tags its rules `MEDIUM CONFIDENCE` and `HIGH CONFIDENCE` beside the same `precision: very-high`. |
| `gitleaks.sarif` | gitleaks | v8.18.4 | Reconstructed from the scanner's SARIF writer, with the secret in its snippet as a run without `--redact` writes it; its commit is in no test repository. The recorded reports are in `gitleaks-8.18.4/` (below). |
| `osv-scanner.sarif` | osv-scanner | 1.9.1 | Reconstructed from the scanner's SARIF writer, without the `level` and `ruleIndex` 1.9.x writes on each result: it stands for a result with no level of its own. |
| `osv-scanner-1.9.2.sarif` | osv-scanner | 1.9.2 | **Upstream's own test output**, verbatim: `TestPrintSARIFReport/#00` in `internal/output/__snapshots__/sarif_test.snap` at tag `v1.9.2`. Every result reads `warning`; no rule carries a score. |
| `osv-scanner-2.7.0.sarif` | osv-scanner | 2.7.0 | **Upstream's own test output**, verbatim: the same test case at tag `v2.7.0`. The rule whose advisory has a CVSS score carries it as `security-severity` (`7.5`); the other carries none. `partialFingerprints` reads `[line-hash]`, as upstream's snapshot normalises it. |
| `bandit.sarif` | Bandit | 1.7.10 | Reconstructed from the scanner's SARIF writer. |
| `gosec.sarif` | gosec | 2.21.4 | Reconstructed from the scanner's SARIF writer. |
| `checkov.sarif` | Checkov | 3.2.255 | Reconstructed from the scanner's SARIF writer: `error` on every failed check, as that writer gives one with no severity, and no `security-severity`. |
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

## Which field carries confidence

Recorded for S-817 (FR-SC-31) from the reports above; the table in
`logos-core/src/history/security/parse.rs` (`read_confidence`) reads exactly this and
nothing else. Provenance is the row's own: **Captured** and **upstream output**
rows are real reports, **Reconstructed** ones are as the scanner's writer defines the
field, and none of those six scanners was installed to check them against a run here.

| Scanner | Field in the report | Provenance of the fixture | Read as |
|---------|---------------------|---------------------------|---------|
| Semgrep | rule `properties.tags` entry `LOW`/`MEDIUM`/`HIGH CONFIDENCE`; rule `properties.precision` is `very-high` on every rule and is **not** confidence | Reconstructed; the tag is the one a real run on a user project showed (CR-253: 7 of 15 production findings carried `LOW CONFIDENCE`) | the tag |
| CodeQL | rule `properties.precision` (`low`, `medium`, `high`, `very-high`) | Reconstructed | `high` and `very-high` read `high` |
| Bandit | result `properties.issue_confidence` (`LOW`, `MEDIUM`, `HIGH`); the rule's `precision` repeats the first result's | Reconstructed | the result's own |
| gosec | rule `properties.precision`, its confidence lower-cased (the `tags` entry beside it is severity) | Reconstructed | the rule's |
| Trivy | none: `precision` is `very-high` on every rule | **Captured** (0.74.0) | `unstated` |
| Gitleaks | none: no rule property or result property beyond `tags: []` | **Captured** (8.18.4) | `unstated` |
| OSV-Scanner | none: 1.9.2 and 2.7.0 write no rule property but 2.x's `security-severity` | **Upstream output** | `unstated` |
| Checkov | none | Reconstructed | `unstated` |
| dependency-check | none: rule properties are `cvssv3_*` scores | Reconstructed | `unstated` |

## Which field carries the package, and why OSV-Scanner repeats a result

Recorded for S-808 (CR-252, FR-SC-02). The table in
`logos-core/src/history/security/parse.rs` (`read_package`) reads exactly the
"package" column and nothing else; a dependency result is keyed by its rule, its
path and that name and version (history.db migration 13). Aliases are recorded
for S-809, which merges one advisory across scanners; nothing reads them yet.

| Scanner | Package name and version | Aliases | Provenance |
|---------|--------------------------|---------|------------|
| OSV-Scanner | result `message.text` `Package '<name>@<version>' is vulnerable to '<id>' (also known as '<alias>', …).` — the name may hold an npm scope's `@` | the message's `also known as` list, and the rule's `deprecatedIds` (the id first) | **Upstream output** (1.9.2, 2.7.0); a real 2.7.0 run on desk-picker matches |
| Trivy | result `message.text` lines `Package: <name>` and `Installed Version: <version>`; also the location `message.text` `<path>: <name>@<version>`. A Go version keeps its `v` (`v2.11.0`, where OSV-Scanner writes `2.11.0`) | none: the rule id is the one advisory id (a CVE, else a GHSA or GO id); no other id appears in the result or rule | **Captured** (0.74.0); a real 0.76.0 run on desk-picker matches |
| dependency-check | the first location's `logicalLocations[0].fullyQualifiedName`, the dependency's first package identifier: a purl `pkg:<type>/<namespace>/<name>@<version>`. The message (`<id> - <description>`) names no package, and `partialFingerprints.vulnerabilityHash` is the MD5 of the advisory id alone | none: the rule id is the advisory id | **Reconstructed**; the fields are as upstream's `sarifReport.vsl` template writes them. That template writes `level: "warning"` on every result, as `dependency-check.sarif` reads (S-811 corrected it from `error`) |

A Trivy secret or misconfiguration result, and every other scanner's, names no
package and keeps its key.

**Why OSV-Scanner writes one result twice.** Its SARIF writer
(`internal/output/sarif.go` `PrintSARIFReport`, with `mapIDsToGroupedSARIFFinding`
in `result.go`, read at 2.7.0; the 1.9.2 output shows the same) maps **every id
of an alias group** to the one group, then walks the map's keys and writes the
group's results once per key. A package matched by two records of one alias
group is written twice, byte for byte. A Go module whose advisory has both a Go
vulnerability-database record (`GO-…`) and a GitHub one (`GHSA-…`) is the usual
case: on desk-picker (`aded6ee`, osv-scanner 2.7.0) the 6 repeated `go.mod`
results are exactly the 6 advisories naming a `GO-` alias, and the 29 results
are 23 distinct (rule, path, package) triples. Upstream's own test output repeats
`regex@1.5.1` the same way (`RUSTSEC-2022-0013` and `GHSA-m5pq-gvj9-9vr8`). It
is the scanner's behaviour, not the module being listed twice in `go.mod`.

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

## Which scanners write a constant level

Recorded for S-811 (CR-252, FR-SC-01). The table in
`logos-core/src/history/security/parse.rs` (`fixed_levels`) reads exactly the
"constant" column: a result at that level with no score reads `unrated`. Only
Trivy is installed here (its report is **Captured**), and the sandbox rules forbid
installing another, so each other row is read from the scanner's SARIF writer at
its upstream default branch, 2026-10-28.

| Scanner | Level writer | Constant |
|---------|--------------|----------|
| OSV-Scanner | `internal/output/sarif.go`: `WithLevel("warning")` on every result (1.9.2 and 2.7.0 upstream output) | `warning` |
| Checkov | `checkov/common/output/sarif.py` `_create_results`: `warning` by default, `error` for a failed check with no severity, the severity's mapped level otherwise (a skipped check with no severity keeps the `warning` default); a check with a severity also writes `security-severity`. A real desk-picker report: 108 of 108 results `error`, none scored | `error`, `warning` |
| dependency-check | `sarifReport.vsl`: `"level": "warning"` on every result | `warning` |
| Bandit | `bandit/formatters/sarif.py` `level_from_severity` | none |
| gosec | `report/sarif/formatter.go` `getSarifLevel(severity)` | none |
| Semgrep | the rule's severity, in `defaultConfiguration.level` (the fixture shows `warning` and `error`) | none |
| Trivy | `pkg/report/sarif.go` `toSarifErrorLevel(severity)`; the captured report reads `error`, `warning` and `note` | none |
| CodeQL | the query's severity, in `defaultConfiguration.level` | none |
| Gitleaks | no `level` is written (the captured 8.18.4 report and the upstream writer agree) | none |
