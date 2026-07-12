# Usage

Logos has two surfaces over the same engine: the **CLI** (this page, first
half) and the **MCP server** for AI agents (second half). Both read the same
index, honor the same configuration, and record into the same local stats.

## The core loop: index → navigate → keep fresh

```bash
cd /path/to/project
logos index          # build the code graph (full rebuild, idempotent)
logos status         # health: file/node/edge counts, resolution coverage, freshness
logos sync           # after edits: incrementally fold changed files in
logos sync src/a.rs  # or sync specific paths
```

`logos index` walks the tree (gitignore-aware, symlink-contained), extracts
symbols and references per language, resolves references into call/import
edges, detects framework routes, and annotates the graph (dead code,
duplicates, export status) — one command, exit 0 on success.

**Freshness model:** navigation commands serve the latest committed snapshot
and never reconcile per call — they are fast point queries. Re-run
`logos sync` (or let the MCP watcher do it) after editing.

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

> **Reading `resolution_coverage` — why ~50% is normal, not a defect.** The
> bound-ratio is measured over *every* syntactic reference the extractor sees,
> and the denominator deliberately includes references that can never bind to a
> workspace symbol — by design, never as a failure:
>
> - **Stdlib & external-crate calls.** Logos indexes only your workspace and
>   never fabricates a node for code it didn't parse ([NFR-RA-05](../specs/requirements/NFR-RA-05.md),
>   [ADR-26](../specs/architecture/decisions/ADR-26.md)). In idiomatic code, a
>   large share of references are calls like `.unwrap()`, `.iter()`, `.clone()`,
>   `Some`, `Ok`, `Vec::new`, or `std::fs::*` — these stay unresolved forever
>   because their targets live outside the graph. Receiver-method calls
>   (`x.foo()`) are the biggest such bucket: the receiver's type is unknown, so
>   they bind **only on genuine lexical/module scope evidence** and otherwise
>   stay honest misses. The workspace unique-name fallback that used to (mis)bind
>   a bare `x.map()` to a lone same-named `fn map` in another module is gated off
>   for the method form ([CR-066](../requests/CR-066-receiver-method-overbinding.md),
>   [FR-RS-06](../specs/requirements/FR-RS-06.md)), so these calls no longer
>   fabricate cross-module `Calls` edges — they stay in `unresolved_refs`. A
>   bare-path call (`foo()`, single segment) is complementary: when a free
>   function and same-named associated methods both exist at the call scope, the
>   free function wins the tie ([CR-068](../requests/CR-068-reachability-binding-precision.md)
>   Part B, [FR-RS-07](../specs/requirements/FR-RS-07.md)) — recovering the
>   correct target instead of leaving it ambiguous — while a scope with only the
>   method(s) still binds them (a strictly additive tie-break, never a drop).
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
| `1` | Completed, but violations/threshold failures found (`check`, `gate`) or structural drift detected (`doctor`, `verify`). |
| `2` | Usage error: bad flags, invalid config/rules file. |
| `3` | Internal/environment error — e.g. no index present (the message tells you to run `logos index`), engine failure. Never a raw panic. |

These codes are the CLI projection of Logos's **fail-soft / fail-loud** error
contract: a *degraded* condition (a skipped file, a partial resolution) warns
and still exits `0`, while a *correctness* condition (no index, corrupt store,
bad config) aborts loud. The full contract — both surfaces, every condition, and
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
by [`logos init`](commands.md#init--i---hooks) (the freshness hooks, the `pre-push` gate,
and the SessionEnd quality-report hook). For the full copy-pasteable CI recipe —
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

The host then sees **27 `logos:*` tools**, all live:

| Navigation (8) | Quality & Governance (10) | Evidence tiers (4) | Source wiki (5) |
|---|---|---|---|
| `search`, `node`, `callers`, `callees`, `impact`, `explore`, `context`, `status` | `scan`, `rescan`, `check_rules`, `health`, `doctor`, `verify`, `evolution`, `dsm`, `session_start`, `session_end` | `hotspots`, `coverage_ingest`, `coverage_status`, `coverage_refresh` | `wiki_read`, `wiki_search`, `wiki_status`, `wiki_write`, `wiki_materialize` |

Each MCP tool is a thin twin of the CLI command of the same name and returns a
byte-identical payload. The four evidence-tier tools are **non-gated** — like
their CLI counterparts they never move the `session_end` gate signal — and the
source-wiki tools read/write the gate-immune `wiki.db` store, never the graph.

Server behavior guarantees:

- **Fast cold start** — the server answers its first request (the `initialize`
  handshake) in well under a second, even in a repository with a large build-output
  tree. Watcher registration prunes ignored directories (`target/`, `node_modules/`,
  `dist/`, `build/`, `vendor/`, `.git/`, and anything in `[semantics].ignored_dirs`)
  through the same admission authority that guards `index`/`sync`, so it never walks
  build output to seed rename tracking.
- **Stdout purity** — stdout carries only JSON-RPC frames, even at
  `RUST_LOG=trace`; logs go to stderr. A malformed frame gets a structured
  parse error (`-32700`) and the server keeps answering.
- **Clean teardown** — when the host disconnects (stdin EOF, even before the
  handshake), the server winds down by itself with exit 0: no orphaned
  processes to clean up.
- **Honest telemetry** — agent calls are stamped `surface: "mcp"` in
  `logos stats`, CLI calls `surface: "cli"`, one record per call.

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
without) an index. Web-dashboard activity (`surface="web"`) is **excluded from
every figure**: viewing the stats emits its own telemetry, so counting it would
be self-referential noise, not a measure of the tool's value.

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
it — while the outbound LLM egress client stays opt-in behind a separate `agents`
feature:

```bash
# Default: the dashboard, egress-free. Listens on loopback, links no HTTP client.
cargo install --path cli                 # or: cargo build -p logos --release
logos serve --ui --port 4983             # then open http://127.0.0.1:4983
```

### Build matrix — dashboard, chat/wiki generation, or slim headless

CR-078/ADR-60 separates *listening* (the read-only dashboard) from *dialing* (the
LLM egress client). Three build modes result:

| Build | Command | Dashboard (`serve --ui`) | Chat + in-app wiki generation | Outbound HTTP client |
|-------|---------|--------------------------|-------------------------------|----------------------|
| **Default** | `cargo install --path cli` (no explicit features) | ✅ served | ⚠️ **tab visible but inert** — the SPA still renders the Chat tab, but a turn fails `405` (no agent backend); wiki generation likewise absent | ❌ **none** — links the loopback *server* only, never a client |
| **Agents** | `cargo install --path cli --features agents` | ✅ served | ✅ Chat tab + Wiki-tab generation | ✅ `rig`/`reqwest`, **opt-in**, user-initiated and consent-gated at runtime ([ADR-40](../specs/architecture/decisions/ADR-40.md)) |
| **Slim headless** | `cargo build -p logos --no-default-features --features lang-all` | ❌ no `--ui`, no listener | ❌ absent | ❌ **none** — the absolute-offline binary; no socket stack at all |

The default and agents builds are the shipped shapes: cargo-dist release artifacts
enable `agents`, so downloaded binaries carry both the dashboard and chat/wiki
generation. The slim `--no-default-features` build is the headless CLI/MCP binary
with no web surface. Two structural fitness checks
(`logos-core/tests/no_network_deps.rs`) keep the postures honest: the **default**
tree may link the loopback HTTP *server* but is denied every egress *client* crate
(it can never phone home), and the **slim** tree is denied the entire socket stack
(it links nothing that can open or accept a connection) —
[NFR-SE-01](../specs/requirements/NFR-SE-01.md), ADR-60.

The surface binds **`127.0.0.1` only** (a compile-time constant — no flag or
env var moves it), rejects non-loopback `Host` headers with `403`, and stamps a
self-only `Content-Security-Policy` on every response. Every non-GET request is
answered `405` **except** a `POST` to one of a small, enumerated set of mutating
routes — the config-write/apply routes (`/config/save`, `/config/apply`,
`/config/secret`) and the Chat routes (`/chat`, `/chat/clear`); those — and only
those — are additionally gated by a same-origin + per-session intent (CSRF)
guard, so a request missing the `x-logos-intent` token or arriving cross-origin
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

Twelve views, each sidebar item rendered with an inline-SVG icon and grouped into
three use-driven **navigation groups**. Each tab is a client-side route the SPA
renders inside the app shell — navigation never reloads the page, and a refresh
on any client route resolves back to the shell via the server's history fallback.
The **Dashboard is served at the root `/`**; the legacy `/overview` path that
older bookmarks point at **redirects to `/`** (a history `replaceState`, so it
leaves no extra back-button entry). The browser tab carries the Logos **favicon**,
an embedded same-origin SVG served from the bundle root.
Each read view's figures trace to a read-model, and an empty store renders an
honest empty state naming the producing command:

- **Group A — read & navigate:** Dashboard `/`, Health `/health`, Graph `/graph`, Chat `/chat`, Wiki `/wiki`, Architecture / Cycles `/architecture`.
- **Group B — analyse:** Files & Risk `/files`, Rule findings `/gaps`, Coverage `/coverage`.
- **Group C — configure:** Statistics `/statistics`, Config `/config`.

**Workspace mode.** When `logos serve --ui` starts at a workspace parent (a
`logos.workspace.toml` is discovered up-tree), the app shell renders a **member
selector** that scopes every view to one member (the member is part of each
view's cache key, so switching re-fetches). Alongside the per-member views it
exposes the cross-service surfaces: an **app-level service map** — the ECharts
graph canvas drawing services as nodes and cross-service bindings as edges,
including first-class **topic hops** (`A → topic → B`) once broker coupling is
promoted — a **cross-service coverage dashboard** (bound / ambiguous /
unbound-with-reasons), and a cross-service impact view. In a plain single repo
(no manifest) **no selector is rendered and the UI is byte-for-byte unchanged**;
`--standalone` forces single-repo focus even under a workspace parent. Workspace
read-models are served under `/api/v1/workspace/*`; the single-root `/api/v1/*`
surface and the self-only CSP are untouched.

The **Files & Risk** view (`/files`) is the merge of the former Hotspots and
Commits views into one risk-ranked per-file table — commits, churn (`+/−`), age,
co-change, defect density, complexity, and a coverage cell — with `?untested`
keeping only files lacking fresh positive coverage. The per-file table renders
inside a white widget card on the muted `NON-GATED TIER` band, and its `n/a`
cells (e.g. churn/age for a file with no history) right-align to their numeric
columns (CR-042). On **Health** (`/health`) the Quality-signal table carries a
**Score** column rendering each metric's normalized value as a CSP-safe `<meter>`
bar — the same widget the Dashboard roll-ups use; applicability drop-outs
(Cohesion/Focus) show a muted, right-aligned `n/a` with no bar (CR-042). The **Architecture / Cycles**
view (`/architecture`) leads with the dependency-cycle list, then the DSM matrix;
its cycle participants deep-link into `/graph?seed=<module>`.

The root `/` view is the **Dashboard**: a verdict-rich roll-up that leads with
the gate `PASS`/`FAIL` verdict, then a hero row of at-a-glance figures — a banded
*Quality index* (the 0–10000 signal), *Code coverage* (the overall line-%
aggregate), and the per-project **language composition** — each fed by a
read-only accessor, never a fabricated number. The body is recomposed (CR-037)
into **equal-size paired widget rows** plus a **full-width Project Overview**:
the `overview/project-overview` wiki page snippet with a link into `/wiki`
(CR-034); when that page has not been generated yet it shows an honest "not yet
generated" empty state rather than a fabricated overview. One widget slot holds
the **Rule findings** card projecting `check_rules` (FR-GV-02): it reads **green**
when there are zero rule violations, **red** when there are findings, and a muted
**onboarding** state when no `.logos/rules.toml` exists — with a link into the
Rule findings view (`/gaps`). Loading the Dashboard writes nothing to any
store.

**The `/api/v1/*` data API.** The SPA reads every view's data from a same-origin
JSON read-model API under `/api/v1/*` — one read-only endpoint per view's data,
each a JSON serialization of an `Engine` read-model
([FR-UI-22](../specs/requirements/FR-UI-22.md),
[ADR-43](../specs/architecture/decisions/ADR-43.md)). All are GET-only (`405`
otherwise), loopback-only (`403` for a non-local `Host`), and **write-free** — a
read mutates no store and leaves every metric/cycle/DSM/dead-code scope
byte-identical. The endpoints are: `/api/v1/overview`, `/api/v1/health`,
`/api/v1/graph`, `/api/v1/query`, `/api/v1/impact`, `/api/v1/node`,
`/api/v1/search`, `/api/v1/architecture`, `/api/v1/gaps`, `/api/v1/files`,
`/api/v1/coverage`, `/api/v1/wiki`, `/api/v1/wiki/nav`,
`/api/v1/wiki/search`, `/api/v1/wiki/page/*slug`, `/api/v1/wiki/asset/*path`
(same-origin, read-only, path-sandboxed doc-image serving — CR-064), and
`/api/v1/config`.

`GET /api/v1/graph` returns the bounded graph-elements read-model
(`nodes`/`edges` plus `total_*`/`elided_*` counts and `warnings`) that backs the
Graph canvas. It accepts `?seed=<id>` and `?cap=<N>` plus three
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

### Interacting with the dashboard

The dashboard is interactive, with every interaction handled client-side in the
React app over the same-origin `/api/v1/*` read-models — sorting, paging,
focusing, and filtering mutate no store and contact no external origin:

- **Navigation feedback.** A slow navigation or any in-flight data fetch (e.g.
  Graph/Wiki search) arms an orange top progress bar plus a pending state on the
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
    panel and from `/architecture` cycle participants (which deep-link to
    `/graph?seed=<module>`).
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
- **Interactive tables.** Every data table (Rule findings, Architecture / Cycles, Health,
  Files & Risk, Coverage, Wiki search/anchors, Graph Decisions & docs, and the
  Graph nodes/edges accessible tables) has sortable column headers — click to sort
  the **full dataset** first, then the page is sliced — and consistent pagination.
  A single shared **`DEFAULT_TABLE_PAGE_SIZE = 20`** governs every table so no view
  renders more than 20 rows per page (the Graph **query results** table is the one
  deliberate exception at **15**, [FR-UI-11](../specs/requirements/FR-UI-11.md),
  [FR-UI-14](../specs/requirements/FR-UI-14.md)). Sort/page state is carried in the
  client route's URL (bookmarkable) and namespaced per table (`<tid>_sort` / `_dir`
  / `_page`), so two tables on one view (e.g. Architecture / Cycles) never collide. Numeric columns
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
  saved file. The chat API key is written through the separate masked, write-only
  `POST /config/secret` route (only presence + last-4 is ever rendered, never the
  raw key). All three POSTs ride the same-origin + `x-logos-intent` guard described
  above; the SPA submits them via `fetch` carrying the intent token.
  Beside Apply, a **Check graph consistency** control runs the deep `verify`
  (`POST /api/v1/verify`, the same-origin twin of `logos verify`): it shows an
  explicit loading state while the shadow reindex runs (seconds-to-minutes), then
  renders a **`CONSISTENT`** verdict badge on a healthy graph or a **`DRIFT`**
  callout with the live-vs-reindex deltas and a capped sample of leaked/orphaned
  symbols. On a read fault it shows an honest error panel — never a fabricated
  `CONSISTENT` ([FR-UI-25](../specs/requirements/FR-UI-25.md),
  [NFR-RA-05](../specs/requirements/NFR-RA-05.md)).
- **Per-page Wiki (`/wiki`).** The Wiki is a navigable per-page documentation
  site, not one scroll. `/wiki` is a **Start-here landing** that leads with a
  Summary callout and an "N pages · all fresh" freshness badge (derived from
  `wiki status` counts, no write), and every page renders in a **three-column
  layout**: a tiered doc-tree menu of discrete page links, the page content, and
  an "On this page" TOC rail. The menu has **three consolidated tiers** (CR-035,
  CR-039) — **Summary** (the overview/Getting-Started pages, minus the architecture
  narrative), **Design** (the architecture narrative — listed once and labelled
  *Architecture* — plus the consolidated ADRs / Components / Integrations pages,
  and Frontend Design only when `docs/specs/frontend-design.md` exists), and
  **Specs** (consolidated Functional Requirements / Non-Functional Requirements /
  User Acceptance Tests) — plus a **top-level Search link** rendered as a sibling
  of the tiers (CR-039 dropped the lone-item *Reference* tier and hoisted Search
  out of it), with no redundant "Wiki" heading and no fragmented per-ADR /
  per-requirement / per-Story / per-CR entries. The earlier browsable **native** tier (the `/wiki/native/*`
  routes and the per-file Modules tier) is **retired** (CR-035): the native
  read-model still backs generation internally, but the only browsable content is
  now the **agent** tier of generated prose at `/wiki/page/*slug`, carrying
  revision-keyed staleness flags. A consolidated category page that has not been
  generated yet renders an honest placeholder (HTTP 200), never a 404. Each page
  leads with **one enlarged in-card `h1` title** (CR-035): the title is rendered
  once inside the content card and a duplicate leading heading in the body prose
  is suppressed, so a page never shows its title twice. A **"Wiki"** provenance
  callout carries the `generator · HEAD · @revision` line and any STALE/REGEN
  badges (without an on-page "generated content" disclaimer — the fixed marker
  stays in the `wiki read` / `wiki_read` payload); the empty Anchors card is
  suppressed on unanchored/overview pages. Page prose is rendered through a
  **comrak GFM pipeline** (CR-035): GitHub-flavoured **tables** render as real
  HTML tables, **fenced code blocks** render styled (background/border/padding)
  with long unbroken tokens contained inside the content card — scrolled within
  the block, never overflowing the page (CR-039) — raw-HTML is still stripped for
  safety, and the `toc-<slug>` heading anchors that back the "On this page" rail
  are preserved. Mermaid diagrams (any
  ` ```mermaid ` fences in agent prose) render as **visual diagrams** from a
  vendored, embedded offline bundle under the unchanged self-only CSP, with every
  node label sitting within its box. Diagram colors are **theme-aware** — driven
  through `mermaid.initialize({ themeVariables })` so they land as inline SVG
  attributes (CSP-safe, no injected `<style>`) and **follow the app's light/dark
  mode**, so a diagram is legible in either theme rather than black-on-black
  ([FR-WK-15](../specs/requirements/FR-WK-15.md)). The Wiki tab owns its own client sub-routes —
  `/wiki` (landing), `/wiki/search`, and the `/wiki/page/*` reader — all inside one
  mounted React view; the page bodies and freshness come from `GET /api/v1/wiki`,
  `/api/v1/wiki/nav`, `/api/v1/wiki/search`, and `/api/v1/wiki/page/*slug`. FTS
  search lives on the self-sufficient `/wiki/search` client route (CR-035), reached
  from the **top-level Search link** in the menu (CR-039): it carries its own search
  input and renders live results from `GET /api/v1/wiki/search`. Each result row
  carries the **same unified staleness verdict** as the page view (CR-039): REGEN
  PENDING / STALE / MISSING ANCHOR / FRESH, derived from the one core
  `revision_pending` predicate against the same persisted graph revision, so a page
  can never read "regeneration pending" on its page view and "fresh" in search.
  Every read endpoint is read-only (GET-only; non-GET → 405) and same-origin.
  Regeneration of the agent tier now runs **in-process** (`--features agents`
  builds only): opening the Wiki tab checks the work-list and, if pages have drifted,
  launches a background generation pass on the in-process wiki-agent under a
  single-run lock, streaming each page in over SSE as it completes while existing
  pages stay browsable — gated by a first-use consent disclosure and a dedicated
  `[wiki].model` ([CR-047](../requests/CR-047-internal-wiki-generation-on-agent-substrate.md);
  see [The Wiki tab](#the-wiki-tab) below). The headless `claude -p` SessionEnd
  autogen hook is retired, as is the PostToolUse wiki-augmentation hook
  ([CR-070](../requests/CR-070-retire-wiki-augment-hook.md)); `wiki generate`
  is still available as the CLI queue read, and the embedded `logos-wiki`
  skill is the manual generation path outside `agents` builds.

Run it alongside the MCP server in one process with `logos serve --mcp --ui`.

### The Chat tab

The **Chat** tab (`/chat`, between Graph and Wiki) is an LLM-backed assistant
for compound questions about your codebase — "what's the riskiest untested code
and who calls it?", "what breaks if I change this symbol?". It is a React client
built on **assistant-ui** ([`@assistant-ui/react`](../specs/architecture/decisions/ADR-45.md))
that streams the turn as Server-Sent Events over the unchanged intent-guarded
`POST /chat`. The SSE wire contract, the orchestrator, the budget tree, and the
per-turn memory are all **untouched** — the surface is rebuilt on assistant-ui
through a **custom external-store runtime adapter** over the existing SSE client,
so Logos keeps owning its own message array and renders the planner side-channel
(plan list, activity chips, budget-halt) as custom components. A **planner**
decomposes the question, dispatches
specialized read-only **subagents** over the existing Logos tools, and streams
back a synthesized answer:

- **Graph-Navigator** — structural navigation (callers, callees, impact, search).
- **Governance-Analyst** — the quality/governance read-models (gate, gaps, hotspots).
- **Source-Reader** — sandboxed read/grep/glob over the project source.
- **Synthesizer** — a tool-less subagent that writes the final answer from what the others found.

A working Chat exists **only** in an `--features agents` build. The default
`logos` binary ships the dashboard and no networking client — there is no
outbound call anywhere short of the opt-in `agents` feature (CR-078/ADR-60).
Note that the default build still **renders the Chat tab** in the SPA (the web
bundle is feature-agnostic), but a chat turn fails with `405` because the
`POST /chat` route carries no agent backend without `agents`; the affordance is
a known, harmless cosmetic gap (the server never gains an egress client). The
conversation and its per-turn memory persist in `.logos/chat.db` (created on the
first turn, gitignored, never in the default binary).

**Workflow:**

1. **Configure a provider and key.** Open the **Config** tab and set the chat
   `provider` / `model` (and `base_url` for an OpenAI-compatible endpoint) plus
   the **chat API key** field (stored in the gitignored `.logos/secrets.toml`).
   See [configuration.md](configuration.md#chat--the-agentic-chat-tab) for the
   keys, defaults, and the budget tree.
2. **Open the Chat tab.** Until a provider model **and** an API key are both set,
   the tab shows an honest **configure-first** state — a muted callout linking to
   the Config tab, and **no composer**. This is a state, not an error: until you
   configure it, no outbound call is possible.
3. **Acknowledge the consent banner.** Once configured, the first thing the tab
   shows is a **consent banner** naming the exact configured **endpoint host**
   (e.g. `openrouter.ai`, or `api.anthropic.com` for the Anthropic provider) and
   stating that asking a question sends your message together with **source and
   graph excerpts** from the project to that endpoint. **Nothing is sent until
   you click _Start chatting_** to acknowledge — the acknowledgement persists, so
   you grant it once.
4. **Ask a question.** Type into the composer at the bottom and **Send**. The
   answer streams in over SSE: the planner's **plan** appears first (a list of
   steps), then a **subagent-activity chip** per step (running → done, each naming
   its role), then the synthesized **answer** rendered token-by-token as it
   arrives. The answer renders as **GitHub-flavoured Markdown** — headings, lists,
   tables, and **fenced code blocks**, each block carrying a **copy** control (the
   markdown is built as an escaped React tree, no raw HTML, CSP-clean). While a
   turn streams you can **Stop** it (the in-flight request is aborted — and is also
   aborted automatically if you leave the Chat tab mid-stream); a finished turn
   offers **Copy** (the whole answer) and **Regenerate** (drops the prior assistant
   turn and re-runs your message — never a duplicate). The turn runs under the
   budget tree ([configuration.md](configuration.md#the-budget-tree)) — the
   tool-call ceiling, the per-subagent cap, and the replan limit are shown as a
   note on the empty log.
5. **Clear history.** The **Clear history** control wipes the conversation **and**
   its per-turn agent memory from `.logos/chat.db` (it asks for confirmation
   first). This is irreversible.

**Honest states (never a fabricated answer):**

- If a turn reaches a budget bound it **halts** and the log says so explicitly,
  naming which bound was hit (the global tool-call ceiling, a subagent's cap, or
  the replan limit) and its limit — it never invents an answer to fill the gap.
- A provider error is surfaced **verbatim with its full detail** — the error
  carries the complete provider source chain and distinguishes a **transport**
  failure (endpoint unreachable) from an **HTTP-status** failure (with the `(HTTP
  n)` code) from an **auth** failure, instead of an opaque flattened string. A
  dropped connection or a turn that closes without producing an answer is likewise
  shown as an explicit error line, never a silently empty turn.
- **Configuration preflight.** Before a turn opens any connection, a preflight
  validates the chat config — base_url well-formed and present, key present, model
  set — and on a problem returns an **actionable message** naming the specific
  misconfiguration rather than letting the request fail opaquely downstream.

**One honest limitation:**

- **Multi-turn continuity in the UI is partial.** Cross-turn working memory is
  stored per thread in `.logos/chat.db`, but the stream does not yet surface the
  server-created thread id, so each turn from the UI currently starts a fresh
  thread (and the in-app log starts empty on load). The memory capability is
  present at the store level; threading it through successive UI turns is not yet
  complete.

Every route is **read-only except the explicit, intent-guarded mutating
routes**: the config-write/apply routes (`/config/save`, `/config/apply`,
`/config/secret`), the Chat routes (`/chat`, `/chat/clear`), and the Wiki
generation trigger (`/wiki/generate`). Serving any read
page — the Dashboard, Health, the analytics views, the Wiki, the Chat tab itself
— reads the latest persisted snapshot through dedicated read-model accessors and
never writes back, so browsing leaves `logos.db` and `history.db` byte-identical
and never perturbs the evolution trend it plots. The writes the surface can make
are: a validated config **Save** (to `config.toml` / `rules.toml`) and the chat
**Save key** (to `secrets.toml`); an explicit **Apply** (which reconciles the
graph / re-evaluates the gate exactly as the corresponding CLI path would); and a
Chat turn or **Clear history** (which write only the Chat conversation store
`chat.db`, never the graph); and a **Wiki generation** run (which writes only
generated pages to `wiki.db` via the `wiki write` contract, never the graph) —
all deliberate, acknowledged, and bounded to those enumerated routes. Otherwise
only the explicit `scan` / `hotspots` / `index` / `sync` commands persist; a read
GET writes nothing.

### The Wiki tab

The **Wiki** tab (`/wiki`) renders the source wiki — a durable, agent-authored
documentation layer anchored to graph entities, live-rendered from the deterministic
native tier plus generated prose pages. Its **read** surface is unchanged (page
bodies, nav, and FTS search over `GET /api/v1/wiki*`, all read-only). What Sprint 38
adds is **in-process page generation** ([CR-047](../requests/CR-047-internal-wiki-generation-on-agent-substrate.md)):
opening the tab regenerates drifted pages on the same `rig` agent substrate the Chat
tab uses, replacing the retired headless `claude -p` SessionEnd autogen hook.

Generated pages are **grounded**: for each queue item the binary resolves its
grounding content locally — the referenced `docs/` source(s) for a doc-grounded
page, or a code-graph digest of the anchored symbols for a code-grounded one —
and injects it into the synthesis prompt, so the tool-less agent writes a real
summary of that source rather than free-form prose. The resolution is local I/O
(no extra network egress), and the write-path content-validity guard (above, in
[commands.md](commands.md#wiki-write)) rejects any page that comes back as
agent-noise, so the stored corpus stays trustworthy.

Since CR-062 the tab opens on a **presented** tier first ([ADR-57]): when the
project ships an SRS (`docs/specs/architecture.md` + a requirement — "Case 1"),
the binary runs `wiki materialize` before the LLM queue, presenting the authored
Design/Specs pages deterministically from source (labelled *"presented from
`docs/specs/…`"*, never model prose) and asking the agent to generate only the
Summary/Overview tier. When `docs/howto/` is present, the menu gains a **User
Guide** tier listing the authored guides. Projects without an SRS ("Case 2") keep
the previous behavior — the agent infers Design/Specs from the code graph. Either
way the read surface (bodies, nav, FTS) is unchanged.

**Navigating the presented tier (CR-064).** Sprint 45 shipped the presented tier as
a foundation; Sprint 46 makes it genuinely navigable, so browsing the materialized
wiki reads as a product rather than a page dump:

- **Working cross-references, no dead links.** At `wiki materialize`, a presentation
  transform rewrites each presented page's in-body Markdown links against a
  materialize-time manifest: a link to another presented document resolves to that
  document's wiki page (a same-page target becomes an in-page anchor), a `path.md#frag`
  link resolves to the destination page and its stable section anchor, and a link whose
  target is **not** a presented document renders as plain text (visible text kept, no
  `href`) — so a presented page never 404s. The transform touches only reference
  targets, never prose or code spans, and re-running it on an unchanged source set is
  byte-identical with no LLM/network call.
- **Titled list sections, not filenames.** Each entry on a consolidated page (Components,
  NFRs, ADRs, …) is now headed by the source document's own title with a horizontal rule
  between entries, instead of the bare source filename above a duplicate title. The
  stable section anchor is unchanged, so rewritten cross-links still land correctly.
- **An SRS hub under Specs.** `docs/specs/software-spec.md` is presented as a first-class
  page at slug `specs/srs`, listed at the top of the **Specs** nav tier, so
  `software-spec.md` / `§N` references from other presented pages resolve to it. Only the
  SRS hub is presented — the stray top-level `docs/specs/*.md` analyst inputs are not
  exposed — and the page is never purged by reconciliation while the source exists.
- **Diagrams and doc images render.** Presented `<img>` sources pointing at doc-relative
  images under `docs/specs/**` / `docs/howto/**` are rewritten to a same-origin, read-only
  asset route (`GET /api/v1/wiki/asset/*path`). The route is path-sandboxed
  (canonicalized-prefix containment — `..`, absolute, and symlink escapes are refused),
  serves image content-types only, and adds `X-Content-Type-Options: nosniff`; the
  self-only CSP is unchanged (no `data:` inlining).
- **Search jumps to the match.** Clicking a wiki search result now opens the page and
  scrolls to + highlights the first occurrence of the query term in the body (carried in
  the client navigation state, so a bookmark or menu navigation shows no highlight). The
  FTS backend and read-model are unchanged — this is a presentation-only refinement.
- **Orientation prose written for a reader.** The Overview/Getting-Started generation
  guidance now leads each orientation page with a concrete reader outcome and names the
  actual user-facing commands/workflows, keeping internal symbol/type detail on the
  code-level pages. Existing Overview pages pick up the improved prose the next time the
  connected agent re-runs generation.

Like Chat, generation exists **only** in an `--features agents` build — the
default `logos` binary serves the wiki **view** and parses `[wiki]` as policy but
makes no outbound call and links no HTTP client (the offline invariant holds).
`wiki materialize`,
however, is a **deterministic** presentation step that the default binary *does*
perform (still no LLM/network). Generated and presented pages persist in
`.logos/wiki.db` via the unchanged `wiki write` contract.

**Workflow:**

1. **Configure a provider, key, and (optionally) a wiki model.** Generation reuses
   the `[chat]` provider/`base_url`/API key. Set a **dedicated** `[wiki].model` in
   the Config tab to write the wiki with a model distinct from chat; omit it to fall
   back to `[chat].model`. See
   [configuration.md](configuration.md#wiki--the-source-wiki-generation-model).
2. **Open the Wiki tab.** With no provider configured the tab shows an honest
   **configure-first** state (a link to the Config tab), not an error. When configured,
   opening the tab checks the regeneration work-list; an empty work-list starts **no**
   run and makes no outbound call. The work-list is bounded to the **Overview** pages
   plus **consolidated documentation** categories (dozens of pages, not thousands) —
   per-file and per-module pages are **not** seeded into generation ([CR-056](../requests/CR-056-wiki-generation-usability.md));
   any such pages already in `wiki.db` stay browsable and are still refreshed when their
   anchor drifts, and the native Files view still enumerates every file.
3. **Acknowledge the first-use consent banner.** The first generation in a session is
   preceded by a **consent disclosure** naming the configured **endpoint host** and the
   wiki model — never the key. Nothing is sent until you acknowledge; the acknowledgement
   persists (a distinct `logos.wiki.consent` key, independent of chat consent).
4. **Watch pages stream in — cumulative progress.** When the work-list is non-empty a
   **background** generation run launches under a **single-run lock**: existing (stale)
   pages stay browsable and each page **refreshes in place over SSE** as it completes,
   behind a cumulative **"N of M pages"** indicator that spans the whole run. The run's
   lifetime is **owned server-side, independent of the connection** ([CR-056](../requests/CR-056-wiki-generation-usability.md)) —
   leaving the Wiki tab or losing the connection **no longer aborts it**; the pass keeps
   running and, when it hits its per-chunk page budget with work remaining, **auto-continues**
   into the next chunk until the work-list drains (bounded by a hard safety ceiling that
   halts honestly rather than looping). **Reopening the Wiki tab while a run is in flight
   re-attaches** to that same run and resumes the cumulative progress — it never starts a
   second run or resets the counter. Reopening after the run has finished shows an
   **"up to date"** state.

**Honest states (never a fabricated page):**

- **Configure-first / empty work-list** start no run and make no outbound call.
- A **provider failure** — or a stalled/dead-air provider call, bounded by a per-page
  synthesis timeout — **halts the pass honestly**: pages already written persist, no page
  is fabricated, and the halt reason surfaces to the tab. Once a halt is known the banner
  reads **"Generation halted"** with its reason — never a stale "Generating…" or a false
  "complete". While a page is in flight the tab shows a **visible liveness hint** naming
  the per-page synthesis timeout (a long page reads as *working*, not *stuck*); the hint
  clears the moment that page finishes. A per-page write rejection is **attempted once**,
  recorded, and the run moves on without spinning.
- The API key never appears in any SSE frame, page body, or page provenance field
  (`generator` records the resolved model id, not the key).

**One honest limitation:**

- Automatic generation is **`ui`-only**. Non-`ui` / headless / CI builds have **no**
  automatic wiki regeneration by design (the offline guarantee is preserved); manual
  regeneration remains available via `logos wiki skill --emit` + your own Claude Code,
  and `logos wiki generate` still prints the deterministic queue for a CLI-driven flow.

### The Statistics tab

The **Statistics** tab (`/statistics`, directly above **Config** in Group C)
gives a first-class, in-app view of Logos's value in your development process —
the same local telemetry the `logos stats` CLI reports, now visualized
([CR-058](../requests/CR-058-durable-worktree-telemetry-and-statistics-tab.md),
[FR-UI-27](../specs/requirements/FR-UI-27.md)). It reads the enriched read-model
over `GET /api/v1/statistics[?window=<days>]` — a thin, read-only pass-through of
`Engine::stats(window)` — and renders four surfaces:

1. A **value-estimate callout** leading with the reads/tokens-saved figures (the
   dogfood metric), clearly labeled as *estimates*, not measured truth.
2. A **daily-activity line** — calls per UTC day over the window.
3. A **top-tools / by-surface** ranking bar. The by-surface breakdown covers
   `cli` / `mcp` / `watcher` only — dashboard (`web`) activity is excluded, so
   opening this tab never inflates its own numbers.
4. A **dev-vs-`main` split** — usage attributed by `origin` (a worktree's branch
   name, or `"main"`), charted as-is and **never normalized to total calls**
   (rolled-up days carry no `origin`, so the split can legitimately sum to less
   than the total — the tab does not hide that gap).

A **7 / 30 / 90-day window selector** (default 7) re-queries the endpoint and
re-renders every surface. Each chart is paired with an accessible data-table twin
(WCAG 2.1 AA). Viewing the tab **writes nothing** — it is a read-only GET, no
intent token, same-origin, self-only CSP preserved.

**Honest empty state.** Against a repository with no telemetry history, the tab
renders an explicit "No telemetry recorded yet" awaiting-data state — never
fabricated zeros — and the sidebar item is **muted** (dimmed, with a tooltip) but
still clickable. Because telemetry is repo-global and durable (see
[commands.md § `stats`](commands.md)), the figures include usage recorded from
linked worktrees that have since been removed.

The Statistics view is part of the dashboard, so it is present in the default
build (`ui` is the shipped default, CR-078/ADR-60). Only the slim
`--no-default-features` build has no web surface, so the tab is simply absent
there.
