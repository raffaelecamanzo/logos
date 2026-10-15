# Usage

Logos has two surfaces over the same engine: the **CLI** (this page, first
half) and the **MCP server** for AI agents (second half). Both read the same
index, honor the same configuration, and record into the same local stats.

## The core loop: index → navigate → keep fresh

```bash
cd /path/to/project
logos index          # build the code graph (full rebuild, idempotent)
logos status         # health: file/node/edge counts, resolution coverage, freshness
logos sync src/a.rs  # after edits: fold the changed paths in (a bare `sync` reads none)
```

`logos index` walks the tree (gitignore-aware, symlink-contained), extracts
symbols and references per language, resolves references into call/import
edges, detects framework routes, and annotates the graph (dead code,
duplicates, export status) — one command, exit 0 on success.

**Freshness model:** navigation commands serve the latest committed snapshot
and never reconcile per call — they are fast point queries. Re-run
`logos sync` (or let the MCP watcher do it) after editing.

## Language support: what each language binds across files

Every code language extracts symbols, but they do not all **bind references
across a file boundary**, and the questions that need that — `callers`,
`impact`, `precedent`, `affected` — are only as good as it is. Each language
plugin declares its reach in its `plugin.toml` (`[reach]`), a fixture per language
verifies the declaration, and `logos languages` prints it (`reach.level` and
`reach.cross_file` under `--json`). The table below is generated from those
descriptors and a test fails when it drifts.

<!-- reach:begin (generated from plugins/*/plugin.toml — see logos-core/tests/reach_docs.rs) -->
| Language | Reach | Bound across files |
|---|---|---|
| Java | `resolved` | `calls`, `imports`, `type_relations` |
| Rust | `resolved` | `calls`, `imports`, `type_relations` |
| C# | `partial` | `imports`, `type_relations` |
| Go | `partial` | `calls`, `imports` |
| Kotlin | `partial` | `imports`, `type_relations` |
| PHP | `partial` | `imports`, `type_relations` |
| Python | `partial` | `calls`, `imports`, `type_relations` |
| Scala | `partial` | `imports`, `type_relations` |
| TSX (incl. JSX) | `partial` | `calls`, `imports` |
| TypeScript (incl. JavaScript) | `partial` | `calls`, `imports` |
| C | `same-file` | none |
| C++ | `same-file` | none |
| Ruby | `same-file` | none |
<!-- reach:end -->

- `resolved` — calls, imports and type relations bind across files.
- `partial` — some relations bind across files, not all (see the last column).
- `same-file` — references bind only inside the file that wrote them, so a
  `callers` answer never crosses a file in that language.
- `symbols` — declarations are extracted; nothing binds across files.

This is what the plugin is built to do. What a given repository actually
achieves is measured per language, with its denominator, in
`logos status --json` (`resolution_by_language`) — read that before trusting a
relational answer.

## Navigating the graph

```bash
# Find symbols by name (FTS5 full-text, optionally filtered by kind)
logos search "user" --kind function --limit 10

# One symbol in full: metadata, edges, optionally its source
logos node fetch_user --code

# Who calls it / what does it call
logos callers fetch_user
logos callees fetch_user

# Transitive blast radius of changing it, both directions labeled
logos impact fetch_user --depth 3

# Which FILES are affected by a changed set (reverse-transitive closure) —
# ideal for choosing what to re-test in CI
logos affected src/auth.rs src/db.rs
logos affected src/auth.rs --tests-only

# Explore a neighbourhood, source grouped by file
logos explore "payment flow" --max-files 5

# The token-saving tool: a deterministic context bundle for a task
logos context "add rate limiting to the login endpoint" --max-nodes 25
```

`logos query` is a convenience façade: plain search by default,
`--callers`/`--callees` to pivot (the two flags conflict by design).

A symbol argument can be a bare name or a full symbol. A full symbol names its
repository (`logos . github.com/acme/payments . src/`auth.rs`/fetch_user().`). A
symbol saved before symbols carried that coordinate
(`logos . . . src/`auth.rs`/fetch_user().`) still resolves: the answer is the
canonical symbol's, with a one-line note in `warnings` naming the form to use.
See [Navigation](commands.md#navigation).

### Documentation traceability

Markdown under `docs/` (and top-level `*.md`/`README*`) is indexed alongside
code, on by default — see the `[documentation]` table in
[configuration.md](configuration.md#documentation--indexing-markdown). It adds
doc→code traceability to the same graph:

```bash
# Search the documentation nodes by kind
logos search "metric neutrality" --kind doc_section

# Which code implements a requirement / heading (doc → code)
logos implements FR-DG-06

# Which docs reference a symbol — what to update when it changes (code → doc)
logos referencing-docs aggregate_signal

# Exported symbols with no documentation
logos doc-gaps --limit 50
```

Doc→code links obey the same **never-fabricate** rule as code: an ambiguous
mention yields *no* edge and stays in the unresolved-reference ledger.
Documentation is **metric-neutral** — it never moves the quality signal. The
`logos node` / `logos context` queries (and their MCP equivalents) automatically
fold in referencing doc sections for any symbol they surface.

### Cross-artifact references

Config and spec artifacts bind to the code, files, and objects they reference, on
the same graph — so a spec or an infra file is navigable to its implementation
without re-deriving the wiring from text. The bindings surface through the same
`node` / `context` / `explore` queries:

```bash
# An OpenAPI operation → the framework route handler that serves it
# (method + positionally-normalized path template must match exactly)
logos node "GET /users/{id}"

# A code handler → back to its OpenAPI operation and spec section
logos context "rename the user-lookup handler"

# Protobuf / GraphQL: an import → the imported file; a field/RPC type → its
# declaring message; a declared schema name → the one type-like code symbol
logos node UserService

# Infra & shell: a SQL view/FK → its table; a Terraform module call → every
# admitted .tf in the source dir; var/local/module refs → their declaring block;
# a shell `source "lib.sh"` → the sourced script
logos node user_summary_view
```

Overall resolution health is in `logos status` (`refs_resolved` /
`refs_unresolved` / `resolution_coverage`); the per-relation-class breakdown of
cross-artifact bindings is in `logos stats` (the `artifact_bindings` field, keyed
by each relation's payload token), read live from the graph.

> **Reading `resolution_coverage` — why ~50% is normal, not a defect.** This
> per-repo ratio ([FR-RS-04](../specs/requirements/FR-RS-04.md)) is measured over
> *every* syntactic reference the extractor sees,
> and the denominator deliberately includes references that can never bind to a
> workspace symbol — by design, never as a failure:
>
> - **Stdlib & external-crate calls.** Logos indexes only your workspace and
>   never fabricates a node for code it didn't parse ([NFR-RA-05](../specs/requirements/NFR-RA-05.md),
>   [ADR-26](../specs/architecture/decisions/ADR-26.md)). In idiomatic code, a
>   large share of references are calls like `.unwrap()`, `.iter()`, `.clone()`,
>   `Some`, `Ok`, `Vec::new`, or `std::fs::*` — these stay unresolved forever
>   because their targets live outside the graph. Receiver-method calls
>   (`x.foo()`) are the biggest such bucket. Logos records each one's
>   **receiver shape** — `self` (the caller's own instance: `this.foo()`,
>   `self.foo()`), `super`, or `other` — and binds by it
>   ([CR-169](../requests/CR-169-a-call-on-another-object-never-binds-to-the-callers-own-method.md),
>   [FR-EX-13](../specs/requirements/FR-EX-13.md),
>   [FR-RS-12](../specs/requirements/FR-RS-12.md)): a `self` call binds to the
>   one `foo` of the caller's own class (or the nearest proven base class that
>   has one), a `super` call only through a proven base class, and a call on
>   any other receiver binds **nowhere** unless the receiver's type is proven
>   (Java's typed receivers, [FR-RS-10](../specs/requirements/FR-RS-10.md);
>   Rust's, [FR-RS-42](../specs/requirements/FR-RS-42.md), below).
>   Where the caller sits — its file, class or module — is not evidence of what
>   `x` is: an `other.foo()` inside a method `foo` never binds to that method
>   itself, and never to a same-named function beside it
>   ([FR-RS-06](../specs/requirements/FR-RS-06.md)). Such calls stay in
>   `unresolved_refs`; `logos status` counts them under `no-receiver-evidence`
>   in the `call_residue` of the languages that carry one (the Java, PHP, C#,
>   Kotlin, Scala and Rust rows, see [commands](commands.md)), and every other
>   language's row counts them among its unbound calls without a reason. A language whose
>   plugin does not record receiver shapes yet leaves all its method calls
>   unbound rather than guess. In TypeScript, JavaScript, Python, PHP and Ruby
>   the `self` forms are `this.foo()`; `self.foo()` and `cls.foo()`;
>   `$this->foo()`, `self::foo()` and `static::foo()`; and Ruby's `self.foo()`
>   or a bare `foo()` inside a class — the `super` forms `super.foo()`,
>   `super().foo()`, `parent::foo()` and Ruby's `super`. None of these plugins
>   proves a base class yet, so their `super` calls stay unbound.
>   In C#, Kotlin, Scala and C++ a bare call `foo()` written inside a class (or
>   Kotlin/Scala `object`, Scala `trait`) is a call on the current instance,
>   exactly like `this.foo()` / `this->foo()`: it binds to that type's own `foo`
>   and never to a same-named top-level function — unless the calling function
>   itself (or one enclosing it) declares a local function or nested `def foo`,
>   which the call then names, as the language's own scoping does; outside any
>   type — a free function, a Scala 3 top-level `def`, an out-of-line C++
>   `Type::foo` definition — it is a plain call, bound by the file's scope.
>   `base.foo()` / `super.foo()` stay unbound (these plugins record no proven
>   base class), and so does C++'s `Base::foo()`, which reads like any other
>   qualified call. In a Kotlin chain `a.b.c()` only `c` is a call. One receiver
>   *is* known for Rust: `self.helper()` and `Self::helper()` inside an `impl`
>   method have the impl's own type. A Rust call to a type's method — those
>   two, a written `T::helper()`, `<T as Tr>::helper()`, and a call on a
>   receiver whose type the file proves — binds through one associated-item
>   lookup ([FR-RS-47](../specs/requirements/FR-RS-47.md)): among the
>   functions of every `impl` block whose self type resolves to `T`, in any
>   crate and through `pub use` re-exports (a `pub use inner::*` glob
>   included; a private glob re-exports nothing), plus the default bodies of
>   the traits `T` implements without overriding them. An inherent function
>   beats a trait's, and a trait's function is a candidate only where the
>   trait is in scope. Syntax decides the filters: `x.helper()` never reaches
>   an associated function without `self` (`fn helper() -> u8`), while
>   `T::helper(x)` counts `x` as an argument. A method call that finds nothing
>   on `T` retries on its `Deref` target. A call on a trait-typed receiver
>   (`self.m()` in a trait's default body, `&dyn Tr`, `impl Tr`, a generic
>   bounded by `Tr`, a written `Tr::m(x)`) binds every repository impl of the
>   method plus the default body. A required trait method (a signature with no
>   body) is a node but binds no call. The proofs of a receiver's type are a
>   typed parameter or `let`, `let x = T::new(…)` whose `new` returns `Self`, a
>   struct literal `T { … }`, or `self.field` of the caller's own struct, after
>   peeling `&`, `&mut`, `Box`, `Arc` and `Rc`
>   ([FR-RS-42](../specs/requirements/FR-RS-42.md)). The constructor proof holds
>   only for a `T` the caller's own module declares, so `let e = Engine::new();
>   e.run()` in another module stays unbound, while a typed `let e: Engine =
>   Engine::new();` binds it. A call inside a macro (`format!("{}", m.f())`,
>   `assert!(T::make::<u8>())`) is read exactly as outside one. A call stays
>   unbound, with the reason `status` reports per call
>   ([commands.md](commands.md) § `call_residue`), when `T` is not declared in
>   the repository (`String`, `Vec`, an external crate's type), when a derive
>   or a wrapper supplies the method (`#[derive(Default)]`, `Arc::clone`), when
>   the trait is not in scope, or when two candidates of one rank remain. A
>   name a file imports twice (at top level and in an inline `mod tests`) binds
>   nothing. Chained calls, closure parameters and untyped `let`s prove nothing. Go reads the
>   receiver the same way, by name: inside `func (s *Svc) Run()` a call on `s`
>   (`s.Work()`) is `self` and binds to the one `Work` declared on `Svc`;
>   `x.Work()`, `s.next.Work()` and a parameter that merely shares the
>   receiver's name in another function are `other`, bind nowhere, and never
>   reach a free `func Work` (a name the method itself rebinds —
>   `for _, s := range …` — is still read as the receiver)
>   ([CR-169](../requests/CR-169-a-call-on-another-object-never-binds-to-the-callers-own-method.md)).
>   A bare-path call (`foo()`, single segment) is complementary: when a free
>   function and same-named associated methods both exist at the call scope, the
>   free function wins the tie
>   ([CR-068](../requests/CR-068-reachability-binding-precision.md) Part B,
>   [FR-RS-07](../specs/requirements/FR-RS-07.md)) — recovering the correct
>   target instead of leaving it ambiguous. In the languages whose bare call can
>   never reach a method — Go, Rust, Python, PHP, JavaScript and TypeScript,
>   whose plugins declare `implicit_receiver = "none"` — a bare call binds free
>   callables only: it never binds a member of a named class, nor a Go or Rust
>   method of a named type, so a bare `fwrite()` inside a PHP method `fwrite`,
>   or a bare `performWebSearch()` beside a Go method of that name, binds the
>   free or imported function of that name when there is one and otherwise
>   stays unresolved (a builtin is external), never the method. A function
>   nested in a method still binds, a Python call still constructs a
>   module-level class, and a TypeScript/JavaScript class method no longer
>   hides a same-named `import { f }` from the rest of its file. Members the
>   graph records without a named class or type — an object-literal or
>   class-expression method, a PHP anonymous class's or a TypeScript abstract
>   class's method, a Rust `impl` for `str`, `&T` or a foreign type — are not
>   yet covered
>   ([CR-189](../requests/CR-189-a-bare-call-never-reaches-an-instance-member.md)).
>   Where a bare call can mean the current instance — C#, Kotlin, Scala, C++,
>   Ruby, and Java, whose `m()` inside a class is `this.m()` — a scope with only
>   the method(s) still binds them.
>   Every one of these choices first keeps only the callables whose parameter
>   count admits the call's argument count
>   ([FR-RS-43](../specs/requirements/FR-RS-43.md)): a `self` or `super` call
>   in any language, a typed `T::m` call (Java, Kotlin, Scala, C#, C++, and
>   Rust's proven receivers, after the `self` rule above), and a bare call in a
>   language that overloads by name (Java, Kotlin, Scala, C#, C++). A default
>   or variadic parameter (`b = 0`, `vararg`, `params`, `...`, `*args`) widens
>   what a callable admits; a count logos cannot read — a spread `f(*xs)`, a
>   Python `cls.m(…)` (which passes the instance itself to an instance method)
>   — and a callable whose parameters it cannot count (a C++ out-of-line
>   definition, whose defaults sit on a prototype) filter nothing. So an
>   own-class `inh(a)` no longer takes `inh(1, 2)`: the call binds the base
>   class's `inh(a, b)` when the language records its classes' bases (Java,
>   Kotlin, C#, Python, PHP), and stays unbound in Scala, C++ and TypeScript,
>   which record none. A Kotlin unqualified call whose same-named members all
>   mismatch goes on to the top-level function or import of that name, as
>   Kotlin's resolution does — but only when the class's supertypes are all in
>   the graph, since an external base class or an interface's default body may
>   hold the overload it reaches, and never for `equals`, `hashCode` or
>   `toString`, which every class inherits from `Any`. In Java and Kotlin, a call that finds no applicable method anywhere along the
>   class's `extends` chain goes on to the bodies its implemented interfaces
>   provide (a Java `default` method, a Kotlin interface method with a body), a
>   superclass's method always first; a `static`, `private` or abstract
>   interface member is never a candidate
>   ([FR-RS-48](../specs/requirements/FR-RS-48.md)). C# does not inherit
>   interface bodies, so its classes never reach them. In Java, C#, Scala and C++ the member hides
>   every outer name even when no overload applies, so such a call stays
>   unbound. Two candidates that both admit the count
>   stay unbound (`overload-ambiguous`): logos reads no argument types. A call
>   that fits nothing is counted `no-applicable-overload`. JavaScript enforces
>   no arity, so a `.js`, `.mjs`, `.cjs` or `.jsx` call is never filtered;
>   `.ts`/`.tsx` calls are. Go and Rust have no overloading, so compiling code
>   binds as it did.
> - **Ambiguous doc/prose tokens.** Doc→code links bind only on *exactly one*
>   candidate. Common words that appear in prose but match zero or many code
>   symbols (`index`, `sync`, `node`, `context`, …) correctly bind to nothing.
>
> So a figure around 50% on a stdlib-heavy, documentation-rich repo reflects the
> **honest recall** of a never-fabricate, exactly-one-candidate resolver — not a
> broken index. The number is a transparency signal only: it gates nothing and
> never moves the quality signal ([FR-RS-04](../specs/requirements/FR-RS-04.md)).
> To gauge *workspace-internal* resolution, look at the `refs_resolved` count and
> the `artifact_bindings` breakdown rather than the raw ratio.

All of this obeys **never-fabricate**: a reference binds only when exactly one
candidate matches — an ambiguous template, a duplicate schema name, or a registry
/ vendored / interpolated target binds to *nothing* and stays in the
unresolved-reference ledger, retried on the next `sync` as targets land. Like
documentation, these cross-artifact edges are **metric-neutral** — they never move
the quality signal.

## Global flags and exit codes

Every command accepts:

| Flag | Effect |
|---|---|
| `--project <PATH>` | Operate on that project root instead of the current directory. |
| `--json` | Machine-readable JSON on stdout — always parseable, logs stay on stderr. |
| `--quiet` | Suppress non-essential output. `--json --quiet` still emits the JSON: machine output is essential output. |

Exit codes are a stable contract for scripting:

| Code | Meaning |
|---|---|
| `0` | Success. |
| `1` | Completed, but the answer is not the answer it claims to be: violations/threshold failures (`check`, `gate`), structural drift (`doctor`, `verify`), **result-level incompleteness** — a `workspace` subcommand that could not **open** one or more members ([FR-CL-03](../specs/requirements/FR-CL-03.md), [FR-WS-16](../specs/requirements/FR-WS-16.md)) — or an `index`/`sync` that **persisted nothing** of the files it reached, or whose pipeline itself failed ([FR-EH-05](../specs/requirements/FR-EH-05.md)). A run in which only *some* files failed to persist exits `0` and lists them in `files_failed`; an index that admits no file at all still exits `0`. |
| `2` | Usage error: bad flags, invalid config/rules file. |
| `3` | Internal/environment error — e.g. no index present (the message tells you to run `logos index`), engine failure. Never a raw panic. |
| `4` | `check` only: **no rules contract was loaded and nothing fired** ([FR-GV-22](../specs/requirements/FR-GV-22.md)). A verdict over an empty evaluated set is not a verdict, so `check` reports `passed: null` rather than a vacuous `true`. Note an always-on fold-in (structural, admission) can still raise a real violation with no `rules.toml` present — that stays exit `1`, not `4`. Pass `--allow-no-rules` to restore exit `0` when you have deliberately authored no contract yet. |

These codes are the CLI projection of Logos's **fail-soft / fail-loud** error
contract: a *degraded* condition (a skipped file, a partial resolution) warns
and still exits `0`, while a *correctness* condition (no index, corrupt store,
bad config) aborts loud. A file whose facts cannot be persisted is degraded too:
it fails alone, every other file persists, and the run exits `0` — only a run that
persisted *nothing* exits `1`. The one place a degraded condition *does* move the exit
code is a workspace member that could not be opened: the command still returns
its partial answer, but exits `1` so a CI step cannot pass over a
three-quarters-missing payload. A member that is merely un-indexed, never
needed, or evicted to stay inside the workspace budget leaves the code at `0` —
see [commands.md](commands.md#workspace-status). The full contract — both surfaces, every condition, and
a troubleshooting table — is in [error-handling.md](error-handling.md).

Scripting example:

```bash
# Re-run only the tests affected by the current diff
CHANGED=$(git diff --name-only HEAD)
logos affected $CHANGED --tests-only --json | jq -r '.affected[].file'
```

## Quality and governance

```bash
logos scan                       # compute metrics, persist snapshot, report signal
logos check                      # evaluate rules.toml — exit 1 on violations
logos gate --save                 # save today's signal as CI baseline
logos gate                       # compare to baseline; exit 1 on regression
logos doctor                     # fast structural-integrity check — exit 1 on drift
logos verify                     # deep shadow-reindex consistency check — exit 1 on drift
logos evolution                  # signal trend over snapshots
logos dsm                        # dependency-structure-matrix coupling clusters
```

`doctor` is the cheap always-on guard — structural integrity **and** an
admission tripwire that flags any indexed file the current admission rules
(gitignored, nested-`.git`-boundary, `ignored_dirs`, glob-excluded) would now
reject — folded into `health` and hard-failing `check`/`session_end` on either
kind of drift, independent of the signal, with a distinct `graph-admission-drift`
rule id in `check`'s report. `verify` is the deliberate deep check that
shadow-reindexes and diffs against the live graph, embedding the same `doctor`
verdict (including its `unadmitted_files`/`unadmitted_sample` fields). See
[commands.md](commands.md#doctor) for the full contract, including the nuance
that `check`/`gate`/`session_end` usually self-heal a still-on-disk admission
drift via their own reconcile before it would hard-fail.

Every quality command follows the **reconcile-then-score** contract: changed
files are synced before analysis runs. The `freshness` field in every
response confirms what was reconciled (e.g. `"reconciled 3 files · HEAD
abc1234 · 0 unresolved refs"`). Pass `--no-reconcile` to score the last
committed state — useful in CI after a pre-built index step.

**Typical CI setup:**

```yaml
- run: logos index
- run: logos check                      # fail fast on constraint violations
- run: logos gate --threshold 7500      # fail if signal drops below floor
```

This is the **freshen → enforce → report → bless** loop: `index` freshens,
`check` enforces (exit 1 fails the build), `scan --json` reports the signal
without blocking, and `gate --save` blesses a new baseline — **at release only,
never in the PR path**. The same loop is wired into your editor and `git push`
by [`logos init`](commands.md#init--i---hooks---workspace---yes---exclude-glob) (the freshness hooks, the `pre-push` gate,
and the session-start quality-report hook — which uses
[`quality-report`](commands.md#quality-report---hook-json), the write-free
readout, precisely so a per-session trigger never grows the snapshot series).
For the full copy-pasteable CI recipe —
GitHub Actions and a plain-shell variant, plus a local reproduction — see
[CI integration](ci-integration.md).

**Session gating (AI agent loop):** call `logos:session_start` before a batch
of edits and `logos:session_end` after — the MCP tools save a baseline at the
start and compare the final state at the end, exiting the gate if the agent
session introduced a regression.

See [commands.md](commands.md#quality--governance) for all flags and
[metrics.md](metrics.md) for how the signal is computed.

## Worktree-based development

A linked `git worktree` is a first-class Logos root, with no dependency on any
external orchestration tooling
([FR-WT-05](../specs/requirements/FR-WT-05.md)):

- **Opening the engine in a worktree just works.** `.logos/` resolves against
  *that worktree's* root (`git rev-parse --show-toplevel`), never the primary
  checkout's. With no `.logos/logos.db` yet, Logos **seeds from the primary
  checkout's DB** (found via `git rev-parse --git-common-dir`) and reconciles
  only the git-diff from the primary — O(diff-from-main), not a cold O(repo)
  index ([ADR-15](../specs/architecture/decisions/ADR-15.md)).
- **The seed carries the governance contract too.** Alongside the DB, the
  first-time seed copies `.logos/rules.toml` and `.logos/config.toml` from the
  primary checkout, byte for byte
  ([FR-WT-06](../specs/requirements/FR-WT-06.md)) — so a seeded session
  evaluates the *same* rules as its primary and `logos check` does not exit
  [`4`](#global-flags-and-exit-codes) merely because `.logos/` is gitignored.
  It **never fabricates** (a primary missing a file seeds a worktree missing
  it) and **never overwrites** (a policy file that arrived through git on the
  branch, per [FR-WT-02](../specs/requirements/FR-WT-02.md), is left untouched).
  A failed copy is fail-soft, not fatal.
- **Installed git hooks fire in the worktree too.** `logos init --hooks` sets
  `core.hooksPath` to the *relative* `.logos/hooks`, and git resolves a relative
  `core.hooksPath` against the top level of the working tree the command runs in
  — so in a linked worktree it resolves to `<worktree>/.logos/hooks`, which the
  DB seed alone never created. The first-time seed therefore also creates that
  directory, as **symlinks** back to the primary's scripts, so a re-install that
  updates a script can never leave a worktree running a stale copy
  ([CR-106](../requests/CR-106-git-hooks-never-fire-in-a-linked-worktree.md),
  [FR-IN-06](../specs/requirements/FR-IN-06.md)). `core.hooksPath` itself stays
  relative — reachability comes from seeding, never from absolutising the path.
  A hook that fires in a worktree acts on *that worktree's* graph, not the
  primary's. On a platform without usable symlinks the fallback is copies plus a
  staleness marker that `logos doctor` reads; silent copies are never left
  behind. `logos init --hooks --uninstall` removes both the primary installation
  and any worktree `hooks/` directories the seed created, so an upgrade leaves
  no dangling symlinks.

- **A gitignored file inside the worktree is never indexed.** The same
  `AdmissionAuthority` that guards `sync`/the watcher in the primary checkout
  guards the worktree's own `sync` too — parity, not a separate rule.
  Symmetrically, a worktree living *under* the primary root is excluded from
  the **primary's** graph by the nested-`.git`-boundary rule.
- **Wrap the work in a session bracket.** `logos:session_start` /
  `logos gate --save` on entering the worktree and `logos:session_end` /
  `logos gate` on leaving it score *that worktree's own* `.logos/logos.db`, so
  the aggregate quality delta you read at the end reflects only what happened
  in the worktree.
- **Merge-back is reconcile-to-source — never a cross-DB graph merge.** After
  the worktree's branch is merged into the primary, run any aggregate command
  (`scan`, `check`, `gate`, `logos index`, …) in the primary checkout: the
  reconcile prologue and the FullWalk sweep convergence
  ([FR-RC-01](../specs/requirements/FR-RC-01.md),
  [FR-RC-06](../specs/requirements/FR-RC-06.md)) bring the primary graph to
  exactly `index(merged source)` — byte-identical in population to a fresh
  `logos index` ([NFR-RA-06](../specs/requirements/NFR-RA-06.md)). Node ids are
  per-database rowids, so diffing a worktree's graph and applying its deltas
  into the primary's DB is deliberately unsupported; reconcile-to-source is the
  only blessed merge-back path. Run `logos verify` afterward as the deliberate
  proof — it reports `ok` when the primary equals a fresh reindex.
- The worktree's own `.logos/logos.db` is ephemeral: once its branch is
  merged, discard the `.logos/` directory or remove the worktree — nothing
  carries derived graph state back into the primary, only the source diff
  does.

See [ADR-48](../specs/architecture/decisions/ADR-48.md) for the design
rationale (why a cross-DB graph merge was rejected) and
[CR-054](../requests/CR-054-graph-update-admission-unification.md) for the
full change.

## Evidence tiers: hotspots and coverage

Two advisory tiers layer over the graph from a second store, `.logos/history.db`
— neither moves the quality signal (the gate never opens `history.db`):

```bash
# Temporal tier — where risk concentrates over time (churn × complexity)
logos hotspots --limit 20            # top 20 ranked files (lazily mines git history)
logos hotspots --untested            # only files with no fresh positive coverage

# Coverage tier — fold in an external coverage report, then read freshness
logos coverage ingest target/coverage/lcov.info     # LCOV or Cobertura (auto-detected)
logos coverage refresh                               # run [coverage_ingest].refresh_cmd, then ingest its artifact
logos coverage status --json                         # per-file freshness + overall fraction
```

The **untested-hotspots** join (`logos hotspots --untested`) is the headline
report: high-churn, structurally-complex files that have no fresh test coverage.
With coverage ingested, each hotspot row also carries a `coverage` cell
(`fresh`/`stale`/`n/a`); with nothing ingested, `--untested` falls back to a
clearly-labeled static-reachability signal rather than guessing. Coverage
freshness is content-hash based — edit a covered file and it flips to `stale`
on the next `coverage status`, carrying no stale line numbers.

**Automatic ingest (CR-036).** With a `[coverage_ingest]` table configured, the
`serve` **watcher** auto-ingests a coverage artifact the moment it appears or
changes on disk (matched by the built-in conventions ∪ a configured
`artifact_glob`) — so a coverage run in your own CI/test loop folds in without a
manual `coverage ingest`. Auto-ingest is **non-load-bearing**: any failure
degrades to a warning, it is a local read+parse only, and it **never spawns a
subprocess** (the watcher path stays side-effect-free, [ADR-38](../specs/architecture/decisions/ADR-38.md)).
`logos coverage refresh` is the one place Logos ever *runs* a coverage command:
it executes the author-configured `[coverage_ingest].refresh_cmd` as a subprocess
(only on explicit invocation, never on the serve/watcher path), then discovers
and ingests the artifact it produced — erroring loudly if no `refresh_cmd` is
configured, the command fails, or it produces no recognizable artifact. The
`[coverage_ingest]` table changes *when* ingest happens, never which sources are
admitted, so it is excluded from the admission fingerprint and leaves every
metric/cycle/DSM scope byte-identical. `coverage_refresh` has an MCP twin. See
[commands.md](commands.md#evidence-tiers-history--coverage) for full flags and the
`[history]`/`[coverage]` tuning tables in
[configuration.md](configuration.md#history--coverage--the-evidence-tiers).

## Setting up the MCP server (AI agents)

`logos serve --mcp` speaks the Model Context Protocol over **stdio**. The
recommended setup uses `logos init -i`, which injects the server block into
`.mcp.json` automatically — restart your agent and the `logos:*` tools appear.
For manual wiring:

```bash
claude mcp add logos -- /path/to/logos --project /path/to/project serve --mcp
```

The host then sees **25 `logos:*` tools**, all live (34 when the server starts in a
workspace: the nine `xservice_*`/`workspace_*` tools join them):

| Navigation (11) | Quality & Governance (10) | Evidence tiers (4) |
|---|---|---|
| `search`, `node`, `callers`, `callees`, `impact`, `impact_intersection`, `precedent`, `branch_overlap`, `explore`, `context`, `status` | `scan`, `rescan`, `check_rules`, `health`, `doctor`, `verify`, `evolution`, `dsm`, `session_start`, `session_end` | `hotspots`, `coverage_ingest`, `coverage_status`, `coverage_refresh` |

Each MCP tool is a thin twin of the CLI command of the same name and returns a
byte-identical payload — `check_rules` is spelled `logos check`, and `rescan` is
the one deliberate MCP-only tool (it replays the last scan's parameters *within a
server process*, which a one-shot CLI invocation has no equivalent of; use
`logos scan`). The session gate is reachable from either surface:
`logos session-start` / `logos session-end`, with a failing `session-end`
exiting 1. The four evidence-tier tools are **non-gated** — like
their CLI counterparts they never move the `session_end` gate signal.

Server behavior guarantees:

- **Fast cold start** — the server answers its first request (the `initialize`
  handshake) in well under a second, even in a repository with a large build-output
  tree. Watcher registration prunes ignored directories (`target/`, `node_modules/`,
  `dist/`, `build/`, `vendor/`, `.git/`, and anything in `[semantics].ignored_dirs`)
  through the same admission authority that guards `index`/`sync`, so it never walks
  build output to seed rename tracking. A language's tree-sitter queries compile on
  the first extraction that needs them, once per process, so start-up pays nothing for
  languages the repository does not use; each first compile is logged once with its
  duration at `RUST_LOG=info`. An override under `.logos/plugins/<lang>/` still
  compiles at start-up and fails naming its file.
- **Stdout purity** — stdout carries only JSON-RPC frames, even at
  `RUST_LOG=trace`; logs go to stderr. A malformed frame gets a structured
  parse error (`-32700`) and the server keeps answering.
- **Clean teardown** — when the host disconnects (stdin EOF, even before the
  handshake), the server winds down by itself with exit 0: no orphaned
  processes to clean up.
- **Honest telemetry** — agent calls are stamped `surface: "mcp"` in
  `logos stats`, CLI calls `surface: "cli"`, one record per call.

### Asking about one workspace member (`repo`)

When the server starts at a [workspace](commands.md#init--i---hooks---workspace---yes---exclude-glob)
(a `logos.workspace.toml` found up-tree), its read-only tools answer for **one member**, chosen
by an optional `repo` argument — the member's name. Without it the **default member**
answers: `[workspace] default`, else the first member. Every answer under a workspace carries
`member`, the default member's included, so a reader never has to infer which repository
answered; in a single-root server the payload has no `member` and is exactly what it was.

| Takes `repo` (and the CLI twin takes `--repo`) | Cannot target another member |
|---|---|
| `search`, `context`, `explore`, `node`, `callers`, `callees`, `impact`, `impact_intersection`, `precedent`, `branch_overlap`, `status`, `health`, `hotspots`, `dsm`, `evolution`, `coverage_status` | `scan`, `rescan`, `check_rules`, `verify`, `doctor`, `session_start`, `session_end`, `coverage_ingest`, `coverage_refresh` — they write or gate, act on the default member, say so in their description, and refuse a `repo` |

The `xservice_*` tools keep their own `repo`: for `xservice_callers`/`xservice_impact` it
names which member's symbol is meant, for the others it scopes the fan-out; a name that is no
member is reported inside the answer, not refused. A `repo` that names no member is refused with the list of members; a
member whose store will not open, with the reason; a `repo` on a single-root server, with
"no workspace is loaded". `health` and `dsm` freshen the member's index before scoring and
`hotspots` caches mined git history in it; none of the listed tools writes a gate, baseline,
snapshot, rule verdict or coverage evidence.

The CLI asks the same question the same way — `logos node beta_only --repo svc-b` — and is
refused in the same words, with one difference: a member that was never indexed is indexed by
the server on first selection (as the default member always was), while the CLI refuses it
and writes nothing; see
[commands.md](commands.md#answering-for-one-workspace-member---repo) for the flag, the exit codes,
and how a command run at the workspace root, or inside a member, chooses its member.

### Try it without a host

A raw JSON-RPC session over a pipe is enough to smoke-test:

```bash
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
  | logos serve --mcp 2>/dev/null
```

## Watching usage and savings

```bash
logos stats               # trailing 7 days
logos stats --window 30   # trailing 30 days
```

Reports calls per tool split by surface (`cli`/`mcp`), latency percentiles,
and an estimate of file reads (and tokens) saved by graph navigation versus
raw file reading. Stats read only `telemetry.db` — they work even before (or
without) an index. **Self-referential reads are excluded from every figure** —
`stats` reading the telemetry store and the dashboard shell's own `status`
readout, because counting them would measure the measurement, not the tool's
value. A graph query a person issues *through* the dashboard counts normally
(see [commands.md § `stats`](commands.md#stats)).

## The web UI dashboard

A localhost dashboard renders the same read-models the CLI/MCP surfaces expose,
plus an interactive **Config** editor (the one bounded exception to read-only —
see below). It is a single embedded **React single-page application (SPA)** —
served at `/`, client-side routed, reading its data from a same-origin JSON API
under `/api/v1/*` ([CR-049](../requests/CR-049-decouple-web-ui-into-embedded-spa.md),
[FR-UI-22](../specs/requirements/FR-UI-22.md),
[ADR-43](../specs/architecture/decisions/ADR-43.md)). Since
[CR-078](../requests/CR-078-ui-default-egress-carve-out.md)/[ADR-60](../specs/architecture/decisions/ADR-60.md)
the dashboard ships **by default** — a plain `cargo build`/`cargo install` carries
it — and it links no HTTP client:

```bash
# Default: the dashboard, egress-free. Listens on loopback, links no HTTP client.
cargo install --path cli                 # or: cargo build -p logos --release
logos serve --ui --port 4983             # then open http://127.0.0.1:4983
```

### Build matrix — dashboard or slim headless

CR-078/ADR-60 separates *listening* (the read-only dashboard) from *dialing* (an
outbound HTTP client, which no build links). Two build modes result:

| Build | Command | Dashboard (`serve --ui`) | Outbound HTTP client |
|-------|---------|--------------------------|----------------------|
| **Default** | `cargo install --path cli` (no explicit features) | ✅ served | ❌ **none** — links the loopback *server* only, never a client |
| **Slim headless** | `cargo build -p logos --no-default-features --features lang-all` | ❌ no `--ui`, no listener | ❌ **none** — the absolute-offline binary; no socket stack at all |

The default build carries the dashboard together with the CLI and the MCP server.
The slim `--no-default-features` build is the headless CLI/MCP binary with no web
surface. Two structural fitness checks
(`logos-core/tests/no_network_deps.rs`) keep the postures honest: the **default**
tree may link the loopback HTTP *server* but is denied every egress *client* crate
(it can never phone home), and the **slim** tree is denied the entire socket stack
(it links nothing that can open or accept a connection) —
[NFR-SE-01](../specs/requirements/NFR-SE-01.md), ADR-60.

The surface binds **`127.0.0.1` only** (a compile-time constant — no flag or
env var moves it), rejects non-loopback `Host` headers with `403`, and stamps a
self-only `Content-Security-Policy` on every response. Every non-GET request is
answered `405` **except** a `POST` to one of a small, enumerated set of mutating
routes — the config save and apply routes (`/config/save`, `/config/apply`),
the workspace manifest and config saves, and the on-demand `verify`
(`/api/v1/verify`); those are additionally gated by a
same-origin + per-session intent (CSRF) guard, so a request missing the `x-logos-intent` token or arriving cross-origin
is rejected `403` before any handler runs. The whole React app — its hashed
JS/CSS and fonts — is built at build time (Node/Vite run only during the build,
never at serve time) and **embedded in the binary**, so a page load fetches
nothing from the network. The intent token reaches the SPA as a
`<meta name="logos-intent">` tag injected into the served shell; the app reads it
once at startup and echoes it in the `x-logos-intent` header on every mutating
request. **JavaScript is required** — the SPA is the only rendering model; the
accessibility intent formerly served by no-JS data-table fallbacks is preserved
as explicit in-SPA WCAG 2.1 AA affordances
([FR-UI-22](../specs/requirements/FR-UI-22.md),
[ADR-43](../specs/architecture/decisions/ADR-43.md)).

The views, each sidebar item rendered with an inline-SVG icon and grouped into
three use-driven **navigation groups**. Each tab is a client-side route the SPA
renders inside the app shell — navigation never reloads the page, and a refresh
on any client route resolves back to the shell via the server's history fallback.
The **Dashboard is served at the root `/`**; the legacy `/overview` path that
older bookmarks point at **redirects to `/`** (a history `replaceState`, so it
leaves no extra back-button entry). The browser tab carries the Logos **favicon**,
an embedded same-origin SVG served from the bundle root.

**The app header reports graph state, on every view.** It renders
`rev <graph_revision> · <node_count> nodes · <edge_count> edges`, thousands-separated,
read from `GET /api/v1/status` — a thin projection of the same `status` read-model
`logos status` prints, so the figures agree with the CLI for the same project. This
replaced a green "Read-model connected" badge that was decided by a loopback request to
the process answering it, and so could only ever display success; it also cost ~993 KB per
navigation to set one boolean, against the ~831-byte block actually worth rendering.
**Refresh is navigation-driven and nothing else** — there is no timer and no polling, so
editing a file and letting the watcher sync leaves the figures unchanged until you next
navigate, which then shows the advanced `graph_revision`. A read fault renders a red
"API unavailable" badge *instead of* the figures, and an unindexed project renders an
honest not-indexed state — never a zero, never a blank, and never the last-read figures
presented as current. In workspace mode the figures describe the **selected** member and
re-read on member switch.

**Below 1024px the readout is dropped whole** — the figures, `Connecting…`, the
not-indexed state and the "API unavailable" badge alike, because all four render into one
element and hiding it drops the readout rather than clipping it. Nothing is truncated: a
clipped `rev 3,9…` presented as a figure, or a fault badge standing in a dropped
readout's place, would each report something that is not so. The brand lockup, the member
selector and the theme toggle survive to the narrowest supported viewport; the brand
subtitle gives way below 768px. Widening the window brings the readout back with the
figures from the last navigation. The request itself is unaffected — the header still
reads `/api/v1/status` on navigation at every width, and those reads are classified as
shell chrome, so they never enter the tool-usage figures.
Each read view's figures trace to a read-model, and an empty store renders an
honest empty state naming the producing command:

- **Group A — read & navigate:** Dashboard `/`, Health `/health`, Graph `/graph`. The Architecture view is hidden (see [Hidden widgets](#hidden-widgets)): `/architecture` and the retired `/dsm` bookmark land on Health.
- **Group B — analyse:** Files & Risk `/files`, Rule findings `/gaps`, Coverage `/coverage`.
- **Group C — configure:** Statistics `/statistics`, Config `/config`.
- **Workspace scope (workspace mode only):** Workspace Dashboard `/workspace-dashboard`,
  Workspace Health `/workspace-health`, Workspace `/workspace`, Workspace Statistics
  `/workspace-statistics`, Config `/workspace-config`. These answer for the **whole
  workspace**, not for the selected member, and the member selector does not govern them.

### Reading a widget

Every figure on every view sits in a **widget**, and every widget reads in the
same three parts, top to bottom (CR-203 and CR-206, since S-617 on every view):

1. **Title and figure.** The title names the widget, with at most one status
   badge beside it (`PASS`, `FAIL`, `Advisory`, …). Below it, the figure: the key
   number — and a share always with what it is out of (*2 of 3 members
   answered*, never *2* alone). When there is nothing to show yet, this line says
   so in words, in place of the figure, and names the command that fills it when
   one does (*No coverage ingested yet; run `logos coverage ingest <report>` on
   the LCOV or Cobertura report your test run writes.*).
2. **Explanation.** What the widget shows, then why it matters — which decision
   it supports or what it protects you from. A term of the project's own
   vocabulary is underlined where it is first used; hover over it or focus it
   with the keyboard for its meaning in plain words.
3. **Evidence.** The table, chart or list the figure comes from.

The widgets on a view are stacked in one column, an equal gap apart, every part
left-aligned and the explanation set in the body text size.

**Tool panels** — the graph query form, the Graph view's accessible tables, Decisions &
docs and the Config editors — present no figure. They keep the same frame and say in
one line what they are for, and have no explanation of why and no evidence of
their own.

**Workspace mode.** When `logos serve --ui` starts at a workspace parent (a
`logos.workspace.toml` is discovered up-tree), the app shell renders a **member
selector** that scopes the per-member views to one member (the member is part of
each view's cache key, so switching re-fetches).

**Every view declares the scope it answers for**, and the sidebar is split to match:
a **Workspace** section for the views that answer for the whole workspace, and a
**Service** section whose header carries the member selector — so the control sits
inside the boundary it governs and nothing outside that section is scoped by it. The
Service header is a column — the section label on its own row, the selector at full
width beneath it — so the label stays readable however long the member name is. The
Workspace section reads as one list (no divider between its groups), so the workspace
Statistics entry sits visibly with the other workspace views.
Scope is a declared field on each navigation entry, not a path-prefix guess, so a
view cannot silently fall into the wrong scope by being named differently.

**The selected member is in the URL.** A workspace URL carries `?repo=<member>`, so
it can be bookmarked and shared and it opens on that member on first paint — with no
unscoped read issued first. Switching members writes through history (no reload, and
no extra back-stack entry for a same-view switch); back and forward restore the
member the entry names. A `?repo=` naming a member the workspace does not have
renders a state **naming the members it does have**, and **no view renders any
member's figures** while it is displayed — the failure being prevented is a
`200`-shaped page showing the default member's numbers under a requested name. A
blank `?repo=` is unscoped. In a plain single repo the parameter is inert.

The three workspace-scoped views answer questions the per-member views cannot:
**Workspace Dashboard** renders the server's own composed cross-service
edge-count/egress-rate line (never the count without the rate beside it) with
coverage by intake (the per-arm board is hidden — see [Hidden widgets](#hidden-widgets)); **Workspace Health** renders the degraded roll-up in
words with a per-member table, the advisory governance findings and the topic
inventory (the **Broker topics** widget, formerly *Promoted broker topics*),
drawing every unopenable member as degraded and naming it; and
**Workspace Statistics** sums telemetry over members. On the Workspace Dashboard,
the Workspace tab's **Cross-service coverage** tab and Workspace Health, each
widget states what it shows and why it matters (since S-613; Workspace Health's
other four widgets since S-617). **Resolved cross-service edges** shows why the
unresolved outbound calls did not resolve as a table of reason and count, largest
reason first, whose counts sum to the unresolved figure; **Cross-service
reachability** leads with the callables to keep because another service calls
them; **Members** draws a member that could not be opened with a red *degraded*
State badge and the reason beside it; and **Workspace rules** with no
rules declared says so and names `[[governance.boundaries]]` in
`logos.workspace.toml` in that sentence. Every ratio on these views
carries its denominator and its exclusion — a zero denominator renders the ratio
**absent** ("not measured", no bar) with the excluded count still shown, rather than
a `0%` that reads like a measurement. Workspace Statistics states its **member
denominator** ("summed over N of M workspace members") and names every member whose
telemetry could not be read; it reserves "a lower bound" for members that **failed**
a read, never for one that simply has no store yet, since an absent store is a
successful determination of no store. It is read **engine-free** — the load leaves
the resident-engine count unchanged — which is why it omits `artifact_bindings` and
latency percentiles: both are `Engine`-bound, and the ceiling is the point.
No workspace-level quality score is rendered anywhere, and no per-member signal is
aggregated into one.

The workspace **Config** view (`/workspace-config`, labelled *Config* in the Workspace
section, beside Statistics) edits the workspace manifest, `logos.workspace.toml`, in the
same hybrid grammar as the member Config tab — typed fields plus a raw TOML pane, with
live validation. It covers `[workspace]` (members, `default`, `autodiscover`),
`[workspace.warm]` and the full `[governance]` family, shown beside the advisory
findings `GET /api/v1/workspace/check` reports. The panel states **at the point of
editing** that workspace governance is advisory and never moves a member's gated
signal. The running serve keeps the manifest it started with, so after a save that
changes members, warm concurrency or the rules the view says so; they take effect after
`logos serve` is restarted.

The view has three properties. **A save is validated before anything is written**
(a rejected document leaves the file byte-identical, and a document identical to disk
writes nothing). **A file changed on disk since the view loaded it** — by hand, or by
`logos init --workspace` — is **not overwritten silently**: the save is refused and the
view shows what is on disk now, offering to load that copy (your edits are discarded) or
to overwrite it with your edits, and says which happened. **A file broken on disk still
opens**, with its fault named by file and line/column, so the view is the repair path.
Saving touches no member's `.logos/` and reindexes nothing. In a plain single repo the
view does not exist.

Alongside the per-member views it
exposes the cross-service surfaces: an **app-level service map** — the ECharts
graph canvas drawing services as nodes and cross-service bindings as edges,
including first-class **topic hops** (`A → topic → B`) once broker coupling is
promoted — a **cross-service coverage dashboard** (bound / ambiguous /
unbound), and a cross-service impact answer (served, but hidden from the
web UI — see [Hidden widgets](#hidden-widgets)). Since logos 1.4.13 every
service-map link also carries its **provenance**, so an edge admitted from committed
configuration is never drawn as though it had been observed at the call site: a link with
any non-literal binding renders a distinct stroke while its relation colour stays the
relation arm, the accessible table gains a **Provenance** column giving the per-kind
breakdown (`literal` / `config-bound` / `config-unresolved` / `unstated`) rather than
labelling an aggregated link as a single kind, and the edge detail names the evidence —
for each `config-bound` end the key, its defining source paths and the profile set, one row
per overlay; for a `config-unresolved` end its keys and the refusal. Rows identical in
every column are merged into one, with a **Calls** count of how many calls it stands for
(since S-614). A row with no value states why in its *Committed value* cell — *No committed
source defines it*, *The committed value is itself a placeholder*, *Not committed by the
repository*, or a newer refusal named as the server sent it — and *Defining sources* names
the files that define the key. The legend's
provenance section, the Provenance column and filter, and the **Binding evidence** widget
render only when at least one link is non-literal, so a workspace with no admitted binding
gains none of them (S-419, CR-132).

**Filtering the bindings (since S-614).** The **Cross-service bindings** widget below
the map filters its table by text (a consumer or provider name, case-insensitive), by
binding kind (HTTP, gRPC, broker) and, when the Provenance column exists, by provenance
kind; its figure reads *n of m bindings shown*, counting rows, while the table's
**Calls** column counts the calls behind each row. The three filter inputs sit on one
line whether or not a field has a hint under it. The filter narrows that table and
**Binding evidence** together and never the map, which always draws every binding. Each
service-map widget states what it shows and why it matters.

**The build layer (since S-464).** When any member holds a Maven or Gradle manifest, the
service map's legend gains a **Build dependencies** checkbox — *off* by default, so the
picture you land on is still runtime coupling only. Checking it draws what each member
builds against, in its own `build` edge class (a hue no runtime arm uses), with an
accessible **Build dependencies** table giving each pair's kinds, artifacts and reference
count. A member declared [`kind = "platform"`](configuration.md#kind--platform--build-hubs)
is **collapsed**: its inbound build edges are not drawn, and a line names it with how many
members build against it. A **Cross-context model hint** widget, badged *Review hint*,
lists the members that depend on the model libraries (`<group>.<context>:kafka-models`, or
`<context>-kafka-models`) of two or more bounded contexts, each library named — a report you
review in the member's build manifest (`pom.xml` / `build.gradle`), never a failure and
never drawn as an edge. The **Cross-service coverage**
tab gains a **Build dependencies** card after every runtime board, rendering the server's
own headline line (pairs by kind beside their denominator), the declared platforms apart,
the platform candidates and any colliding artifact. A build dependency is never a runtime
coupling: no runtime figure counts it. A workspace with no build manifest shows no toggle
and makes no extra request; its **Build dependencies** card states that no member holds a
Maven or Gradle manifest.

**The declared layer (since S-461).** When a member holds a
[vendored spec](configuration.md#vendored-specs--declared-contracts-and-named-externals),
the service map draws each declared contract as a **dotted** edge in its own
`declares-contract` class and hue — to the member whose own spec the document is,
or to a **named-external node**. External nodes are keyed by identity and
labelled by name, so the reference estate's two `PSS` groups are two nodes; they
share the topic hue, and the tooltip gives their kind as `external`. Clicking one
selects no member. Every registry external gets a node, including one only a
`mock` stands in for, drawn with no edge. The layer is always on (there is no
toggle) and does not count toward node size, which reads as observed coupling.
The legend gains a **Declared contracts** section: the edge row *Declares a
contract (a vendored spec)* (only when an edge is drawn), the named-external row
and the server's own headline line. The **Declared contracts** widget that used
to sit below the map — its documents, each with its identity score or the external
it groups into, the calls bound to each external with their matched operation and
base-path source, and the named externals — is hidden from the web UI (CR-208; see
[Hidden widgets](#hidden-widgets)). The same facts are in `logos workspace status`
and `logos xservice route-providers` (`declared_contracts`, `bound_external`). A
refused call is never drawn; the server's summary counts it. On the
**Cross-service coverage** tab, a
**Declared contracts and named externals** card follows the runtime boards (before
the build card) with the two server lines — declared contract pairs over the spec
documents read, and bound calls over the no-provider REST rows with their
refusals. A bound call **stays under *No provider here***: the binding is
reported beside its row, and no runtime figure counts it. A workspace with no
vendored spec and no `kind` shows no declared edge, legend section or card.

In a plain single repo
(no manifest) **no selector is rendered and the UI is byte-for-byte unchanged**;
`--standalone` forces single-repo focus even under a workspace parent. Workspace
read-models are served under `/api/v1/workspace/*`; the single-root `/api/v1/*`
surface and the self-only CSP are untouched.

The **Files & Risk** view (`/files`) is the merge of the former Hotspots and
Commits views into one risk-ranked per-file table — commits, churn (`+/−`), age,
co-change, defect density, complexity, and a coverage cell — with `?untested`
keeping only files lacking fresh positive coverage. The per-file table is the
evidence of the **Files ranked by risk** widget, and its `n/a`
cells (e.g. churn/age for a file with no history) right-align to their numeric
columns (CR-042). A path longer than 40 characters is abbreviated to its first
segment, an ellipsis and its last two segments (`logos-core/…/resolve/binder.rs`),
in both the risk table and **Ownership dispersion**; hover it for the full path,
or focus it with the keyboard to see it as a tip. Two rows that would abbreviate
the same keep more segments until they differ, and the File column sorts by the
full path. Each of the two widgets says what it ranks and why it matters; when
nothing is ranked yet the board says so and names `logos hotspots`, and a
single-author history is stated as such. The
**Co-change** and **Defect** headers explain themselves on hover and focus
(CR-203). On **Health** (`/health`) the Quality-signal table carries a
**Score** column rendering each metric's normalized value as a CSP-safe `<meter>`
bar — the same widget the Dashboard roll-ups use; applicability drop-outs
(Cohesion/Focus) show a muted, right-aligned `n/a` with no bar (CR-042).

Since logos 1.4.14 the Health page's two cards — the gate and the quality
signal — are built from **one** read of the last persisted snapshot rather than two
independent ones, so a `logos scan` landing while the page loads can no longer
render two cards describing different generations of the same figures. The
interleaving window does not exist rather than being narrowed. The same change
adds a **stated-stale** branch: when a signal is present but the graph is *no
longer indexed*, both cards still show their figures, but labelled as describing
a graph that is no longer indexed, carrying the snapshot's date and naming
`logos index` as the way to refresh. The figures are genuine history, so they are
neither hidden nor restyled away — they simply stop being presented as a current
verdict, and the unqualified current-verdict wording is absent. While the graph
*is* indexed the cards render exactly as before. An **absent** signal keeps its
own three-way distinction (never checked / checked-and-empty / unavailable),
which this change deliberately leaves alone.

Since logos 1.4.15 that staleness branch has a **second** condition beside the
de-indexed one. A snapshot persisted before the graph was last re-indexed is a
snapshot the graph has **moved past**, even though the store is still indexed and
the old page called it `current`: the band now drops the word `current` and
carries the snapshot's own date instead. The comparison is the snapshot's
`created_at` against the last index or sync — *not* a comparison of node counts,
which cannot work, because the snapshot counts a production-scoped graph with
test vertices dropped while the status counts the whole graph, so the two differ
on every project that has tests.

Read what this claims precisely: it detects **disagreement, never currency**. A
re-index that changed nothing still trips it, so it over-reports rather than
under-reports — the safe direction for a readout, and the wording claims only
what the comparison establishes. Where the ordering cannot be established at all
— an absent or future-dated sync stamp, or an undated snapshot — the page renders
**neither** `current` nor a stale date. The de-indexed branch is unchanged and
takes precedence, and no fourth state was added.

**Worst offenders are the ones the snapshot computed (since logos 1.9.0).** The
Nesting, Conciseness, Cohesion, Focus and Uniqueness widgets on Health — and,
since CR-209, Acyclicity, Depth, Equality and Redundancy — list the
offenders the last `scan`, `gate` or session snapshot persisted, in the order it
computed them, read in the same single snapshot read as the signal and the gate.
The page never recomputes them. Each of these widgets renders one of three states
and never infers the state from an empty list:

| State | What the widget shows |
|---|---|
| Offenders recorded | The offender table, in persisted order |
| Recorded, none flagged | *No offenders flagged within thresholds.* — the only state that means a clean result |
| Not recorded | *Offenders were not recorded for this snapshot; run `logos scan` to record them.* — a snapshot written before 1.9.0, or a store never scanned |

A not-applicable dimension (Cohesion or Focus with no classes) keeps its `n/a`
rendering in every state. After upgrading, the existing snapshot reads "not
recorded" until the next `logos scan`. The same holds for the four CR-209 lists
on a snapshot recorded before them: each one whose score says it is not empty
reads "not recorded". One whose score is clean (no cycle, depth 1, an even
spread, nothing dead or duplicated) reads "none flagged", which is then true.

**Every dimension has a widget, and every widget explains itself (unreleased,
CR-203, CR-206).** Health renders the Gate, the Quality signal, one widget per
quality dimension — all ten, in the Quality signal table's order — and the Signal
trend, each stating what it shows and why it matters. The Gate states its pass
condition, *passes at ≥ baseline − ε*, with ε read from the gate result; a pass
the gate reached without comparing (no baseline, or one recorded under other
thresholds or metric semantics) is stated as an informational pass instead; a
FAIL names the lowest-scoring dimension under the figure. An absent reading names
the command that fills it in its own sentence — `logos index` when nothing is
indexed, `logos scan` when no scan has run — and so does a reading that is not
current (`logos index` after a de-index, `logos scan` once the graph has moved
past the snapshot); every dimension widget then says it is not current and points
to the Gate. The Quality signal folds in what the Aggregate scope card used to show: the functions
scored and excluded, and the thresholds fingerprint with what a change to
`[metric_thresholds]` does to the gate. Modularity carries no offender list in
the payload: it scores the directory layout as a whole, and its widget says so,
never an empty table. The other nine list their worst offenders (CR-209 added
Acyclicity, Depth, Equality and Redundancy).

**The same release stops the dashboard inventing an age it cannot establish.**
Every relative age the SPA renders (`just now`, `5m ago`, `6d ago`) is derived
from a unix stamp, and one of its inputs — `status.last_sync_at` — is a **file
mtime**, so a stamp *ahead of now* is routine on copied trees, NFS mounts and
restored backups rather than theoretical. Before 1.4.15 such a stamp was clamped
to zero and rendered `just now`: the most reassuring reading of a fact the page
could not establish. It now names the condition instead, in the CLI readout's own
words, so one product says one thing about one condition:

```
at an unknown age (recorded ahead of now — check the clock)
at an unknown age (the recorded time is implausibly old — check the store)
```

Ordinary past ages render exactly as before. These are the same two strings the
Health page's indeterminate branch above reuses — imported, not restated, so a
future rewording changes both surfaces or neither. The **Architecture** view — the
module dependency matrix (DSM) — is hidden from the web UI (CR-208; see
[Hidden widgets](#hidden-widgets)); `logos dsm`, MCP `dsm` and
`GET /api/v1/architecture` serve the matrix unchanged.

The root `/` view is the **Dashboard**: a roll-up that leads with the index's
freshness, then one column of widgets (since S-617; the equal-size pairs of CR-037
are gone, because every view stacks its widgets in one column): the banded
**Quality index** (the 0–10000 signal) with the gate's `PASS`/`FAIL` badge;
**Languages**, the per-project language composition; **Graph**; **Activity**;
**Rule findings**; and **Code coverage** (the overall line-% aggregate) — each
fed by a read-only accessor, never a fabricated number. **Rule findings** projects
`check_rules` (FR-GV-02): it reads **PASS** when a contract was evaluated and there
are zero rule violations, **FAIL** when there are findings, and states that
nothing was checked — naming `.logos/rules.toml` as the place to declare rules —
both when no `.logos/rules.toml` exists and when a contract authored **zero
rules**: a check that evaluated nothing is not a pass, so no PASS badge is
rendered over it. Violations still win: a finding raised by an always-on fold-in
reads FAIL even on a zero-rule contract. The Rule findings view (`/gaps`) shares
this widget's words and treats a zero-rule contract the same way. **Graph** leads
with the share of references resolved and lists Files / Nodes / Edges and, between
them, the source/test **lines-of-code roll-up** — Total, Source, and Test LOC —
mirrored from the `status` read-model's `total_line_count` / `source_line_count` /
`test_line_count`, with a caption noting the figures reflect the last full index.
When the roll-up has not been computed the widget says so and names a full
`logos index` rather than printing a fabricated `0`. Loading the Dashboard writes
nothing to any store.

**The `/api/v1/*` data API.** The SPA reads every view's data from a same-origin
JSON read-model API under `/api/v1/*` — one read-only endpoint per view's data,
each a JSON serialization of an `Engine` read-model
([FR-UI-22](../specs/requirements/FR-UI-22.md),
[ADR-43](../specs/architecture/decisions/ADR-43.md)). All are GET-only (`405`
otherwise), loopback-only (`403` for a non-local `Host`), and **write-free** — a
read mutates no store and leaves every metric/cycle/DSM/dead-code scope
byte-identical. The endpoints are: `/api/v1/overview`, `/api/v1/health`,
`/api/v1/graph`, `/api/v1/query`, `/api/v1/impact`,
`/api/v1/impact-intersection` (repeatable `item=<id>=<symbol>[,<symbol>]`,
optional `depth`), `/api/v1/precedent` (`target=<symbol|path>`, optional
`limit`), `/api/v1/branch-overlap` (repeatable `ref=<git-ref>`, optional `base`
and `merge`), `/api/v1/node`,
`/api/v1/search`, `/api/v1/code-refs` (repeatable `ref=<span>`, at most 24
resolved), `/api/v1/architecture`, `/api/v1/gaps`, `/api/v1/files`,
`/api/v1/coverage`, `/api/v1/status` (the index-health projection the app
header reads — see below), `/api/v1/config`, and `/api/v1/extensions`.

`GET /api/v1/extensions` states which extensions the running server composes
(CR-213, FR-UI-51): `{"extensions": [...]}`, one entry per **active** extension
— its `id` and the `ui` manifest it contributes (`nav`, `views`,
`config_panels`; `null` for an extension with no UI). The single-page app reads
it once on boot to decide what to mount (S-642), so an extension that goes
inactive mid-session leaves the shell at the next load. The default build
composes no extension and answers `{"extensions": []}`. Like every read route it is `GET`-only, loopback-only and
same-origin, and it reads the server's composition, not the index — it records
no telemetry.

`GET /api/v1/graph` returns the bounded graph-elements read-model
(`nodes`/`edges` plus `total_*`/`elided_*` counts and `warnings`) that backs the
Graph canvas. It accepts `?seed=<id>` (a seed symbol saved before symbols
carried their repository coordinate still anchors: the snapshot echoes the
canonical seed and a note in `warnings`) and `?cap=<N>` plus three
progressive-disclosure parameters: `?layers=code,docs,artifacts` and
`?edge_types=calls,imports,…` **re-budget** the view server-side (the filter is
applied to the in-scope set *before* the degree-rank+cap, so deselecting a layer
backfills the freed budget with previously-elided nodes of the remaining scope
rather than merely shrinking the graph), and `?granularity=module|file|symbol`
selects the **rollup tier** (module-rollup / file-rollup clusters, or the
default symbol view). When the graph exceeds the cap it returns at most `cap`
nodes and reports the remainder honestly in `elided_nodes` rather than
truncating silently; the elided counts re-base on whatever filter/tier snapshot
is in effect. Unknown `layers`/`edge_types`/`granularity` tokens are dropped
(never a `4xx`); an unknown seed yields empty arrays plus a `warnings` entry —
never a fabricated node. All parameters are additive — a request that omits them
gets the same response as before.

`GET /api/v1/impact?seed=<symbol>` returns the **Decisions & docs** read-model as
JSON — the symbol's traced requirements, ADRs, and stories plus its doc-reference
table, read from `Engine::impact()`. The SPA's Decisions panel renders it
client-side. An omitted or empty `seed` returns the opening-prompt state; an
unknown or untraced seed returns an honest empty result at `200` (never a
`4xx`/`5xx`).

`GET /api/v1/query` backs the Graph view's **query box**. It composes the
read-only `Engine` query surface (search + callers/callees/impact) into a single
ranked JSON result set so you can find nodes by structure rather than scrolling
the canvas. It takes a `q=<term>` search fragment optionally refined by `kind=`,
`layer=` (code/docs/artifacts), and `file=` post-filters, or a relational form
`verb=callers-of|callees-of|impact-of` + `target=<symbol>`. Each hit carries its
id, name, kind, layer, and `file:line`, plus a `total` "showing N of M" count so
a capped result is honest rather than silently truncated. An unrecognised
`kind`/`layer`, a no-results filter, and a leaf relational query all return `200`
with a guidance note — never a `4xx`/`5xx` and never a fabricated hit.

### Hidden widgets

Six widgets are hidden from the web UI **only** (S-612, CR-208); one of them is a
whole view. Their data is still
served, unchanged, by every other surface, so nothing is lost — each answer is one
command or request away:

| Hidden widget | Was on | Its data is still available from |
|---|---|---|
| Coverage by relation arm | Workspace Dashboard; the Workspace tab's **Cross-service coverage** panel | `GET /api/v1/workspace/status` (`coverage`); `logos workspace status`; MCP `workspace_status` |
| Cross-service impact | The **Workspace** tab, which now has two tabs: Service map and Cross-service coverage | `GET /api/v1/workspace/impact`; `logos xservice impact`; MCP `xservice_impact` |
| Non-gated tier callout | Health | **Files & Risk** in the sidebar; `logos hotspots` |
| Cycles band and cycle list | Architecture (inside the view, which is itself hidden — see the next row) | `GET /api/v1/architecture`; `logos dsm`; MCP `dsm` |
| Dependency matrix — the whole **Architecture** view | The sidebar; `/architecture` and the retired `/dsm` bookmark land on **Health** | `GET /api/v1/architecture`; `logos dsm`; MCP `dsm` |
| Declared contracts | Workspace → **Service map** (the map's declared layer and its legend stay) | `GET /api/v1/workspace/status` (`coverage.declared_contracts`, `coverage.bound_external`); `logos workspace status`; `logos xservice route-providers` (`declared_contracts`, `bound_external`); MCP `workspace_status`; MCP `xservice_route_providers` |

Each widget is hidden through one register in the SPA source
(`web/ui/src/views/hiddenWidgets.ts`), which records the same reason and surfaces;
deleting a widget's entry there brings it back — for the Architecture view, its
sidebar entry and its route together.

### Interacting with the dashboard

The dashboard is interactive, with every interaction handled client-side in the
React app over the same-origin `/api/v1/*` read-models — sorting, paging,
focusing, and filtering mutate no store and contact no external origin:

- **Navigation feedback.** A slow navigation or any in-flight data fetch (e.g.
  Graph search) arms an orange top progress bar plus a pending state on the
  clicked sidebar item after a short delay, clearing when the view or fetch
  settles. Instant/cached views (e.g. Overview) never flash it.
- **Interactive graph canvas (`/graph`).** The Graph & Decisions view is a
  pan/zoom/drag force-directed canvas over `GET /api/v1/graph`, rendering the full
  **code + docs + artifacts** graph (the doc and config/artifact layers surface
  alongside code, not code alone). The canvas is **square** (`aspect-ratio: 1 / 1`,
  bounded max-height) and reflows on resize. Nodes are **colored by layer** — code
  blue, docs green, artifacts amber — and edges are **colored by relationship
  type**, each kind carrying a distinct `(color, line-style)` pair with a
  `forbidden_dependency` edge the heaviest red. A collapsible **on-canvas legend**
  names every node-layer row (including the *Selected* ring), all edge-type
  swatches, and the line-width/dash convention, so color is never the only signal.
  It opens **sparser than the raw graph** — a loosened force layout (more
  repulsion, longer edges) and a zoom-driven visible-element budget (250 at the
  home zoom) keep the first view legible — with level-of-detail labels that appear
  on zoom and an honest "N more not shown" notice naming the remainder when the
  graph is capped.
  - **Progressive disclosure (zoom = density).** Zoom is real level-of-detail, not
    just visual scale, but it drives **only the density budget** — never the
    semantic tier (that is the explicit *Tier* control below). **Zooming out**
    shrinks the visible-element budget toward the **highest-degree hub nodes** (the
    structure's "highways") down to a **floor**: the home view is the detail floor,
    so zooming out past it is **camera-only** (it frames the same graph rather than
    reshaping it), and the `−` button reports a brief "maximum zoom-out reached"
    notice at the limit instead of silently no-op-ing. **Zooming in** re-fetches a
    larger budget that admits the next tier of lower-degree nodes — the server's
    degree ranking *is* the detail ladder. Re-fetches are debounced (one fetch per
    settled gesture), and the "N more not shown" notice re-bases on each budget. A
    manual `?cap=<N>` pins the budget (zoom then only pans/labels); **Expand
    neighbours** still adds local depth on top (zoom = breadth, Expand = depth).
  - **Locked selection.** Click a node to **lock** it: it gains a red selection
    ring (the layer color survives underneath, never a fill swap), its directly
    connected neighbours stay bright, and everything else **dims**. Click the same
    node to unlock (ring clears, full opacity returns); click a different node to
    move the lock. With nothing locked, hovering a node shows only its tooltip and
    **dims nothing** — emphasis is a deliberate selection, not an accident of the
    cursor.
  - **Expand neighbours.** The *Expand neighbours* button is disabled until a node
    is locked; clicking it **additively** admits the locked node's neighbours —
    newly admitted nodes are added to the view, the "N more not shown" count is
    drawn down by exactly what was admitted, and a status line reports the gain.
    When the node has no further neighbours to admit it is flagged with an explicit
    "already fully expanded" message rather than appearing dead.
  - **Query box.** A search field over `GET /api/v1/query` finds nodes by structure:
    type a name fragment (optionally narrowed by **kind**, **layer**, or **file**
    filters) or pick a **relational verb** (*callers of / callees of / impact of*)
    plus a target symbol, then Query for a ranked hit list. Results render as a
    **paginated table** — `#` (rank) / `Name` / `Path` columns, sortable, **15 rows
    per page** — and each name cell is a select-to-lock button. The top hit is
    auto-centered and locked on the canvas (its identity surfaces in the Decisions
    panel), and clicking any hit locks that node — fetching its neighbourhood if it
    was outside the current view. The submit button is baseline-aligned with the
    other query fields. An empty or no-match query renders an honest guidance note,
    never an error.
  - **Semantic tier (Symbols / Files / Modules).** A **Tier** control selects the
    `?granularity=` rollup view **independently of zoom**: **Symbols** is the
    default per-symbol view, **Files** renders file-rollup clusters, and **Modules**
    renders module-rollup clusters (a deliberate Google-Maps-style altitude pick,
    not a side effect of the camera). Switching tiers swaps the node id-space and
    cleanly clears the prior selection/scope (no stale ring or orphaned lock),
    returns to the home-zoom budget, names the active tier in the legend, and
    re-bases the "N more not shown" notice on the new tier. Cluster tiers come from
    the existing rollup views and exclude documentation files/modules; reading them
    is metric-neutral. A chosen Files/Modules view **survives** a lock/expand/
    filter rebuild — only the Tier control changes it.
  - **Intent / governing docs toggle.** Off by default, this toggle surfaces the
    **documentation→code intent layer**: when on, the canvas fetch admits the
    governing-doc nodes adjacent to the visible code (up to a bounded overlay
    budget) and shows their `DocReference`/`TracesTo` edges in a distinct purple
    style, grouped under their own *Intent / governing docs* legend heading so the
    rationale layer reads apart from structure. With it **off** the view is
    byte-identical to a plain graph fetch (the overlay adds no nodes); turning it on
    also reveals the doc layer and intent edge types so a prior deselection can't
    hide them, and Reset clears it.
  - **Other controls.** **Zoom** `+`/`−` buttons step the zoom within bounds
    (scroll/pinch still work) and drive the density budget above;
    **layer** and **edge-type** filters **re-budget** the view (a deselected layer
    is re-fetched server-side so the freed budget backfills with the remaining
    scope, keeping the graph full rather than just smaller — not a client-side
    hide); **depth** scopes the focused neighbourhood; and **Reset to whole graph**
    returns the canvas to the full unfiltered view at the home-zoom budget and
    clears any lock (and the intent overlay). Click-to-focus also arrives from the Decisions
    panel.
  - **Decisions panel.** Selecting a node populates the side panel — built
    client-side from `GET /api/v1/impact?seed=<symbol>` — led by a **node identity
    header** (the node's name, a kind badge, a layer badge, and its `file:line`)
    above its traced requirements, ADRs, and stories and its doc-reference table. A
    node with nothing traced to it still shows the identity header followed by an
    honest empty state; Reset clears the panel back to the opening prompt. Each
    governing ADR/requirement/story/doc reference carries a **↪ graph** pivot
    action: it re-seeds the canvas on that documentation node (turning the intent
    overlay on so its `DocReference`/`TracesTo` edges come along), letting you jump
    from "this symbol's governing docs" straight into that doc's graph
    neighbourhood. An edgeless doc node pivots to its own honest small
    neighbourhood, never an error.
  - **Accessible graph tables (1-hop).** Beneath the canvas, the WCAG-conformant
    **Graph nodes** and **Graph edges** tables (captioned, keyboard- and
    screen-reader-navigable) are the no-canvas path through the same data. With
    **nothing locked** they list the full currently-loaded set; **lock a node** and
    they switch to that node's **1-hop neighbourhood** — the selected node plus its
    directly connected nodes and the edges incident to it — with the card heading
    naming the focus (e.g. *1-hop neighbourhood of `parse`*). A visible **separator**
    divides the nodes table from the edges table. Every row traces to a real
    graph-element field — none fabricated ([FR-UI-08](../specs/requirements/FR-UI-08.md),
    [NFR-RA-05](../specs/requirements/NFR-RA-05.md)).
- **Interactive tables.** Every data table (Rule findings, Health,
  Files & Risk, Coverage, Graph Decisions & docs, and the
  Graph nodes/edges accessible tables) has sortable column headers — click to sort
  the **full dataset** first, then the page is sliced — and consistent pagination.
  A single shared **`DEFAULT_TABLE_PAGE_SIZE = 20`** governs every table so no view
  renders more than 20 rows per page (the Graph **query results** table is the one
  deliberate exception at **15**, [FR-UI-11](../specs/requirements/FR-UI-11.md),
  [FR-UI-14](../specs/requirements/FR-UI-14.md)). Sort/page state is carried in the
  client route's URL (bookmarkable) and namespaced per table (`<tid>_sort` / `_dir`
  / `_page`), so two tables on one view (e.g. the Graph nodes and edges tables) never collide. Numeric columns
  right-align with their headers. The React view re-renders just the table in place.
- **Editing config (`/config`) — the one mutating view.** The Config view is a
  React hybrid editor over the two policy files: typed form fields for the
  scalar/list keys (`config.toml` admission lists + the `rules.toml`
  `[constraints]` / `[metric_thresholds]` scalars) plus an authoritative raw-TOML
  pane for the repeated-table sections. The raw pane is the single source of truth
  POSTed verbatim; a typed field patches only its one key into that text. **Save**
  (`POST /config/save`) validates the whole document and atomically writes it — an
  invalid edit returns an inline `422` naming the offending key and leaves the file
  byte-identical (no partial write). Save is deliberately **not** Apply: it changes
  the file on disk but does not touch the graph or re-run the gate. **Apply &
  reindex** (`POST /config/apply`) is the separate, explicit step that reconciles
  the graph (config) or re-evaluates governance (rules), with an in-flight
  affordance and an honest outcome panel. A `rules.toml` save requires an explicit
  confirmation checkbox before any write and stamps a provenance comment into the
  saved file. Both POSTs ride the same-origin + `x-logos-intent` guard described
  above; the SPA submits them via `fetch` carrying the intent token.
  Beside Apply, a **Check graph consistency** control runs the deep `verify`
  (`POST /api/v1/verify`, the same-origin twin of `logos verify`): it shows an
  explicit loading state while the shadow reindex runs (seconds-to-minutes), then
  renders a **`CONSISTENT`** verdict badge on a healthy graph or a **`DRIFT`**
  callout with the live-vs-reindex deltas and a capped sample of leaked/orphaned
  symbols. On a read fault it shows an honest error panel — never a fabricated
  `CONSISTENT` ([FR-UI-25](../specs/requirements/FR-UI-25.md),
  [NFR-RA-05](../specs/requirements/NFR-RA-05.md)).

Run it alongside the MCP server in one process with `logos serve --mcp --ui`.

### The Statistics tab

The **Statistics** tab (`/statistics`, directly above **Config** in Group C)
gives a first-class, in-app view of Logos's value in your development process —
the same local telemetry the `logos stats` CLI reports, now visualized
([CR-058](../requests/CR-058-durable-worktree-telemetry-and-statistics-tab.md),
[FR-UI-27](../specs/requirements/FR-UI-27.md)). It reads the enriched read-model
over `GET /api/v1/statistics[?window=<days>]` — a thin, read-only pass-through of
`Engine::stats(window)` — and renders five widgets, each stating what it shows
and why it matters (CR-203):

1. An **Estimated value** widget leading with the reads/tokens-saved figures (the
   dogfood metric), clearly labeled as *estimates*, not measured truth.
2. A **daily-activity line** — calls per UTC day over the window.
3. A **top-tools / by-surface** ranking bar. Self-referential reads — the tab's
   own `stats` request and the shell's `status` readout — are excluded per event,
   so opening this tab never inflates its own numbers; a graph query issued
   through the dashboard is counted like any other.
4. A **dev-vs-`main` split** — usage attributed by `origin` (a worktree's branch
   name, or `"main"`), charted as-is and **never normalized to total calls**
   (rolled-up days carry no `origin`, so the split can legitimately sum to less
   than the total — the tab does not hide that gap).
5. A **Tool attribution by class** widget — the tool × origin cross-tab
   (`calls_by_tool_origin`) as one table with a **Class** column (`navigation`,
   `quality-gate`, …), ordered by class and then by calls, so *"which navigation
   came from dev panes?"* is readable in place. **Answered** explains itself on
   hover and focus. Each row states what the calls **answered**: `N of M answered` for
   a tool whose outcomes are recorded (today `callers`, `impact`, `precedent`
   and `affected`), or **none recorded** when no call in that cell was
   classified — never a percentage. The coverage limits render in the widget's
   explanation, verbatim from the read-model's `attribution_coverage.notes`: the
   cross-tab covers raw events only, a window past raw retention is truncated,
   and events written before the origin stamp existed came from every surface of
   the time with their dev/`main` split unknown.

A **7 / 30 / 90-day window selector** (default 7) re-queries the endpoint and
re-renders every surface. Each chart is paired with an accessible data-table twin
(WCAG 2.1 AA). Viewing the tab **writes nothing** — it is a read-only GET, no
intent token, same-origin, self-only CSP preserved.

**Honest empty state.** Against a repository with no telemetry history, the tab
renders an explicit "No telemetry recorded yet" awaiting-data state in the
Estimated value widget — never fabricated zeros — naming `logos stats` to check
what has been recorded, and the sidebar item is **muted** (dimmed, with a tooltip) but
still clickable. Because telemetry is repo-global and durable (see
[commands.md § `stats`](commands.md)), the figures include usage recorded from
linked worktrees that have since been removed.

The Statistics view is part of the dashboard, so it is present in the default
build (`ui` is the shipped default, CR-078/ADR-60). Only the slim
`--no-default-features` build has no web surface, so the tab is simply absent
there.
