# Metrics

Logos scores code quality with **ten orthogonal metrics** combined into a
single deterministic **0–10000 integer signal**. The commands that surface
it — `scan`, `check`, `gate`, `evolution`, `dsm` — are all live;
see [commands.md](commands.md#quality--governance) for flags and exit codes.
This page explains what the numbers mean so your `rules.toml` thresholds can
be chosen deliberately.

## The ten metrics

The signal is built from two families. Metrics **1–5** measure the *shape of
the dependency graph* — how the project's modules relate. Metrics **6–10**
measure the *internal structure of the code itself* — how individual functions
and classes are written. Each metric is normalized into `[0, 1]` (higher is
better):

### Graph-shape metrics (1–5)

### 1. Modularity — *do directories form real modules?*

Newman's Q over the **directory partition**: every symbol belongs to the
directory of its defining file, and dependency edges inside a directory count
as community-internal. A codebase whose directories are cohesive units scores
high; one where every directory reaches into every other scores low.
Normalized `(Q + 0.5) / 1.5`; an edgeless graph scores the neutral `1/3`.

### 2. Acyclicity — *are there dependency cycles?*

Counts strongly-connected components larger than one symbol — mutual
recursion between distinct units. A single function that calls itself
(self-recursion) is *not* a dependency cycle and is not counted.
Normalized `1 / (1 + cycles)` — zero cycles scores a perfect 1,
one cycle exactly ½, and each further cycle decays the score. The same
cycle-detection feeds the `max_cycles` rule in `rules.toml`, so the gate and
the rule can never disagree about what a cycle is.

### 3. Depth — *how long are dependency chains?*

The longest path through the graph after collapsing each cycle to a single
node (so a tangle can't masquerade as healthy layering — a pure cycle has
depth 1). Normalized `1 / (1 + depth/8)`: shallow, wide structures score
higher than deep chains.

### 4. Equality — *is complexity evenly spread?*

`1 − Gini` of per-function cyclomatic complexity. A codebase where a few god
functions concentrate all the complexity scores low; evenly distributed
complexity scores high. Functions whose complexity is unknown are **excluded,
not counted as zero** — missing data never flatters the score. Empty or
single-function projects score the neutral 1.

### 5. Redundancy — *how much code is dead or duplicated?*

`1 − redundant/total`, where a function is redundant if flagged **dead**
(unreachable from any export, route, or configured entry point) or
**duplicate** (identical shape fingerprint) — counted once even when both.

### Structural metrics (6–10)

These five score the production code's *internal* structure, function by
function and class by class. Their detection thresholds are tunable — see
[Tuning the structural thresholds](#tuning-the-structural-thresholds) below
and the [`[metric_thresholds]`](configuration.md#metric_thresholds--tuning-the-structural-dimensions)
table in `rules.toml`.

#### 6. Nesting — *how deeply is control flow nested?*

`1 − deep-nesting ratio`: the fraction of production functions whose maximum
block-nesting depth exceeds `nesting_depth` (default **4**). A flat, guard-clause
style scores high; pyramids of nested `if`/`for`/`while` score low.

#### 7. Conciseness — *how many "brain methods" are there?*

`1 − brain-method ratio`. A **brain method** is a function that trips *all three*
floors at once: cyclomatic complexity ≥ `brain_complexity` (default **15**),
line count ≥ `brain_lines` (default **100**), **and** nesting depth ≥
`brain_nesting` (default **3**). The conjunction targets the genuinely
hard-to-hold-in-your-head functions, not merely long or merely branchy ones.

#### 8. Cohesion (LCOM4) — *do classes hang together?*

`1 − low-cohesion ratio` over classes, using LCOM4: a class is low-cohesion when
its methods and fields split into more than one connected component (the methods
don't share state, so the class is really several classes in a trench coat).
**n/a drop-out:** a repo with no classes that have production methods reports
Cohesion as `n/a` rather than a fabricated score — see
[Applicability and the n/a drop-out](#applicability-and-the-na-drop-out).

#### 9. Focus — *are containers god-objects?*

`1 − god-container ratio`: the fraction of classes/structs that are "god"
containers — those with more than `god_methods` methods (default **20**) **or** a
line span wider than `god_span` (default **500**). The `no_god_containers` budget
([configuration.md](configuration.md#metric_thresholds--tuning-the-structural-dimensions))
counts the *same* containers, so the gate and the dimension never disagree.
Carries the same **n/a drop-out** as Cohesion when the repo has no applicable
containers.

#### 10. Uniqueness — *how much code is near-duplicated?*

`1 − near-clone ratio`: the fraction of production functions that belong to a
near-clone group (structurally similar after identifier/literal normalization,
detected by shingle fingerprints). Distinct from Redundancy's *exact*-duplicate
flag — Uniqueness catches the copy-paste-then-tweak family that exact matching
misses. Its two detection parameters — `clone_similarity` (the Jaccard threshold,
default 0.85) and `clone_min_tokens` (the eligibility floor, default 50) — are
tunable `[metric_thresholds]` keys folded into the hashed effective set, so
re-tuning either re-baselines the gate like any other threshold.

## Production scope — test code is excluded

All ten metrics are computed over the **production subgraph only**. Every
function Logos classifies as test code (`is_test`, the single annotation that
the `[[require_tested]]` rule and the dead-code roots also read) is dropped
before scoring. A
function is `is_test` from extraction evidence (a `#[test]`/`#[cfg(test)]`
marker) **or** from its file: Logos recognizes the common Rust test-file
conventions — a `tests/` integration directory, a bare `tests.rs`, the
snake_case `*_tests.rs` suffix, and the CamelCase `*Tests` form. The exclusion
then applies as:

- **Modularity, Acyclicity, Depth** drop each `is_test` vertex and its incident
  edges, so the graph they measure is the production code's shape alone. The
  `max_cycles` rule follows the same scope — a cycle entirely within test code
  is not an architecture violation.
- **Equality, Redundancy** count production functions only, in both the
  numerator and the denominator. A dead or duplicated *test* never lowers
  Redundancy.
- **Nesting, Conciseness, Uniqueness** count production functions only; a deeply
  nested, brain-method, or near-cloned *test* function is never counted.
- **Cohesion, Focus** exclude test containers entirely, and exclude any
  test-scoped method from the production containers they do score (so a test
  method nested inside a production class never inflates its method count).

The consequence is the property the signal needs to be trustworthy:
**adding or removing tests does not move the number.** Adding structurally
identical test functions leaves every normalized metric and the aggregate
byte-identical. The count of excluded functions is reported as
`test_function_count` on every snapshot (and on `gate`/`session_end`/`scan`
output) — it is the "N test functions excluded from metrics" surface, carried as
the `test_function_count` field rather than a prose line.

## Documentation is excluded too

Documentation nodes (`DocFile`, `DocSection`, and the typed
`Requirement`/`Adr`/`Story` nodes) and every documentation edge are excluded
from scoring by the same principle that drops test code — applied both at graph
hydration (so the five metrics, cycle detection, and DSM never see docs) and at
governance constraint evaluation (`no_god_files`, `max_fan_in`, `max_fan_out`,
and the layer/boundary checks all skip doc kinds). The result is the guarantee
documentation needs to be safe to add: **adding or removing markdown leaves the
aggregate signal byte-identical** and raises no new constraint violations. See
the `[documentation]` table in
[configuration.md](configuration.md#documentation--indexing-markdown).

## The aggregate signal

```
signal = geometric_mean(every applicable dimension) × 10000
```

rounded to an integer, over the canonical order: modularity, acyclicity, depth,
equality, redundancy, nesting, conciseness, cohesion, focus, uniqueness. The
geometric mean was chosen over the arithmetic mean deliberately — three
properties follow:

- **A hard zero collapses the signal to 0.** You cannot compensate a
  catastrophic metric (say, rampant cycles) with good scores elsewhere.
  Anti-gaming by construction.
- **Empty graph reports "n/a", not a number.** With zero nodes the snapshot
  stores an explicit empty marker and a NULL signal rather than the
  misleading mid-range value a naive formula would produce.
- **The five new dimensions are floored, never zeroing.** Nesting, Conciseness,
  Cohesion, Focus, and Uniqueness are clamped to a small floor (0.01) rather
  than 0, so a structural problem *drags* the signal without single-handedly
  collapsing it — only the original five can hard-zero. This keeps the
  structural dimensions informative without making them gameable kill-switches.

### Applicability and the n/a drop-out

Cohesion and Focus only mean something when the repo has the structures they
measure. A repo with no classes (pure functions only), or whose only classes
have no production methods, has nothing for LCOM4 or god-container detection to
score. Rather than fabricate a flattering `1.0`, the engine **drops the
dimension out**: it stores NULL for that metric with an `applicable = 0` flag,
and the geometric-mean denominator shrinks accordingly — a class-less repo is
scored on 8 or 9 dimensions, not 10. The other eight dimensions always apply.
This is the never-fabricate guarantee (see [usage.md](usage.md)) applied to the
metric signal: absent data reads as **n/a**, never as a number.

### Reading the number

The signal is a **trend instrument, not a grade**. Absolute values depend on
project shape and size; what carries meaning is the *direction* across
snapshots and the *delta* a change introduces. Practical guidance:

- Establish a baseline on first scan; gate CI on "no regression below
  baseline − margin" rather than a universal constant.
- A sudden drop traces to exactly one of five named causes — the per-metric
  breakdown in every snapshot says which.

## Determinism guarantee

Same tree in, same signal out — bit-for-bit, across runs and machines. Every
order-sensitive reduction runs in a fixed order and four of the five metrics
accumulate in exact integer arithmetic. Re-scanning an unchanged tree appends
an identical snapshot. This is what makes the signal CI-gateable: a changed
number always means changed code, never floating-point weather.

## Snapshots

Every scan will append one row to `metric_snapshots` inside
`.logos/logos.db` — raw and normalized values for all ten metrics (the five new
dimensions each carry a raw+normalized pair; Cohesion and Focus also carry an
`*_applicable` 0/1 flag for the n/a drop-out), node/edge/function counts, the
excluded `test_function_count`, the `metric_version` the row was scored under,
the `thresholds_hash` of the effective structural thresholds, optional commit
SHA and label, and the signal. The table is **append-only by construction** (no
update/delete path exists in the engine), so the history `evolution` reads
cannot be quietly rewritten. A snapshot written before the structural dimensions
existed carries NULL in the new columns — read as "not scored", distinct from a
real 0.

Inspect raw snapshot data directly:

```bash
sqlite3 .logos/logos.db \
  "SELECT created_at, aggregate_signal, test_function_count, metric_version, commit_sha FROM metric_snapshots ORDER BY created_at DESC LIMIT 10;"
```

### Versioned baseline — automatic re-baseline on semantics or threshold change

A baseline is only comparable to a snapshot scored under the **same metric
semantics** *and* the **same effective thresholds** (the structural detection
thresholds plus the two near-clone parameters). Two fields guard this:

- **`metric_version`** records which semantics each row used; the current version
  is **3** (the ten-dimension signal). A formula change bumps it.
- **`thresholds_hash`** records the effective `[metric_thresholds]` set the row
  was scored under. Editing any threshold (or a budget that feeds one) changes
  the hash.

When `gate` finds a baseline whose `metric_version` differs from the engine's
current version, comparing the two numbers would be meaningless — so instead of
failing against an incomparable baseline, the gate **re-baselines
automatically**: it records a fresh baseline, reports `baseline reset: metric
semantics changed`, and passes informationally. The analogous case for a changed
`thresholds_hash` reports `baseline reset: metric thresholds changed`. Either
way, the *next* gate finds a matching version/hash and compares normally — so an
existing project can upgrade across a semantics change, or an operator can re-tune
a threshold, without a spurious gate failure. (The version guard is checked
first, so a pre-v3 baseline re-baselines once on the upgrade, not twice.)

### Tuning the structural thresholds

The detection thresholds for the five structural dimensions (Nesting,
Conciseness, Cohesion, Focus, Uniqueness) are tunable via the
[`[metric_thresholds]`](configuration.md#metric_thresholds--tuning-the-structural-dimensions)
table in `rules.toml`; omitted keys keep the documented defaults. This includes
Uniqueness's two near-clone parameters (`clone_similarity`, `clone_min_tokens`) —
they are full members of the hashed effective set, not fixed constants. Because
every threshold feeds the `thresholds_hash`, a re-tune triggers the one-time
informational re-baseline described above rather than a silent shift in the
number. The four matching `[constraints]` budgets (`max_nesting_depth`,
`max_brain_methods`, `max_clone_ratio`, `no_god_containers`) turn the same
dimensions into hard `logos check` gates — see
[configuration.md](configuration.md#metric_thresholds--tuning-the-structural-dimensions).

### Worst offenders — naming the cause

`logos scan` reports, per dimension, the **worst offenders** that drag the
score: a deterministically ordered, top-10-capped list of the specific functions
or containers responsible (e.g. the deepest-nested functions, the brain methods,
the god containers, the largest near-clone groups). Each entry names the symbol,
its file, its line, and a short detail (the offending measurement). The list is
report-only — it never gates — and is emitted in the `worst_offenders` field of
`logos scan --json`. It is the "which code do I fix first?" surface that turns a
dropped dimension into an actionable to-do list.

## The non-gated evidence tiers

Two surfaces sit **outside** the 0–10000 signal entirely and never feed it:

- **Hotspots** (`logos hotspots`) — the git-history churn × complexity ranking.
- **Coverage** (`logos coverage ingest` / `status`) — ingested LCOV/Cobertura
  evidence and per-file freshness.

Both are **advisory** and live in a separate store (`.logos/history.db`)
that the quality `gate` never opens. This is enforced **structurally**, not by
convention: the engine holds no `history.db` connection on the gate path, so the
gate physically cannot read evidence data. The guarantee that follows is the
property these tiers need to be safe to run in CI alongside the gate:

> The `gate` / `session_end` signal is **byte-identical** before and after
> `hotspots` or `coverage ingest` — and identical
> again after `.logos/history.db` is deleted. Coverage state (fresh, stale,
> absent) never enters the gated computation.

The evidence tiers are still **deterministic**: `hotspots` anchors its window to
the HEAD committer timestamp (never the wall clock), and re-running at the same
HEAD yields a byte-identical ranking. They obey the same never-fabricate rule as
the metrics — a file with no in-window history is excluded from the board, not
zero-scored; stale coverage reads as absent, not as a stale number.

## Relationship to `rules.toml`

Metrics and rules are complementary: the signal measures *gradual* structural
health; `[constraints]`, `[[layers]]`, and `[[boundaries]]` in
[`rules.toml`](configuration.md#rulestoml--the-architecture-contract) encode
*binary* contracts (`check` exits 1 on violation). A healthy CI setup uses
both: `check` for "never allowed", `gate` for "never worse".
