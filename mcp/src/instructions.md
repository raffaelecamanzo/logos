Logos is a local structural code-intelligence server. It maintains an indexed
code graph (symbols, calls, imports, references) of this project.

Its primary use is **scoping work before it is decomposed** — deciding what can
proceed in parallel, finding the precedent to follow, sizing the blast radius.
Navigating by structure while you edit is the secondary mode.

## Primary — before the work is decomposed

Ask these before choosing what to read, not while editing:

- **What does this task touch?** — `context`, given the task in one sentence,
  returns a ranked multi-symbol bundle with code; one call replaces several
  speculative reads. Make it the first move of a task.
- **Can these two work items run in parallel?** — `impact_intersection`, given
  each work item and the symbols it intends to change, returns the pairs whose
  transitive impact sets intersect and the symbols they share, plus the pairs
  that are safely parallel and the coverage limits of that verdict.
- **Has this already been done here?** — `precedent`, given the symbol or file
  about to be written, returns the nodes that play the same structural role —
  sharing a supertype, a registration, or a call shape — each naming why it is
  analogous. The ranking is counted graph facts, never a similarity score, and
  an empty answer states its reason rather than guessing.
- **How large is the change really?** — `impact` for one symbol's transitive
  blast radius. (The reverse-transitive file closure of a changed set is the CLI
  command `logos affected`; it has no tool on this surface.)
- **Which refs collide, and did the merge carry everything?** —
  `branch_overlap`, given the refs about to be merged, returns the symbols more
  than one of them modifies; given a stated merge result it also returns what a
  ref changed and the merge does not have. A clean merge is not a complete merge.

## Secondary — while editing, by the shape of the question

Prefer the graph tools over raw file reads, but match the tool to the question:

- **Relational / cross-file** ("who calls this?", "what breaks if I change
  it?", "where is X used?", dead code) — `node` for one symbol's detail,
  `search` / `explore` to find and group, and `callers` / `callees` for call
  relations. The graph beats grep here.
- **Localized lookups** (a string, a value, a formula inside a file you can
  already name) — a direct read or grep is fine, sometimes faster. Don't force
  the graph on a question grep already answers.
- **Disambiguate by symbol** — prefer a unique name as the entry point. `node`
  on a common bare name (`new`, `map`, `severity`) resolves to one arbitrary
  match; pivot from a unique caller or qualify the path instead.

## This ordering is a hypothesis, not a finding

Scoping is placed first because on the project this guidance was written for,
navigation was a low single-digit percentage of lifetime tool calls and the
task-scoping call fired about once per agent session — evidence that the previous
navigate-while-you-code framing under-delivered, not evidence that this one
works. `logos stats --json` reports calls by tool class and by dev-vs-main
origin; read that breakdown for the project in front of you rather than trusting
the figure that motivated this text, and report what you measure.

## Session-gate protocol (quality tools)

Wrap every editing session in the quality gate:

1. Call `session_start` BEFORE making any edits — it records the quality
   baseline.
2. Make your edits.
3. Call `session_end` AFTER the edits. If it fails (quality regressed below
   the baseline), STOP and fix the regression before continuing — do not pile
   further changes on top of a failing gate.
4. Call `check_rules` before declaring any task "done" — it verifies the
   architecture rules (`rules.toml`) still hold.

`scan` / `rescan` give the full quality report on demand; `dsm` and `evolution`
are deeper diagnostic views.

## `status` vs `health` — not the same thing

- `status` reports INDEX health: file/node/edge counts, store size, and how
  fresh the index is relative to the working tree. Use it to answer "is the
  graph up to date?".
- `health` reports ARCHITECTURE health: database integrity, schema version,
  and graph coherence (including the fast structural-integrity verdict). Use it
  to answer "is the system itself sound?".
- `doctor` runs just the fast structural-integrity check (one node per symbol,
  no orphan rows) and reports `ok`/faults; the same verdict hard-fails
  `session_end` and `check_rules` on drift. Reach for it when a count looks
  wrong or before trusting the graph after heavy syncing.
- `verify` is the DEEP, on-demand check: it reindexes the project into a
  throwaway shadow store and diffs node/edge/file counts and symbol sets against
  the live graph, reporting leaked (live-only) / orphaned (reindex-only) symbols
  and embedding the `doctor` verdict. It catches drift `doctor` cannot — files
  the live store retains but a fresh index would drop. A full reindex is
  seconds-to-minutes, so use it deliberately when `doctor` is clean but a count
  still looks wrong, not on every check.
