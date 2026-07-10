//! `logos-core` — the thick-core engine crate (ADR-01).
//!
//! All business logic lives here. The two thin adapter crates (`cli`, `mcp`)
//! call [`Engine`] methods exclusively; they carry no logic of their own
//! (NFR-MA-02: adapters ≤ 200 non-blank LOC each).
//!
//! # Architecture
//! - [`Engine`] is the sole instrumented chokepoint (NFR-CC-01, ADR-01).
//! - Every method returns a [`serde::Serialize`] read-model defined in [`models`].
//! - Quality/governance methods return `Result` (ADR-14 fail-loud on
//!   structural faults); the typed [`CoreError`](error::CoreError) enum and its
//!   [`Severity`](error::Severity) classification carry the fail-soft /
//!   fail-loud contract (S-026).

/// Pass 3 of the pipeline (S-014): the annotation engine — dead-code via
/// exported-is-live reachability (FR-AN-01), duplicate detection over the
/// Pass-1 AST-shape fingerprints (FR-AN-02), and `rules.toml` layer membership
/// with derived `layer`/`boundary` policy nodes and `forbidden_dependency`
/// edges (FR-AN-03) — all written to native node columns (FR-AN-04).
pub mod annotate;
/// Configuration loading and validation (S-006): the `config.toml`/`rules.toml`
/// read-models ([`Config`](config::Config) / [`Rules`](config::Rules)) and the
/// contained, gitignore-aware discovery walk ([`config::discover`]).
pub mod config;
pub mod engine;
/// The typed core error contract (S-026, ADR-14): the [`CoreError`](error::CoreError)
/// enum and its [`Severity`](error::Severity) classification — fail-soft-where-safe
/// (Degraded: warn + continue) vs fail-loud-on-correctness (Correctness: abort) —
/// plus the [`exit_code`](error::exit_code)/[`classify`](error::classify) surface
/// mappers both adapters share so the boundary is decided once (FR-EH-01, FR-EH-02).
pub mod error;
/// Pass 1 of the pipeline (S-007): the rayon data-parallel extraction engine
/// that turns source files into `NodeFact`/`EdgeFact`s with canonical-ordinal
/// SCIP IDs, cyclomatic complexity, and per-function line counts (ADR-07,
/// ADR-08). One `tree_sitter::Parser` per rayon worker (AR-05).
pub mod extract;
/// Workspace **federation** (S-243, CR-061, ADR-52): the in-memory overlay that
/// turns a parent folder of sibling repositories into one queryable workspace —
/// `logos.workspace.toml` manifest parse + the up-tree `discover` walk that
/// resolves and validates the member set (FR-WS-01). No manifest → single-root,
/// byte-for-byte unchanged. Never a graph union, never persisted (ADR-52).
pub mod federation;
/// The governance engine (S-020): `rules.toml` evaluation (constraints,
/// layer ordering, boundaries — unassigned files exempt), the session/CI
/// gate with baseline + epsilon regression check, evolution/dsm/doc_gaps,
/// and the reconcile-then-score freshness contract wrapping every aggregate
/// run (ADR-11, FR-GV-01..09, FR-RC-01..04).
pub mod governance;
/// The canonical SQLite graph store (S-005): schema, FTS5 search, forward-only
/// migrations, the per-connection WAL pragma contract, and point queries — the
/// system-of-record for `logos.db` (ADR-05).
pub mod graph_store;
/// The git-history evidence store (S-046, CR-006, ADR-22): the separate
/// `.logos/history.db` on its own forward-only migration track, and the
/// incremental, HEAD-anchored `git log --numstat -M` miner that populates it —
/// lazily, off the gate/sync/navigation paths (FR-GH-01, FR-GH-02, BR-26).
pub mod history;
/// The optional `core.hooksPath` git-hook installer (S-022): managed
/// post-commit/post-checkout/post-merge scripts triggering a targeted
/// `logos sync` — best-effort freshness, never a correctness dependency
/// (FR-SY-05, FR-SY-06); the seam `init -i` (S-023) calls for its optional
/// hook-install step.
pub mod hooks;
/// Graph hydration (S-009): derived, ephemeral petgraph views over the canonical
/// store for whole-graph algorithms, plus the Engine-owned bounded cache keyed by
/// `(scope, last_sync_at)` that amortises hydration across aggregate runs and
/// invalidates on sync (ADR-04, ADR-05, NFR-PE-07).
pub mod hydrate;
/// Project setup (S-023): the full `logos init` experience over the Sprint 4
/// bootstrap seam — starter policy templates, the generated `.logos/.gitignore`
/// managed block, `.mcp.json` MCP-server-block injection, the managed
/// `CLAUDE.md` usage steer, and the optional `core.hooksPath` git-hook
/// installer. Idempotent, non-clobbering, managed blocks (DL-07).
pub mod init;
/// The quality metrics engine (S-018): the five orthogonal metrics
/// (modularity, acyclicity, depth, equality, redundancy) over the hydrated
/// dependency view, combined by canonical-order geometric mean into the
/// deterministic 0–10000 signal with zero short-circuit and the empty-graph
/// "n/a" sentinel (ADR-08, ADR-12), persisted raw + normalized per snapshot
/// (FR-QM-07).
pub mod metrics;
/// The canonical SCIP-conformant data model (S-002): symbol identity, the
/// node/edge ontology, and the `scip` convert seam. Distinct from [`models`]
/// (plural) — see below.
pub mod model;
/// The `Serialize` read-model DTOs returned by [`Engine`] methods (S-001).
/// These are *output* shapes for adapters; [`model`] (singular) is the
/// *internal* data vocabulary the engine computes over.
pub mod models;
/// The navigation service (S-013): the eight best-effort-fresh navigation
/// tools — point queries on the read-only pool that never reconcile per call
/// (FR-RC-05, ADR-11), with whole-graph traversals (`context`/`explore`/
/// `impact`) on the cached hydrated views.
pub mod navigate;
/// Observability (S-019): the single `tracing` emission point (NFR-OO-01),
/// the stderr-only fmt layer (NFR-RA-01), the custom telemetry Layer batching
/// events into the separate `.logos/telemetry.db` (ADR-13, survives reindex),
/// and the `stats` read-models including the tokens-saved dogfood estimate.
pub mod observability;
/// The performance envelope (S-024): the ~100k-LOC target the latency budgets
/// are tuned for ([NFR-PE-01](../../docs/specs/requirements/NFR-PE-01.md)..07)
/// and the beyond-envelope advisory `index`/`status` emit when a repo exceeds it
/// ([NFR-PE-09](../../docs/specs/requirements/NFR-PE-09.md)).
pub mod perf;
/// The discover → extract → persist pipeline (S-010): the orchestrator behind
/// [`Engine::index`](engine::Engine::index) / [`Engine::sync`](engine::Engine::sync),
/// with blake3 dirty tracking and capture-before-delete incremental sync (ADR-10).
pub mod pipeline;
/// The declarative plugin substrate (S-004): the `LanguageRegistry`, the
/// `LanguagePlugin` trait, `plugin.toml` parsing, ABI assertion, query
/// compilation, and on-disk overrides — binding compiled-in tree-sitter
/// grammars to droppable declarative assets (ADR-09).
pub mod plugin;
/// Pass 2 of the pipeline (S-011): the resolution engine — binds the
/// `unresolved_refs` ledger to edges by scope-hierarchy rules (function-local
/// → module → crate → workspace), never fabricating an edge it cannot prove
/// to exactly one candidate (NFR-RA-05, ADR-10), and surfacing the
/// coverage/confidence read-model (FR-RS-04).
pub mod resolve;
/// The core execution runtime (S-008): the single-writer actor, the read-only
/// WAL connection pool, and the shared `rayon` worker pool — the whole
/// in-process concurrency layer the long-lived [`Engine`] owns (ADR-02, ADR-03,
/// ADR-04).
pub mod runtime;
/// The debounced filesystem watcher (S-022): `notify` + `notify-debouncer-full`
/// hosted under `serve --mcp`, coalescing edit storms into one batched
/// incremental sync per debounce window with drop-and-coalesce backpressure
/// (the AQ-01 resolution) — non-load-bearing for correctness; reconcile is the
/// backstop (FR-SY-06, ADR-11).
pub mod watch;
/// The agent-generated source wiki store (S-052, CR-008, ADR-24): the separate
/// `.logos/wiki.db` on its own forward-only migration track (survives `index`,
/// never `ATTACH`-ed, never gated — BR-29), the slug-upsert write path with
/// write-time anchor hashes + HEAD and a mandatory generator label, read-time
/// per-anchor freshness against the working tree, and the orphan lifecycle
/// (flag missing anchors; auto-delete + log when all are gone) (FR-WK-01..07).
pub mod wiki;
/// Worktree-aware root resolution and seed-from-main discovery (S-021,
/// ADR-15): `.logos/` resolves to the working-tree root via `git rev-parse
/// --show-toplevel` (FR-WT-01), and a DB-less linked worktree locates the
/// primary checkout's DB to seed from via `--git-common-dir` (FR-WT-03).
pub mod workspace;

pub use engine::Engine;
pub use error::{CoreError, Severity};
pub use hydrate::{Granularity, GraphView, HydrationConfig, HydrationStats, Scope, SyncStamp};
pub use runtime::{Runtime, RuntimeConfig};
