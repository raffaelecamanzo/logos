# Changelog

All notable changes to Logos are recorded here. This project adheres to
[Semantic Versioning](https://semver.org/).

The dogfood point releases between 0.2.0 and 0.7.6 advanced the self-dogfood pin
without a capability change and were recorded only in `VERSIONS` / commit history;
0.8.0 is the next notable, capability-bearing release.

## [Unreleased]

## [1.0.7] — 2026-07-09

**Sub-second `serve` cold start (CR-077).** The `serve --mcp` filesystem watcher
no longer pre-walks build-output trees at registration time, restoring the
NFR-PE-05 cold-start budget. No schema, reconcile-contract, or public-interface
change; the quality signal is unchanged (delta 0).

### Fixed
- **Watcher registration prunes ignored directories (CR-077, S-285, NFR-PE-05,
  FR-SY-04, FR-SY-11, ADR-48).** `notify-debouncer-full`'s file-ID cache used to
  seed itself by walking the *entire physical tree* on `.watch()` — `target/`
  included (measured ~1.2M entries) — before the server could answer its first MCP
  request, a ~58–93 s stall on every `serve` boot. A custom `PrunedFileIdCache` now
  routes that registration-time seed walk through the same `AdmissionAuthority`
  predicate the full `index` walk and event-time `classify` already share, so it
  visits only admitted source directories. Cold start returns to sub-second
  (measured ~1.7 s cold / ~0.5 s warm on the ~221k-LOC dogfood repo, was ~58–93 s).
  Directory descent is name-pruned (`target`, `node_modules`, `dist`, `build`,
  `vendor`, `.git`, the feedback-loop set, and `[semantics].ignored_dirs`); leaf
  files are gated through the admission authority for full-walk parity; the walk
  degrades to a name-only prune when no authority is available. Rename tracking for
  admitted source paths and event-time `classify` are unchanged — the watcher stays
  best-effort and non-load-bearing for correctness (FR-SY-06, ADR-11).

## [1.0.6] — 2026-07-09

**Reachability & metric-scope precision (CR-073 + CR-074 + CR-075 + CR-076).**
Four independent graph-precision fixes closing standing CRs, all monotonic and
never-fabricate. No schema or reconcile-contract change; the signal-vs-baseline
gate shifts by design (recovered coupling + production-scoped metric values) and is
re-blessed at release via `logos gate --save`.

### Added
- **Optional production-scope filter for the hotspot surface (CR-076, FR-GH-06,
  FR-CV-07).** `logos hotspots --production-scope` (and the MCP `hotspots` tool's
  `production_scope` arg + the web Files & Risk view's "Production files only"
  toggle) drops whole test files (`tests.rs`/`*_tests.rs`/`tests/`) from the
  candidate set *before* ranking, so the `--untested` board surfaces production
  files instead of test files. Opt-in and gate-immune (BR-26): the default board is
  byte-identical and toggling never moves a gated signal. CLI/MCP/web return
  identical rankings for the same state (NFR-CC-01).

### Fixed
- **Trait-object dynamic-dispatch reachability (CR-073 / CR-068 Part C, FR-RS-08,
  ADR-39).** A `&dyn Trait` receiver-method call now fans out to the trait's default
  body ∪ its concrete workspace impls via real `Calls` edges when the receiver is a
  *provable* trait object (explicit `&dyn T`/`Box<dyn T>`/`Arc<dyn T + …>` binding),
  and the dispatch live-rooting pass now roots trait-default bodies. The six
  `LanguagePlugin` trait methods reached only through `dyn` dispatch stop reading as
  dead (dead-callable census down); the CR-066 receiver-method guard (FR-RS-06) is
  not loosened and never-fabricate (NFR-RA-05) is preserved — a non-provable receiver
  is an honest miss, not a guess. A new (previously-unemitted) `Implements` edge is
  fenced out of every metric subgraph, so only the intended dead-code recovery moves.
- **Redundancy budgets production-scoped (CR-074, FR-QM-08).** `check_redundancy`'s
  `max_dead`/`max_duplicates` budgets now exclude `is_test` functions via the shared
  `test_node_ids` set, matching the Redundancy metric (FR-QM-05) and the sibling
  fan/structural budgets. `max_dead` is behaviour-preserved; `max_duplicates` drops
  by the excluded test-duplicate count.
- **Plural Rust test-file conventions recognized in `is_test` detection (CR-075,
  FR-AN-05).** The shared `is_test_path` helper now matches bare `tests.rs` and the
  snake_case `*_tests.rs` suffix (it already matched CamelCase `*Tests`). Non-`#[test]`
  helpers in those files are now correctly `is_test=true`, removing test-code
  contamination from every production-scoped metric via the single shared column; all
  consumers correct in lock-step with no per-consumer edits.

## [1.0.5] — 2026-07-08

**Graph-precision & config-surfacing cleanup (CR-071 + CR-068 Part B + CR-067).**
Three independent precision/UX fixes closing standing CRs, all additive/read-model.
No schema, reconcile-contract, or gate-baseline change.

### Added
- **Config parameter defaults surfaced in the web Config editor (CR-067, FR-UI-12).**
  `GET /api/v1/config` gained a read-model `defaults` projection — code-sourced and
  computed independently of the live documents. The Config editor now renders a
  `Default: …` hint beside every `config.toml`/`[metric_thresholds]` field and
  `unset → not enforced · Recommended: …` beside every `[constraints]` field. The
  save/apply/validate path is byte-identical, and the chat API key is never present
  in the projection (NFR-SE-07).

### Fixed
- **Sanctioned external-docs symlink now followed on git-ignoring checkouts (CR-071,
  FR-IX-10, ADR-59).** Resolves the CR-069/1.0.4 known limitation: the discovery walk
  built its `WalkBuilder` with git-ignore filtering upstream of the `.swe-skills`
  follow-branch, so a repo whose `docs/{specs,planning,requests}` symlinks are
  git-ignored indexed **zero** doc nodes. A dedicated git-ignore-bypassing detection
  pass — confined to the documentation subtree, one-hop, containment-gated, sanctioned
  root only — now follows the symlink and indexes the docs behind it. Admission parity
  (`admits_path`, ADR-48) mirrors the carve-out so `doctor`'s admission tripwire does
  not flag the freshly-indexed docs as drift. Source-code symlinks are still skipped
  wholesale; an escaping/unsanctioned target is refused, not followed.
- **Associated-function `Method` kinding and bare-path binder tie-break (CR-068 Part B,
  FR-EX-05, FR-RS-07, ADR-39).** Rust `impl`-nested associated functions are now kinded
  `NodeKind::Method` (distinct from free `NodeKind::Function`) at emission — symbol IDs
  and ordinals byte-identical (NFR-RA-06). In the binder, a single-segment bare-path
  call now prefers a free function over same-named associated methods (a monotonic
  tie-break, never a drop; the full callable set stands when no free function exists).
  On the Rust dogfood this recovers the `graph_store/mod.rs` `insert_node`/`insert_edge`/
  `upsert_symbol` cluster: resolved `Calls` edges **3694 → 3716 (+22)** with **0 lost**
  and **0** live callable turned dead (BR-38 monotonic). Receiver-method calls and the
  CR-066 unique-name fallback are untouched.

### Added (diagnostics)
- **`doctor --json` `doc_symlink_warnings` (CR-071, FR-IX-11).** A new advisory array
  naming any documentation directory-symlink that exists under the doc-include set but
  ended up unindexed (no sanctioned root, or target escapes containment). Purely
  diagnostic — it never flips `ok` or changes the exit status. `index`/`sync` fold the
  same drops into their warnings.

## [1.0.4] — 2026-07-08

**Binding precision & discovery fidelity (CR-068 + CR-069 + CR-070).** Continues the
measurement-precision arc into *binding & discovery* precision, with the same
never-fabricate, monotonic, honest discipline. No schema, reconcile-contract, or
gate-baseline change.

### Added
- **Function-pointer handoffs recognized as live roots (CR-068 Part A, FR-AN-01,
  ADR-39).** The framework-dispatch pass now recognizes axum function-pointer
  handoffs — `.fallback(fn)`, `middleware::from_fn(fn)` / `from_fn_with_state(_, fn)`,
  and every method-router handler inside `route(path, get(fn)|post(fn)|…)` including
  chained setters (`get(a).post(b)`) — as live roots via the existing `RoutesTo`
  self-marker. A handler name is bound only to the one same-file callable of that
  name (exactly-one-or-nothing), so nothing is fabricated (NFR-RA-05). On the Rust
  dogfood this drops false-positive dead functions **61 → 49** (12 `web/src/lib.rs`
  handlers/guards/fallbacks recovered) with **zero** previously-resolved `Calls`
  edges lost and **no** live function turned dead (BR-38 monotonic); a full re-index
  is required to observe the new liveness (an incremental `sync` over unchanged files
  keeps the prior markers). The `name_matcher` is untouched (the CR-066 fallback is
  not loosened).

### Removed
- **PostToolUse wiki-augmentation hook retired (CR-070, FR-WK-14).** The advisory
  augmentation hook — which surfaced the `wiki generate` work-list to the connected
  agent on every tool call — is deleted from the binary (`AUGMENT_SPEC`, the augment
  script/constants, and the augment `materialize()` entry point are gone). `logos
  init -i` and `logos wiki hook --emit [--force]` now install/emit only the SessionEnd
  quality-report hook (FR-IN-07); `wiki hook --emit --json` consequently returns a
  single summary object rather than a two-element array. The deterministic `wiki
  generate` queue and the `ui`-gated in-process generator (FR-WK-18) are unchanged.

### Known limitations
- **External docs-root symlink following (CR-069, FR-IX-10) landed but is inert on
  checkouts that git-ignore the doc symlinks.** The discovery carve-out that follows
  a `.swe-skills`-sanctioned docs symlink is correct in isolation, but the walk's
  git-ignore/exclude filtering runs upstream of it, so on a repo whose `docs/{specs,
  requests}` symlinks are git-ignored the doc nodes are not indexed. Resolving whether
  a sanctioned docs root should override its own git-ignore is a discovery-contract
  decision deferred to a follow-up CR (amending FR-IX-10 / ADR-59 / NFR-SE-04).

## [1.0.3] — 2026-07-07

**Graph measurement precision (CR-065 + CR-066).** Two core graph measurements
are re-based to reflect genuine structure rather than measurement artifacts, with
no change to the tool surface.

### Changed
- **Module-grain, production-scoped coupling budgets (CR-065, FR-GV-11, FR-QM-08).**
  `check_coupling`'s `max_fan_in` / `max_fan_out` now count each *module's* distinct
  neighbouring modules over the canonical dependency view (reusing the module rollup
  that backs `logos dsm`), excluding `is_test` nodes before the rollup. A shared
  helper called from many symbols in one module counts that module once, not once
  per call site — so the budget flags genuine cross-module coupling, not the name
  popularity of standard-library method names. Deterministic, module-key-ordered
  output (NFR-RA-06); the coupling budget stays a `check_rules`-only budget,
  orthogonal to the quality-metric gate signal (ADR-21).
- **Receiver-unqualified method binding no longer fabricates `Calls` edges (CR-066,
  FR-RS-06, FR-RS-03, NFR-RA-05).** A bare `x.f()` method call now resolves only on
  genuine lexical/module scope evidence; the workspace unique-name and suffix
  fallbacks are gated off for the receiver-method form, so a `.map()`/`.collect()`/
  `.join()`/`.path()` with no in-scope target stays in `unresolved_refs` and retries
  on sync instead of binding to a same-named callable in an unrelated module. Removes
  the ~29.5% of `Calls` edges that funnelled into ~15 std-method-named targets
  (~1065 edges / −21.3% on the dogfood self-graph). Path-qualified and typed calls,
  and free-call resolution across C++/Java/Kotlin/Scala/C#, are unchanged. The
  downstream dead-code/cycles signal shift is re-blessed deliberately at release per
  FR-GV-16, with CR-066 as the accepting authority.

## [1.0.1] — 2026-07-06

**Trustworthy wiki reframe (CR-062).** The source wiki gains a third tier: the
binary now *presents* the project's authored `docs/specs/**` and `docs/howto/**`
verbatim instead of paraphrasing them with a model, reserving LLM inference for
the Summary/Overview tier alone. This retires the per-file wiki pages that added
volume without traceable provenance, so the corpus converges on three canonical
tiers (extracted / presented / generated) — see [ADR-57].

### Added
- **Deterministic presented tier — `logos wiki materialize` (FR-WK-20, ADR-57).**
  A pure, offline, idempotent command assembles one wiki page per Design/Specs
  category (section-per-source-file) directly from the authored SRS sources,
  labelled `generator = logos:doc-present` ("Presented verbatim … Not
  model-generated") — copied verbatim, never paraphrased. No LLM, no network.
  `materialize` has a payload-identical MCP twin (`wiki_materialize`), bringing
  the wiki tool set to five (`write`/`read`/`search`/`status`/`materialize`).
- **User Guide tier (FR-WK-23).** `wiki materialize` also presents one
  `guide/<name>` page per `docs/howto/*.md` (e.g. `guide/overview` ← `README.md`),
  so the rendered manual is copied verbatim from source rather than regenerated.
- **SRS-mode bimodal generation gate (FR-WK-21).** When the project ships an SRS
  (`docs/specs/architecture.md` + a requirement), the Design/Specs and User
  Guide pages are presented and the connected agent generates only the
  Summary/Overview tier; otherwise it infers the full set from the code graph.

### Changed
- **User-needs-aware Overview generation (FR-WK-24).** The generated
  Summary/Overview pages are grounded in `README.md` + `docs/howto/**` and
  prompted to read as user-facing (goals/workflows), not a symbol tour.

### Removed
- **Per-file wiki pages retired (FR-WK-22).** The `files/*` pages are excluded
  from the generation work-list and from the status page count, and a
  reconciliation sweep purges unreachable orphan pages so a previously bloated
  corpus self-prunes to the three canonical tiers.

## [1.0.0] — 2026-07-05

**First stable release.** Logos reaches 1.0.0 with a trustworthy source-wiki
generation pipeline (Sprint 44 — CR-059) on top of the structural code-graph,
architecture-quality gate, agentic chat, and single-binary web UI shipped across
the 0.x line. The command surface, `.logos/` store layout, MCP tool set, and CLI
JSON contracts are now considered stable under [Semantic
Versioning](https://semver.org/).

### Added
- **Grounded wiki generation (FR-WK-18, ADR-51).** Each queue item's grounding
  content — the referenced `docs/` source read, or a token-bounded code-graph
  digest as fallback — is resolved in-binary and injected into the synthesis
  prompt, so the tool-less generator writes page prose only from supplied
  context instead of hallucinating or emitting planning noise. Generation stays
  offline; no network egress on the grounding path (NFR-SE-01).
- **Write-path content-validity guard (FR-WK-19, NFR-CC-04).** The shared
  `wiki write` façade — CLI stdin/`--body-file` and the in-process generator
  alike — rejects a body that is agent-noise rather than page prose (a
  `<tool_call>` token, an `Error:`/`cmd:` transcript, a first-person
  planning/refusal preamble, no Markdown heading, or below a minimum length). A
  rejected write leaves the store byte-identical and is reported as an honest
  per-page failure, never a fabricated page. The two noise-content signatures
  are scanned with fenced code blocks stripped, so a page that legitimately
  *quotes* one of those patterns inside a ` ``` `/`~~~` fence is accepted.
- **Wiki run-state legibility (S-239).** The web Wiki tab shows cumulative
  "N of M" progress and a visible per-page synthesis-timeout hint; a halted run
  reads "Generation halted" rather than a stale "Generating…" or a false
  "complete", and a drained work-list launches no redundant run.

### Changed
- **Corpus regeneration under the fixed pipeline (S-238).** Regeneration drives
  grounding + guard together and purges orphaned pages, so a previously
  corrupted corpus is replaced with grounded, guard-checked prose.
- **Statistics "Dev vs main" card aggregates worktree branches (FR-OB-08).** The
  origin split (`calls_by_origin`, surfaced in the web Statistics tab and `logos stats`)
  now collapses every non-`main` origin into a single cumulative `"dev"` bucket, so the
  card is a two-way comparison of all development-increment work versus `main` — instead
  of one bar per (often stale) worktree branch, which added noise over a wide window. The
  stored per-event `origin` is unchanged; only the read-model aggregation changed.

## [0.14.0] — 2026-07-04

**Recoverable-fault degradation across the agent substrate (Sprint 43 — CR-060).**
A single recoverable subagent fault (a transient provider hiccup, a missing-path tool
error) no longer kills the whole chat turn. The runtime now recovers at three layers —
transparent provider retry, tool-errors-as-observations, and cross-step degradation —
so the turn continues and returns a best-effort grounded answer.

### Added
- **Provider-call retry with backoff (S-240).** A transparent `RetryingModel` decorator
  over rig's `CompletionModel`, wired into both provider constructors, retries retryable
  faults (transport, 429, 5xx, deserialization hiccups) with exponential backoff + jitter;
  `Auth` and other terminal errors are never retried, and exhaustion returns the original
  error. Two new `[chat]` keys — `max_provider_retries` (default `2`, `0` disables) and
  `provider_retry_base_ms` (default `200`, `0` rejected at load) — are inherited by the
  wiki-agent.
- **Tool errors as self-correcting observations (S-241).** In `run_tool_subagent`, tool
  errors and out-of-domain requests become model-visible `tool_result` observations so a
  subagent adapts instead of dying, bounded by a consecutive-error soft-close cap
  (`CloseReason::ToolErrors`) that closes well-formed with a `[bounded — …consecutive tool
  errors…]` marker; a success resets the streak.
- **Cross-step fault degradation (S-242).** A new `StepError::Unavailable` reclassifies the
  three recoverable roster fault sites; a step that stays unavailable after retries degrades
  to a `[unavailable — …]` observation and the turn continues, answering best-effort when any
  usable observation exists (and halting honestly when none do). Synthesizer/structural faults
  stay turn-fatal; sustained outage terminates via `max_replans`.

### Changed
- Recoverable subagent faults are now degradation events, not turn-fatal errors — extending the
  bounded-degradation posture established in 0.13.0 (CR-048) from budget exhaustion to provider
  and tool faults.

## [0.13.0] — 2026-07-04

**Bounded, graceful degradation of generative work (Sprint 42 — CR-048 + CR-044).**
The agent budget tree stops being a hard tripwire and becomes a soft, self-summarizing
bound; subagent preambles are budget-aware; wiki regeneration cadence is dampened.

### Changed
- **Soft per-subagent budget cap (S-181).** A subagent that reaches its per-subagent
  tool-call cap no longer hard-halts the turn — it closes well-formed, summarizes its
  findings tool-free, and returns a marked `[bounded — …cap…]` observation. Only the
  global `max_tool_calls` ceiling and `max_replans` hard-halt; on a hard halt the
  orchestrator returns a best-effort grounded answer over the scratchpad (or an honest
  bare halt when nothing was gathered) instead of an error.
- **Budget-aware subagent preambles (S-182).** Each tool-bearing subagent's preamble
  names its cap and running "calls remaining", steering it to prefer the
  breadth-efficient `context` tool; the preamble is rebuilt from the live budget at
  every model round, including the soft-close step.

### Added
- **`[wiki].revision_stale_threshold` (S-164).** A new config key (default `5`, min `1`,
  `0` rejected at load) dampens the re-queue cadence of anchorless prose wiki pages.
  `revision_stale_count` stays truthful; the regeneration queue stays a pure offline read.

## [0.12.1] — 2026-07-04

**Web-UI polish (dogfood fixes).** Three rendering/aesthetic fixes surfaced while
dogfooding 0.12.0; no behavior or API change.

### Fixed
- **Files & Risk section spacing.** `FilesView` returned bare fragments, so the
  hotspot Callout and the risk/ownership Cards stacked flush; it now uses the shared
  `.view` section-stack (`gap: var(--space-5)`) like the Coverage/Quadrant tabs.
- **Wiki search form.** The hand-rolled raw `<input>` (TOC-heading label voice, no
  label↔control spacing, browser-default ~20-char width) is replaced by the shared
  `TextField`: a real form label, `var(--space-2)` spacing, and a full-width input.
- **Mermaid diagram legibility.** Under the self-only CSP, Mermaid's injected style
  is stripped, leaving arrowhead markers on the SVG default black fill and edges at
  an uncontrolled weight. The external CSS now sets a light edge `stroke-width` and
  reliably re-colors the arrowhead markers to `--text-2`, so diagrams read cleanly.

## [0.12.0] — 2026-07-03

**Wiki generation, made usable end-to-end (CR-056).** The in-process wiki
generation run no longer floods its work-list with per-file pages, survives a
dropped SPA connection, and reports honest cumulative progress on re-attach.

### Changed
- **Pruned generation work-list (S-221).** `status()` / `structured_sections()` /
  `generation_queue()` no longer seed per-file `objectives` pages or unanchored
  File/Module entities — the cold-start queue collapses from ~1600 to dozens on
  this repo. Existing `wiki.db` pages are still served and refreshed on drift;
  determinism (NFR-RA-06) preserved.
- **Connection-resilient, auto-continuing run (S-222).** The run's lifetime is
  owned in app state rather than the SSE response body, so dropping the stream no
  longer aborts generation — it auto-continues across budget chunks until the
  work-list drains, bounded by a hard safety ceiling. A per-page synthesis timeout
  prevents a hung provider from orphaning the single-run lock.
- **Wiki-tab re-attach + cumulative progress (S-223).** Reopening the tab mid-run
  subscribes to the live run (exactly-once delivery) and shows a whole-run
  cumulative "N of M", not a per-chunk reset; reopening after completion reads
  "up to date" and starts no second run.

### Added
- **Typed `[wiki].model` field in the Config tab (S-224).** The wiki synthesis
  model is editable in the UI; a valid save round-trips through an atomic
  write-back, an invalid document is rejected inline with no partial write, and an
  absent `[wiki]` section renders blank rather than crashing.

## [0.11.0] — 2026-07-03

**Durable telemetry + Statistics tab (CR-058).** Logos tool usage is recorded in a
store that survives worktree teardown, and the dashboard surfaces it.

### Added
- **Durable, shared-primary telemetry store.** Tool invocations are persisted to a
  telemetry store rooted in the git common directory (surviving worktree teardown),
  stamped with an `origin`, and queryable via `logos stats`.
- **Statistics dashboard tab.** The web UI exposes recorded tool usage.

### Fixed
- **HF-1:** web-UI-originated activity is excluded from the Statistics read-model so
  the dashboard reflects agent/CLI usage rather than its own rendering.

## [0.10.0] — 2026-07-03

**Cold-index performance — measure-first de-serialization of the write path (CR-057).**
A full cold index is materially faster with byte-identical graph output. Nothing
about the produced graph changes; only how fast it is built.

### Added
- **Per-phase index instrumentation (S-225).** `logos --json index` now carries a
  `phases` object with per-phase wall-clock durations (`discover`, `load`, `extract`,
  `persist`, `resolve`, `framework`, `dispatch`, `annotate`), derived from the same
  `tracing` seam as the logs and reconciling to `total_ms`. A repeatable cold-index
  benchmark (`cold_index_phase_baseline`) records total + per-phase + peak RSS.

### Changed
- **Parallel annotation compute (S-229).** Near-clone clustering (the measured ~83%
  of the annotate phase) and the per-node verdict loop now run on the shared worker
  pool via keyspace-sharded pair counting — **annotate −54% (2.2×)**, byte-identical
  across every worker count, peak RSS held under the 1 GB ceiling.
- **Chunked Pass-1 persistence (S-226).** The per-file commit storm collapses into
  bounded write batches (≈958 → ≈4 transactions on this repo), single-writer
  invariant preserved.
- **Parallel file-load and discovery walk (S-228).** Read+hash and the directory
  walk fan out on the shared pool; order-deterministic, byte-identical.
- **Writer bulk-load pragmas (S-227).** The writer connection sets `cache_size` /
  `mmap_size` / `temp_store` for the index write window; the reader pool is untouched.

Overall cold-index total is down ~25% on a real repo, driven by S-229 and S-226.
Full analysis: `docs/perf/cold-index-0.10.0.md`.

## [0.9.8] — 2026-07-02

**Web-UI packaging hardening — no more silent white page (CR-049 follow-up).** A
`--features ui` binary built without a matching `npm run build` no longer serves a
blank page. This is a build/packaging robustness fix; the offline default binary is
unaffected.

### Fixed
- **Hash-free committed placeholder (`web/ui/dist/index.html`).** An earlier revision
  committed a real Vite build's `index.html` as the "placeholder", so its frozen
  `/assets/index-<hash>.js` reference never matched any fresh build — embedding it
  produced a `200` shell whose JS bundle `404`'d (a blank page). The placeholder is
  now a genuine hash-free stub that references no build artifacts and shows a visible
  "UI bundle not built — run `npm run build`" message, so it can never create the
  hash mismatch. This also fixes `web/tests/spa_shell.rs`'s
  `every_shell_referenced_asset_resolves_through_the_router`, which was red at HEAD.

### Added
- **Serve-time SPA consistency guard (`web`).** When the embedded shell references an
  `/assets/*.{js,css}` the binary does not embed, `/` (and the history fallback) now
  return a `503` self-describing diagnostic naming the missing assets and the rebuild
  recipe, instead of the silent white page. Detection is a pure ref-extractor over
  the embedded `index.html`; the happy path (consistent bundle) is unchanged.

## [0.9.7] — 2026-07-02

**Internalized wiki generation (CR-047).** Source-wiki prose generation moves
**in-process** onto the shared `rig` agent substrate the Chat tab already uses,
replacing the out-of-process headless `claude -p` SessionEnd autogen hook. The
default (offline) binary is unchanged and remains provably offline — the wiki
generator, like chat, compiles only under `--features ui`.

### Added
- **Dedicated `[wiki].model` config (S-176, FR-CF-07).** An optional `[wiki]`
  section selects a wiki-generation model distinct from `[chat].model`, inheriting
  `provider` / `base_url` / the `secrets.toml` key from `[chat]` (no separate wiki
  provider, endpoint, or secret). Omit it to fall back to the chat model.
- **In-process wiki-agent (S-177, FR-WK-18).** A new `ui`-gated `wiki-agent` crate:
  a single-purpose `rig` agent on the shared `agent-core` substrate that loops the
  deterministic `wiki generate` queue, uses the embedded `logos-wiki` skill body as
  its system prompt, and persists pages via the unchanged `wiki write` contract. No
  planner/subagent roster; a per-run budget bounds the pass.
- **Wiki-tab generation trigger (S-178, FR-WK-18, NFR-SE-07).** Opening the Wiki tab
  launches a background generation run under a single-run lock, renders existing
  pages immediately, and streams per-page refreshes over a same-origin SSE endpoint
  (`POST /wiki/generate`). The first outbound call in a session is gated by a
  first-use consent disclosure naming the configured endpoint; an unconfigured
  provider shows a configure-first state, not an error.

### Changed
- **Retired the headless `claude -p` wiki-autogen hook (S-179, FR-WK-16 → CR-047).**
  `logos init -i` no longer installs the SessionEnd autogen hook or its
  `.claude/settings.local.json` materialization. The embedded `logos-wiki` skill is
  now the wiki-agent's system prompt (single source of guidance) and is still
  materialized via `logos wiki skill --emit` for manual regeneration. The advisory
  PostToolUse augmentation hook is unchanged.

### Verified
- **Offline carve-out regression + UAT (S-180, UAT-WK-06, NFR-SE-01).** The default
  build links no HTTP client (no-networking-crate fitness test byte-identical); the
  full configured→open→consent→regenerate→stream→dual-axis-fresh flow is exercised
  end-to-end via the mock `CompletionModel` with zero real egress and a loopback bind.

## [0.9.6] — 2026-07-02

**Standalone quality integration (CR-055).** The full quality loop —
**freshen / enforce / report / bless** — is now a first-class capability any
adopter gets from `logos init`, decoupled from the swe-skills harness (ADR-49).
The existing `check` / `scan` / `gate` commands are unchanged; two new
`init`-installed triggers plus a documented CI recipe wire them into everyday
workflow.

### Added
- **Enforcing `pre-push` git gate (S-218, FR-IN-06).** `logos init --hooks` now
  installs a fourth, **blocking** git hook alongside the exit-0 freshness hooks:
  a `pre-push` gate that runs `logos check` and **propagates its non-zero exit**,
  so a rule / structural / admission / dead-code regression makes `git push` fail
  (exit 1) and names the offending contract. It bails **open** (exit 0) when the
  `logos` binary is absent — never a false block — carries the managed marker, and
  is bypassed by `git push --no-verify`.
- **Harness-agnostic Claude Code SessionEnd quality-report hook (S-219,
  FR-IN-07).** `logos init -i` now registers a non-blocking SessionEnd hook that
  runs `logos check` + `logos scan`, prints the current signal, the baseline
  signal, and any violations to the terminal, and **always exits 0** (never blocks
  session teardown). Disable it without uninstalling via the
  `LOGOS_QUALITY_REPORT_DISABLE=1` off-switch (mirroring the wiki-autogen hook).
- **CI recipe and workflow-integration docs (S-220).** New
  [`docs/howto/ci-integration.md`](docs/howto/ci-integration.md) documents the
  freshen / enforce / report / bless model and ships a copy-pasteable CI recipe —
  `logos check` as the enforcing build step, `logos scan --json` as the
  non-blocking signal report, and `logos gate --save` blessing a new baseline **at
  release only, never in the PR path**. Cross-linked from the how-to README,
  usage, and the `init` command reference; `error-handling.md` documents the
  `pre-push` exit-1 contract.

## [0.8.0] — 2026-06-27

**Agentic Chat.** A new `ui`-gated Chat tab brings an orchestrated LLM agent over
the code graph to the localhost dashboard (Sprint 30, CR-045 + CR-046).

### Added
- **Agentic chat orchestrator** — an LLM planner runs a plan→act→observe→replan
  loop over a fixed roster of four specialized subagents (Graph-Navigator,
  Governance-Analyst, Source-Reader, and a tool-less Synthesizer), each driven at
  the completion-model level through a least-privilege bounded tool dispatcher, all
  bounded by a budget tree (global tool-call ceiling / per-subagent cap / max
  replans) that halts honestly rather than fabricating an answer.
- **Chat tab UI** — a consent-gated composer that streams the plan, live
  subagent-activity chips, and the final answer over Server-Sent Events; the SSE
  rides the intent-guarded `POST /chat` (a `GET` `EventSource` cannot carry the
  CSRF intent header) under the unchanged self-only CSP, with a no-JS buffered
  fallback and Clear-history.
- **Token-by-token answer streaming** — the Synthesizer's answer now types out live,
  token by token (`answer_delta` SSE events), reconciling to the authoritative final
  answer when the turn completes.
- **`[chat]` configuration** — provider (Anthropic native / OpenAI-compatible,
  default OpenRouter), model, budget-tree params, and sampling, with the API key in
  a `0600` `secrets.toml` that is masked and never echoed. The Config tab gives the
  provider, model, and base_url their own typed controls (a provider select + a
  model input + a base_url input, patched into the validated raw-TOML candidate
  like every other typed field, in a full-width `[chat]` fieldset), so the settings
  that gate whether Chat is usable — and the endpoint it talks to — are discoverable
  rather than hidden in the raw pane; the remaining `[chat]` keys stay in the raw pane.
- **Multi-step agent memory** — per-thread scratchpad + working memory in
  `.logos/chat.db`, with the Synthesizer grounded on the persisted scratchpad.

### Security / carve-out
- The entire chat stack is gated behind the non-default `ui` feature: the default
  binary links **no** networking or LLM crate and stays byte-identical to 0.7.6
  (the `no_network_deps` fitness function and three further carve-out guards hold).
  The first and only outbound egress is the explicit, consent-gated chat turn.

## [0.2.0] — 2026-06-14

Language-breadth release — the first 0.x increment to carry new capability
rather than a pure dogfood re-pin. The default binary now indexes **twelve**
programming languages out of the box (up from five), and three resolution/config
correctness fixes land. The self-dogfood pin advances to this version.

- **Feature — seven more languages in the default build (CR-009).** Kotlin, C,
  C#, C++, Ruby, PHP, and Scala join the out-of-the-box code-language set, taking
  it from five to twelve (`logos languages` now lists 24 grammar rows including
  artifacts). Each ships as pure plugin data — one grammar crate, one feature
  line, one `plugins/<lang>/` descriptor — riding the existing plugin substrate
  and the load-time ABI assertion (ADR-09) without touching core extraction logic
  (NFR-MA-01). The stripped default binary stays within the NFR-PC-04 ≤ 50 MB
  budget. The NFR-PE-05 cold-start budget is revised 200 → 500 ms to reflect the
  larger grammar set compiled at registry load.

- **Fix — reconcile-purge demotes inbound references (CR-017 Defect A).** When a
  config change narrows the indexed set and purges a file, references from
  still-indexed code that resolved to the purged file are now correctly returned
  to unresolved instead of keeping a stale resolved row (NFR-RA-05 honesty).
  Surfaced by 0.1.2's incremental resolution, whose change-delta did not see the
  out-of-band purge.

- **Fix — OpenAPI operations bind to their framework routes (CR-017 Defect B).**
  An `ApiOperation` reference now resolves to the route node the framework pass
  promotes, producing the operation → route edges that previously came out empty
  on a no-op sync. Same incremental-resolution interaction as Defect A: the route
  is promoted after the resolve pass, so a focused re-resolve over the newly
  promoted names is now run.

- **Fix — the `languages` config field gates indexing (CR-017 Defect C).**
  Previously inert (it fed only the admission fingerprint), `languages` now
  restricts which code grammars are admitted: omitted or empty means all
  compiled-in languages (preserving the twelve-language default), a non-empty
  list narrows, and narrowing purges the dropped languages' files.

## [0.1.2] — 2026-06-13

Dogfooding bugfix release. Re-pins the self-dogfood binary. Two resolution-engine
performance fixes that, together with 0.1.1's watcher-exclusion fix, eliminate the
CPU melt that had forced dev-pane Logos off.

- **Fix — `serve --mcp` whole-ledger re-resolution under churn (CR-015).** The
  watcher-fired `Engine::sync` re-bound the *entire* reference ledger (~40k rows)
  on every sync via an all-core parallel pass, even when nothing relevant changed
  — N concurrent panes saturated the machine. Resolution is now **incremental**:
  a sync re-binds only the change-affected rows (the changed files' rows plus the
  untouched rows whose target token a change moved), and the watcher coalesces
  bursts via a settle window + rate-limit floor + staleness cap. A no-op sync on
  the self-graph drops ~152 s → ~1.6 s. Guarded by a sync≡reindex equivalence net.

- **Fix — cold `logos index` exponential glob resolution (CR-016).** The binder's
  `through_globs` resolved each glob import's own module path by re-entering
  `through_globs`, so a file with `G` glob imports did `O(G^8)` work per reference;
  a file with 14 `use super::*` imports drove a single bind to ~1.5e9 operations
  (~150 s) and, in parallel, pegged every core. A re-entrancy guard collapses this
  to `O(G)`. Cold self-index ~7 min → **3.8 s**, restoring the NFR-PE-02 budget.
  Edge output is byte-identical (verified by the equivalence net + exhaustive glob
  classification).

## [0.1.1] — 2026-06-12

Dogfooding bugfix release. Re-pins the self-dogfood binary.

- **Fix — `serve --mcp` watcher CPU storm.** The hosted filesystem watcher
  excluded only `.logos`/`.git`, so build-output churn under indexer-ignored
  directories (`target/`, `node_modules/`, `dist/`, `build/`, `vendor/`) flooded
  the debounced sync worker — a single `cargo build` drove `Engine::sync` across
  every core, and N concurrent dev panes saturated the machine. The watcher now
  drops events under the same `ignored_dirs` set the indexer prunes (unioned with
  the always-excluded internal dirs, so feedback-loop containment is not
  configurable away). The watched set now matches what indexing admits; real
  source edits still sync. Regression-tested in `logos-core/src/watch`.

## [0.1.0] — 2026-06-11

First tagged release and the baseline pin for self-dogfooding.

Logos at 0.1.0 is a single static binary providing structural code intelligence
for AI-assisted development: deterministic, offline, never-fabricating. Headless
CLI plus a stdio MCP server (`logos serve --mcp`).

Capabilities in this release:

- **Code graph** — multi-language extraction (Rust, Python, TypeScript, Go,
  Java, Kotlin, Swift, Ruby, PHP, C#, C/C++, Scala, …), SCIP-conformant data
  model, resolution, annotation, and the eight navigation tools
  (`search`/`query`/`context`/`callers`/`callees`/`impact`/`affected`/`explore`).
- **Quality metrics & governance** — full architecture-quality `scan`, the
  versioned-baseline `gate` (CI regression check), `check` against `rules.toml`,
  test-aware and production-scope metrics.
- **Extended analytics** — structural metrics and near-clone detection,
  git-history temporal metrics and `hotspots`, external `coverage` ingestion,
  `dsm`, `evolution`, `test-gaps`, `doc-gaps`.
- **Documentation graph** — markdown doc nodes, doc↔code link resolution, and
  doc-aware traceability (`implements`, `referencing-docs`).
- **Config & artifact graph layer** — an `artifact = true` plugin class over 10
  artifact grammars (YAML/JSON/TOML, Dockerfile/Makefile/Shell,
  Protobuf/GraphQL, Terraform/SQL) with content-sniffed OpenAPI promotion;
  metric-neutral by construction.
- **Setup & integration** — `logos init` (self-contained: `.logos/` config +
  rules, optional git hooks for automatic freshness, `.mcp.json` for any MCP
  host).

[0.2.0]: https://github.com/ — local tag `v0.2.0`
[0.1.2]: https://github.com/ — local tag `v0.1.2`
[0.1.1]: https://github.com/ — local tag `v0.1.1`
[0.1.0]: https://github.com/ — local tag `v0.1.0`
