# CI Integration — the freshen / enforce / report / bless loop

Logos governs code quality with **one mental model**, applied the same way in
your editor, at `git push`, and in CI. Four moves, in order:

| Move | What it does | Command | Blocking? |
|---|---|---|---|
| **Freshen** | Bring the graph in step with the code before anything reads it | `logos index` / `logos sync` (every quality command also reconciles first) | — |
| **Enforce** | Fail the build on any regression — rule, structural, admission, or dead-code | [`logos check`](commands.md#check---rules-file---allow-no-rules) → **exit 1** | **Yes** |
| **Report** | Surface the 0–10000 signal without blocking anyone | [`logos scan --json`](commands.md#scan) | No |
| **Bless** | Record the current signal as the new accepted baseline | [`logos gate --save`](commands.md#gate---save---threshold-n---label-l) | **Release only** |

The rule that keeps the loop honest: **enforce and report on every change; bless
only at release.** `logos gate --save` moves the goalposts — it accepts whatever
the signal is *right now* as the new floor. Run it in the PR path and a
regression silently becomes the new normal. Keep `--save` on the release branch
alone; everywhere else the gate only *compares* to the last blessed baseline.

## Where each move already lives

Two of these moves are wired into your local workflow the moment you run
[`logos init`](commands.md#init--i---hooks---workspace---yes---exclude-glob) — this document covers the **CI leg** the hooks
cannot reach (the shared pipeline every push runs), and the release-time bless
cadence.

| Move | Local (installed by `init`) | CI (this doc) |
|---|---|---|
| **Freshen** | `post-commit` / `post-checkout` / `post-merge` hooks run `logos sync` (`init --hooks`) | a `logos index` step |
| **Enforce** | the `pre-push` gate runs `logos check` and **blocks the push** (`init --hooks`) | a `logos check` build step |
| **Report** | the session-start quality-report hook runs [`logos quality-report`](commands.md#quality-report---hook-json) — write-free, always exit 0 — when a session starts, resumes, or is reopened by `/clear` (`init -i`) | a non-blocking `logos scan --json` job |
| **Bless** | — (deliberate, manual) | `logos gate --save` on the release branch only |

The `pre-push` gate is a **local pre-flight**, not a substitute for the CI
enforce step: it can be bypassed with `git push --no-verify` and it never runs
for pushes made from a machine without the binary. CI is the authoritative
enforcement point; the hook just moves the failure earlier and cheaper. See the
[`init` reference](commands.md#init--i---hooks---workspace---yes---exclude-glob) for the full hook set and
[error handling](error-handling.md#the-pre-push-enforcing-gate) for the
`pre-push` exit-1 contract.

## Copy-pasteable recipe (GitHub Actions)

Two workflows: a **PR/push** pipeline that freshens, enforces, and reports on
every change, and a **release** pipeline that blesses. The bless job never
appears in the PR path — that is the whole point.

> **Prerequisite — the rules contract must be tracked in the repository.**
> `logos index` creates `.logos/` with **no contract at all**; only `logos init`
> writes `rules.toml`. Where `.logos/` is gitignored — the common case, since the
> store is a build artifact — a CI checkout therefore has nothing to enforce and
> `logos check` exits **4**, no rules contract loaded
> ([FR-GV-22](../specs/requirements/FR-GV-22.md)). Commit the contract to a
> tracked path first and point `--rules` at it:
> `logos check --no-reconcile --rules ci/rules.toml`.
>
> Before exit 4 existed this state passed **vacuously**, so a pipeline wired from
> this recipe could report success while evaluating nothing at all. That is the
> failure the exit code was added to make visible; do not restore it by passing
> `--allow-no-rules` here.

```yaml
# .github/workflows/logos-quality.yml — runs on every PR and push to main
name: logos-quality
on:
  pull_request:
  push:
    branches: [main]

jobs:
  quality:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0            # full history — evidence tiers read `git log`

      - name: Install logos
        # Build from source, or download a pinned release binary and put it on PATH.
        run: cargo install --path . --bin logos

      # ── Freshen ── build the graph the rest of the pipeline scores.
      - name: Freshen
        run: logos index

      # ── Enforce ── the gate that fails the build. `check` exits 1 on any
      # error-severity violation (rule / structural / admission / dead-code) and
      # names the offending contract. `--no-reconcile` scores the freshly built
      # index without re-syncing.
      - name: Enforce
        run: logos check --no-reconcile

      # ── Report ── non-blocking signal readout. `if: always()` so the signal is
      # still recorded even when Enforce failed above.
      - name: Report
        if: always()
        run: logos scan --no-reconcile --json > logos-signal.json

      - name: Upload signal
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: logos-signal
          path: logos-signal.json
```

```yaml
# .github/workflows/logos-bless.yml — runs ONLY when a release is published
name: logos-bless
on:
  release:
    types: [published]        # never on pull_request — bless is release-only

jobs:
  bless:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - name: Install logos
        run: cargo install --path . --bin logos
      - name: Freshen
        run: logos index
      # ── Bless ── accept the released signal as the new baseline. Commit or
      # publish the updated `.logos/logos.db` (or export the baseline) so the
      # next PR pipeline compares against it.
      - name: Bless
        run: logos gate --save --label "${GITHUB_REF_NAME}"
```

Between releases, the PR pipeline can *compare* to the blessed baseline without
moving it — add a non-blocking, or a hard, floor:

```yaml
      # Optional: fail the PR if the signal regressed past the blessed baseline
      # (or below an absolute floor). This COMPARES; it never saves.
      - name: Gate against baseline
        run: logos gate --no-reconcile --threshold 7500
```

`logos gate` without `--save` exits 1 on a regression past the last saved
baseline (plus a 1-point epsilon) or below `--threshold`; it never re-baselines
on its own except the one benign auto-reset when metric semantics or structural
thresholds change (see [metrics.md](metrics.md#versioned-baseline--automatic-re-baseline-on-semantics-or-threshold-change)).

## Generic recipe (any CI / a plain shell)

No GitHub Actions? The loop is just four commands and their exit codes:

```bash
set -e

logos index                              # freshen

logos check --no-reconcile               # enforce — exit 1 fails the pipeline

logos scan --no-reconcile --json \
  > logos-signal.json                  # report — never blocks; redirect, never a pipe,
                                       # because `tee` would replace scan's exit status

# Bless ONLY on the release branch / tag — guard it explicitly:
if [ "${CI_RELEASE:-}" = "1" ]; then
  logos gate --save --label "$RELEASE_TAG"
fi
```

`--no-reconcile` is the CI-friendly flag: it skips the reconcile-then-score sync
and scores the index you just built with `logos index`, so the pipeline does a
single graph pass. Drop it if you want each command to reconcile defensively.

## Verify the recipe locally

You can reproduce the two load-bearing legs — **enforce fails on a violation**
and **report prints the signal** — against any indexed repo in seconds:

```bash
# Report: the signal is on stdout as JSON.
logos scan --json | python3 -c 'import sys,json; print("signal =", json.load(sys.stdin)["signal"])'
# → signal = 6714   (illustrative — your repo scores its own value)

# Enforce: seed a guaranteed violation and watch `check` exit 1 and name it.
cat > /tmp/seeded-rules.toml <<'RULES'
[constraints]
max_fn_lines = 1        # impossible budget: every function is longer than 1 line
RULES
logos check --rules /tmp/seeded-rules.toml ; echo "exit=$?"
# → …"rule": "max_fn_lines", "message": "fn `…` spans N lines > max_fn_lines = 1"…
# → exit=1
```

A clean tree passes `check` with exit 0; the seeded rules file forces the
error-severity path so you can see exactly what a failing Enforce step looks
like in CI.

## Publishing a release: one commit per tag

This repository publishes each release to `origin` as **one commit** over
`origin/main`, whatever local history led to it. `scripts/publish-public.sh`
builds that commit and tags it. The tag is a source release: no workflow in
this repository runs on it.

```bash
bash scripts/publish-public.sh --version 1.15.7          # dry run: prints, moves nothing
bash scripts/publish-public.sh --version 1.15.7 --push   # tags and pushes
```

**What the commit is.** Its parent is `origin/main`, its tree is the release
tree (`HEAD` by default, `--from REV` otherwise), and its message is every
`CHANGELOG.md` section `origin` has not seen: from `## [X.Y.Z]` down to, not
including, the newest `vMAJOR.MINOR.PATCH` tag reachable from `origin/main`,
newest first. It carries the release commit's dates, so the dry run and the
later `--push` build the same commit id.

**What it checks before writing anything.** The version must be newer than
origin's newest, the release tree's `Cargo.toml` must carry it, `CHANGELOG.md`
must hold its section and origin's newest one below it, and no local tag of that
version may exist. Then `scripts/check-public-terms.sh` runs on the message and on
the release tree exactly as published (read from the commit, never from the
working directory). A hit exits 1 before any commit object exists.

**Dry run by default.** Without `--push` the script writes one unreferenced commit
object and nothing else: no tag, no branch, no network call. It prints the commit
id and the exact `git tag` and `git push` it would run. With `--push` it checks
that origin's `main` still equals the local `origin/main`, creates the annotated
tag `vX.Y.Z` on the commit, and pushes the commit to `main` and the tag in one
`git push --atomic`, never forced. Local `main`, every existing tag, local history
and every other remote stay where they are in both modes.

Exit codes: `0` done, `1` the term gate found a listed term, `2` usage error or a
failed precondition, `3` the term gate could not run or read the wrong number of
files, `4` the push failed (the tag the run created is deleted again).

**What the tag starts.** Nothing in this repository: no workflow runs on a
pushed tag, builds a binary, creates a GitHub Release or updates a package
tap (`.github/workflows/ci.yml` runs on `main` and on pull requests only). The
tag marks the squash commit, and that commit is what a build pins.

The tests are `bash scripts/tests/test-publish-public.sh`: every case runs in a
throwaway clone whose `origin` is a local bare repository.

## See also

- [Commands](commands.md#quality--governance) — full flags for `check`, `scan`, `gate`, `doctor`, `verify`.
- [`init` reference](commands.md#init--i---hooks---workspace---yes---exclude-glob) — the git hooks and the session-start quality-report hook that cover the local legs of the loop.
- [Usage → Quality and governance](usage.md#quality-and-governance) — the same loop from the day-to-day angle.
- [Metrics](metrics.md) — how the 0–10000 signal and the versioned baseline are computed.
- [Error handling](error-handling.md) — exit codes and the fail-soft / fail-loud contract the gate rides on.
