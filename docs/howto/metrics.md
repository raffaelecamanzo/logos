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

**Not applicable below 5 cross-module edges.** A *cross-module edge* is a
production dependency edge between two different directories, both in an
indexed file. When the graph has fewer than **5** of them (an edgeless graph
included), the three graph-shape dimensions — Modularity, Acyclicity and Depth —
are **not applicable** together: with so few edges between modules there is no
module structure to measure. Edges inside one directory do not count, so a
graph whose edges all stay within their directories drops out as well. A small
library whose one to four edges all cross between two directories — a model
depending on its enum — computes Q = −0.5, which normalizes to exactly 0 and
would otherwise zero the whole signal. Instead the snapshot keeps each
dimension's computed values, stores `modularity_applicable`,
`acyclicity_applicable` and `depth_applicable` as 0 beside the
`cross_module_edges` count, and the three dimensions leave both the geometric
mean and the zero short-circuit — the same drop-out Cohesion and Focus take
(see [Applicability and the n/a drop-out](#applicability-and-the-na-drop-out)).
The threshold is a fixed part of the metric semantics (since version 8), not a
`rules.toml` setting.

### 2. Acyclicity — *are there dependency cycles?*

Counts strongly-connected components larger than one symbol — mutual
recursion between distinct units. A single function that calls itself
(self-recursion) is *not* a dependency cycle and is not counted.
Normalized `1 / (1 + cycles)` — zero cycles scores a perfect 1,
one cycle exactly ½, and each further cycle decays the score. The same
cycle-detection feeds the `max_cycles` rule in `rules.toml`, so the gate and
the rule can never disagree about what a cycle is. Not applicable, with
Modularity and Depth, below 5 cross-module edges (see
[Modularity](#1-modularity--do-directories-form-real-modules)).

### 3. Depth — *how long are dependency chains?*

The longest path through the graph after collapsing each cycle to a single
node (so a tangle can't masquerade as healthy layering — a pure cycle has
depth 1). Normalized `1 / (1 + depth/8)`: shallow, wide structures score
higher than deep chains. Not applicable, with Modularity and Acyclicity, below
5 cross-module edges.

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
through an own-field access that binds to exactly one field of that class; an
access that binds to nothing connects nothing.
**Only languages that capture member access are scored** (since version 8): a
class counts only when its language's plugin declares the `member_access`
capability — Java (`this.x`; a bare implicit `x` is not captured), TypeScript
(`.ts`), TSX (`.tsx`), PHP (`$this->x`), C# (`this.x`, and the bare field and
property reads inside a method), Python (`self.x`, since version 9; `.pyi`
stubs excluded), Scala (`this.x`, and the bare reads of a field or
constructor parameter inside a method), Kotlin (`this.x`, and the bare reads
of a property or primary-constructor `val` inside a method), C++ (`this->x`,
and the bare reads of a field inside a method of a class the file declares),
Ruby (`@x` reads and writes inside a method) and JavaScript (`.js`, `.mjs`,
`.cjs`, `.jsx`; `this.x`, since version 9). A
class in any other language is
not scored, because without captured field access its LCOM4 would rest on
intra-class calls alone (see
[configuration.md](configuration.md#member-access-member_access-member_access_excluded_extensions)). In TypeScript
and TSX, declared class fields, `#private` fields and constructor parameter
properties (`constructor(private readonly http: Client)`) are fields of the
class. Getters, inherited members, and the fields of an abstract class (whose
methods are its members) or a class expression are not, so accesses to them
stay unbound. A TypeScript class
whose methods share fields can therefore score higher on Cohesion after a
re-index with this version than it did before.
In JavaScript, a class's fields are its body's `x = …` and `#x` declarations
and the `this.x = …` writes of its constructor and methods, once per class
(a constructor's `this.pubClient = …` is the Field `pubClient`); `this.x`
reads and writes bind to them. A `this.x += …`, a `this.#x` write (the body
declares it), a destructuring target, a `this.x = …` inside a `function`
expression's own body (a callback binds its own `this`; an arrow function does
not), and a `this.x = …` of a constructor *function* or an object literal's
method (no class owns it) declare no field.
In JavaScript, TypeScript and TSX alike, `this.f()` is a call, not an access of
a field named `f`: a method that only calls a field holding a function
(`this.callback()`) links nothing through it, so a class that leans on such
callbacks reads less cohesive than it is.
In Python, a class's fields are its body's assignments and annotations (a
dataclass's, attrs', a Django or pydantic model's fields) and the `self.x = …`
writes of its methods, each name of a tuple target included, once per class;
`self.x` reads and writes bind to them. An attribute only a base class
declares, the names after the first of a chained assignment (`a = b = 0`), an
augmented assignment (`self.n += 1`), a `cls.x = …`, an assignment under an
`if` or other compound statement of the class body, and a `self.x = …` outside
a method of the class (in a function nested in one, say) declare no field, so
accesses to them stay unbound.
**Inherited members link too** (since version 9): two methods that access the
same name through their own receiver (`this.state`), where the class declares
no member of that name, are one component, as if they shared a field. React's
`render` and `componentDidUpdate` both reading the inherited `this.state` are
one component, not two. The access still binds to no field, and a name the
class does declare links only through the access that binds to it.
**n/a drop-out:** a class with no bodied production method, or in a language
that does not declare `member_access`, is not scored, and a repo with no
scoreable class reports Cohesion as `n/a` rather than a
fabricated score. So does a repo with fewer scoreable classes than
`[metric_thresholds] cohesion_min_classes` (default 5, since version 9): a mean
over one or two classes lets one class move the whole signal. Below the minimum
Cohesion reads `n/a` ("too few classes") and no class is ranked for it in
"where to start", but the Cohesion worst-offender list still lists every class
at LCOM4 ≥ 2 — see
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

**A Scala trait with a body is a container** (since version 9): a trait with at
least one method with a body counts for Focus, and for Cohesion, as a class does —
Scala writes its services as traits (the cake pattern's `trait AccountService`).
A trait of abstract declarations only is no container. Only Scala declares this
(`bodied_traits_are_containers`,
[configuration.md](configuration.md#trait-containers-bodied_traits_are_containers)):
a Rust trait's default methods never make a container.

**A C# partial type is one container** (since version 9): the `partial`
declarations of one fully-qualified name in one directory tree (every part in or
below one part's directory) — `Form1.cs` and `Form1.Designer.cs` — are counted once, with the methods of every part and the sum of the parts'
spans, under the first-declared part (by file path). Each part keeps its own
node, and every later part records an `Extends` to the first. Cohesion reads the
same union: a `this.x` read of a field another part declares links the reading
method, but a bare `x` does not — a bare name is an access only of a field the
method's own part declares.

Two limits of the method count, both from what extraction records rather than
from the formula. A C++ class whose members are all defined out of line in a
`.cpp` has only bodyless in-class prototypes in its header: each `.cpp`
definition is a method of its own, not merged into a class another file
declares (a definition in the class's own file is merged, and counts), so the
class counts zero bodied methods: it is never god by method count, only by
span, and Cohesion does not score it.

**Rust and Go methods count toward their struct** (since version 9). Extraction
attaches a Rust `impl` method or a Go receiver method to its module, not its
struct, and leaves it there. Focus attributes each one to the `Struct` its
recorded type names: a Rust method to the struct of that name in its **crate**
(the `impl` may sit in any module of it, inherent or trait, associated functions
included; a crate is the directory above its `src/`), a Go method to the struct
of that name in its **package** — the directory of its file, pointer and value
receivers alike. A candidate is also declared in a file of the method's own
language. A method naming no
struct of its crate/package (an enum, an alias, a type from elsewhere) or two
same-named ones counts toward none, and a same-named struct of another
crate/package is never a candidate. `scan --json` gives the count under
`metrics.focus_attribution` as `{"unattributed": N, "denominator": M}` — the
methods that reached no struct, of every production method that records a type
(`null` when none does, and on a stored snapshot). The attribution feeds Focus
only: node ids, symbols and edges, and Cohesion, are unchanged.

Two cases leave a method unattributed that a reader may not expect. A struct of
the same name declared inside a Rust `#[cfg(test)]` module, and a Go external
test package (`package svc_test`) declaring the type again, are each a second
same-named struct of the crate/package, so the production struct's methods count
toward none; `focus_attribution` is where that shows.

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
- **Scala:** a `*Spec.scala`, `*Suite.scala` or `*Test.scala` file is a test file
  (beneath a production source root it is not, as for any JVM file). A `def` of a
  class, trait or object that extends a ScalaTest, MUnit or Specs2 base —
  `AnyFunSuite`, `AnyFlatSpec`, `FunSuite`, `Specification`, … — counts anywhere, and
  so does one inside a `test("…")`/`it("…")` call. Only the framework's own base names
  match: a class extending a base of your own (`class FooTests extends BaseSuite`) is
  not recognised by its base, though its file name or path still marks it.

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

## Dispatch through a trait is excluded too

A call through a trait (`t.m()` with `t: &dyn Tr` or `&impl Tr`) binds to the
trait's method and to every implementation of it. Each edge to an
implementation is marked **dispatch**: the caller reaches it at run time, but
depends only on the trait. Every architecture view skips a dispatch edge — the
metric graph and its module rollup (Modularity, Acyclicity, Depth), the DSM and
the module and file tiers of the graph view, the layer and boundary rules, and
the `max_fan_in` / `max_fan_out` budgets — so a
dependency-inversion seam no longer reads as the core depending on its plugins,
or as a cycle between them. Every runtime view keeps it: `callers`, `callees`
and `impact` (which label the row `via: "<Trait>::<method>"`), `affected`,
reachability, and dead code, so an implementation reached only through its trait
is never dead. A direct call to an implementation is a dependency, even when the
same caller also reaches it through the trait. Since
[metric semantics version 9](#metric-semantics-version-9--cohesion-links-inherited-members-and-needs-enough-classes).

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
  Anti-gaming by construction. A not-applicable Modularity, Acyclicity or Depth
  (fewer than 5 cross-module edges) is outside this rule: it is not measured, so
  it cannot be a systemic zero.
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
declarations), or whose classes are all in languages that do not declare
`member_access`, has nothing for LCOM4. Rather than fabricate a flattering `1.0`, the engine **drops the
dimension out**: it stores NULL for that metric with an `applicable = 0` flag,
and the geometric-mean denominator shrinks accordingly — a class-less repo is
scored on 8 or 9 dimensions, not 10.

Cohesion also drops out this way when it has classes to score, but fewer than
`[metric_thresholds] cohesion_min_classes` (default 5, since version 9; see
[configuration.md](configuration.md#metric_thresholds--tuning-the-structural-dimensions)). `scan --json` carries
the reason under `metrics` as `cohesion_not_applicable`, for example
`{"classes": 2, "min_classes": 5, "reason": "2 of 5 classes — too few classes
to score"}` (`null` when Cohesion applies or has no class to score), and
`quality-report --json` carries it at the top level beside the graph
dimensions' reasons. The stored snapshot records the drop-out but not its
cause, so readers of a stored snapshot — the dashboard — show the plain `n/a`.
The classes stay in the Cohesion worst-offender list; only the signal and "where
to start" skip them.

Modularity, Acyclicity and Depth drop out the same way, together, on a graph
with fewer than 5 cross-module edges, with one difference: their computed raw
and normalized values are still stored (beside `modularity_applicable`,
`acyclicity_applicable` and `depth_applicable` = 0), because each is always
computable — it is the evidence that is too thin. `scan --json` and
`quality-report --json` carry the reason as `modularity_not_applicable`,
`acyclicity_not_applicable` and `depth_not_applicable` (under `metrics` in
`scan --json`), for example
`{"edges": 3, "min_edges": 5, "reason": "3 of 5 cross-module edges — too few to
measure module structure"}` (`null` when the dimension applies), and
`scan --json` carries the count as `cross_module_edge_count`. The dashboard's
Quality view shows **not applicable** with that reason in place of a score.
`evolution` marks the same point's entry `not_applicable` with that reason and
reports no delta for it across a drop-out, and `gate`'s per-metric regression
detail never names a graph dimension that is not applicable on either side — a
dimension outside the signal is not a movement of the signal. A snapshot scored
before version 8 reads Acyclicity and Depth as applicable and keeps the
Modularity reason it was decided on (`"<m> of 5 dependency edges — too few for
community structure"`, counted over every edge). Equality, Redundancy, Nesting,
Conciseness and Uniqueness always apply.

**Coverage warning.** A language whose resolver binds no call and no import
across a file boundary adds no edge between modules, so the graph dimensions
cannot see its structure even when the graph as a whole is large enough to
score. When such a language holds at least **10%** of the production callables,
`scan` (and `scan --json`, the MCP `scan`), `gate`, `session-start` and the
session-start readout each carry one warning for it (in `warnings` under
`--json`), for example:

```text
coverage: c is 33% of production callables (warned at 10% or more) and binds no call or import across files (same-file-only) — Modularity, Acyclicity and Depth cannot see its cross-module structure
```

The share is floored, never rounded up. The cause in parentheses is the same
tag `status` reports for the language (`same-file-only` or
`no-resolved-edges`). The warning is advisory: it never changes an exit code and
never feeds the signal.

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
- **A repository's signal moves on its first index after upgrading past 1.10.0.**
  Rust paths through a `mod x;` declaration, and the imports and inherited calls of
  PHP, C#, Kotlin, Scala and Python, now bind across files. More cross-directory
  edges usually lower modularity and can lengthen the dependency depth. Fewer
  functions read as dead, which helps redundancy. On Logos's own repository the
  signal moved 8402 → 8319, almost all of it modularity. This is the same kind of
  correction: after that first index, re-establish the baseline with
  `logos gate --save` rather than chasing the delta.

## Determinism guarantee

Same tree in, same signal out — bit-for-bit, across runs and machines. Every
order-sensitive reduction runs in a fixed order and four of the five metrics
accumulate in exact integer arithmetic. Re-scanning an unchanged tree appends
an identical snapshot. This is what makes the signal CI-gateable: a changed
number always means changed code, never floating-point weather.

## Snapshots

Every scan will append one row to `metric_snapshots` inside
`.logos/logos.db` — raw and normalized values for all ten metrics (the five new
dimensions each carry a raw+normalized pair; Cohesion, Focus, Modularity,
Acyclicity and Depth also carry an `*_applicable` 0/1 flag for the drop-out —
`NULL` on a row written before Logos recorded the flag, read as applicable),
the `cross_module_edges` count the graph drop-out was decided on (`NULL` before
version 8), node/edge/function counts, the
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
  is **9** (Cohesion links inherited members and needs `cohesion_min_classes`
  classes to score — see
  [Metric semantics version 9](#metric-semantics-version-9--cohesion-links-inherited-members-and-needs-enough-classes)).
  A formula change bumps it, so the first `gate` after an upgrade that bumps it
  re-baselines once.
- **`thresholds_hash`** records the effective `[metric_thresholds]` set the row
  was scored under. Editing any threshold (or a budget that feeds one) changes
  the hash.

When `gate` finds a baseline whose `metric_version` differs from the engine's
current version, comparing the two numbers would be meaningless — so instead of
failing against an incomparable baseline, the gate **re-baselines
automatically**: it records a fresh baseline, reports `baseline reset: metric
semantics changed (v7 8292 → v8 8295)` — the version and signal of the baseline
it replaced, then of the run — and passes informationally. The analogous case
for a changed `thresholds_hash` reports `baseline reset: metric thresholds
changed (v8 8292 → v8 8295)`. A side with no signal to score shows its version
alone. `--json` carries the same figures as `baseline_reset` (`cause`, and
`from` / `to`, each with `metric_version` and `signal`). The exit code is the
pass it always was. Either way, the *next* gate finds a matching version/hash
and compares normally — so an existing project can upgrade across a semantics change, or an operator can re-tune
a threshold, without a spurious gate failure. (The version guard is checked
first, so a pre-v3 baseline re-baselines once on the upgrade, not twice.)

### Metric semantics version 9 — Cohesion links inherited members and needs enough classes

Version 9 ([CR-252](../requests/CR-252-findings-and-offenders-without-the-noise.md))
changes Cohesion in three ways, counts a C# partial type once in Focus and
Cohesion, and counts Rust and Go methods toward their struct in Focus
([CR-166](../requests/CR-166-rust-and-go-methods-count-toward-their-type-for-focus.md)),
so a project re-baselines **once** for all of them.

| Change | Before (v8) | Since v9 |
|---|---|---|
| **Inherited members** (LCOM4) | an access that bound to no field linked nothing, so two methods sharing only React's `this.state` were two components | two methods accessing one name through `this` that the class does not declare are one component; declared-field links are unchanged |
| **Cohesion minimum** | scored over any number of classes, one included | n/a ("too few classes") below `[metric_thresholds] cohesion_min_classes` scoreable classes (default 5); out of the aggregate and "where to start", still in the worst-offender list |
| **C# classes** ([FR-EX-22](../specs/requirements/FR-EX-22.md)) | n/a: C# declared no `member_access` | scored, from `this.x` and the bare field and property reads inside a method (`_count`, `Name`, `_log.Add(x)`) |
| **C# partial types** (Focus, Cohesion) | each `partial` declaration was a container of its own, scored over its own methods and span | the parts of one type are one container: its methods are every part's, its span the sum of the parts', so a type split across files can be god where no part is ([S-557](../planning/journal.md#s-557-the-parts-of-a-c-partial-type-form-one-type)) |
| **Rust and Go methods** (Focus) | an `impl` method or receiver method counted toward no container, so a Rust or Go struct was god by span alone | counted toward the one same-crate/same-package struct its recorded type names ([S-510](../planning/journal.md#s-510-focus-counts-rust-and-go-methods-toward-their-type-and-metric-semantics-move-to-v8)); a method naming none or several is unattributed and reported with its denominator |
| **Python classes** (Focus, Cohesion; [FR-EX-20](../specs/requirements/FR-EX-20.md)) | a `def` in a class body was a Function, counted toward no container, and Python declared no `member_access`, so every Python class had 0 methods and Cohesion over Python classes was n/a | a `def` of a class body — decorated, `@staticmethod`, `@classmethod` and `@property` ones too — is a Method of its class, and Python declares `member_access` (`.pyi` stubs carved out): its classes are scored from the `self.x` accesses that bind to their fields ([S-543](../planning/journal.md#s-543-python-classes-have-methods-fields-and-self-accesses)) |
| **Scala classes and traits** (Cohesion, Focus) ([FR-EX-28](../specs/requirements/FR-EX-28.md)) | n/a: Scala declared no `member_access`; a trait was no container | scored, from `this.x` and the bare reads of a field or constructor parameter inside a method (`semaphore.acquire(n)`, `rate - n`); a trait with a bodied method is a container for Focus and Cohesion ([S-575](../planning/journal.md#s-575-scala-class-parameters-relations-and-field-access-are-captured-and-bodied-traits-count-for-cohesion)) |
| **Kotlin classes** (Cohesion) ([FR-EX-25](../specs/requirements/FR-EX-25.md)) | n/a: Kotlin declared no `member_access` | scored, from `this.x` and the bare reads of a property or primary-constructor `val` inside a method (`balance + amount`, `repo.find(id)`, `"${name}"`) ([S-562](../planning/journal.md#s-562-a-bare-kotlin-property-read-is-an-access)) |
| **Kotlin member functions and containers** (Focus) | a member `fun` was a Function, counted toward no container, so a Kotlin class was god by span alone; an `enum class` was a container, and a `companion object` was none | a member `fun` of a class, object, interface or companion is a Method, counted toward its container ([S-559](../planning/journal.md#s-559-kotlin-declarations-are-kinded-and-complete-and-a-misparse-is-never-silent)); an `enum class` is an Enum, which is no container, and a `companion object` is a container of its own, holding its members |
| **Ruby methods and containers** (Focus) | a compact `class A::B` spanned its header alone, so its methods belonged to the file and counted toward no class; a setter or operator `def` (`def x=`, `def []`, `def <=>`) was no node | a compact class holds its body, and every `def` is a Method counted toward its class ([S-564](../planning/journal.md#s-564-ruby-declarations-are-complete-and-ruby-build-files-are-indexed)); `attr_*` symbols and the first assignment of each `@ivar` are Fields of their class |
| **Ruby classes** (Cohesion; [FR-EX-27](../specs/requirements/FR-EX-27.md)) | n/a: Ruby declared no `member_access`, and no `@x` was captured, so its Fields linked nothing | scored, from the `@x` reads and writes inside a method (`@q`, `@q = …`, `@total ||= 0`) that bind to the Field their class declares ([S-569](../planning/journal.md#s-569-ruby-instance-variable-reads-and-writes-are-accesses)) |
| **C++ classes** (Cohesion; [FR-EX-22](../specs/requirements/FR-EX-22.md)) | n/a: C++ declared no `member_access`, and only `this->x` was captured — which also recorded `this->f()` calls as accesses | scored, from `this->x` and the bare reads of a field inside a method (`m_balance`, `m_log.push_back(x)`) unless a parameter or local of the method hides it; `this->f()` is a call, not an access ([S-580](../planning/journal.md#s-580-c-tests-are-named-and-bare-member-reads-are-accesses)) |
| **C++ methods** (Focus, Cohesion) | a method defined out of line below its class (`void Widget::set() {…}`) was not extracted, so its in-class declaration stayed bodiless and counted as no bodied method of the class; a constructor was no method | a definition in its class's own file merges into the in-class declaration, which is bodied and counts toward the class; a constructor is a method; a definition whose class another file declares stays a method of its own ([S-577](../planning/journal.md#s-577-c-out-of-line-members-merge-into-their-declaration-prototypes-pair-with-definitions-and-constructors-are-methods)) |
| **C structs** (Focus; [FR-EX-31](../specs/requirements/FR-EX-31.md)) | C extracted no struct, so a `.c` file held no container; a header's structs were read as C++, one per mention (`struct X *p` was a struct) | a `struct` or `union` with a body is one container, scored by its span (C has no methods), so Focus applies to a C repository; a mention or a forward declaration is none, in C++ too ([S-582](../planning/journal.md#s-582-c-repositories-own-their-headers-and-c-declarations-are-faithful)) |
| **C++ members** (Cohesion, Focus; [FR-EX-31](../specs/requirements/FR-EX-31.md)) | a pointer, array or function-pointer member (`T *p_`, `int a_[4]`, `R (*f_)(…)`) was no field, so a method reading one linked to nothing; `struct X *p` and `struct X;` each made a struct | every member declarator is a field, so a method reading one links through it in LCOM4, and only a struct with a body is a container (ccache at `052c47fb`: fields 666 → 770, structs 213 → 167, Cohesion 0.572 → 0.613, Focus 0.978 → 0.975) |
| **JavaScript classes** (Cohesion; [FR-EX-19](../specs/requirements/FR-EX-19.md)) | n/a: JavaScript declared no `member_access`, a constructor's `this.x = …` declared no Field, and a callee through `this` (`this.setState()`) was recorded as an access of a field of that name | scored, from the `this.x` accesses that bind to the Field their class declares — one per name per class, for each `this.x = …` of a constructor or method; a callee is a call, not an access, in JavaScript, TypeScript and TSX alike ([S-542](../planning/journal.md#s-542-constructor-assigned-fields-are-declared-and-a-callee-is-never-an-access)) |
| **Trait dispatch** (Modularity, Acyclicity, Depth, DSM, layer and boundary rules, coupling budgets; [FR-QM-20](../specs/requirements/FR-QM-20.md)) | a call through a trait counted as a dependency of the caller on every implementation, so a crate calling its own trait depended on each crate implementing it — a cycle Cargo forbids | an edge to an implementation reached through a trait is skipped by every architecture view and kept by every runtime one ([S-713](../planning/journal.md#s-713-architecture-views-exclude-dispatch-edges-under-metric-semantics-v8)); see [Dispatch through a trait is excluded too](#dispatch-through-a-trait-is-excluded-too) |

**Why signals move.** A React class component read as several unrelated
classes, and with two classes in a project one of them could become the top
"where to start" entry at hundreds of points. Class-based code with
inherited state now scores more cohesive, and a project with fewer than five
scoreable classes loses Cohesion from its signal. A C# project gains Cohesion:
its methods' bare reads of their own class's fields and properties now link
them, so LCOM4 has its shared-field edges. A bare name only an inherited base
class or another part of a `partial` type declares, and a constructor's or
accessor's body, link nothing, so a class that
leans on them reads less cohesive than it is. A C# type split across `partial`
files is one container, so a type that is large only as a whole — an EF Core
migration with its generated `.Designer.cs` — can turn god and lower Focus. The
C# changes are part of the same v9 re-baseline. A Rust or Go struct with 20 or
more methods across its impls or receivers is now god, which it was not before
unless its span was 500 lines: Focus on a Rust or Go project can fall, and the
`no_god_containers` budget (which counts the same containers) can start
reporting such a struct.
A Kotlin class, object or companion with 20 or more member `fun`s is god in the
same way, and a Kotlin project's containers change: its enums leave them and
each companion object joins them.
A Kotlin project gains Cohesion: a method's bare reads of its class's properties
and primary-constructor `val`s link it, so LCOM4 has its shared-field edges. A
property a class inherits or an outer class declares links nothing, a name any
local of the method declares hides the property in the whole method, the bodies
of an `object : T { … }`, of an extension function and of an `apply`, `run` or
`with` lambda are not read, a property
initialiser, `init` block or accessor is no method, and a short `"$name"`
interpolation is not seen, so a class that leans on them reads less cohesive
than it is. The Kotlin change is part of the same v9 re-baseline; the property
links read the rows a Kotlin file records when it is extracted, so they count
once the file is next extracted.
A Python project gains Cohesion, scored from its `self.x` accesses, and its
classes gain their methods: a Python class (a `.pyi` stub's included) with 20 or
more methods is now god, so Focus on a Python project can fall. No symbol
changes: a method keeps the symbol it had as a function.
A Ruby class that is written compact (`class Mod::FlaggedController`) now owns
its methods, so it can be god where its file was not, and a class's setter and
operator methods count toward its Focus. `.rake`, `.ru`, `.gemspec`, `Gemfile`
and `Rakefile` are now Ruby files, so a Ruby project's file count, and with it
the per-file measures, include them.
A Ruby project gains Cohesion: a method's `@x` reads and writes link it to the
Field its class declares, so LCOM4 has its shared-field edges. A `@x` no class
declares (one an `attr_*` or an assignment of a superclass or mixin owns) binds to
no Field, but two methods that both access it are one component, as for any
inherited member (see **Inherited members link too** above). A
`@@x` class variable, a `module`'s own `@x` (a mixin's `self` is whatever includes
it, so a module nested in a class binds nothing either), a bare call to an `attr_*`
reader (`name` for `@name`) and a class body's, `def self.x`'s or `class << self`
method's `@x` — the class object's own state, bound to the instance Field of the
same name when the class declares it once; a class body's is recorded from the
class itself, which Cohesion ignores — are the reach of the capture: a class that leans on a reader reads
less cohesive than it is. The Ruby change is part of the same v9 re-baseline;
the field links read the rows a Ruby file records when it is extracted, so they
count once the file is next extracted.
A Scala project gains Cohesion: a method's bare reads of its class's fields and
constructor parameters link it, so LCOM4 has its shared-field edges, and each
trait with a bodied method joins the containers, so Focus moves too — a trait
with 20 or more bodied methods is god. A field a class inherits or an outer class
declares links nothing, and a name any local of the method declares hides the
field in the whole method, so a class that leans on them reads less cohesive
than it is. The Scala changes are part of the same v9 re-baseline. The trait
containers count from the next `scan`; the field links read the rows a Scala file
records when it is extracted, so they count once the file is next extracted.
A C++ project gains Cohesion: a method's bare reads of its class's fields link it,
so LCOM4 has its shared-field edges, and `this->f()` no longer links a method to a
callee's name as if it read a field. A pointer, array or function-pointer member is
a field like any other (since S-582, see the v9 table above); a
field a base class declares, one an outer class declares, and one a method reads in
a source file that does not declare the class (a class written in a header and
defined in a `.cpp`) link nothing either, so a class that leans on them reads less
cohesive than it is. A name any parameter or local of the method declares hides the
field in the whole method. The C++ changes are part of the same v9 re-baseline; the
field links read the rows a C++ file records when it is extracted, so they count once
the file is next extracted. The tests it names (`Suite.Name`, a `TEST_CASE`
string) carry test evidence like every test function, so a Catch2 or doctest
`TEST_CASE`, which was no node, joins the test nodes the production-scope measures
leave out. A test named like a function it calls (`TEST_CASE("parse")` calling
`parse`) can bind that call to itself, which adds a self-call edge to the test.

A JavaScript project with `class` declarations gains Cohesion: a class's constructor and methods assign
`this.x`, which are now its Fields, and a method's `this.x` reads link it to
them, so LCOM4 has its shared-field edges. The constructor's own writes are
accesses too, so a class whose constructor assigns every field links the
methods that read any of them. A field a base class declares, a `this.x` of a
constructor function or an object literal's method, and an assignment through
any receiver but `this` declare nothing, so a class that leans on them reads
less cohesive than it is. A callee through `this` (`this.setState()`) is no
longer an access in JavaScript, TypeScript or TSX, so the accesses of a React
class component are its field reads alone. The JavaScript change is part of the
same v9 re-baseline; the fields and links read the rows a JavaScript file
records when it is extracted, so they count once the file is next extracted.

A Rust project whose code calls through its traits loses those calls from
its architecture views. Modularity usually rises (fewer edges cross between
directories), and a module cycle held together only by dispatch splits, which
can raise Depth: the condensed tangle becomes the layers it is. On this
repository the 70 production dispatch edges that crossed a directory all pointed
along existing dependencies (into `graph_store` and `plugin`), so Modularity
moved 0.6905 → 0.6948, Depth and Acyclicity stayed put and the signal moved
8276 → 8279; the 14 cross-crate edges that ran against Cargo's dependency
direction (`logos-ext`'s extension registry reaching the test extensions of
`cli`, `mcp` and `web` through its trait) left the DSM. The markers are already in the store
(the upgrade re-binds every trait call once), so the change counts from the
next `scan`.

**What you see on upgrade.** The effective-thresholds hash always includes
`cohesion_min_classes`, so it changes too; a baseline taken before resets
**once**: the first `gate` (or `session_end`) after the upgrade reports
`baseline reset: metric semantics changed (v8 8292 → v9 8295)` and passes
informationally, and the next one compares normally. Nothing is re-extracted:
the next `scan` scores under v9 — except the partial-type union, which reads
the rows a C# file records when it is extracted, so it counts from the next
`index`. The Rust and Go attribution reads the self type those files already
record, so it counts from the next `scan`.

### Metric semantics version 8 — the signal drops what it cannot measure

Version 8 changes two things, so a project re-baselines **once** for both:

| Change | Before (v7) | Since v8 |
|---|---|---|
| **Graph-dimension applicability** | Modularity alone dropped out, below 5 dependency edges of any kind | Modularity, Acyclicity and Depth drop out together, below 5 **cross-module** edges |
| **Cohesion** (LCOM4) | classes of every language, scored from intra-class calls where no field access was captured | only classes of a language that declares `member_access` (Java, TypeScript, TSX, PHP) |

**Why signals move.** Under v7 intra-module edges kept a graph with nothing
between its modules looking measured, and a class whose language captures no
field access was scored from its calls alone. A thin graph now scores on fewer
dimensions, and a codebase whose classes are Python, C++, Ruby, JavaScript or
another non-declaring language loses Cohesion (or keeps it only over its
declaring-language classes). A graph with at least five cross-module edges whose
classes are all in a declaring language scores exactly as under v7.

**What you see on upgrade.** The first `gate` (or `session_end`) after the
upgrade reports `baseline reset: metric semantics changed (v7 8292 → v8 8295)`
and passes informationally; the next one compares normally. Nothing is
re-extracted: the next `scan` scores under v8.

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

**What you see on upgrade.** The first `gate` (or `session_end`) after the
upgrade reports `baseline reset: metric semantics changed (v6 8290 → v7 8291)`
(figures illustrative) and passes informationally; the next one compares
normally. Nothing needs re-blessing by hand. The upgrade also re-extracts every file once to record which callables
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
a group's members stay adjacent.

Nine of the ten dimensions carry a list. Beside Nesting, Conciseness, Cohesion,
Focus and Uniqueness, four more name what drives their score
([CR-209](../requests/CR-209-health-records-the-worst-items-of-four-more-dimensions.md)):

| Dimension | What is listed (worst first) | Tie-break | Row detail |
|---|---|---|---|
| Acyclicity | Each cross-directory cycle the dimension counts, most members first. The row names the cycle's lowest-id member, with its file and line | Lowest member id | `N symbols across D directories: dir1, dir2, …` |
| Depth | The longest directory chain, then each other chain head (a directory nothing depends on) by its longest chain. Chains of one directory are not listed | Head name | `dir1 → dir2 → … (L directories)` |
| Equality | Functions above the mean cyclomatic complexity, the ones that raise the Gini, highest first | Node id | `complexity C` |
| Redundancy | Dead or duplicate production functions, most lines first | Node id | `dead`, `duplicate` or `dead, duplicate` |

A few specifics:
- Each list is empty exactly when its dimension scores clean: no cycle, depth 1, an even complexity spread, or no dead or duplicate function.
- A Depth row is a chain of directories, so it has no file or line. The project root reads `.`. A directory cycle is one layer, spelled `{a, b}`, and the count then names layers.
- A row names at most three directories. A larger directory cycle reads `{a, b, c +4 more}`, and an Acyclicity row spanning more than three directories ends in `…` after its stated count.
- At a fork, a Depth chain follows the longer continuation, then the lower name.
- Redundancy lists a function as dead only where its language's reachability analysis gives a verdict. Where it does not (`is_dead` is NULL), the function is never listed as dead.
- An Equality row is not a violation. Equality has no per-function threshold: its list holds the functions with above-average complexity, the members of the distribution the score measures. The Quality page labels the list that way, and `logos offenders` lists these functions only under `--dimension equality`, so its total leaves them out.

Modularity has no list: it scores the directory layout as a whole, so no single
unit is responsible for it.

The lists are report-only: they never gate, and they never change a dimension's
score. They are emitted in the `worst_offenders` field of `logos scan --json`,
persisted with the snapshot, and shown under each Quality dimension widget. A
snapshot recorded before CR-209 has no rows for the four newer lists. The read
names each of them whose score says it is not empty in `worst_offenders.unrecorded`,
and the Quality page shows it as "not recorded", never as "none flagged". Run `logos scan`
to record them. Together the lists are the "which code do I fix first?" surface
that turns a dropped dimension into an actionable to-do list.

Each list is capped at ten entries. Every entry also carries the symbol's id,
the number it was judged on and the threshold it met. `logos offenders` (MCP:
`offenders`) lists every entry the last snapshot recorded, without the cap, and
can narrow the list by path glob and dimension. It leaves out Equality's
functions unless `--dimension equality` asks for them. See
[commands.md](commands.md#offenders---path-glob---dimension-d).

### Where to start — what an estimate means

The worst-offender lists say what is wrong in each dimension. The **where to
start** list says which single fix would move the signal most. Every snapshot
records it beside the lists, and it is shown by the Quality page's **Where to start**
card, `logos improvements --json`, the `improvements` MCP tool and
`scan.where_to_start` in `GET /api/v1/health`. All four return the same list for
one snapshot. See [commands.md](commands.md#improvements).

Each entry is one **fix unit**: a class (Cohesion, Focus), a function (Nesting,
Conciseness), an exact-duplicate group (Redundancy) or a near-clone group
(Uniqueness). Its **points** are what the signal would gain if that unit stopped
offending:

> points = the signal recomputed with this one unit's contribution removed, and
> nothing else changed, minus the current signal.

What each unit's contribution is:

| Unit | Contribution removed |
|---|---|
| Function | It is no longer deeply nested and no longer a brain method |
| Class | Its `1/LCOM4` becomes 1, or it is no longer a god container |
| Exact-duplicate group of *k* | The redundant count falls by *k* − 1: every member but the lowest-id one stops being a duplicate. A member that is also dead stays redundant |
| Near-clone group | Every member leaves the near-cloned count |

Read the figure as an **estimate**, for three reasons. It changes one unit and
holds everything else fixed, so two fixes recover about the sum of their
points, not exactly that. The signal is an integer on a 0–10000 scale, so a
unit that would recover less than one whole point is **not listed** at all. And
it says what removing the contribution would be worth, not how hard the fix is.
The payload states this in its `basis` field, and the Quality page's card shows it
under the table; the card's caption says the figures are advisory and never gate.

Equality, Depth, Acyclicity and Modularity never carry points. Splitting a
function or a module has no single result, so those dimensions appear only as
their offender lists (and, for Modularity, as the note that no single unit is
responsible). At most ten units are listed.

An empty list needs careful reading. `recorded: false` means the snapshot holds
no ranking (never scanned, or written before 1.20.0): run `logos scan`.
`recorded: true` with no units means **no single unit recovers a whole point**.
On a large codebase one deep function often recovers 0 points, so that is not
the same as clean.

**Outlier labels.** Beside the ranking, seven per-unit measures are checked for
outliers against the member's **own** distribution (a workspace member is never
calibrated against another): LCOM4, bodied method count and line span for
classes, cyclomatic complexity and nesting depth for functions, and the exact-
duplicate and near-clone group masses. A unit is labelled an outlier on a measure
when its value is above `Q3 + 1.5 × IQR` of that distribution **and** at or
above the dimension's detection threshold. With fewer than 8 units in the
distribution no label is given and the payload's `fence` is `null`. The
multiplier and the minimum are constants of the metric semantics, not
`rules.toml` keys. The label separates "bad against this codebase" from "bad by
the absolute threshold", and a ranked unit shows the measures it carries in the
card's **Outlier** column.

The list and the labels are advisory. They never enter the signal, the gate
verdict or any `check` exit code.

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
