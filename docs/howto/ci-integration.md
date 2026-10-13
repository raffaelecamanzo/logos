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

## Publishing a sprint: one pull request

An approved sprint reaches `origin` as **one reviewable pull request**, not as part
of a later release. The sprint runs on the local branch `sprint/N` and is merged
into local `main`; `scripts/publish-sprint.sh` then builds one commit over
`origin/main` whose tree is local `main`'s tree, pushes it to the branch
`sprint/N` on `origin`, and opens the pull request against `main`.

```bash
bash scripts/publish-sprint.sh --sprint 101           # dry run: prints, moves nothing
bash scripts/publish-sprint.sh --sprint 101 --push    # pushes sprint/101, opens the pull request
```

**What the commit is.** Its parent is `origin/main`; its tree is local `main`'s;
it carries the dates of local `main`'s tip, so the dry run and the later `--push`
build the same commit id. Its message is a neutral summary: the title of each
story of the sprint that has a `- **Tracker:** github#N` line, each followed by
`Closes #N`; a sprint with no such story gets a one-line summary. Stories are
read from the sprint's plan file (`docs/planning/sprints/sprint-N.md`) and the
journal (`docs/planning/journal.md`), which must agree on a story's issue number.
Those files are not part of a public checkout: without them the message is the
one-line summary, and the output says so. `--plan DIR` points the script at
another directory holding them, and a `--plan` that lacks the sprint file is
refused rather than summarised. Relative paths among the options are relative to
where you run it, which may be a subdirectory or a linked worktree.

**What it checks before writing anything** (exit 2, no object written):
`origin/main` is an ancestor of local `main` (otherwise a publish would revert
what `origin/main` holds and `main` lacks; run `git fetch origin && git merge
origin/main`); the local branch `sprint/N` exists and is merged into local `main`
(`--unbranched` accepts a sprint that ran before sprint branches, and relaxes
nothing else); local `main`'s tree differs from `origin/main`'s; and the plan
files are sound: a `--plan` directory holds `sprint-N.md`, each Tracker bullet is
`github#N` or `none`, the two files agree on a story's issue number, and a
mirrored story has a title that does not name an issue after a closing keyword
(it would close that issue when the pull request merges). Then
`scripts/check-public-terms.sh` runs on the message and on the tree exactly as
published (read from the commit, never from the working directory). A hit exits 1
before any commit object exists. Last, once the commit id is known, a
remote-tracking `origin/sprint/N`, if present, must be that commit; this one
refusal (exit 2) follows the build and leaves the unreferenced commit object behind.

**Dry run by default.** Without `--push` the script writes one unreferenced commit
object and nothing else: no branch, no pull request, no network call. It prints
the commit id, the message, and the exact `git push origin <sha>:refs/heads/sprint/N`
and `gh pr create --base main --head sprint/N --fill` it would run. With `--push`
it needs `gh` on `PATH` (checked before anything is pushed), reads `origin`
itself: `main` must still equal the local `origin/main`, and `sprint/N` must be absent (it is pushed, never forced) or already the commit built
(a re-run skips the push and retries the pull request). `--fill` makes the pull
request's title and body the commit's message.

**Merging it.** The pull request is merged on GitHub with **Rebase and merge**, so
`origin/main` gains exactly the one commit with its message unchanged. Afterwards
run `git fetch origin && git merge origin/main`: the trees are equal, so the merge
is trivial, and it restores `origin/main` as an ancestor of local `main` for the
next publish.

Exit codes: `0` done, `1` the term gate found a listed term, `2` usage error or a
failed precondition, `3` the term gate could not run or read the wrong number of
files, `4` the push failed, `5` the branch is on `origin` but the pull request
could not be created (re-run to retry it).

The tests are `bash scripts/tests/test-publish-sprint.sh`: every case runs in a
throwaway clone whose `origin` is a local bare repository, with a recording
stand-in for `gh`, and the plan files are fixtures.

## Publishing a release: a tag on a published commit

A release is a **tag**, and the tag marks one commit on `origin/main`. When the
sprints a release holds were already published as pull requests, that commit is
already on `origin/main` and `--tag-only` (below) tags it. When a release is
published without them, `scripts/publish-public.sh` builds **one commit** over
`origin/main`, whatever local history led to it, and tags it. The tag is a source
release: no workflow in this repository runs on it.

```bash
bash scripts/publish-public.sh --version 1.15.7          # dry run: prints, moves nothing
bash scripts/publish-public.sh --version 1.15.7 --push   # tags and pushes
bash scripts/publish-public.sh --bootstrap [--yes]         # record origin/main as merged
bash scripts/publish-public.sh --version 1.15.7 --tag-only [--push]   # tag origin/main
```

**What the commit is.** Its parent is `origin/main`, its tree is the release
tree (`HEAD` by default, `--from REV` otherwise), and its message is every
`CHANGELOG.md` section `origin` has not seen: from `## [X.Y.Z]` down to, not
including, the newest `vMAJOR.MINOR.PATCH` tag reachable from `origin/main`,
newest first. It carries the release commit's dates, so the dry run and the
later `--push` build the same commit id.

**What it checks before writing anything.** `origin/main` must already be an
ancestor of the release commit: a commit built over `origin/main` from a tree that
lacks something merged there would publish it reverted, so otherwise the script
exits 2 naming `git fetch origin && git merge origin/main` and writes no object.
The version must be newer than
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
and every other remote stay where they are in both modes. (`--tag-only` without
`--push` is the exception: its result is the local tag, below.)

**Recording the ancestry.** The commit a publish pushes is not in local history,
so after each publish `origin/main` is not yet an ancestor of local `main`.
`bash scripts/publish-public.sh --bootstrap` records it with
`git merge -s ours origin/main`, which keeps local `main`'s tree. It does so
only when `origin/main`'s tree is the tree of a commit in `HEAD`'s history (the
one the published commit was built from); any other `origin/main` holds content
local history never had, and the script exits 2. It prints the merge by default
and runs it only with `--yes`; either way it needs a branch, a clean tracked tree
and a history shared with `origin/main`, and refuses otherwise. It never pushes.
When the ancestry is already recorded it says so and makes no merge. It takes no
other option.

**Tagging what origin already holds.** `--version X.Y.Z --tag-only` is for a
release that reached `origin/main` by other means (a merged pull request, say).
It creates the annotated tag `vX.Y.Z` on `origin/main`, and only when the release
tree equals `origin/main`'s tree and the version and `CHANGELOG.md` checks above
hold; no commit is built, so no term gate runs. The tag is created locally;
`--push` pushes the tag alone, never forced, and a failed push deletes the tag
the run created.

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
