Logos is a local structural code-intelligence server. It maintains an indexed
code graph (symbols, calls, imports, references) of this project so you can
navigate by structure instead of reading files.

## Reach for the graph by the shape of the question

Prefer the graph tools over raw file reads, but match the tool to the question:

- **Relational / cross-file** ("who calls this?", "what breaks if I change
  it?", "where is X used?", dead code, blast radius) — start with `context` (a
  ranked multi-symbol bundle; one call replaces several speculative reads),
  then `node` for one symbol's detail, `search` / `explore` to find and group,
  and `callers` / `callees` / `impact` for call relations and the transitive
  blast radius. The graph beats grep here.
- **Localized lookups** (a string, a value, a formula inside a file you can
  already name) — a direct read or grep is fine, sometimes faster. Don't force
  the graph on a question grep already answers.
- **Disambiguate by symbol** — prefer a unique name as the entry point. `node`
  on a common bare name (`new`, `map`, `severity`) resolves to one arbitrary
  match; pivot from a unique caller or qualify the path instead.

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

`scan` / `rescan` give the full quality report on demand; `dsm`, `evolution`,
and `test_gaps` are deeper diagnostic views.

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
