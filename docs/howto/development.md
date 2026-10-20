# Development — hand-run harnesses

This page is for maintainers. It covers the scripts that are deliberately kept
outside every gate: they run real tools or real history, which tests never do,
so no `scripts/gate.sh` tier and no CI workflow calls them. The scanner harness
writes committed evidence that the code and the specifications then rely on: run
it by hand when the thing it measures changes, read what it wrote, and commit
the result. The sync replay is a review check: it prints a verdict and commits
nothing.

## Scanner exclusions: what each scanner does with ignored paths

`scripts/verify-scanner-exclusions.sh` records what the Logos-managed Semgrep,
OSV-Scanner, gosec and CodeQL actually do with a project's ignored directories.
Tests use stub scanners only, so a stub argv test can pin what Logos passes but
never what a scanner does with it. This harness is the evidence an exclusion
needs before it ships: an exclusion is added only where the harness proved it,
and the scan discloses the rest.

**When to run it:** whenever the pinned version of one of the four scanners
changes in the tool manifest (install the new pin first with
`logos security install --update`, or the run uses the copy you already have
and the evidence marks it **not the pin**), and after installing CodeQL
(`logos security install codeql --accept-codeql-licence --consent`, see
[Security](security.md)) to fill in its leg. A scanner whose exclusion flag the
registry calls proven (gosec) is tied to its rows: a change to its pin, its
template or the flag's spelling fails the logos-core test
`a_proven_exclusion_flag_is_the_argv_its_committed_harness_rows_ran` until the
harness has been re-run and its evidence committed.

**What it needs:** the managed copies (`logos security install`), plus `git`,
`python3` and `perl`; gosec also needs a local Go toolchain. It needs no network:
every probe reads which paths the scanner selected, not advisories. Without
CodeQL a run takes well under a minute.

```bash
bash scripts/verify-scanner-exclusions.sh
```

| Option | Meaning |
|---|---|
| `--out DIR` | where the evidence goes (default `scripts/tests/fixtures/scanner-exclusions`) |
| `--work DIR` | where the fixtures and raw outputs go (default a fresh directory under `$TMPDIR`, removed afterwards); must not be inside a git repository |
| `--scanners a,b` | a subset of `semgrep,osv-scanner,gosec,codeql` |
| `--timeout SECONDS` | wall-clock limit per scanner run (default 900) |

Exit codes: `0` the evidence was written, whatever the verdicts; `1` a fixture
could not be built, or the work directory is inside a repository; `2` a usage
error or a missing prerequisite (including a scanner whose pin the tool
manifest does not name); `130`/`143` interrupted, with no evidence written.

### What it runs

It builds three fixtures, each with the `.gitignore` line `/target/`:

- **git** — a git checkout, everything committed except the ignored `target/`;
- **worktree** — a linked worktree of that checkout, the kind every Logos dev
  session uses;
- **non-git** — a plain copy with the same `.gitignore`.

Each plants the same finding in four directories: `app/` (the control),
`target/` (ignored, with a second copy in `target/nested/` that an exclusion
matching `target` alone still reads), and the near misses `src/target/` and
`targets/`. Each scanner found under the user tools directory then runs once
per candidate mechanism — the plain template that walks `.` (`template`; Logos
has passed Semgrep and OSV-Scanner their `explicit-targets` argv instead since
S-772, and gosec its `exclude-dir-tree` argv since S-773), and each exclusion an implementation could use — with the registry's argv, changed only where the
probe needs it offline: Semgrep gets a local one-rule config in place of
`p/default`, and OSV-Scanner lists the lockfiles it read (offline JSON) in place
of its SARIF. The managed copies are found the way Logos finds them, never on
`PATH`: the version the tool manifest pins if it is installed, else the newest
installed version at or above the manifest's minimum. The evidence names the
pin beside the version that ran.

### Reading the evidence

The harness writes two files under `scripts/tests/fixtures/scanner-exclusions/`:

- `verdicts.tsv` — one row per scanner, fixture and mechanism, with the version,
  the verdicts, the paths reported per zone, the first diagnostic the scanner
  printed, and the argv. Re-running the harness on the same versions reproduces
  it.
- `evidence.md` — the same rows per scanner, with the versions and what each
  mechanism means.

| Verdict | Meaning |
|---|---|
| `ignored skipped` / `ignored scanned` | whether the finding in `target/` was reported |
| `near miss kept` / `near miss dropped (…)` | whether both near-miss findings were reported; a dropped one is named |
| `inconclusive` | the control in `app/` was not reported, so the leg proves nothing |
| `failed` | the scanner exited outside its success set or wrote nothing readable; its last stderr line is kept |
| `not installed` | no managed copy; the leg was skipped |

Only a mechanism that reads `ignored skipped` and `near miss kept` on all three
fixtures is anchored. `inconclusive`, `failed` and `not installed` are never a
pass: a scanner with no anchored mechanism is left to disclose that it ran over
ignored paths.

In the hit counts, `unrooted` counts reported paths that do not name a file
relative to the project root. Logos binds a finding to the project by that path,
so a mechanism with unrooted paths needs its ingest adjusted before it can
ship.

## Sync replay: incremental sync against a fresh index, on real commits

`scripts/verify-sync-history.sh` replays pinned commit ranges of a repository
through `logos sync`, the way a dev session's worktree store is maintained, and
checks that the result equals a fresh index of the same tree. The sync tests in
the default gate tier use small fixtures; real history once found an
inbound-edge loss that every fixture missed (CR-236), so this replay is the
check that real history still agrees. A replay takes minutes, so it is an
on-demand review check, not a gate tier.

**When to run it:** at review, for any change to sync, extraction, binding or
the store's write path, with a release build of the branch under review. Run it
beside the estate harnesses that review already runs by hand (the
`LOGOS_REF_WORKSPACE` tests).

**What it needs:** the binary under test, `git`, `python3` and `perl`, and a
repository that holds the pinned commits — this repository's own history. A
clone of the public repository does not hold the Sprint 106 commits; pass your
own ranges there. It needs no network. On this repository the three default
ranges take about two and a half minutes with a release build.

The binary must name drifted edges in `logos verify --json`
(`lost_edges_total`, `extra_edges_total`), which 1.20.2 and earlier releases do
not: with such a binary every range reads `FAILED` (`logos verify wrote no
readable report`) and the run exits 3. To see what the replay reports without
the S-751 fix, build this tree with that fix reverted rather than running an
older release.

```bash
cargo build --release -p logos --no-default-features --features lang-all
bash scripts/verify-sync-history.sh --logos target/release/logos
```

| Option | Meaning |
|---|---|
| `--logos PATH` | the binary under test (default `logos` on `PATH`); it is copied into the work directory first, so a rebuild during the run changes nothing |
| `--repo DIR` | the repository whose history is replayed (default the one holding the script); only read |
| `--range LABEL=BASE..TIP` | a range to replay, repeatable; `BASE` must be an ancestor of `TIP`, with no merge commit between them |
| `--work DIR` | where the clones and raw outputs go (default a fresh directory under `$TMPDIR`, removed afterwards; a directory you name is kept); must not be inside a git repository |
| `--timeout SECONDS` | the wall-clock budget for the whole run (default 3600). Every `logos` command runs with what is left of it and is killed when it runs out; the local git steps between commands are checked against it before the next command |
| `--list` | print the pinned default ranges and exit |

The default ranges are the three Sprint 106 session branches that first showed
the loss: S-681 (5 commits), S-685 (15) and S-747 (9). Before the S-751 fix,
they lost 1, 32 and 171 edges.

Exit codes: `0` every range is equivalent; `1` at least one range drifted;
`2` a usage error, a missing prerequisite, a range that does not resolve, is not
ancestral, is empty or holds a merge, or a work directory inside a repository;
`3` a step of at least one range could not run (see `FAILED` below), so that
range has no verdict;
`4` the time budget ran out, with no verdict; `130`/`143` interrupted.

### What it runs

For each range, in clones under the work directory:

1. **primary** — a `--shared` clone at the base, with the repository's
   `.logos/config.toml` and `rules.toml` copied in (unless the repository
   tracks them, when each checkout carries its own), and `logos index`;
2. **worktree** — a linked worktree of the primary at the base, opened with
   `logos status --json`. It must have no store before that first open, and the
   open must log that it seeded the store from the primary. A failed seed falls
   back to a full index, which leaves a store too, so the replay requires the
   log line rather than the store;
3. **replay** — each commit of the range in order is checked out in the
   worktree, then `logos sync` runs on the paths that commit changed. A rename
   syncs both paths, a deletion syncs the deleted path, and an empty commit
   syncs nothing;
4. **verify** — `logos verify --json` in the worktree;
5. **cold** — a second clone at the tip, `logos index`, and the edge sets of
   the two stores compared by kind, source symbol and target symbol.

The original checkout is only read. Every clone takes its objects from it
through alternates, every `logos` command runs inside a clone, and the
checkout's own `.logos/logos.db` is never opened. Git hooks are off in the
clones, so a checkout runs no sync of its own behind the replay.

### Reading the result

Each range prints its replay counts, the `verify` reading, the cold comparison
and a verdict:

```text
range S-685  ff0bc3bd..5e9a4e46
  replay:  15 commits, 61 paths synced
  verify:  exit 0, ok true; node Δ 0, edge Δ 0, file Δ 0; lost 0, extra 0
  cold:    55164 edges at the tip, 55164 incremental; lost 0, extra 0
  verdict: EQUIVALENT
```

| Verdict | Meaning |
|---|---|
| `EQUIVALENT` | every step ran; `verify` is clean (`ok`, zero node, edge and file deltas, no lost or extra edges) and the cold comparison finds no edge on one side only |
| `DRIFT` | either comparison differs. Up to 20 drifted edges of each side follow, each with its kind, both files and both symbols. `lost` is in the fresh index but not in the worktree store; `extra` is the reverse |
| `FAILED` | a step could not run: a clone, the linked worktree, `index`, the seed, a checkout, a `sync`, `verify`, or reading a store. The step is named, with the first lines of its output where it has any. A failed range is never a pass |

The two comparisons cover each other. `verify` reindexes the worktree's own
tree into a shadow store, and the cold comparison checks the same thing against
an independent clone. The cold comparison prints each kind as its stored
number (2 is `calls`, 3 is `imports`, 11 is `doc_reference`), while `verify`
names it. With `--work DIR`, every command's output is kept under
`DIR/ranges/<label>/out/`, with the three stores beside it.
