---
name: logos-review
description: Review what this branch changed, symbol by symbol, and fix only what is trusted, covered by tests and worth the change. Reads the logos branch delta, gives every entry one verdict (fix, test-first, no-action-warranted or untrusted), edits only the fix symbols, and claims a fix only when a re-run of the delta shows it resolved or improved (or cleared, for an entry new on the branch that is no longer listed) and the tests pass. Use when asked to review the branch, check a change before merging, or find what to improve. Debt mode, on request only, reads the "where to start" list instead.
---

# logos-review

An honest assessment, not work for the sake of work. Every entry of the delta gets
one verdict. A clean branch gets "no entries" and no edit. A fix is claimed only
when the delta itself says so, never because a number went up.

Everything with a yes-or-no answer is a script in this skill's `scripts/` directory
(paths below are relative to the skill's base directory). You judge; the scripts count. Run them, read their output, and do not redo their
work by hand. Each prints a `denominator:` line on stderr: what it looked at. Quote
it when you report. A run that looked at nothing is not a run that found nothing.

`$W` below is a work directory outside the repository (the session scratchpad when
the host names one). Exit codes are the same everywhere: 0 the good answer, 1 the
other answer, 2 usage, 3 the environment or an unreadable input (no python3, no
logos 1.20.0 or newer, no `.logos/`, not a git repository, a file that cannot be
read). Exit 3 ends the review: say what is missing. A script runs in the project
(its working directory, or `--project DIR`).

## Branch mode (the default)

1. **Resolve the base.** `scripts/merge-base.sh [--base REF]` prints the base, the
   merge-base and `identical=yes|no`. With `identical=yes` the branch has no commit
   and no edit past its merge-base: report `NO ENTRIES` and stop. Exit 1 means no
   base resolves: say so and stop.
2. **Run the delta.** `scripts/review-delta.sh --out $W/delta.json [--base REF]`.
   The before-state is the merge-base commit and never another commit. The first run
   on a cold cache indexes that tree once (about 25 s here); say so rather than
   re-running. Exit 1 is "not measured" and its line says why: report that, never
   guess an answer. Edits not yet synced read `stale`: run `logos sync <paths>`
   first if you made any.
3. **Look up test reach.** `scripts/coverage-lookup.sh --delta $W/delta.json --out $W/cov.json`.
4. **Gate.** `scripts/trust-gate.sh --delta $W/delta.json --coverage $W/cov.json --out $W/triage.json`
   prints one row per entry with its proposed verdict and the reason. Zero rows is
   a clean branch.

## The verdicts

Each entry gets exactly one.

| Verdict | Means | Set by |
|---|---|---|
| `untrusted` | the entry's label is not `resolved`: `stale`, `same-file-only`, `no-resolved-edges`, or any label you do not know | the gate; final |
| `test-first` | trusted, but no test reaches the symbol (`uncovered`, `file-only`, `unknown`) | the gate; final |
| `no-action-warranted` | with a reason | the gate for an entry the branch already `resolved` or `improved`, or one that names no single symbol; you for the rest (below) |
| `fix` | trusted, covered by tests, and worth the change | the gate proposes (`fix?` in the table), you confirm |

You may move a proposed `fix` to `no-action-warranted`. You may not move anything
else to `fix`: an untrusted or untested entry does not become editable because the
code looks wrong to you.

The gate also sets two reasons of its own for `no-action-warranted`, beside the four
below: *the branch already resolved or improved it*, and *it names no single symbol* (a
chain of directories, a cycle of modules, a file-level rule: nothing to edit as one unit).

**Judging a `fix?` row.** Read the symbol, then choose. It is a `fix` only when the
change is clearly worth its cost. Otherwise it is `no-action-warranted`, with one
reason in the table's own words:

- *below threshold*: the value sits at its threshold and the code reads fine as it is;
- *cold file*: nobody has touched it in a long time and nothing depends on a change
  (`logos hotspots --json` gives churn when you need it);
- *cost exceeds benefit*: the fix would spread through callers, or add more than it removes;
- *intentional*: a dispatch table, a generated file, a parser state machine, a comment
  that says why.

**`untrusted` is never edited.** For `stale`, say to sync and re-run. For
`same-file-only` or `no-resolved-edges`, say the graph binds no call across files in
that language, so its callers and its tests cannot be seen. Report the entry and move on.

**`test-first` is not edited either.** Name the characterisation test that would pin
the behaviour (what to call, what to assert) and stop: one run is one set of verdicts.
Once the test exists, the next review finds the symbol covered.

## Editing

1. `scripts/edit-scope.sh begin --state $W/scope.state` before the first edit.
2. Edit only the symbols of `fix` entries, one behaviour-preserving change each. Run
   the project's tests for what you touched, with the command the project itself uses.
   Do not commit: the check below compares the worktree with `begin`.
3. `scripts/edit-scope.sh check --state $W/scope.state --allow <file> [--allow <file> ...]`
   with the file of each fix entry and any existing test file the fix had to update.
   Exit 1 names a path outside that set, or says HEAD moved: undo that edit (or that
   commit). A file that was already dirty before you began and is clean now counts as
   a change too, so leave the user's own edits alone.

## Proving a fix

`scripts/remeasure.sh --triage $W/triage.json --out $W/after.json --entry e1 [--entry e3 ...]`
(with the same `--base` the triage used) syncs the edited files, runs the delta again and
compares each entry with its state before.

- `proven`: the entry now reads `resolved` or `improved`, with trust `resolved`.
- `cleared`: the entry was `new` on this branch and is no longer listed. The merge-base
  never had it, so the delta cannot say `resolved`; report it as `cleared` and nothing stronger.
  Check first that the symbol still exists and did not come back under another name:
  deleting or renaming an offender is not fixing it.
- `not proven`: anything else, including a re-run that lists an offender the triage did not
  have (the edit moved the offence into a new helper, say). Report `UNPROVEN` with the
  script's reason and undo your edit.

A fix is claimed only for `proven` or `cleared` **and** a passing test run, with its
count: `P passed, F failed (of T)`. A runner that prints no count is no evidence;
report `UNPROVEN`. **Never cite the aggregate quality number**, before or after, as
evidence or as colour. It is not what was measured. The one thing a fix proves is that
its entry resolved.

## The report

Free prose is fine around these lines; the lines themselves are exact (put them in a
code block). One `ENTRY` per entry, one outcome line per `fix`. No line of the report,
prose included, may use the words signal, aggregate or score, or the number 10000, apart
from the file and name of an entry's subject: the check scans every other line.

```
NO ENTRIES
ENTRY | <verdict> | <file>:<line> <name> | <reason>
FIXED | <file>:<line> <name> | remeasured: <resolved|improved|cleared> <before> -> <after> | tests: <command> | <P> passed, <F> failed (of <T>)
UNPROVEN | <file>:<line> <name> | <why>
```

Start the report with the `base=`, `merge_base=` and `identical=` lines of the merge-base
script and end it with the scripts' denominators (entries listed of entries counted).
Save it and run `scripts/check-report.sh --report $W/report.md --triage $W/triage.json`,
which also holds the verdicts to the gate. Exit 1 lists what is wrong: fix the report,
never loosen the check.

## Debt mode (on request only)

When asked for debt, or "where to start", not for the branch:
`scripts/debt-list.sh --out $W/delta.json` reads the recorded "where to start" list
(at most ten units, ranked by estimated points). Cross-rank it with churn: run
`logos hotspots --json`, and put a unit in a hot file ahead of one in a cold file of equal
rank; a unit in a cold file is a `no-action-warranted` (cold file) candidate. Then the same
steps 3 and 4, the same verdicts, editing and proof. A unit has no `resolved` label of its
own, so the script derives one (dirty file reads `stale`; a language the graph binds within
files only reads its cause). The points are an estimate and decide nothing.

**How a unit is named and matched.** A function or class is known by its `node_id` when that
is a string: the entry's `symbol` is that id and its `name` is the bare name, kept for
reading. The coverage lookup asks for the id first. A fix is proven by the delta against the
merge-base: `remeasure.sh` matches the unit to the offender with the same `node_id` and
dimension, so a symbol of the same name and another id (a rename, a second one in the same
file) is never the unit. A duplicate or near-clone group (`duplicate_group:`, `clone_group:`)
names no single symbol: its entry has no `symbol`, carries its own `group_reason`, reads
`no-symbol` and `no-action-warranted` (`untrusted` first when its file is stale), and is
matched to no offender, so it is reported and never edited.
Against Logos 1.20.0-1.20.2 a unit's id is an integer or null: that unit falls back to name,
file and dimension, and two entries of the re-run with that name, file and dimension are not
proven. The denominators of `debt-list.sh`, `coverage-lookup.sh`, `trust-gate.sh` and
`remeasure.sh` state how many units were known by id, how many were groups and how many
fell back: quote them. An offender that is simply not listed afterwards is unchanged, not
fixed. A list that is `not recorded` is "no scan yet": run `logos scan`.

## What this skill does not do

- It does not edit an entry it did not judge `fix`, and it does not push, commit or
  bless anything; the human decides what to do with the diff.
- It does not relax the quality contract (`rules.toml`, thresholds, a baseline) to make
  an entry go away; the plugin asks the human before that.
- It does not read the delta of any commit but the merge-base.
- Test reach is by test-marked *files*. A unit test inside the offender's own file is not
  seen, so a symbol covered only that way reads `file-only` and goes to `test-first`: the
  safe side. Say so when it applies.
