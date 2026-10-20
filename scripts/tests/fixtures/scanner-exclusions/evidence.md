# Scanner exclusion evidence

Written by `bash scripts/verify-scanner-exclusions.sh` (S-771). Hand-run only: no `gate.sh` tier and no workflow runs it. `verdicts.tsv` beside this file holds every row; re-running the harness on the same versions reproduces it.

- Run: 2026-10-19 on Darwin arm64
- git version 2.50.1 (Apple Git-155); go version go1.27.2 darwin/arm64
- Scanners asked for: semgrep, osv-scanner, gosec, codeql
- Network: none needed — every probe reads path selection offline.

## Fixtures

Each fixture has the `.gitignore` line `/target/` and plants one finding of each scanner's kind in `app/` (the control), `target/` and `target/nested/` (ignored; an exclusion that matches `target` alone still reads the second), and the near misses `src/target/` and `targets/`.

| Fixture | What it is |
|---|---|
| git | a git checkout, everything committed except the ignored `target/` |
| worktree | a linked worktree of that checkout (`.git` is a file) |
| non-git | a plain copy with the same `.gitignore`, no repository above it |

A leg whose control is not reported is `inconclusive`; one that exits outside its success set is `failed`; a scanner that is not installed is `not installed` and runs nothing. None of the three is a pass.

`Hits` counts the reported paths per zone (`other`: in none of the four). `unrooted` counts those that do not name a file relative to the fixture root — a path Logos could not bind to the project as reported; the planted file's zone-bearing name still places it.

## Versions

A run on a version other than the manifest's pin is no evidence for the pin; it is marked **not the pin** (re-run after `logos security install --update`).

| Scanner | Managed version | Pinned | Reports |
|---|---|---|---|
| semgrep | 1.85.0 | 1.85.0 | `1.85.0` |
| osv-scanner | 1.9.2 | 1.9.2 | `osv-scanner version: 1.9.2` |
| gosec | 2.21.4 | 2.21.4 | `Version: 2.21.4` |
| codeql | not installed | - | `-` |

## semgrep

Probe: SARIF result URIs from the registry argv, with a local one-rule config (`eval(...)` in Python) in place of `p/default`, which needs the network.

| Fixture | Mechanism | Ignored | Near miss | Hits | Status |
|---|---|---|---|---|---|
| git | `template` | ignored skipped | near miss kept | app=1 target=0 src/target=1 targets=1 other=0 unrooted=0 | ran (exit 0) |
| git | `exclude-name` | ignored skipped | near miss dropped (src/target) | app=1 target=0 src/target=0 targets=1 other=0 unrooted=0 | ran (exit 0) |
| git | `exclude-anchored` | ignored skipped | near miss dropped (src/target) | app=1 target=0 src/target=0 targets=1 other=0 unrooted=0 | ran (exit 0) |
| git | `explicit-targets` | ignored skipped | near miss kept | app=1 target=0 src/target=1 targets=1 other=0 unrooted=0 | ran (exit 0) |
| worktree | `template` | ignored skipped | near miss kept | app=1 target=0 src/target=1 targets=1 other=0 unrooted=0 | ran (exit 0) |
| worktree | `exclude-name` | ignored skipped | near miss dropped (src/target) | app=1 target=0 src/target=0 targets=1 other=0 unrooted=0 | ran (exit 0) |
| worktree | `exclude-anchored` | ignored skipped | near miss dropped (src/target) | app=1 target=0 src/target=0 targets=1 other=0 unrooted=0 | ran (exit 0) |
| worktree | `explicit-targets` | ignored skipped | near miss kept | app=1 target=0 src/target=1 targets=1 other=0 unrooted=0 | ran (exit 0) |
| non-git | `template` | ignored scanned | near miss kept | app=1 target=2 src/target=1 targets=1 other=0 unrooted=0 | ran (exit 0) |
| non-git | `exclude-name` | ignored skipped | near miss dropped (src/target) | app=1 target=0 src/target=0 targets=1 other=0 unrooted=0 | ran (exit 0) |
| non-git | `exclude-anchored` | ignored skipped | near miss dropped (src/target) | app=1 target=0 src/target=0 targets=1 other=0 unrooted=0 | ran (exit 0) |
| non-git | `explicit-targets` | ignored skipped | near miss kept | app=1 target=0 src/target=1 targets=1 other=0 unrooted=0 | ran (exit 0) |

Argv per mechanism (`<root>` is the fixture, the child's working directory; `<run>` the harness's output directory):

- `template` — no exclusion: Semgrep's own target selection over .: `semgrep scan --config <run>/rule.yml --sarif --output <run>/out.sarif --metrics off --disable-version-check --quiet --exclude .logos .`
- `exclude-name` — --exclude target (a name, as Semgrep matches it): `semgrep scan --config <run>/rule.yml --sarif --output <run>/out.sarif --metrics off --disable-version-check --quiet --exclude .logos --exclude target .`
- `exclude-anchored` — --exclude /target (a root-anchored pattern): `semgrep scan --config <run>/rule.yml --sarif --output <run>/out.sarif --metrics off --disable-version-check --quiet --exclude .logos --exclude /target .`
- `explicit-targets` — every unignored top-level entry as a target: .gitignore app go.mod src targets: `semgrep scan --config <run>/rule.yml --sarif --output <run>/out.sarif --metrics off --disable-version-check --quiet --exclude .logos .gitignore app go.mod src targets`

## osv-scanner

Probe: the lockfiles it scanned (`results[].source.path` of `--format json --experimental-offline --experimental-all-packages`, in place of `--format sarif`): path selection without an advisory and without the network.

| Fixture | Mechanism | Ignored | Near miss | Hits | Status |
|---|---|---|---|---|---|
| git | `template` | ignored skipped | near miss kept | app=1 target=0 src/target=1 targets=1 other=1 unrooted=0 | ran (exit 127) |
| git | `explicit-targets` | ignored skipped | near miss kept | app=1 target=0 src/target=1 targets=1 other=1 unrooted=0 | ran (exit 127) |
| worktree | `template` | ignored scanned | near miss kept | app=1 target=2 src/target=1 targets=1 other=1 unrooted=0 | ran (exit 127) |
| worktree | `explicit-targets` | ignored skipped | near miss kept | app=1 target=0 src/target=1 targets=1 other=1 unrooted=0 | ran (exit 127) |
| non-git | `template` | ignored scanned | near miss kept | app=1 target=2 src/target=1 targets=1 other=1 unrooted=0 | ran (exit 127) |
| non-git | `explicit-targets` | ignored skipped | near miss kept | app=1 target=0 src/target=1 targets=1 other=1 unrooted=0 | ran (exit 127) |

Diagnostics the scanner printed (first occurrence of each):

- worktree / template: `Unable to parse git ignores: open <root>/.git/info/exclude: not a directory`

Argv per mechanism (`<root>` is the fixture, the child's working directory; `<run>` the harness's output directory):

- `template` — no exclusion: --recursive . with OSV-Scanner's own .gitignore handling: `osv-scanner --format json --experimental-offline --experimental-all-packages --recursive .`
- `explicit-targets` — every unignored top-level entry: --recursive .gitignore app go.mod src targets: `osv-scanner --format json --experimental-offline --experimental-all-packages --recursive .gitignore app go.mod src targets`

## gosec

Probe: SARIF result URIs (a planted G101 hardcoded credential) from the registry argv.

| Fixture | Mechanism | Ignored | Near miss | Hits | Status |
|---|---|---|---|---|---|
| git | `template` | ignored scanned | near miss kept | app=1 target=2 src/target=1 targets=1 other=0 unrooted=0 | ran (exit 0) |
| git | `exclude-dir-name` | ignored skipped | near miss dropped (src/target, targets) | app=1 target=0 src/target=0 targets=0 other=0 unrooted=0 | ran (exit 0) |
| git | `exclude-dir-caret` | ignored skipped | near miss dropped (targets) | app=1 target=0 src/target=1 targets=0 other=0 unrooted=0 | ran (exit 0) |
| git | `exclude-dir-anchored` | ignored scanned | near miss kept | app=1 target=1 src/target=1 targets=1 other=0 unrooted=0 | ran (exit 0) |
| git | `exclude-dir-tree` | ignored skipped | near miss kept | app=1 target=0 src/target=1 targets=1 other=0 unrooted=0 | ran (exit 0) |
| git | `explicit-packages` | ignored skipped | near miss kept | app=1 target=0 src/target=1 targets=1 other=0 unrooted=3 | ran (exit 0) |
| worktree | `template` | ignored scanned | near miss kept | app=1 target=2 src/target=1 targets=1 other=0 unrooted=0 | ran (exit 0) |
| worktree | `exclude-dir-name` | ignored skipped | near miss dropped (src/target, targets) | app=1 target=0 src/target=0 targets=0 other=0 unrooted=0 | ran (exit 0) |
| worktree | `exclude-dir-caret` | ignored skipped | near miss dropped (targets) | app=1 target=0 src/target=1 targets=0 other=0 unrooted=0 | ran (exit 0) |
| worktree | `exclude-dir-anchored` | ignored scanned | near miss kept | app=1 target=1 src/target=1 targets=1 other=0 unrooted=0 | ran (exit 0) |
| worktree | `exclude-dir-tree` | ignored skipped | near miss kept | app=1 target=0 src/target=1 targets=1 other=0 unrooted=0 | ran (exit 0) |
| worktree | `explicit-packages` | ignored skipped | near miss kept | app=1 target=0 src/target=1 targets=1 other=0 unrooted=3 | ran (exit 0) |
| non-git | `template` | ignored scanned | near miss kept | app=1 target=2 src/target=1 targets=1 other=0 unrooted=0 | ran (exit 0) |
| non-git | `exclude-dir-name` | ignored skipped | near miss dropped (src/target, targets) | app=1 target=0 src/target=0 targets=0 other=0 unrooted=0 | ran (exit 0) |
| non-git | `exclude-dir-caret` | ignored skipped | near miss dropped (targets) | app=1 target=0 src/target=1 targets=0 other=0 unrooted=0 | ran (exit 0) |
| non-git | `exclude-dir-anchored` | ignored scanned | near miss kept | app=1 target=1 src/target=1 targets=1 other=0 unrooted=0 | ran (exit 0) |
| non-git | `exclude-dir-tree` | ignored skipped | near miss kept | app=1 target=0 src/target=1 targets=1 other=0 unrooted=0 | ran (exit 0) |
| non-git | `explicit-packages` | ignored skipped | near miss kept | app=1 target=0 src/target=1 targets=1 other=0 unrooted=3 | ran (exit 0) |

Argv per mechanism (`<root>` is the fixture, the child's working directory; `<run>` the harness's output directory):

- `template` — no exclusion: ./...: `gosec -fmt sarif -out <run>/out.sarif -no-fail -quiet ./...`
- `exclude-dir-name` — -exclude-dir target: `gosec -fmt sarif -out <run>/out.sarif -no-fail -quiet -exclude-dir target ./...`
- `exclude-dir-caret` — -exclude-dir ^target: `gosec -fmt sarif -out <run>/out.sarif -no-fail -quiet -exclude-dir ^target ./...`
- `exclude-dir-anchored` — -exclude-dir ^target$: `gosec -fmt sarif -out <run>/out.sarif -no-fail -quiet -exclude-dir ^target$ ./...`
- `exclude-dir-tree` — -exclude-dir ^target(/|$) (the directory and every directory below it): `gosec -fmt sarif -out <run>/out.sarif -no-fail -quiet -exclude-dir ^target(/|$) ./...`
- `explicit-packages` — the unignored top-level directories as packages (gosec takes no file): ./app/... ./src/... ./targets/...: `gosec -fmt sarif -out <run>/out.sarif -no-fail -quiet ./app/... ./src/... ./targets/...`

## codeql

Not installed under the tools directory: every fixture's leg was skipped. No verdict is recorded for it.
