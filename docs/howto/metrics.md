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
Normalized `(Q + 0.5) / 1.5`; an edgeless graph computes the neutral `1/3`.

**Not applicable below 5 edges.** When the graph Modularity is computed on has
fewer than **5** dependency edges (an edgeless graph included), Modularity is
**not applicable**: with so few edges there is no community structure to
measure. A small library whose one to four edges all cross between two
directories — a model depending on its enum — computes Q = −0.5, which
normalizes to exactly 0 and would otherwise zero the whole signal. Instead the
snapshot keeps Modularity's computed values, stores `modularity_applicable = 0`,
and the dimension leaves both the geometric mean and the zero short-circuit —
the same drop-out Cohesion and Focus take (see
[Applicability and the n/a drop-out](#applicability-and-the-na-drop-out)). The
threshold is a fixed part of the metric semantics (since version 6), not a
`rules.toml` setting. Every graph with 5 or more edges scores exactly as before.

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
**duplicate** (identical shape fingerprint, and the function has a body of at
least `duplicate_min_tokens` normalized tokens — default 50, see
[`[metric_thresholds]`](configuration.md#metric_thresholds--tuning-the-structural-dimensions);
bodyless declarations and tiny constant overrides never count) — counted once
even when both.

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

The mean of `1/LCOM4` over production classes. LCOM4 counts the connected
components a class's methods split into: a class whose methods all share state
scores 1, and a class whose methods split into two groups that never touch each
other's state scores LCOM4 2 (its `1/LCOM4` is 0.5) — it is really several
classes in a trench coat, and it is listed as a Cohesion worst offender.
Only methods **with a body** count (since metric-semantics version 7): an
`abstract` declaration, an interface-style method without a default, or a C++
in-class prototype shares no field and calls nothing by construction, so it
would otherwise be a component of its own. A MapStruct mapper of 17 abstract
declarations and 6 helpers is scored over the 6 helpers. A declaration is
still a link, though: two methods that both call the same abstract hook (the
template-method pattern) remain one component, as they always were.
"Sharing state" means two methods read or write a field of their own class
through an own-field access (`this.x`, `self.x`) that binds to exactly one field
of that class; an access that binds to nothing connects nothing. In TypeScript
and TSX, declared class fields, `#private` fields and constructor parameter
properties (`constructor(private readonly http: Client)`) are fields of the
class. Getters, inherited members, and the fields of an abstract class or a
class expression are not, so accesses to them stay unbound. A TypeScript class
whose methods share fields can therefore score higher on Cohesion after a
re-index with this version than it did before.
**n/a drop-out:** a class with no bodied production method is not scored, and a
repo with no class that has one reports Cohesion as `n/a` rather than a
fabricated score — see
[Applicability and the n/a drop-out](#applicability-and-the-na-drop-out).

#### 9. Focus — *are containers god-objects?*

`1 − god-container ratio`: the fraction of classes/structs that are "god"
containers — those with at least `god_methods` methods **with a body** (default
**20**) **or** a line span of at least `god_span` lines (default **500**). A
container of bodyless declarations — a 23-method, 106-line mapper of which 6
methods have a body — is no longer god by method count (since version 7); a
container with 25 bodied methods still is. The `no_god_containers` budget
([configuration.md](configuration.md#metric_thresholds--tuning-the-structural-dimensions))
counts the *same* containers, so the gate and the dimension never disagree.
Carries the same **n/a drop-out** as Cohesion when the repo has no applicable
containers.

Two limits of the method count, both from what extraction records rather than
from the formula. A C++ class whose members are all defined out of line in a
`.cpp` has only bodyless in-class prototypes in the graph (out-of-line
definitions are not captured), so it counts zero bodied methods: it is never
god by method count, only by span, and Cohesion does not score it. And a Rust
`impl` method or a Go receiver method is attached to its module rather than to
its struct, so Rust and Go containers are god by span alone.

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
marker, a JUnit/pytest/PHPUnit test annotation, …) **or** from its file path:

- **Test directories:** a `test/`, `tests/`, `__tests__/` or `spec/` segment —
  unless it sits under a **production source root** (`src/main/…`, or a Gradle
  `*Main` source set such as `commonMain/`), so a library package like
  `src/main/kotlin/org/koin/test/` stays production.
- **Test source sets:** `src/it/` and any Gradle `src/<name>Test/`
  (`commonTest`, `jvmTest`, `androidInstrumentedTest`) are test code.
- **File names:** a bare `tests.rs`; a stem ending `_test`, `_tests`, `_spec`,
  `Test` or `Tests` (`foo_test.go`, `parser_tests.rs`, `UserServiceTest.java`);
  `test_*.py`; and a three-part `*.test.*` / `*.spec.*` tag (`foo.test.ts`). A
  two-part name such as `test.py` is **not** a test by name. Beneath a
  **production source root** (`src/main/…`, a Gradle `*Main` source set) no
  **JVM** file name (`.java`, `.kt`, `.kts`, `.scala`, `.groovy`) marks test — `commonMain/…/KoinTest.kt` and `src/main/…/AutoCloseKoinTest.kt`
  are library classes, since runners collect only from test source sets. Other
  extensions keep their filename rule under a root: a non-JVM tree's
  `src/main/foo.test.ts` is still test. Test source sets (`src/it/`,
  `src/*Test/`) still mark beneath a root, and extraction evidence (`@Test`,
  `#[test]`, PHPUnit markers) is unaffected by the root.
- **PHP:** a `test*` method counts as a test only inside a `*TestCase` subclass
  or in a test file; `#[Test]` and `@test` count anywhere.

The exclusion then applies as:

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
  Anti-gaming by construction. A not-applicable Modularity (fewer than 5 edges)
  is outside this rule: it is not measured, so it cannot be a systemic zero.
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
measure. A repo with no class-like containers (pure functions only) has nothing
for LCOM4 or god-container detection to score, and a repo whose classes have no
production method **with a body** (since version 7 — e.g. only `abstract`
declarations) has nothing for LCOM4. Rather than fabricate a flattering `1.0`, the engine **drops the
dimension out**: it stores NULL for that metric with an `applicable = 0` flag,
and the geometric-mean denominator shrinks accordingly — a class-less repo is
scored on 8 or 9 dimensions, not 10.

Modularity drops out the same way on a graph with fewer than 5 dependency edges,
with one difference: its computed raw and normalized values are still stored
(beside `modularity_applicable = 0`), because Modularity is always computable —
it is the evidence that is too thin. `scan --json` and `quality-report --json`
carry the reason and the count as `modularity_not_applicable`, for example
`{"edges": 3, "min_edges": 5, "reason": "3 of 5 dependency edges — too few for
community structure"}` (`null` when Modularity applies), and the dashboard's
Health view shows **not applicable** with that reason in place of a score.
`evolution` marks the same point's Modularity entry `not_applicable` with that
reason and reports no delta for it across a drop-out, and `gate`'s per-metric
regression detail never names a Modularity that is not applicable on either
side — a dimension outside the signal is not a movement of the signal. The
other seven dimensions always apply.

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
- **A Java repository's signal moves on its first index with Logos 1.6.** Java
  imports, typed calls and inherited calls now bind, and fabricated self-calls
  are gone. Fan-in, coupling, dead code and the signal move with them, in either
  direction. The move is a correction, not a regression: re-establish the baseline
  after that first index (`logos gate --save`) instead of chasing the delta. The
  four new Java type-relation edges (`Extends`, `Implements`, `Instantiates`,
  `TypeUses`) are fenced out of the metric views, so they do not move the signal
  by themselves. Rust and every other language are unchanged.

## Determinism guarantee

Same tree in, same signal out — bit-for-bit, across runs and machines. Every
order-sensitive reduction runs in a fixed order and four of the five metrics
accumulate in exact integer arithmetic. Re-scanning an unchanged tree appends
an identical snapshot. This is what makes the signal CI-gateable: a changed
number always means changed code, never floating-point weather.

## Snapshots

Every scan will append one row to `metric_snapshots` inside
`.logos/logos.db` — raw and normalized values for all ten metrics (the five new
dimensions each carry a raw+normalized pair; Cohesion, Focus and Modularity also
carry an `*_applicable` 0/1 flag for the drop-out — `NULL` on a Modularity row
written before Logos recorded the flag, read as applicable), node/edge/function
counts, the
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
  is **7** (structural metrics count only code with a body — see
  [Metric semantics version 7](#metric-semantics-version-7--declarative-code-stops-counting)).
  A formula change bumps it, so the first `gate` after an upgrade that bumps it
  re-baselines once.
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

### Metric semantics version 7 — declarative code stops counting

Version 7 ([CR-163](../requests/CR-163-structural-metrics-stop-misfiring-on-declarative-code.md))
changes three things at once, so a project re-baselines **once** for all of
them:

| Change | Before (v6) | Since v7 |
|---|---|---|
| **Exact duplicates** (Redundancy, `max_duplicates`) | any two functions with the same shape fingerprint | only functions **with a body** and at least `duplicate_min_tokens` normalized tokens (default 50) |
| **Cohesion** (LCOM4) | every production method of a class | the class's **bodied** methods |
| **Focus** (god containers, `no_god_containers`) | every production method counted toward `god_methods` | only **bodied** methods counted |

**Why signals move.** Before v7 the structural metrics read declarative code as
if it were implementation. A MapStruct `@Mapper` abstract class of 17 abstract
declarations scored LCOM4 23 and was a god container at 106 lines; 26 four-line
constant overrides counted as copy-paste. On a codebase with mappers, generated
interfaces or polymorphic constant overrides, Redundancy, Cohesion and Focus
typically **rise** under v7 and the top offenders change to genuinely
duplicated or tangled code. Redundancy moves in any language, not only where
there is declarative code: every short copy-identical function (fewer than
`duplicate_min_tokens` normalized tokens — generated accessors, one-line
delegates, constant returns) stops counting as a duplicate. A repo with no
bodyless callables and no exact duplicate shorter than the floor scores as
before. The Uniqueness value is unchanged; only its offender list is reordered (largest
duplicated mass first).

**What you see on upgrade.** The first `gate` (or `session_start`) after the
upgrade reports `baseline reset: metric semantics changed` and passes
informationally; the next one compares normally. Nothing needs re-blessing by
hand. The upgrade also re-extracts every file once to record which callables
have a body: until a file is re-extracted its callables count as **bodied**
(never as bodyless), so the scores move only as the fact is recorded — run
`logos scan` (or `logos index`) once to score the whole tree under v7. A bare
`logos sync` is not enough: with no paths it re-reads no file.

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
its file, its line, and a short detail (the offending measurement). Uniqueness
lists near-clone groups by their duplicated mass — members × mean line count,
largest first, then group id, then member id — so a pair of 30-line copies
(`clone group #12 · 2 members × 30 lines`) outranks six 4-line look-alikes, and
a group's members stay adjacent. The list is
report-only — it never gates — and is emitted in the `worst_offenders` field of
`logos scan --json`. It is the "which code do I fix first?" surface that turns a
dropped dimension into an actionable to-do list.

## The non-gated evidence tiers

Three surfaces sit **outside** the 0–10000 signal entirely and never feed it:

- **Hotspots** (`logos hotspots`) — the git-history churn × complexity ranking.
- **Test coverage** (`logos coverage ingest` / `status`) — ingested
  LCOV/Cobertura evidence and per-file freshness.
- **Cross-service coverage** (`logos workspace status`) — the federated
  bound / ambiguous / unbound classification, its unbound reasons, the intake
  split, and the `resolved_cross_service_edges` headline
  ([ADR-53](../specs/architecture/decisions/ADR-53.md),
  [FR-WS-05](../specs/requirements/FR-WS-05.md)).

**The two "coverage" tiers are unrelated**, and the shared word is the only
thing they have in common: the test tier answers "how much of this repo does its
own test suite execute?" from ingested LCOV, while the cross-service tier answers
"do this workspace's calls resolve to a provider in it?" from the graph. They
have separate stores, separate commands and separate requirements
([FR-CV-01](../specs/requirements/FR-CV-01.md) vs
[FR-WS-05](../specs/requirements/FR-WS-05.md)).

All three are **advisory**. The first two live in a separate store
(`.logos/history.db`) that the quality `gate` never opens; the third is
federated read-model state the gate has no path to at all — it is reachable only
through a workspace `EngineRegistry`, which the gate never constructs. In every
case this is enforced **structurally**, not by convention: the engine holds no
`history.db` connection on the gate path, so the gate physically cannot read
evidence data, and a test asserts that none of the cross-service vocabulary
reaches `scan`, `gate` or `check_rules`. The guarantee that follows is the
property these tiers need to be safe to run in CI alongside the gate:

> The `gate` / `session_end` signal is **byte-identical** before and after
> `hotspots`, `coverage ingest` or `workspace status` — and identical
> again after `.logos/history.db` is deleted. Neither test-coverage state
> (fresh, stale, absent) nor any cross-service coverage figure ever enters the
> gated computation.

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
