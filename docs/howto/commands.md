# Command Reference

All 27 subcommands of the `logos` binary. Every command accepts the global
flags `--project <PATH>`, `--json`, and `--quiet`; see
[usage.md](usage.md#global-flags-and-exit-codes) for those and for the
`0/1/2/3` exit-code contract.

| Command | Status | One-liner |
|---|---|---|
| [`init`](#init) | ✅ | Initialise `.logos/`, policy files, MCP host, and git hooks |
| [`index`](#index) | ✅ | Build or rebuild the full code-graph index |
| [`sync`](#sync) | ✅ | Incrementally sync changed files into the index |
| [`status`](#status) | ✅ | Index and sync health |
| [`search`](#search) | ✅ | Full-text search over the code graph |
| [`query`](#query) | ✅ | Façade over search/callers/callees |
| [`context`](#context) | ✅ | Deterministic context bundle for a task |
| [`explore`](#explore) | ✅ | Neighbourhood exploration, grouped by file |
| [`node`](#node) | ✅ | Full info for one symbol |
| [`callers`](#callers) | ✅ | Direct callers of a symbol |
| [`callees`](#callees) | ✅ | Direct callees of a symbol |
| [`impact`](#impact) | ✅ | Transitive impact, both directions |
| [`affected`](#affected) | ✅ | Files affected by a changed set |
| [`implements`](#implements) | ✅ | Code that implements a doc node or requirement |
| [`referencing-docs`](#referencing-docs) | ✅ | Doc sections that reference a symbol |
| [`stats`](#stats) | ✅ | Usage/performance statistics |
| [`languages`](#languages) | ✅ | Registered language grammars |
| [`serve`](#serve) | ✅ | MCP server over stdio and/or the localhost web UI (`--ui`, requires a `--features ui` build) |
| [`scan`](#scan) | ✅ | Full architecture-quality scan |
| [`check`](#check) | ✅ | Architecture-rules compliance check |
| [`gate`](#gate) | ✅ | CI quality gate on the signal |
| [`evolution`](#evolution) | ✅ | Signal evolution over snapshots |
| [`dsm`](#dsm) | ✅ | Dependency-structure-matrix clusters |
| [`doc-gaps`](#doc-gaps) | ✅ | Undocumented exported symbols |
| [`hotspots`](#hotspots) | ✅ | Churn × complexity ranking — the non-gated temporal tier |
| [`coverage ingest`](#coverage-ingest-report---format-fmt) | ✅ | Ingest an LCOV/Cobertura report into the evidence store |
| [`coverage status`](#coverage-status) | ✅ | Per-file coverage freshness + the overall fraction |
| [`wiki write`](#wiki-write) | ✅ | Upsert a generated wiki page with provenance + anchors |
| [`wiki read`](#wiki-read) | ✅ | Read a page with provenance + per-anchor freshness |
| [`wiki search`](#wiki-search) | ✅ | FTS5 search over pages (or `--list` to enumerate) |
| [`wiki status`](#wiki-status) | ✅ | Store summary + the regeneration work-list |
| [`wiki generate`](#wiki-generate) | ✅ | Format the work-list into an offline generation queue (prompt block / `--json`, CLI-only) |
| [`wiki materialize`](#wiki-materialize) | ✅ | Deterministically present the authored `docs/specs/**` + `docs/howto/**` as wiki pages (SRS mode); no LLM/network |
| [`wiki delete`](#wiki-delete) | ✅ | Explicitly delete a page by slug (CLI-only) |
| [`wiki skill --emit`](#wiki-skill---emit-dir---force) | ✅ | Materialize the embedded wiki-generation skill (CLI-only) |
| [`wiki hook --emit`](#wiki-hook---emit---force) | ✅ | Install the Claude Code SessionEnd quality-report hook (CLI-only) |

---

## Index & freshness

### `init [-i] [--hooks] [--workspace] [--yes] [--exclude <GLOB>]`

```bash
logos init                     # create .logos/ and the canonical store (bare)
logos init -i                  # interactive: policy files + .mcp.json + CLAUDE.md
logos init --hooks             # install managed git hooks only
logos init -i --hooks          # interactive setup plus git hooks
logos init --workspace         # federate sibling repos into a workspace (prompts per member)
logos init --workspace --yes   # non-interactive: approve every discovered member
logos init --workspace --exclude 'vendor-*'  # skip members whose name matches the glob
```

Bootstraps the project. All forms are **idempotent and non-clobbering** —
re-running on an already-initialised project applies any pending migrations
and leaves existing content untouched.

**Bare (`logos init`)** — creates `.logos/` and `logos.db`; the same implicit
bootstrap that `logos index` performs.

**Interactive (`-i`)** — additionally writes starter policy templates
(`config.toml`, `rules.toml`), a managed `.logos/.gitignore` block, a
`logos` entry in the project's `.mcp.json`, a managed usage block in
`CLAUDE.md`, the embedded `logos-wiki` generation skill, and — default-on —
the **SessionEnd quality-report** hook
([FR-IN-07](../specs/requirements/FR-IN-07.md)), merged into the shared
`.claude/settings.json`, which prints a signal/baseline/violations readout at
session end. The merge is non-clobbering and the binary stays offline (no LLM
call, no outbound connection). Each step reports its action (`Created` /
`Updated` / `Unchanged` / `Skipped`). On non-TTY (CI, piped input): safe
defaults, no prompts.
Wiki prose generation itself now runs in-process (`ui` builds only) when the
Wiki tab is opened — there is no more headless `claude -p` autogen hook.
Non-`ui` builds regenerate manually via the materialized `logos-wiki` skill
(`wiki skill --emit`, below).

**Hooks (`--hooks`)** — installs managed git hook scripts under
`.logos/hooks/` and sets `core.hooksPath = .logos/hooks` in the project's git
config. Never overwrites unmanaged hook files. Can be combined with `-i`. Two
flavours are installed:

- **Freshness hooks** — `post-commit`, `post-checkout`, and `post-merge` each
  run `logos sync` on the changed file set after their git event, keeping the
  graph fresh. They are best-effort: they bail out silently when `logos` is
  absent and always exit 0, so they never block or fail a git operation.
- **Enforcing gate** — a `pre-push` hook runs [`logos check`](#check) and
  **propagates its exit code**: a `severity='error'` violation
  (rule / structural / admission / dead-code) makes `git push` fail with exit
  1 and names the offending violation, turning "code about to leave the
  machine" into an enforced non-regression checkpoint. Unlike the freshness
  hooks it is *not* exit-0-swallowed. It still bails **open** (exit 0) when the
  `logos` binary is genuinely absent — never a false block — and
  `git push --no-verify` bypasses it (git skips the hook natively).

After running `logos init -i`, restart your agent — the `logos:*` MCP tools
appear automatically.

Together these hooks realize the local legs of the **freshen / enforce / report
/ bless** loop: the freshness hooks *freshen*, the `pre-push` gate *enforces*,
and the SessionEnd quality-report hook *reports*. The CI leg (and the
release-only *bless* with `logos gate --save`) is documented in
[CI integration](ci-integration.md).

**Workspace (`--workspace`)** — turns a parent folder of sibling repositories
into a **Logos workspace** ([FR-WS-02](../specs/requirements/FR-WS-02.md)). Run
it from the directory that contains your service repos. It:

1. **Discovers members** — scans immediate child directories for distinct git
   roots (a repo already carrying `.logos/logos.db` also counts).
2. **Gates approval** — prompts per candidate on stderr (y/n). `--yes` approves
   all discovered members non-interactively; `--exclude <GLOB>` drops members
   whose workspace-relative name matches the glob (repeatable). The exclude
   applies only to newly-proposed candidates — members already in an existing
   manifest are never re-prompted or re-excluded.
3. **Initialises each member** — runs the ordinary per-member `init`
   (write-if-absent, **never** clobbering an existing member config).
4. **Writes the manifest** — a `logos.workspace.toml` at the parent listing the
   approved members, plus your hand-written `default`/`autodiscover`/`[[links]]`
   preserved verbatim on a re-run.
5. **Injects one MCP entry** — a single `logos-workspace` server key in the
   parent `.mcp.json` (distinct from the per-repo `logos` key so a member's own
   entry is never shadowed).

Indexing is **hybrid**: the command returns immediately while each approved
member warms its index in a detached background process; a member that has not
finished warming still indexes correctly on first real use (lazy
`ensure_indexed`). A member that fails to initialise is reported **degraded**
without aborting the others. stdout stays machine-clean (the approval prompt is
on stderr), so `logos init --workspace --yes` is safe to script. Re-running is
incremental — `manifest`/`mcp` actions report `unchanged` and no duplicate MCP
entry is written. When no sibling repos are found, nothing is written and the
command exits 0.

> **Federation is an in-memory overlay, never a graph union.** Each member keeps
> its own `.logos/logos.db` and its single-root behaviour unchanged; the
> workspace is assembled on demand and never persisted across a database
> boundary ([ADR-52](../specs/architecture/decisions/ADR-52.md)). With **no**
> `logos.workspace.toml` present, every command behaves exactly as a single-repo
> checkout — federation is entirely dormant.

### `index`

```bash
logos index
```

Full pipeline run: discover → extract → resolve → detect frameworks →
annotate. Creates `.logos/` (and `logos.db`) if absent. Idempotent — safe to
re-run any time; the result depends only on the source tree and config.
Reports per-phase counts (files indexed, nodes/edges created, resolution
coverage, routes found, dead/duplicate annotations).

With `--json`, the result also carries a `phases` object with the per-phase
wall-clock duration in milliseconds — `discover`, `load`, `extract`, `persist`,
`resolve`, `framework`, `dispatch`, `annotate` — derived from the same `tracing`
seam as the logs (the durations sum to ≤ `total_ms`, never double-counted). Use
it to see where a cold index spends its time before optimizing.

### `sync [PATHS]...`

```bash
logos sync                 # all changed files
logos sync src/auth.rs     # specific paths
```

Incremental fold-in of changes — much faster than a full `index` on large
trees. Deleted files' symbols are captured before removal so inbound
references degrade gracefully rather than dangle.

### `status`

```bash
logos status --json
```

Index health: file/node/edge counts, store size, unresolved-reference ledger,
resolution coverage, last index/sync timestamps, a persisted monotonic
`graph_revision` (a counter bumped once per `index`/`sync` that actually
changes the graph — a no-op `sync` leaves it untouched; consumers can compare
it across processes to detect a stale cache), and the freshness posture
(navigation serves the latest committed snapshot; it never reconciles per
call).

## Navigation

### `search`

```bash
logos search <QUERY> [--kind <KIND>] [--limit <N>]    # default limit 20
```

FTS5 full-text search over symbol names. `--kind` filters by node kind
(`function`, `struct`, `route`, …) — including the documentation kinds
`doc_file`, `doc_section`, `requirement`, `adr`, and `story` once markdown is
indexed (see [Documentation graph](#documentation-graph)), and the config &
artifact kinds once config indexing is on (see [Configuration & artifact
graph](configuration.md#configuration--artifact-graph--indexing-config-and-infra-files)):
`config_file`, `config_section`, `dockerfile_stage`, `make_target`,
`shell_function`, `proto_message`, `proto_service`, `gql_type`, `tf_block`,
`sql_object`, `api_path`, and `api_operation`. Near-miss queries return
suggestions.

### `query`

```bash
logos query <SYMBOL> [--kind <KIND>] [--limit <N>]
logos query <SYMBOL> --callers
logos query <SYMBOL> --callees
```

One entry point for the three most common questions. `--callers` and
`--callees` conflict by design (exit 2 if both given).

### `context`

```bash
logos context <TASK>... [--max-nodes <N>] [--no-code]   # default 25 nodes
```

The token-saving tool: given a free-text task description, assembles a
deterministic bundle of the most relevant symbols with their declarations —
one call replacing many file reads. `--no-code` returns the structural map
only. Anchoring spans code **and** documentation: a multi-word prose task whose
terms match a requirement or doc section anchors there and expands along
doc→code edges to the implementing symbols, so prose phrasing still yields a
non-empty bundle (it is kind-balanced so documentation matches cannot crowd out
code symbols).

### `explore`

```bash
logos explore <QUERY> [--max-files <N>]                 # default 10 files
```

Anchors on the best-matching symbol and walks its neighbourhood, returning
source grouped by file — the "show me around this area" tool.

### `node`

```bash
logos node <SYMBOL> [--code]
```

Everything about one symbol: kind, location, export status, annotations
(dead/duplicate), complexity, immediate edges; `--code` includes the
declaration source. For a code symbol referenced from documentation, the
response also lists the referencing doc sections; for a documentation node, its
doc→code edges.

### `callers` / `callees`

```bash
logos callers <SYMBOL> [--limit <N>]                    # default 50
logos callees <SYMBOL> [--limit <N>]
```

Direct call-graph neighbours, one hop each way.

### `impact`

```bash
logos impact <SYMBOL> [--depth <N>]                     # default depth 3
```

Transitive closure in both directions, labeled: *upstream* (what breaks if
this changes) and *downstream* (what this depends on).

### `affected`

```bash
logos affected <FILES>... [--tests-only]
```

Whole reverse-transitive closure at file granularity: given changed files,
which files are affected, ordered nearest-first. Unknown paths are reported
in an `unknown` list — never an error. `--tests-only` narrows the closure to
test-convention paths (the CI use case). Leading `./` is normalised.

## Documentation graph

Markdown documentation is indexed as first-class graph nodes (`DocFile`,
`DocSection`, and — on swe-skills repos — typed `Requirement`/`Adr`/`Story`
nodes) with doc→doc and doc→code edges. Indexing is on by default and
configured by the `[documentation]` table in
[configuration.md](configuration.md#documentation--indexing-markdown). Doc→code
links obey the same **never-fabricate** rule as code: an ambiguous mention
resolves to *no* edge and stays in the unresolved-reference ledger. Documentation
is **metric-neutral** — adding or removing it never moves the quality signal
(see [metrics.md](metrics.md)).

### `implements`

```bash
logos implements <DOC>
```

Lists the code symbols a documentation node points at over doc→code edges —
"which code implements this requirement / heading". `<DOC>` is a documentation
node, requirement, ADR, or heading, given either as a canonical symbol or a
human-facing name (e.g. `FR-DG-06`, or a heading title). The inverse of
[`referencing-docs`](#referencing-docs).

### `referencing-docs`

```bash
logos referencing-docs <SYMBOL>      # alias: referencing_docs
```

Lists the documentation sections that reference a code symbol — the docs a
change to that symbol may oblige updating. The inverse of
[`implements`](#implements).

## Observability

### `stats`

```bash
logos stats [--window <DAYS>]                           # default 7
```

Aggregated local telemetry: calls per tool split by surface (`cli`/`mcp`/`watcher`),
ok-rates, latency p50/p95/p99, and reads/tokens-saved estimates. `--json` also
carries `activity_by_day` (a per-UTC-day activity series over the window,
oldest-first) and `calls_by_origin` (a per-`origin` usage breakdown, where
`origin` is a worktree's branch name or `"main"`). Reads only `telemetry.db` —
works without an index. **Web-dashboard activity (`surface="web"`) is excluded
from every figure** — totals, per-tool, daily series, origin split, latency, and
the estimate — because serving the dashboard emits its own telemetry, so counting
it would only measure viewing, not tool value.

**Telemetry is repo-global and durable across worktrees.** The store lives at
the **primary** repository's `.logos/telemetry.db`, resolved via
`git --git-common-dir`. A command run inside a linked git worktree writes
*through* to that primary store (it never creates a `telemetry.db` inside the
worktree), so usage recorded during a dev-session worktree survives
`git worktree remove` — and `logos stats` from any worktree reports
repository-wide usage, not an empty per-worktree slice. From the primary
checkout, behavior is unchanged. Independent clones (a separate
`--git-common-dir`) remain independent stores by design. Legacy rows written
before the `origin` column existed read as `"main"`. Note that
`sum(calls_by_origin)` can be *less* than `calls_total` over a window old enough
to reach aged-out daily rollups (rolled-up days carry no `origin`) — an honest
gap, never silently reconciled.

### `languages`

```bash
logos languages --json
```

The registered grammar table: name, extensions (and `filenames` for
basename-claimed formats like `Dockerfile`/`Makefile`), module separator,
capabilities, tree-sitter ABI version, and an `artifact` flag — plus any
grammars skipped for ABI mismatch (`skipped` should be empty). A full `lang-all`
build lists 24 plugins: the twelve code languages (thirteen grammar rows —
TypeScript and TSX/JSX register separately), `markdown`, and the ten
`artifact: true` config/infra grammars (yaml, json, toml, dockerfile, makefile,
shell, protobuf, graphql, terraform, sql).

## Serving

### `serve`

```bash
logos serve --mcp [--project <PATH>]                 # MCP server over stdio (AI agents)
logos serve --ui [--port <N>] [--project <PATH>]     # localhost web dashboard (default port 4983)
logos serve --mcp --ui [--port <N>]                  # both surfaces in one process
```

At least one of `--mcp` / `--ui` is required. `--mcp` starts the MCP server
over stdio (see [usage.md](usage.md#setting-up-the-mcp-server-ai-agents) for
host setup, the 20-tool surface, and the stdout-purity / clean-teardown
guarantees). `--ui` starts the localhost web dashboard (see
[usage.md](usage.md#the-web-ui-dashboard)) — **available only in a build
compiled with `--features ui`**; the default binary has no web surface and no
networking crate. The dashboard is a single embedded React SPA served at `/`,
client-side routed over a same-origin `/api/v1/*` JSON read-model API
([ADR-43](../specs/architecture/decisions/ADR-43.md)); the whole app is built at
build time and embedded in the binary, so a page load fetches nothing from the
network, and it binds `127.0.0.1` only with a self-only CSP on every response. It
is read-only except the intent-guarded mutating routes — the config-write/apply
routes (`/config/save`, `/config/apply`, `/config/secret`) and the Chat routes
(`/chat`, `/chat/clear`); every other non-GET request is answered `405`. Combined
`--mcp --ui` runs both on one engine and one watcher: stdout stays JSON-RPC-clean
for the MCP host while the web surface logs to stderr.

A **debounced filesystem watcher** runs alongside the engine: file changes
are coalesced over a 300 ms window (configurable via `[watcher] debounce_ms`
in [configuration.md](configuration.md)) and folded into a single `sync`
batch. Navigation and governance responses always reflect the current on-disk
state; the reconcile backstop in every quality command is the correctness
safety net regardless.

---

## Quality & Governance

Every quality command follows the **reconcile-then-score** contract: changed
files are synced first, then the analysis runs against the freshened graph.
The `freshness` field in every response confirms what was reconciled (e.g.
`"reconciled 3 files · HEAD abc1234 · 0 unresolved refs"`). Pass
`--no-reconcile` to skip the sync and score the last committed state — useful
in CI after a pre-built index step.

### `scan`

```bash
logos scan [--no-reconcile]
```

Full code-quality scan: reconcile the index, compute the ten quality
metrics (see [metrics.md](metrics.md)), persist a timestamped snapshot into
`metric_snapshots`, and report the 0–10000 signal with a per-metric breakdown.
The `--json` output also carries a `worst_offenders` field — a per-dimension,
deterministically ordered, top-10 list of the specific functions/containers
dragging each score (report-only; it never gates). Constraints declared in
`rules.toml` are not evaluated here — use `check` for that.

### `check [--rules <FILE>]`

```bash
logos check                              # use .logos/rules.toml
logos check --rules path/to/rules.toml
```

Architecture-rules compliance check: reconciles the index, then evaluates
every `[constraints]`, `[[layers]]`, and `[[boundaries]]` declaration in
`rules.toml` against the live graph. This includes the four structural budgets
(`max_nesting_depth`, `max_brain_methods`, `max_clone_ratio`,
`no_god_containers` — see
[configuration.md](configuration.md#metric_thresholds--tuning-the-structural-dimensions)),
the hard-gate counterparts of the structural metric dimensions. Violations with
severity `error` cause exit 1; warnings are reported but do not fail. The
structured report names each violated rule, the offending node or pair, and the
contract it breaks, in a deterministic order.

`check` also always folds in the same `doctor` verdict (below) as two
additional error-severity findings, independent of any `rules.toml` contract —
a `graph-structural-integrity` rule id for the one-node-per-`symbol_id` and
orphan-row invariant, and a distinct `graph-admission-drift` rule id for the
admission tripwire. Neither is persisted to the `violations` table (they are
live invariant checks, not authored rules), but both fail `check` (exit 1) the
same way an authored `error`-severity rule would.

### `gate [--save] [--threshold <N>] [--label <L>]`

```bash
logos gate                      # compare to last saved baseline; exit 1 on regression
logos gate --save               # score and persist a new baseline
logos gate --threshold 8000     # also fail if signal drops below 8000
logos gate --save --label "v1.2.0"
```

The CI quality gate. Without `--save`, compares the current signal to the
last saved baseline plus an epsilon tolerance (1 point on the 0–10000 scale).
Exit 1 if the signal regressed past epsilon or below `--threshold`. `--save`
persists the current scored snapshot as the new baseline — use on release
branches. If neither side has a baseline yet (n/a graph), the gate is
informational (exit 0) unless `--threshold` is set.

If the baseline was scored under a different `metric_version` or a different
structural-threshold set (`thresholds_hash`), the two signals aren't comparable;
the gate **auto-re-baselines once**, reports `baseline reset: metric semantics
changed` or `baseline reset: metric thresholds changed`, and passes
informationally. The next gate compares normally. This is what lets you re-tune
a `[metric_thresholds]` value without a spurious CI failure — see
[metrics.md](metrics.md#versioned-baseline--automatic-re-baseline-on-semantics-or-threshold-change).

### `doctor`

```bash
logos doctor            # fast structural-integrity + admission check; exit 1 on drift
logos doctor --json
```

The fast, always-on graph-integrity guard, with two dimensions. In a handful of
indexed queries it asserts the core structural invariant — **one node per
`symbol_id`** — plus zero orphan rows (dangling `file_id`, dangling edge
endpoints, orphan shingles). It also runs the **admission tripwire**
([FR-GV-20](../specs/requirements/FR-GV-20.md),
[CR-054](../requests/CR-054-graph-update-admission-unification.md)): every
indexed file the *current* `AdmissionAuthority` would reject — gitignored,
under a nested `.git` boundary, in `ignored_dirs`, or glob-excluded — is
flagged, closing the blind spot where `doctor` reported a graph "sound" while
it silently held scratch (e.g. a dev worktree, or a `.playwright-mcp/`
browser-test directory) the whole-graph quality signal then reflected. It
reports `ok` or names each fault. It is a pure read (no reconcile, no
filesystem walk — O(files) matcher checks against the already-indexed paths),
cheap enough to run after every `index`/`sync` as a debug-build assertion.
Drift exits 1.

`--json` adds three additive fields alongside the pre-existing structural ones
(`node_count`, `distinct_symbol_ids`, `duplicate_symbol_nodes`,
`dangling_file_refs`, `dangling_edge_endpoints`, `orphan_shingles`): the exact,
never-truncated `unadmitted_files` count and a capped, lexically-ordered
`unadmitted_sample` of the offending paths, plus a **diagnostic-only**
`doc_symlink_warnings` array. Each entry names a documentation directory-symlink
that exists under your doc-include set but ended up **unindexed** — either
because no sanctioned docs root (`.swe-skills`) is configured, or because the
symlink target escapes the sanctioned containment (see
[configuration.md § Documentation](configuration.md#documentation--indexing-markdown)).
It is advisory: a populated `doc_symlink_warnings` **never** flips `ok` to `false`
or changes the exit status — it flags docs you likely meant to index but aren't.
A full `logos index` purges every unadmitted file (its inbound edges return to
`unresolved_refs`), healing the admission drift.

The same verdict is folded into `health` and — critically — **hard-fails the
quality gate**: `check` (as `check_rules`, under a distinct
`graph-admission-drift` rule id, separate from the structural
`graph-structural-integrity` one) and `session_end` exit 1 on structural **or
admission** drift **independent of the metric signal**, so a corrupted or
admission-drifted graph whose score happens not to move is still caught. This
is a correctness gate, not an evidence tier.

One nuance: `check`/`gate`/`session_end`/`health` all reconcile first (the
**reconcile-then-score** contract, below), and that reconcile's FullWalk sweep
now purges any still-on-disk file the current admission rejects too — so a
`.gitignore` edit or a newly-added nested-`.git` boundary is usually healed by
the very call that would otherwise hard-fail on it. `doctor` itself never
reconciles, so it is the most direct way to see admission drift as it exists
right now; `--no-reconcile` on `check`/`gate` surfaces the same un-healed
verdict (and still exits 1).

### `verify`

```bash
logos verify            # deep shadow-reindex consistency check; exit 1 on drift
logos verify --json
```

The deep, on-demand consistency check. It reindexes the project into a
throwaway **shadow store**, censuses both stores, and diffs node/edge/file
counts and symbol sets against the live graph — surfacing **leaked** symbols
(present live, absent from a fresh index) and **orphaned** symbols (the reverse),
with a capped sample of each, plus the embedded `doctor` report — which carries
the same structural fields *and* the admission tripwire's `unadmitted_files`/
`unadmitted_sample`, read inside `verify`'s single read-only checkout so every
count in the payload reflects one consistent snapshot. It catches drift `doctor`
cannot: a file the live store retains but a fresh index would drop. A full
reindex is **seconds-to-minutes**, so run it deliberately when `doctor` is clean
but a count still looks wrong — not on every check. The live store is opened
read-only for the census; the shadow store (and its `-wal`/`-shm` sidecars) is
torn down on completion. Drift exits 1.

### `evolution`

```bash
logos evolution [--limit <N>]    # default: all snapshots
```

Signal trend over stored snapshots — the architecture-drift detector. Reports
each snapshot's date, label, signal, and per-metric delta from the previous
snapshot. The first snapshot shows null deltas. Reads the append-only
`metric_snapshots` table; history cannot be quietly rewritten.

### `dsm [--granularity <G>]`

```bash
logos dsm                          # module-level coupling (default)
logos dsm --granularity file       # file-level coupling
```

Dependency-structure-matrix view: a square coupling matrix between directories
(module granularity) or individual files. Rows and columns are sorted by layer
order (from `rules.toml`); unassigned files appear last. High off-diagonal
values identify coupling hotspots and layering violations.

### `doc-gaps [--limit <N>]`

```bash
logos doc-gaps                     # all gaps        (alias: doc_gaps)
logos doc-gaps --limit 50          # top 50
logos doc-gaps --no-reconcile      # score the last committed state
```

Exported symbols with no referencing `DocSection` — a prioritised list of the
public surface that documentation does not yet cover. Only doc-less **exported** symbols are
reported; doc nodes themselves never appear in the list. Use the
`[[require_documented]]` contract in
[configuration.md](configuration.md#rulestoml--the-architecture-contract) to
turn a chosen `paths` glob into an enforceable `logos check` gate.

---

## Evidence tiers (history & coverage)

Two **non-gated, advisory** tiers built from git history and external coverage
reports. They live in a separate store, `.logos/history.db`, on its own
forward-only migration track — created on demand by the first `hotspots` or
`coverage ingest` call. The quality `gate` never opens `history.db`, so nothing
in this section can move the 0–10000 signal (see
[metrics.md](metrics.md#the-non-gated-evidence-tiers)).

### `hotspots`

```bash
logos hotspots                     # all ranked files
logos hotspots --limit 20          # top 20 by score
logos hotspots --untested          # only files with no fresh positive coverage
logos hotspots --untested --production-scope   # exclude whole test files from the board
```

Ranks files high in **both** git churn (change frequency over a HEAD-anchored
window) **and** structural complexity — the "where is risk concentrating over
time" report. The first call lazily mines git history into `history.db`;
subsequent calls mine only commits since the last mined SHA. The window, the
co-change mega-commit cap, and the defect-message patterns are tunable via the
`[history]` table in
[configuration.md](configuration.md#history--coverage--the-evidence-tiers).

Each ranked row carries a `coverage` cell — a `state` (`"fresh"`, `"stale"`, or
`"n/a"`) plus a `coverage_bp` percentage in basis points — joining the temporal
tier with the coverage tier. `--untested` keeps only files with no fresh
positive coverage (never-covered, stale, or fresh-0%); a file with *any* fresh
positive coverage is treated as tested and excluded. The kept files are ranked
by score. When **no** coverage has been ingested, `--untested` falls back to a
labeled static-reachability signal: the report carries
`coverage_basis = "static-reachability"` and an explicit `coverage_label`
caveat, so the fallback is never silently conflated with execution coverage.

`--production-scope` (optional, off by default) narrows the board to production
files: a file is dropped from the candidate set **before** ranking when *every*
one of its complexity-contributing functions is `is_test` (a whole test file —
`tests.rs`, `*_tests.rs`, `tests/`), so the `--untested` view surfaces the
production code the surface exists to highlight instead of test files that have
high churn and no coverage of themselves. A production file with an in-file
`#[cfg(test)] mod tests` keeps its production functions and stays on the board.
The flag composes with `--limit`/`--untested`, is opt-in and **gate-immune** (the
hotspot tier is non-gated; toggling it never moves a gated signal), and returns
identical rankings across the CLI, the MCP `hotspots` tool (`production_scope`),
and the web Files & Risk view.

Determinism: the window cutoff is computed from the **HEAD committer
timestamp**, never the wall clock — same HEAD in, byte-identical ranking out.
Files with no in-window history or no parsed functions are **excluded**, never
zero-scored.

### `coverage ingest <REPORT> [--format <FMT>]`

```bash
logos coverage ingest target/coverage/lcov.info        # auto-detect format
logos coverage ingest coverage.xml --format cobertura   # force the parser
```

Parses an external **LCOV** or **Cobertura** coverage report and folds it into
the evidence store as a new snapshot. Format is auto-detected from the content;
`--format` (`lcov` | `cobertura`) forces it. Report-file paths are matched to
indexed files by longest-unique-suffix; absolute build-dir prefixes can be
stripped first via `[coverage] path_strip_prefixes` in
[configuration.md](configuration.md#history--coverage--the-evidence-tiers).
Parsing is **all-or-nothing**: a malformed report is rejected loudly and never
writes a partial store. An ambiguous report path that matches no single indexed
file is reported `unmatched`, never guessed. Exit 3 on an unreadable report,
unknown format, or absent HEAD.

### `coverage refresh`

```bash
logos coverage refresh             # run [coverage_ingest].refresh_cmd, then ingest the artifact it produces
```

Runs the author-configured `[coverage_ingest].refresh_cmd` (see
[configuration.md](configuration.md#history--coverage--the-evidence-tiers)) as a
subprocess via `sh -c`, then discovers and ingests the coverage artifact it
produced. This is the **only** place Logos ever *runs* a coverage command — never
on the `serve`/watcher path ([ADR-38](../specs/architecture/decisions/ADR-38.md)),
only on this explicit invocation. Errors loudly (exit 3) if no `refresh_cmd` is
configured, the command fails, or it produces no recognizable artifact. Artifact
discovery resolves the built-in conventions plus *literal* `artifact_glob`
entries (newest by mtime); a wildcard-only glob that matches no convention or
literal yields a loud error. With a `[coverage_ingest]` table configured, a
running `serve` watcher **auto-ingests** a matching artifact whenever it appears
or changes (a local read+parse, degraded to a warning on any failure — never a
subprocess); `coverage refresh` is the manual counterpart that also produces the
artifact first.

### `coverage status`

```bash
logos coverage status              # human summary
logos coverage status --json       # per-file freshness + overall fraction
```

Reports per-file coverage **freshness** and the overall fresh-coverage
fraction. Freshness is content-hash based: a file whose content changed since
the report was ingested flips to `stale`, and **stale coverage carries no line
data** (it reads as absent, not as the old number). With nothing ingested, the
command returns `n/a` plus a notice (`no coverage ingested — run 'logos coverage
ingest <report>' …`) and exits 0 — absent evidence is never fabricated into a
zero.

---

## Source wiki

A **gate-immune** store of generated, human-readable pages about the codebase,
anchored to the symbols and files they describe. It lives in its own store,
`.logos/wiki.db`, on a forward-only migration track created on demand by the
first `wiki write` or `wiki status`. Like the evidence tiers, **nothing here
moves the 0–10000 signal** — `logos gate`/`logos scan` are byte-identical
whether `wiki.db` is absent, populated, or stale; no governance path holds a
connection to it.

The wiki serves **three tiers** (CR-062, [ADR-57]): an *extracted* tier
live-rendered from the graph; a *presented* tier that the binary assembles
**deterministically** from the project's authored `docs/specs/**` and
`docs/howto/**` sources (`wiki materialize`, below) — copied verbatim, never
paraphrased; and a *generated* tier written by an external generator (an LLM, a
tool) and stored byte-verbatim. Each page carries tier-correct mandatory
**provenance**: a presented page is labelled `"presented from docs/specs/… — not
model-generated"` (`generator = logos:doc-present`), while a generated page
carries the `generator` label plus the explicit `"generated content — not
extracted by Logos"` marker. Every page records its `written_head` commit and a
per-anchor `freshness` state. **Anchors** tie a page to entities it describes —
`file:<path>` or `symbol:<name>` — and each anchor's freshness is recomputed
against the working tree on every read:

- `fresh` — the anchored file/symbol exists and its defining file is unchanged
  since `written_head`;
- `stale` — it still exists but the defining file changed (regenerate the page);
- `missing` — the file or symbol is gone from the graph.

When **all** of a page's anchors go `missing`, the page is auto-pruned on the
next read/status and recorded in the pruned log (`wiki status`) — a page never
outlives every entity it documents.

Generation runs **off the request path**: an external generator works the
`wiki status` work-list, and `wiki generate` formats that work-list into a
ready-to-run queue (a prompt block, or `--json`) — a pure, offline read that the
connected agent's own skill loop or the `ui`-gated in-process generator
consumes; the binary itself stays offline.

`write`/`read`/`search`/`status`/`materialize` have **payload-identical MCP twins**
(`wiki_write`/`wiki_read`/`wiki_search`/`wiki_status`/`wiki_materialize`) for
agents — five wiki tools. `generate`, `delete`, `skill`, and `hook` are
**CLI-only** — `generate` formats the work-list into a runnable generation queue,
`delete` is destructive (kept off the agent surface), and `skill`/`hook` are
local materialization steps.

### SRS mode (Case 1) vs. inference (Case 2)

`wiki materialize` and the generation queue are **bimodal** (CR-062, FR-WK-21):

- **Case 1 — SRS present** (`docs/specs/architecture.md` **and** ≥1
  `FR-*`/`NFR-*`/`UAT-*` file under `docs/specs/requirements/`): the Design/Specs
  pages are *presented* deterministically from source, and the connected agent is
  asked to generate **only** the Summary/Overview tier (grounded on the presented
  pages). When `docs/howto/` is present, a **User Guide** tier is presented too.
- **Case 2 — no SRS:** unchanged — the agent infers the full set (Overview +
  present categories) from the code graph.

Per-file `files/*` pages from earlier schemes are retired by a forward-only
migration, and a reconciliation sweep (run from `materialize`) purges any stored
page outside the active-mode valid set (Overview ∪ present categories ∪
`guide/*`), each removal logged to the pruned log (FR-WK-22).

### `wiki write`

```bash
logos wiki write <SLUG> --title <TITLE> --generator <LABEL> \
  [--anchor file:<path>]... [--anchor symbol:<name>]... \
  [<BODY> | --body-file <PATH>]
# short flags: -t <title>, -g <generator>
echo "## Notes" | logos wiki write arch/auth -t "Auth" -g "claude-opus-4-8" \
  --anchor symbol:authenticate --body-file -
```

Upserts a page by slug (a path-like id of lowercase/digit/`-`/`_` segments). The
body is stored **byte-verbatim** up to a 1 MiB cap; pass it as the positional
argument, or use `--body-file <PATH>` to read from a file (or `-` for stdin) so a
large markdown body never hits the shell's argv limit. `--generator` is
**mandatory** (provenance is not optional). Anchors are resolved at write time;
a `symbol:` anchor that resolves to no graph node is recorded but reads back as
`missing`. Re-writing the same slug replaces the page.

**Content-validity guard.** The write path rejects a body that is agent-noise
rather than documentation — one that contains a tool-call token or a
command-error transcript, opens with a first-person planning or refusal
preamble, or has no Markdown heading / falls below a minimum length. The check
is **structural**, so a page that legitimately contains a fenced code block
(e.g. a ` ```bash ` example) is never rejected on that basis. The tool-call and
command-error signatures are scanned with fenced code blocks stripped, so a
page that *quotes* one of those patterns inside a ` ``` ` fence — e.g. docs that
describe this guard — is accepted; the same token outside a fence is rejected.
A rejected write
leaves the store byte-identical and returns an honest per-page failure; it
applies identically to the positional/`--body-file`/stdin paths and to the
in-process generator, where a rejected page is recorded and skipped without
aborting the run.

### `wiki read`

```bash
logos wiki read <SLUG>
logos wiki read arch/auth --json
```

Returns the page body plus its full provenance block and the current per-anchor
freshness. A slug miss — a never-written slug, or a page whose last anchor just
went missing and was auto-pruned — is an **exit-zero miss**: the command prints
`null` (an empty `--json` payload) and exits 0, never an error. This is the
empty-store posture — absent evidence is never fabricated into a failure, the
same discipline the rest of Logos follows.

### `wiki search`

```bash
logos wiki search <QUERY>          # FTS5 bm25 over titles + bodies
logos wiki search --list           # enumerate all pages (omit the query)
```

Full-text search (SQLite FTS5, bm25 ranking) over page titles and bodies. Each
hit carries its staleness flag so a stale page is visibly flagged in results.
`--list` enumerates every page instead of searching.

### `wiki status`

```bash
logos wiki status
logos wiki status --json
```

The store summary and the **regeneration work-list**: stale pages, pages with
missing anchors, the pruned-page log, and **page-worthy entities that have no
page yet** (so a generator knows what to write next).

The work-list is **consolidated and doc-grounded** (CR-034). It seeds exactly
**one entry per documentation category** whose source files exist on disk —
ADRs, Components, Integrations (from `docs/specs/architecture/…`), Functional
Requirements, Non-Functional Requirements, User Acceptance Tests, and Frontend
Design (from `docs/specs/…`) — rather than one fragmented page per ADR /
requirement / Story / CR node. A category whose source files are absent is
simply not seeded (never fabricated); the per-source-file Modules pages are
unchanged.

### `wiki generate`

```bash
logos wiki generate          # human prompt block, one ready-to-run item per page
logos wiki generate --json   # the same queue as machine JSON
```

Formats the `wiki status` work-list into a deterministically ordered **generation
queue**. The default output is a prompt block — one entry per absent/stale
agent-tier section (the Summary/overview pages, the consolidated documentation
categories, and per-file objectives), each carrying its target slug and a
runnable `logos wiki write …` skeleton (slug positional, body on stdin, anchors
and free-text fields prefilled).

Consolidated and overview items also carry a **doc-grounding directive** (CR-034):
each names the `docs/` source file(s)/glob the generator should summarize into
that page (e.g. the ADRs page is grounded in `docs/specs/architecture/decisions/*.md`),
so generated pages reuse the project's own documentation rather than re-deriving
it. The Summary and Architecture overviews fall back to **code-reading** only
when their mapped doc is absent. The consolidated items serialize as
`"category":"consolidated-doc"` in `--json`, each with an optional `grounding`
object (`sources`, `fallback_to_code`, `directive`).

`--json` emits the same queue as one compact `{"items": […]}` object —
**byte-identical** for a fixed `wiki.db` + graph revision. Native (extracted)
sections are never queued. A pure read: no `wiki.db` write, no LLM, no network.
An empty work-list prints `Nothing to generate — the wiki work-list is empty.`
and exits 0. CLI-only.

### `wiki materialize`

```bash
logos wiki materialize
```

Deterministically assembles the **presented** tier (CR-062, FR-WK-20). In SRS
mode (Case 1) it presents each present Design/Specs category — and the single-file
Architecture page — from the project's authored `docs/specs/**` sources into
`wiki.db`, with `generator = logos:doc-present`, one source-file anchor per
document, and the current built-at revision; when `docs/howto/` is present it also
materializes each guide as a `guide/<name>` page (`README.md` → `guide/overview`)
under the **User Guide** tier. It then runs the reconciliation sweep that purges
any stored page outside the active-mode valid set.

A **pure deterministic write** — no LLM, no network (NFR-SE-01) — and
byte-identical on re-run. Outside SRS mode (Case 2) it is a no-op. It runs
automatically ahead of the LLM queue in the UI-gated generation flow
(FR-WK-18); running it manually is safe. Has a payload-identical MCP twin,
`wiki_materialize`.

### `wiki delete`

```bash
logos wiki delete <SLUG>
```

Explicitly deletes a page by slug. An unknown slug exits non-zero. CLI-only —
not exposed over MCP.

### `wiki skill --emit [DIR] [--force]`

```bash
logos wiki skill --emit               # materialize into the project root
logos wiki skill --emit --force       # overwrite an existing install
logos wiki skill --emit path/to/dir   # target a specific base directory
```

Materializes the **embedded wiki-generation skill** — the canonical
`.agents/skills/logos-wiki/SKILL.md` (stamped with the binary version) plus the
`.claude/skills/logos-wiki` symlink that points at it. This is the same skill
`logos init -i` offers to materialize; run it standalone to install or, with
`--force`, restore the skill after an upgrade. Without `--force` an existing
install is left untouched (local edits survive). CLI-only.

### `wiki hook --emit [--force]`

```bash
logos wiki hook --emit          # install the Claude Code quality-report hook (no-op if present)
logos wiki hook --emit --force  # re-emit, replacing the managed entry
```

Installs the **SessionEnd quality-report** hook
([FR-IN-07](../specs/requirements/FR-IN-07.md)), merged into the shared
`.claude/settings.json` — at session end it prints the current signal, the
blessed baseline, and any rule violations as a non-blocking readout; always
exits 0. Set `LOGOS_QUALITY_REPORT_DISABLE=1` in the environment to silence
the readout without uninstalling the hook.

The merge is **non-clobbering**: an existing managed entry is left unchanged,
a foreign/unparseable config is never overwritten, and the merge is idempotent
(`--force` re-emits it). `logos init -i` installs it **default-on** alongside
the embedded skill. Installing or running the hook performs no LLM call and
opens no outbound connection inside the binary — the offline boundary holds.
CLI-only. (The PostToolUse wiki-augmentation hook this command once also
installed was retired — [CR-070](../requests/CR-070-retire-wiki-augment-hook.md).)

There is no headless SessionEnd wiki-autogen hook and no `claude -p`
invocation anymore ([CR-047](../requests/CR-047-internal-wiki-generation-on-agent-substrate.md)):
`ui` builds regenerate drifted wiki pages in-process when the Wiki tab is
opened; non-`ui` builds regenerate manually in the user's own Claude Code via
the materialized `logos-wiki` skill (`wiki skill --emit`, above).