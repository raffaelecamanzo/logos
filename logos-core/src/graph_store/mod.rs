//! The canonical SQLite graph store ([graph-store component], [ADR-05]).
//!
//! `graph_store` owns `logos.db`: the schema, the FTS5 search index, the
//! forward-only migrations, the per-connection WAL pragma contract, and the
//! prepared-statement point queries. It is the system-of-record every later
//! component ([execution-runtime], [graph-hydration], [navigation-service])
//! builds on.
//!
//! # Connection contract ([FR-DB-02], [NFR-RA-10])
//!
//! Every connection — opened via [`SqliteGraphStore::open`] or
//! [`SqliteGraphStore::open_in_memory`] — sets `journal_mode=WAL`,
//! `foreign_keys=ON` (it defaults *OFF* — the [ADR-05] footgun), and
//! `synchronous=NORMAL`, plus a 5-second busy-timeout so writers serialize and
//! then fail gracefully rather than blocking readers ([NFR-RA-10]).
//!
//! # The read seam ([`GraphStore`])
//!
//! The [`GraphStore`] trait is the point-query interface consumers depend on
//! ([navigation-service], [resolution-engine], [pipeline-orchestrator]):
//! `node` / `callers` / `callees` / `search`. Every query is a **prepared
//! statement with bound parameters** — never string interpolation. That is the
//! injection boundary: scanned source names reach SQLite only as bound values
//! ([NFR-SE-02]).
//!
//! # Crash-safety ([NFR-RA-07])
//!
//! Multi-row writes go through [`SqliteGraphStore::write_batch`], which wraps
//! the work in one transaction: it commits only if the closure returns `Ok`,
//! and rolls back wholesale on any error — so an interrupted index/sync batch
//! never leaves partial state.
//!
//! [graph-store component]: ../../../docs/specs/architecture/components/graph-store.md
//! [ADR-05]: ../../../docs/specs/architecture/decisions/ADR-05.md
//! [execution-runtime]: ../../../docs/specs/architecture/components/execution-runtime.md
//! [graph-hydration]: ../../../docs/specs/architecture/components/graph-hydration.md
//! [navigation-service]: ../../../docs/specs/architecture/components/navigation-service.md
//! [resolution-engine]: ../../../docs/specs/architecture/components/resolution-engine.md
//! [pipeline-orchestrator]: ../../../docs/specs/architecture/components/pipeline-orchestrator.md
//! [FR-DB-02]: ../../../docs/specs/requirements/FR-DB-02.md
//! [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
//! [NFR-RA-10]: ../../../docs/specs/requirements/NFR-RA-10.md
//! [NFR-SE-02]: ../../../docs/specs/requirements/NFR-SE-02.md

mod migrate;
mod schema;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Serialize;

use crate::model::{
    EdgeKind, LocalBindingKind, LogosSymbol, NodeId, NodeKind, ParamRange, ReceiverMode, ReceiverShape, RefForm,
};
use crate::models::navigation::LanguageCount;
use crate::models::quality::{
    FixUnit, FixUnitKind, Offender, OutlierLabel, OutlierMeasure, WhereToStart, WorstOffenders,
};

/// The `project_metadata` key under which the admission-config fingerprint is
/// stored (CR-004, [ADR-20], [FR-SY-07]).
///
/// The single durable record of the configuration in force at the last
/// reconciliation — the pipeline compares
/// [`Config::admission_fingerprint`](crate::config::Config::admission_fingerprint)
/// against this value to gate the config-narrowing purge.
///
/// [ADR-20]: ../../../docs/specs/architecture/decisions/ADR-20.md
/// [FR-SY-07]: ../../../docs/specs/requirements/FR-SY-07.md
pub const CONFIG_FINGERPRINT_KEY: &str = "config_fingerprint";

/// One committed definition of a configuration key (S-380, [FR-WS-19]): the file
/// that proves it, that file's profile, and the value.
///
/// The provenance half of a configuration-bound resolution — [FR-WS-19] requires
/// an admitted value to carry the key, the defining sources *and* the profile
/// set, so the defining file and profile travel with the value rather than being
/// looked up again afterwards.
///
/// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigDefinition {
    /// Project-relative path of the configuration source.
    pub path: String,
    /// The profile that file declares, or `None` for the unprofiled source.
    pub profile: Option<String>,
    /// The committed literal.
    pub value: String,
}

/// The `project_metadata` key under which the persisted monotonic graph
/// revision is stored (CR-027, [ADR-32], [FR-SY-09]).
///
/// The durable, cross-process successor to the in-memory-only
/// [`SyncStamp`](crate::hydrate::SyncStamp): advanced by one at the post-`index`/
/// `sync` point whenever the graph changes (see
/// [`BatchWriter::advance_graph_revision`]) and read back via
/// [`GraphStore::graph_revision`]. A later process or surface compares it to
/// answer "has the graph changed since revision N?" without re-deriving state —
/// the native wiki tier's cache key and the agent-tier freshness comparator.
///
/// [ADR-32]: ../../../docs/specs/architecture/decisions/ADR-32.md
/// [FR-SY-09]: ../../../docs/specs/requirements/FR-SY-09.md
pub const GRAPH_REVISION_KEY: &str = "graph_revision";

/// The `project_metadata` key under which the wall-clock time of the last full
/// [`index`](crate::pipeline::index) that built a graph is stored ([CR-130],
/// [FR-NV-07], [FR-IX-12]).
///
/// Unix seconds, decimal, as text. The durable successor to the in-process
/// `AtomicU64` the [`status`](crate::Engine::status) read-model used to read:
/// that counter could only ever be set by the process that did the indexing, so
/// every read-only `status` / `workspace status` reported `null` on a fully
/// indexed project ([CR-130] §2.2). Written by the index pipeline beside the
/// [FR-IX-12] LOC roll-up (see [`crate::perf::INDEXED_LOC_KEY`]) and read back by
/// `status` — a pure read, never a write on read ([ADR-28]).
///
/// # Presence means "a graph was built", not "`index` ran"
/// The row is written when a full index persists at least one file, and removed
/// when a full index leaves the store empty; a full index that persists nothing
/// over a graph that still stands leaves the row alone. The three cases live in
/// [`crate::pipeline`]'s `IndexPersistence`, classified once per index and shared
/// with the [FR-IX-12] roll-up, which mirrors them — so the stamp and the roll-up
/// can never disagree about one run ([CR-134]). Presence therefore agrees with
/// [`StatusInfo::indexed`](crate::models::navigation::StatusInfo::indexed)
/// rather than with the bare fact that the command ran: a member whose walk
/// admits nothing reports an empty graph, and dating an empty graph would be the
/// same class of dishonest readout [CR-130] exists to remove ([NFR-CC-04]).
/// Absent is reported as absent — never `0`, never fabricated.
///
/// [CR-130]: ../../../docs/requests/CR-130-a-readout-names-a-remediation-that-cannot-apply.md
/// [CR-134]: ../../../docs/requests/CR-134-a-zero-persist-index-fabricates-a-zero-loc-rollup.md
/// [FR-NV-07]: ../../../docs/specs/requirements/FR-NV-07.md
/// [FR-IX-12]: ../../../docs/specs/requirements/FR-IX-12.md
/// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
pub const LAST_FULL_INDEX_AT_KEY: &str = "last_full_index_at";

/// The `project_metadata` key a **full-walk** build-manifest pass records once
/// it has read the member's whole manifest set (S-462 task 2, [FR-WS-33],
/// [NFR-CC-04]).
///
/// Written by a full [`index`](crate::pipeline::index) and by a
/// [`SyncScope::FullWalk`](crate::pipeline::SyncScope::FullWalk) sync — for a
/// member with no build manifest too — and never by a partial sync, which sees
/// a subset of the member. Its value is `"1"`; only presence is read
/// ([`GraphStore::build_facts_extracted`]).
///
/// It separates two states the `build_manifests` table alone cannot: "read, no
/// manifests" and "never extracted". Migration 22 creates that table empty, so
/// a store upgraded from v21 holds no rows until a full walk re-reads the
/// member — and without the marker it would read as a member with no
/// manifests, the exact report [FR-WS-33] forbids for facts that were never
/// read. The workspace join reports an unmarked member unread instead
/// ([`crate::federation::build_deps`]).
///
/// [FR-WS-33]: ../../../docs/specs/requirements/FR-WS-33.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
pub const BUILD_FACTS_EXTRACTED_KEY: &str = "build_facts_extracted";

/// The `project_metadata` key recording that this member's **declared-type
/// facts** are complete (S-472, [CR-152] §3.2 B) — the
/// [`BUILD_FACTS_EXTRACTED_KEY`] twin for migration 24's tables.
///
/// Written by a full [`index`](crate::pipeline::index), which extracts every
/// source file and reads every schema, and by the first
/// [`SyncScope::FullWalk`](crate::pipeline::SyncScope::FullWalk) sync over a
/// store that lacks it, which re-extracts the member's package-shaped source
/// files once to fill the table — for a member with no Java/Kotlin/Avro file
/// too. Never by a partial sync, which sees a subset of the member. Its value
/// is `"1"`; only presence is read ([`GraphStore::declared_types_extracted`]).
///
/// It separates "read, no declared types" from "never extracted": migration 24
/// creates the tables empty, and a store upgraded from v23 re-extracts a source
/// file only when it changes, so without the marker an upgraded member would
/// read as declaring nothing.
///
/// [CR-152]: ../../../docs/requests/CR-152-cross-member-type-references-overlay.md
pub const DECLARED_TYPES_EXTRACTED_KEY: &str = "declared_types_extracted";

/// The `project_metadata` key recording the project's resolved identity
/// coordinate ([FR-CF-10], [ADR-85], S-702) — `[project] id`, else its derived
/// default — read back by [`status`](crate::Engine::status).
///
/// Written by `logos init`, by every full [`index`](crate::pipeline::index)
/// and by the coordinate migration an engine runs when it opens a store whose
/// symbols carry another coordinate (S-704), from the configuration in force;
/// a read itself never writes it ([ADR-28]).
///
/// It records which coordinate the **project** resolves to, and nothing about
/// the store's symbols: whether those carry a coordinate yet is a separate key,
/// written only when the symbols do, so a store whose symbols still render the
/// `.` package slot never reads as already carrying one.
///
/// [FR-CF-10]: ../../../docs/specs/requirements/FR-CF-10.md
/// [ADR-85]: ../../../docs/specs/architecture/decisions/ADR-85.md
/// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
pub const PROJECT_ID_KEY: &str = "project_id";

/// The `project_metadata` key recording the coordinate this store's **symbols**
/// carry in their SCIP package slot ([ADR-85], S-703), read back by
/// [`GraphStore::symbol_coordinate`].
///
/// Written by a full [`index`](crate::pipeline::index) after every pass, when
/// every symbol in the store is one it built: it re-stored files, or no file of
/// an older graph stood. An index that stores no file while an older graph
/// stands, or that stops part-way, leaves the previous value (or none). Also
/// written by the first [`sync`](crate::pipeline::sync) that persists a file
/// into a store holding none, which adopts the project's coordinate as an
/// index would, so a store whose first write is a sync still carries it. And
/// written by the coordinate migration an engine runs when it opens a store
/// whose recorded value differs from the project's (S-704,
/// [`BatchWriter::migrate_symbol_coordinate`]), in the batch that rewrites the
/// symbols.
///
/// Absent on a store that holds files means a **coordinate-less store**, built
/// before [ADR-85], whose symbols render the empty slot `.` until the next
/// open migrates them. A sync renders the
/// symbols it re-extracts under the recorded value (or `.`), so it never
/// introduces a second coordinate. One case is not covered: a file a full
/// index could not *load* keeps its rows, and so its previous coordinate,
/// until it loads again or a later migration rewrites it — the posture an
/// unloaded file has for every record.
///
/// Distinct from [`PROJECT_ID_KEY`], which records what the project resolves
/// to now: the two differ exactly when the symbols have not yet followed the
/// project's coordinate.
///
/// [ADR-85]: ../../../docs/specs/architecture/decisions/ADR-85.md
pub const SYMBOL_COORDINATE_KEY: &str = "symbol_coordinate";

/// The `project_metadata` key recording which plugin reads each shared
/// extension in this store (S-582, [FR-PL-07]): the extensions the
/// repository's sources gave to the plugin sharing them
/// ([`SharedOwners::record`](crate::plugin::SharedOwners::record)) — `h` in a
/// repository whose C sources outnumber its C++ ones.
///
/// Written by every full [`index`](crate::pipeline::index) and by a
/// [`sync`](crate::pipeline::sync) whose files move the decision, after the
/// headers it re-reads are stored; read when an engine opens the store, so
/// every process routes `.h` as the graph was built. Absent means the
/// default: every shared extension with its outright claimant — C++ reads
/// `.h` — which is what a store built before S-582 holds.
///
/// [FR-PL-07]: ../../../docs/specs/requirements/FR-PL-07.md
pub const SHARED_OWNERS_KEY: &str = "shared_extension_owners";

/// A row read back from the `nodes` table, mapped to model types.
///
/// `kind` is recovered via [`NodeKind::try_from`] and `symbol` via
/// [`LogosSymbol::parse`]; a value that fails either conversion is a corrupt
/// store — surfaced as an error advising a rebuild ([NFR-RA-08]).
///
/// [NFR-RA-08]: ../../../docs/specs/requirements/NFR-RA-08.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeRow {
    /// The storage-local rowid handle.
    pub id: NodeId,
    /// The canonical SCIP symbol identity.
    pub symbol: LogosSymbol,
    /// The node ontology kind.
    pub kind: NodeKind,
    /// The human-facing name (the FTS-indexed column).
    pub name: String,
    /// The defining file's path, if the node is bound to one.
    pub file_path: Option<String>,
    /// 1-based start line of the declaration, when recorded ([FR-NV-04]).
    ///
    /// [FR-NV-04]: ../../../docs/specs/requirements/FR-NV-04.md
    pub start_line: Option<i64>,
    /// 1-based end line of the declaration, when recorded.
    pub end_line: Option<i64>,
}

impl NodeRow {
    /// The node's identity across re-indexing and checkouts ([FR-GV-23]): its
    /// symbol id **without the repository coordinate** — the descriptor tail
    /// ([ADR-85]). The storage id changes on every re-index, and the coordinate
    /// with a fork, a renamed repository or a remote-less clone, so neither can
    /// key a value compared across two measurements. A local symbol (no
    /// coordinate slot) keeps its whole string. The one spelling a ratchet
    /// blessed entry ([FR-GV-23]) and an offender ([FR-QM-19]) are keyed by.
    ///
    /// [FR-GV-23]: ../../../docs/specs/requirements/FR-GV-23.md
    /// [FR-QM-19]: ../../../docs/specs/requirements/FR-QM-19.md
    /// [ADR-85]: ../../../docs/specs/architecture/decisions/ADR-85.md
    #[must_use]
    pub fn identity(&self) -> &str {
        self.symbol.descriptor_tail().unwrap_or(self.symbol.as_str())
    }
}

/// An edge read back from the `edges` table, mapped to model types.
///
/// The whole-graph counterpart to [`NodeRow`]: where the point queries
/// (`callers`/`callees`) return the *nodes* adjacent to one vertex, [`EdgeRow`]
/// is one relationship in the bulk stream [`GraphStore::all_edges`] feeds to
/// petgraph hydration ([graph-hydration], [ADR-05]). `kind` is recovered via
/// [`EdgeKind::try_from`]; an out-of-range discriminant is a corrupt store,
/// surfaced as an error advising a rebuild ([NFR-RA-08]).
///
/// [graph-hydration]: ../../../docs/specs/architecture/components/graph-hydration.md
/// [ADR-05]: ../../../docs/specs/architecture/decisions/ADR-05.md
/// [NFR-RA-08]: ../../../docs/specs/requirements/NFR-RA-08.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EdgeRow {
    /// The source node (the `e.source` endpoint).
    pub source: NodeId,
    /// The target node (the `e.target` endpoint).
    pub target: NodeId,
    /// The relationship kind.
    pub kind: EdgeKind,
    /// Whether the edge's payload is a [`dispatch`](crate::model::dispatch)
    /// marker — read it through [`EdgeRow::is_dispatch`].
    pub dispatch: bool,
}

impl EdgeRow {
    /// `true` for a `calls` edge reached through a trait to one of the trait
    /// method's implementations ([CR-226] BR-76) — a runtime reach, not a
    /// dependency. The one predicate every **architecture** view skips by
    /// ([FR-QM-20]): the metric graph and its module rollup, the DSM, the layer
    /// and boundary evaluation and the coupling budgets. Every **runtime**
    /// reader (callers, callees, impact, reachability, dead code) keeps the edge.
    ///
    /// [CR-226]: ../../../docs/requests/CR-226-dispatch-is-not-a-dependency.md
    /// [FR-QM-20]: ../../../docs/specs/requirements/FR-QM-20.md
    pub fn is_dispatch(&self) -> bool {
        self.dispatch && self.kind == EdgeKind::Calls
    }
}

/// An edge read back **by symbol identity** rather than by storage rowid — the
/// currency the deep `verify` diff ([FR-GV-19], [NFR-RA-06]) compares across two
/// stores.
///
/// A [`NodeId`] is storage-local and differs between the live store and a fresh
/// shadow reindex, so two stores' [`EdgeRow`]s cannot be diffed; the canonical
/// symbol strings of the two endpoints (plus the kind) can. The endpoint files
/// ride along so a drifted edge points at the file whose re-extraction lost or
/// kept it ([CR-236]); a node unbound to a file reads `None`.
///
/// [FR-GV-19]: ../../../docs/specs/requirements/FR-GV-19.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
/// [CR-236]: ../../../docs/requests/CR-236-incremental-sync-loses-inbound-edges.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolEdgeRow {
    /// The relationship kind.
    pub kind: EdgeKind,
    /// The canonical SCIP symbol of the source node.
    pub source: String,
    /// The canonical SCIP symbol of the target node.
    pub target: String,
    /// The defining file's path of the source node, if bound to one.
    pub source_file: Option<String>,
    /// The defining file's path of the target node, if bound to one.
    pub target_file: Option<String>,
}

/// The fields needed to insert a new node.
///
/// `symbol` must already be a resolved [`LogosSymbol`] id (see
/// [`SqliteGraphStore::upsert_symbol`]); `kind` is a typed [`NodeKind`] so the
/// caller cannot pass an out-of-range discriminant past the type system before
/// the `CHECK` constraint even runs.
#[derive(Debug, Clone, Copy)]
pub struct NewNode<'a> {
    /// FK into `symbols(id)`.
    pub symbol_id: i64,
    /// The node ontology kind.
    pub kind: NodeKind,
    /// The human-facing name.
    pub name: &'a str,
    /// Optional FK into `files(id)`.
    pub file_id: Option<i64>,
    /// Optional 1-based start line.
    pub start_line: Option<i64>,
    /// Optional 1-based end line.
    pub end_line: Option<i64>,
    /// `true` for an annotation-materialised policy node
    /// ([`NodeKind::Layer`]/[`NodeKind::Boundary`]) that is cleared and rebuilt
    /// each annotation run (S-014, [FR-AN-03]). `false` for every extracted node.
    ///
    /// [FR-AN-03]: ../../../docs/specs/requirements/FR-AN-03.md
    pub derived: bool,
    /// Declaration visibility captured by Pass 1 — the exported-is-live
    /// dead-code root set ([FR-AN-01]).
    ///
    /// [FR-AN-01]: ../../../docs/specs/requirements/FR-AN-01.md
    pub exported: bool,
    /// Cyclomatic complexity, for `Function`/`Method` nodes ([FR-AN-04]).
    ///
    /// [FR-AN-04]: ../../../docs/specs/requirements/FR-AN-04.md
    pub cyclomatic_complexity: Option<i64>,
    /// Physical line count of the definition, for `Function`/`Method` nodes.
    pub line_count: Option<i64>,
    /// The normalised AST-shape fingerprint duplicate detection groups by
    /// ([FR-AN-02]); `Function`/`Method` nodes only.
    ///
    /// [FR-AN-02]: ../../../docs/specs/requirements/FR-AN-02.md
    pub fingerprint: Option<&'a str>,
    /// Extraction-time test-marker evidence captured by Pass 1 ([FR-EX-06],
    /// S-027): `true` exactly when this function/method carries a
    /// language-native test marker. The persisted **input** to the unified
    /// `is_test` annotation ([FR-AN-05]) — never inferred, never on a
    /// non-callable.
    ///
    /// [FR-EX-06]: ../../../docs/specs/requirements/FR-EX-06.md
    /// [FR-AN-05]: ../../../docs/specs/requirements/FR-AN-05.md
    pub test_evidence: bool,
    /// The FTS-indexed `nodes.body` text. Two producers set it: a `DocSection`'s
    /// own prose beneath its heading, excluding nested sub-sections ([FR-DG-05],
    /// S-037); and a config typed-anchor's **payload subtype** (S-065, [CR-010],
    /// [FR-CG-03]) — e.g. `"object"`/`"interface"` for a `GqlType`. `None` for
    /// every code node and for any node with no searchable body / no payload, so
    /// the column (and its `nodes_fts` posting) stays `NULL`. Note: do **not**
    /// assume `body.is_some()` implies `DocSection` — config anchors set it too.
    ///
    /// [FR-DG-05]: ../../../docs/specs/requirements/FR-DG-05.md
    /// [CR-010]: ../../../docs/requests/CR-010-config-artifact-graph-layer.md
    /// [FR-CG-03]: ../../../docs/specs/requirements/FR-CG-03.md
    pub body: Option<&'a str>,
    /// The per-function maximum block-structure nesting depth captured by Pass 1
    /// (CR-005, [FR-EX-07]); `Function`/`Method` nodes only, `None` otherwise.
    /// The input to the Nesting and Conciseness dimensions ([FR-QM-09]/
    /// [FR-QM-10]).
    ///
    /// [FR-EX-07]: ../../../docs/specs/requirements/FR-EX-07.md
    pub max_nesting_depth: Option<i64>,
    /// Whether the callable carries an implementation body, captured by Pass 1
    /// (S-500, [FR-EX-11]); `Function`/`Method` nodes only, `None` otherwise.
    ///
    /// [FR-EX-11]: ../../../docs/specs/requirements/FR-EX-11.md
    pub has_body: Option<bool>,
    /// The normalized token count of that body (S-500) — the input to the
    /// exact-duplicate token floor (S-501); `Function`/`Method` nodes only.
    pub body_tokens: Option<i64>,
    /// The base type name of the type this method belongs to, as its plugin
    /// query declares it (S-493, [FR-RS-11]) — the `nodes.self_type` column. `None`
    /// for every node no query gives a self type.
    ///
    /// [FR-RS-11]: ../../../docs/specs/requirements/FR-RS-11.md
    pub self_type: Option<&'a str>,
    /// The argument counts a callable admits (S-591, [FR-EX-32]) — the
    /// `nodes.param_min` / `nodes.param_max` columns, migration 33. `None` for
    /// an unknown range.
    ///
    /// [FR-EX-32]: ../../../docs/specs/requirements/FR-EX-32.md
    pub params: Option<ParamRange>,
    /// Whether a Rust `impl` function takes `self` ([CR-200]), and `false` for
    /// a Ruby callable of the class or module object (S-565) — the
    /// `nodes.takes_self` column, migration 33. `None` for every other node.
    ///
    /// [CR-200]: ../../../docs/requests/CR-200-a-rust-method-call-binds-only-a-callable-that-takes-self.md
    pub takes_self: Option<bool>,
    /// How the callable writes its receiver (S-606, [FR-EX-34]) — the
    /// `nodes.receiver_mode` column, migration 34. Recorded wherever
    /// `takes_self` is; `None` wherever `takes_self` is `None`, or when the
    /// mode is unknown.
    ///
    /// [FR-EX-34]: ../../../docs/specs/requirements/FR-EX-34.md
    pub receiver_mode: Option<ReceiverMode>,
    /// An enum's variant names, space-joined (S-606) — the `nodes.variants`
    /// column, migration 34. `None` for every other node.
    pub variants: Option<&'a str>,
    /// `true` for a required signature (S-606, [CR-202] F3), or a C-family
    /// prototype paired with a definition of its file (S-577) — the
    /// `nodes.signature` column, migration 34.
    ///
    /// [CR-202]: ../../../docs/requests/CR-202-one-rust-associated-item-lookup.md
    pub signature: bool,
    /// `true` for an interface member its implementors do not inherit (S-609,
    /// [FR-RS-48]) — the `nodes.uninherited` column, migration 35.
    ///
    /// [FR-RS-48]: ../../../docs/specs/requirements/FR-RS-48.md
    pub uninherited: bool,
    /// A type's generic arity (S-602, [CR-199]) — the `nodes.type_arity`
    /// column, migration 42. `None` for every node of a plugin that does not
    /// tell types apart by arity, and for every non-type node.
    ///
    /// [CR-199]: ../../../docs/requests/CR-199-csharp-resolution-honours-type-arity-and-using-scope.md
    pub type_arity: Option<u32>,
}

impl<'a> NewNode<'a> {
    /// A plain extracted node with no annotation payload: not derived, not
    /// exported, no metrics, no fingerprint. The common starting point — set
    /// the annotation fields the caller actually knows.
    pub fn plain(symbol_id: i64, kind: NodeKind, name: &'a str) -> Self {
        NewNode {
            symbol_id,
            kind,
            name,
            file_id: None,
            start_line: None,
            end_line: None,
            derived: false,
            exported: false,
            cyclomatic_complexity: None,
            line_count: None,
            fingerprint: None,
            test_evidence: false,
            body: None,
            max_nesting_depth: None,
            has_body: None,
            body_tokens: None,
            self_type: None,
            params: None,
            takes_self: None,
            receiver_mode: None,
            variants: None,
            signature: false,
            uninherited: false,
            type_arity: None,
        }
    }
}

/// The annotation-pass snapshot of one node — the columns Pass 3 computes over
/// (S-014, [FR-AN-01..03]), read in `id` order for a deterministic run
/// ([NFR-RA-06]).
///
/// Deliberately a separate row type from [`NodeRow`]: the hydration/navigation
/// read path does not pay for the annotation columns, and the annotation pass
/// does not depend on the point-query projection.
///
/// [FR-AN-01..03]: ../../../docs/specs/requirements/FR-AN-01.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnotationNodeRow {
    /// The storage-local rowid handle.
    pub id: NodeId,
    /// The node ontology kind.
    pub kind: NodeKind,
    /// The human-facing name (matched against `[semantics].entry_points`).
    pub name: String,
    /// Declaration visibility — the exported-is-live root set ([FR-AN-01]).
    ///
    /// [FR-AN-01]: ../../../docs/specs/requirements/FR-AN-01.md
    pub exported: bool,
    /// `true` for a policy node a previous annotation run materialised.
    pub derived: bool,
    /// The AST-shape fingerprint, for `Function`/`Method` nodes ([FR-AN-02]).
    ///
    /// [FR-AN-02]: ../../../docs/specs/requirements/FR-AN-02.md
    pub fingerprint: Option<String>,
    /// Extraction-time test-marker evidence captured by Pass 1 ([FR-EX-06]) —
    /// the persisted input to the unified `is_test` verdict Pass 3 computes
    /// ([FR-AN-05]).
    ///
    /// [FR-EX-06]: ../../../docs/specs/requirements/FR-EX-06.md
    /// [FR-AN-05]: ../../../docs/specs/requirements/FR-AN-05.md
    pub test_evidence: bool,
    /// The defining file's id, if the node is bound to one.
    pub file_id: Option<i64>,
    /// The defining file's path, if the node is bound to one.
    pub file_path: Option<String>,
    /// The dead-code verdict of the last annotation run; `None` = not yet
    /// annotated / not a dead-code candidate ([FR-AN-01]).
    ///
    /// [FR-AN-01]: ../../../docs/specs/requirements/FR-AN-01.md
    pub is_dead: Option<bool>,
    /// The duplicate verdict of the last annotation run; `None` = not yet
    /// annotated / carries no fingerprint ([FR-AN-02]).
    ///
    /// [FR-AN-02]: ../../../docs/specs/requirements/FR-AN-02.md
    pub is_duplicate: Option<bool>,
    /// The unified test verdict of the last annotation run ([FR-AN-05]) — a
    /// definite boolean (`false` before the first run), never the tri-state
    /// `NULL` the dead/duplicate verdicts carry.
    ///
    /// [FR-AN-05]: ../../../docs/specs/requirements/FR-AN-05.md
    pub is_test: bool,
    /// The `[[layers]]` band of the defining file, when one matched
    /// ([FR-AN-03]).
    ///
    /// [FR-AN-03]: ../../../docs/specs/requirements/FR-AN-03.md
    pub layer_membership: Option<String>,
    /// The stable near-clone group id of the last annotation run — the minimum
    /// node id of the function's near-clone component, or `None` when it belongs
    /// to no near-clone group ([FR-AN-06], CR-005). The input the Uniqueness
    /// dimension consumes ([FR-QM-13]).
    ///
    /// [FR-AN-06]: ../../../docs/specs/requirements/FR-AN-06.md
    /// [FR-QM-13]: ../../../docs/specs/requirements/FR-QM-13.md
    pub clone_group: Option<NodeId>,
    /// Whether the callable carries an implementation body (S-500,
    /// [FR-EX-11]) — the `nodes.has_body` column. `None` for a non-callable,
    /// and for a callable indexed before migration 25 that has not been
    /// re-extracted since.
    ///
    /// [FR-EX-11]: ../../../docs/specs/requirements/FR-EX-11.md
    pub has_body: Option<bool>,
    /// The normalized token count of the callable's body (S-500) — the
    /// `nodes.body_tokens` column the exact-duplicate floor reads; `None`
    /// exactly when [`has_body`](Self::has_body) is.
    pub body_tokens: Option<i64>,
}

/// One indexed file with the content hash recorded at its last (re-)index.
///
/// The [pipeline-orchestrator] reads these back to drive incremental-sync dirty
/// detection: a candidate file whose freshly computed blake3 hash equals the
/// stored `content_hash` is unchanged and is skipped without re-extraction
/// ([FR-SY-03]).
///
/// [pipeline-orchestrator]: ../../../docs/specs/architecture/components/pipeline-orchestrator.md
/// [FR-SY-03]: ../../../docs/specs/requirements/FR-SY-03.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileRecord {
    /// The file row's id — the `unresolved_refs.file_id` FK target (S-011).
    pub id: i64,
    /// The project-relative path key (matches `FileInput::path`).
    pub path: String,
    /// The content hash stored at the last index, if one was recorded.
    pub content_hash: Option<String>,
}

/// One file whose latest persistence attempt failed — a row of the migration-27
/// record (S-513, [FR-EH-05]).
///
/// `stale` is the distinction [FR-EH-05] draws: `true` when the graph still
/// holds the file's last successfully persisted facts (a `sync`, the watcher or
/// a reconcile could not replace them), `false` when the graph holds nothing
/// for it (a full index, or a file that never persisted). Either way the file
/// is not indexed as it now stands on disk ([NFR-CC-04]).
///
/// [FR-EH-05]: ../../../docs/specs/requirements/FR-EH-05.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PersistFailureRow {
    /// The project-relative path (the `files.path` key).
    pub path: String,
    /// The error that rolled the file's write back.
    pub reason: String,
    /// The graph still holds the file's last good facts.
    pub stale: bool,
}

/// Whole-store row counts — the cheap index-health snapshot behind the
/// navigation `status` tool ([FR-NV-07]).
///
/// One prepared statement of `COUNT(*)` subselects: no table scan of row
/// *content*, no discovery walk — safe inside the navigation freshness
/// contract ([FR-RC-05]).
///
/// [FR-NV-07]: ../../../docs/specs/requirements/FR-NV-07.md
/// [FR-RC-05]: ../../../docs/specs/requirements/FR-RC-05.md
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct StoreCounts {
    /// Rows in `files` — indexed source files.
    pub files: u64,
    /// Rows in `nodes` — graph vertices.
    pub nodes: u64,
    /// Rows in `edges` — graph relationships.
    pub edges: u64,
    /// Rows in `unresolved_refs` — the reference ledger (S-011) **less its
    /// capture-before-delete rows** (`RefForm::Symbol`, [ADR-10]), each a
    /// duplicate of a reference its source file's own row records (S-598,
    /// [FR-RS-04]).
    ///
    /// [ADR-10]: ../../../docs/specs/architecture/decisions/ADR-10.md
    /// [FR-RS-04]: ../../../docs/specs/requirements/FR-RS-04.md
    pub refs_total: u64,
    /// Those ledger rows currently bound to an edge (`resolved = 1`).
    pub refs_resolved: u64,
}

/// What a coordinate migration changed ([`BatchWriter::migrate_symbol_coordinate`],
/// S-704, [FR-EX-35]): one count per kind of row it rewrote or removed.
///
/// [FR-EX-35]: ../../../docs/specs/requirements/FR-EX-35.md
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct CoordinateMigration {
    /// `symbols` rows with no node, removed before the rewrite.
    pub orphan_symbols_removed: u64,
    /// `symbols.symbol` values rewritten to the new coordinate.
    pub symbols_rewritten: u64,
    /// `unresolved_refs` values rewritten: a row's `source_symbol`, and the
    /// `target` of a capture-before-delete row (`RefForm::Symbol`), which is a
    /// symbol too. A row whose two values both moved counts once per value.
    pub ref_symbols_rewritten: u64,
    /// `declared_types.symbol` values rewritten.
    pub declared_type_symbols_rewritten: u64,
    /// `files` rows whose `content_hash` was cleared, so the next reconcile
    /// re-extracts them.
    pub files_invalidated: u64,
}

/// One relation class's raw resolution counts over one language — the four
/// figures [`GraphStore::resolution_by_language`] reads, before the resolution
/// engine classifies them ([FR-RS-09]).
///
/// [FR-RS-09]: ../../../docs/specs/requirements/FR-RS-09.md
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RelationCounts {
    /// Ledger rows of this kind owned by the language's files.
    pub references: u64,
    /// Of those, rows flagged `resolved = 1`.
    pub bound: u64,
    /// Edges of this kind from the language's nodes to a node in the **same**
    /// file.
    pub same_file_edges: u64,
    /// Edges of this kind from the language's nodes to a node in a
    /// **different** indexed file.
    pub cross_file_edges: u64,
}

/// One language's raw resolution counts ([FR-RS-09]): every language tagged on
/// an indexed file gets one, with zeroed classes where it recorded nothing.
///
/// [FR-RS-09]: ../../../docs/specs/requirements/FR-RS-09.md
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LanguageRefCounts {
    /// `files.language`.
    pub language: String,
    /// Indexed files tagged with this language.
    pub files: u64,
    /// Of those, the files whose path ends in `.h` — the ambiguous header
    /// extension (S-572, [FR-PL-07]).
    ///
    /// [FR-PL-07]: ../../../docs/specs/requirements/FR-PL-07.md
    pub header_files: u64,
    /// Of those, the files whose parse held an ERROR or MISSING node
    /// (`files.parse_damaged = 1`, S-571, [FR-IX-14]).
    ///
    /// [FR-IX-14]: ../../../docs/specs/requirements/FR-IX-14.md
    pub damaged_files: u64,
    /// Of those, the files that record no parse damage either way
    /// (`files.parse_damaged IS NULL`): indexed before migration 44 and not
    /// re-extracted since.
    pub unrecorded_files: u64,
    /// The `Calls` class.
    pub calls: RelationCounts,
    /// The `Imports` class.
    pub imports: RelationCounts,
}

/// The reported reference ledger as the reference classes read it ([FR-RS-49],
/// S-635): the doc and code halves' counts, every **unbound** code reference
/// grouped by what classifies it, and the names the graph could bind a target
/// to — all from one read snapshot of [`GraphStore::reference_population`], so
/// the halves describe one graph generation. The classification itself is the
/// resolution engine's (`resolve::reference_classes`).
///
/// "Reported" is the [`StoreCounts`] population: the ledger less its
/// capture-before-delete rows, so `doc_references + code_references` equals
/// [`StoreCounts::refs_total`] over the same snapshot.
///
/// [FR-RS-49]: ../../../docs/specs/requirements/FR-RS-49.md
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReferencePopulation {
    /// Reported `DocReference` rows.
    pub doc_references: u64,
    /// Of those, the bound ones.
    pub doc_resolved: u64,
    /// Every other reported row — the code references.
    pub code_references: u64,
    /// Of those, the bound ones.
    pub code_resolved: u64,
    /// The unbound code references, one entry per `(language, kind, target)`
    /// with its row count, in that order ([NFR-RA-06]).
    ///
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    pub unbound_code: Vec<UnboundCodeTarget>,
    /// The unbound code references that are the **consumer** side of an
    /// invocation arm (an HTTP client call, a gRPC stub call, a broker publish
    /// — the rows the workspace bridge binds across members, [FR-WS-04]), one
    /// entry per `(source_symbol, target)` with its row count, in that order
    /// ([NFR-RA-06]). A workspace counts the ones its bridge bound as resolved
    /// (S-637); every one is already among [`unbound_code`](Self::unbound_code).
    ///
    /// [FR-WS-04]: ../../../docs/specs/requirements/FR-WS-04.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    pub unbound_invocations: Vec<UnboundInvocation>,
    /// The names the graph could bind a target to — see [`DeclaredNameSources`].
    pub names: DeclaredNameSources,
}

/// One group of [`ReferencePopulation::unbound_invocations`]: the unbound
/// invocation-consumer rows one declaration records for one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnboundInvocation {
    /// The call site's declaration — the ledger's `source_symbol`, as written.
    pub source_symbol: String,
    /// The target as the ledger records it.
    pub target: String,
    /// Rows of the group.
    pub rows: u64,
}

/// One group of [`ReferencePopulation::unbound_code`]: the unbound code
/// references of one language and kind that name one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnboundCodeTarget {
    /// The owning file's `files.language`; `None` for a file that records none.
    pub language: Option<String>,
    /// The `unresolved_refs.kind` discriminant ([`EdgeKind::as_i32`]).
    pub kind: i64,
    /// The target as the ledger records it.
    pub target: String,
    /// Rows of the group.
    pub rows: u64,
}

/// What the graph declares that a reference's target could name — the raw
/// sources of the reference classes' name universe ([FR-RS-49], S-635).
///
/// More than node names, because a binder lookup does not always end on a
/// node's own name: an import binds an alias (`import X as Y`), a file or a
/// directory (`#include "a.h"`, a Go package path), a declared namespace (a C#
/// `using`), or a Kotlin file facade.
///
/// [FR-RS-49]: ../../../docs/specs/requirements/FR-RS-49.md
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeclaredNameSources {
    /// Every node name, repeats included — the universe built from it is a set.
    pub names: Vec<String>,
    /// Every distinct `(alias, target)` pair of the reported ledger, ordered —
    /// the local name an import introduces, beside what it imports.
    pub aliases: Vec<(String, String)>,
    /// Every indexed file's path, with its declared namespace when it records
    /// one.
    pub files: Vec<(String, Option<String>)>,
}

/// Count `row` in `invocations` under its declaration and `target` when it is
/// an invocation call site ([`invocation_consumer`]) — a helper so the ledger
/// scan stays under the nesting threshold.
fn tally_invocation(
    invocations: &mut std::collections::BTreeMap<(String, String), u64>,
    row: &rusqlite::Row<'_>,
    target: &str,
) -> rusqlite::Result<()> {
    if let Some(source_symbol) = invocation_consumer(row)? {
        *invocations.entry((source_symbol, target.to_string())).or_default() += 1;
    }
    Ok(())
}

/// The `source_symbol` of a [`GraphStore::reference_population`] ledger row
/// (columns 5 and 6: `payload`, `source_symbol`) when its payload names the
/// consumer side of an invocation arm, else `None` — the bridge's own test
/// ([`ArtifactRelation::is_invocation_consumer`]). A `source_symbol` that is
/// not UTF-8 is `None` too.
///
/// [`ArtifactRelation::is_invocation_consumer`]: crate::model::ArtifactRelation::is_invocation_consumer
fn invocation_consumer(row: &rusqlite::Row<'_>) -> rusqlite::Result<Option<String>> {
    let rusqlite::types::ValueRef::Text(payload) = row.get_ref(5)? else {
        return Ok(None);
    };
    let consumer = std::str::from_utf8(payload)
        .ok()
        .and_then(crate::model::ArtifactRelation::from_wire)
        .is_some_and(crate::model::ArtifactRelation::is_invocation_consumer);
    if !consumer {
        return Ok(None);
    }
    // A declaration that is not UTF-8 — a corrupt store, an outside writer —
    // names no call site: the bridge could not parse it either, and a strict
    // read would fail the whole `status` this scan serves.
    Ok(row.get_ref(6)?.as_str().ok().map(str::to_string))
}

/// The index of `language` in `languages`, appended on first sight — the
/// interning [`GraphStore::reference_population`] keys its ledger scan by.
fn intern(languages: &mut Vec<Option<String>>, language: Option<String>) -> usize {
    if let Some(index) = languages.iter().position(|known| *known == language) {
        return index;
    }
    languages.push(language);
    languages.len() - 1
}

/// Count one more row naming `target`, allocating its key only on first sight.
fn tally(targets: &mut std::collections::HashMap<String, u64>, target: &str) {
    if let Some(count) = targets.get_mut(target) {
        *count += 1;
        return;
    }
    targets.insert(target.to_string(), 1);
}

/// The class a `kind` discriminant from [`GraphStore::resolution_by_language`]'s
/// two grouped reads lands in; `None` for a kind neither read selects.
fn relation_class(row: &mut LanguageRefCounts, kind: i64) -> Option<&mut RelationCounts> {
    if kind == EdgeKind::Calls as i64 {
        Some(&mut row.calls)
    } else if kind == EdgeKind::Imports as i64 {
        Some(&mut row.imports)
    } else {
        None
    }
}

/// A cross-file edge captured before a synced file's nodes are deleted, so it
/// can be re-attached after re-extraction ([ADR-10] capture-before-delete).
///
/// Endpoints are recorded by their stable [`LogosSymbol`] string rather than by
/// rowid: deleting and re-inserting a file's nodes assigns fresh rowids, but the
/// canonical-ordinal symbol of an unchanged declaration is invariant ([ADR-07]),
/// so the edge rebinds by symbol after the file is re-extracted.
///
/// [ADR-10]: ../../../docs/specs/architecture/decisions/ADR-10.md
/// [ADR-07]: ../../../docs/specs/architecture/decisions/ADR-07.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedEdge {
    /// The source endpoint's symbol (a node in another file — untouched by the sync).
    pub source_symbol: String,
    /// The target endpoint's symbol (a node in the synced file — re-created on re-extract).
    pub target_symbol: String,
    /// The edge kind discriminant ([`EdgeKind::as_i32`]).
    pub kind: i32,
}

/// One row of the `unresolved_refs` reference ledger (S-011, [ADR-10]),
/// mapped to model types.
///
/// `form` is recovered via [`RefForm::try_from`] and `kind` via
/// [`EdgeKind::try_from`]; a value failing either conversion is a corrupt
/// store, surfaced as an error advising a rebuild ([NFR-RA-08]).
///
/// [ADR-10]: ../../../docs/specs/architecture/decisions/ADR-10.md
/// [NFR-RA-08]: ../../../docs/specs/requirements/NFR-RA-08.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnresolvedRefRow {
    /// The ledger row id.
    pub id: i64,
    /// The file whose (re-)extraction produced this row, if still indexed.
    pub file_id: Option<i64>,
    /// The referencing declaration's stable symbol string.
    pub source_symbol: String,
    /// The reference target text, interpreted per `form`.
    pub target: String,
    /// The in-scope name an import binds (`use a::b` → `b`, `use a::b as c`
    /// → `c`); otherwise the marker extraction gave the row
    /// ([`RefFact::alias`](crate::extract::RefFact::alias): a scope marker, a
    /// qualified call's import binding, a bare call's local-binding mark), or
    /// `None`.
    pub alias: Option<String>,
    /// How `target` is to be interpreted by the resolution pass.
    pub form: RefForm,
    /// The edge kind a successful binding produces.
    pub kind: EdgeKind,
    /// 1-based source line of the reference, when known.
    pub line: Option<i64>,
    /// `true` once the resolution pass has bound this ref to an edge.
    pub resolved: bool,
    /// The cross-artifact relation class ([`ArtifactRelation`] wire token) for an
    /// `ArtifactRef`/`ArtifactBinding` ref (CR-011, [FR-CG-07]); `None` for every
    /// code/doc/access ref. Carried so the resolution pass can label the bound
    /// edge and per-relation-class coverage can group the ledger ([FR-CG-11]).
    ///
    /// [ArtifactRelation]: crate::model::ArtifactRelation
    /// [FR-CG-07]: ../../../docs/specs/requirements/FR-CG-07.md
    /// [FR-CG-11]: ../../../docs/specs/requirements/FR-CG-11.md
    pub payload: Option<String>,
    /// The receiver shape of a Method-form call (S-514, [FR-EX-13]; the
    /// `receiver` column, migration 29) — what the binder dispatches the row
    /// on ([FR-RS-12]) — and `other` on a Path-form row retyped from a proven
    /// Rust receiver (S-587), which binds among its type's methods (S-588).
    /// `None` for every other row, and for a call its plugin marks no receiver
    /// of, which binds as `other`.
    ///
    /// [FR-EX-13]: ../../../docs/specs/requirements/FR-EX-13.md
    /// [FR-RS-12]: ../../../docs/specs/requirements/FR-RS-12.md
    pub receiver: Option<ReceiverShape>,
    /// The wrappers peeled to reach a proven Rust receiver's type (S-587; the
    /// `peeled` column, migration 32): `&`, `&mut`, `Box`, `Arc`, `Rc`,
    /// outermost first, space-joined. The binder refuses a method the wrapper
    /// itself provides (S-588, [FR-RS-42]). `None` for every other row.
    ///
    /// [FR-RS-42]: ../../../docs/specs/requirements/FR-RS-42.md
    pub peeled: Option<String>,
    /// How many arguments a `Calls` row's call passes (S-591, [FR-EX-32]; the
    /// `arg_count` column, migration 33). `None` when it cannot be counted, and
    /// for every non-call row but two: a partial type part's own `Extends` row
    /// (S-557), which records the declaration's type-parameter count, and — in
    /// a language that tells types apart by arity (S-602) — a supertype's or a
    /// `new`'s row, which records the type-argument count it writes.
    ///
    /// [FR-EX-32]: ../../../docs/specs/requirements/FR-EX-32.md
    pub arg_count: Option<u32>,
    /// Whether an import row's declaration is a re-export — a Rust `pub use`
    /// (S-606, [FR-EX-34]; the `exported` column, migration 34), or a
    /// TypeScript-family `export` statement (S-534, `extract::ts_export_rows`):
    /// a `Glob` row is `export * from`, a multi-segment `Path` row a named
    /// re-export `<module>::<name>`, a single-segment `Path` row a local export
    /// alias. `None` for every other row.
    ///
    /// [FR-EX-34]: ../../../docs/specs/requirements/FR-EX-34.md
    pub exported: Option<bool>,
}

/// One function/method node's metric inputs for the quality metrics engine
/// (S-018, [FR-QM-04], [FR-QM-05]).
///
/// The minimal per-function slice the Equality (Gini of cyclomatic complexity)
/// and Redundancy (dead/duplicate ratio) metrics need — captured by Pass 1
/// (complexity) and Pass 3 (verdicts). `None` keeps the tri-state honesty of
/// the native columns: "not computed", never a silent zero ([NFR-CC-04]).
///
/// [FR-QM-04]: ../../../docs/specs/requirements/FR-QM-04.md
/// [FR-QM-05]: ../../../docs/specs/requirements/FR-QM-05.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionMetricRow {
    /// The storage-local rowid handle.
    pub id: NodeId,
    /// Per-function cyclomatic complexity from Pass 1; `None` = not computed.
    pub cyclomatic_complexity: Option<i64>,
    /// The dead-code verdict of the last annotation run ([FR-AN-01]).
    ///
    /// [FR-AN-01]: ../../../docs/specs/requirements/FR-AN-01.md
    pub is_dead: Option<bool>,
    /// The duplicate verdict of the last annotation run ([FR-AN-02]).
    ///
    /// [FR-AN-02]: ../../../docs/specs/requirements/FR-AN-02.md
    pub is_duplicate: Option<bool>,
    /// Physical line count of the definition from Pass 1; `None` = not computed.
    /// A brain-method input ([FR-QM-10]).
    ///
    /// [FR-QM-10]: ../../../docs/specs/requirements/FR-QM-10.md
    pub line_count: Option<i64>,
    /// Per-function maximum block-structure nesting depth from Pass 1 (CR-005,
    /// [FR-EX-07]); `None` = not computed / un-re-extracted. The Nesting input
    /// ([FR-QM-09]) and a brain-method input ([FR-QM-10]).
    ///
    /// [FR-EX-07]: ../../../docs/specs/requirements/FR-EX-07.md
    /// [FR-QM-09]: ../../../docs/specs/requirements/FR-QM-09.md
    pub max_nesting_depth: Option<i64>,
    /// The stable near-clone group id of the last annotation run (CR-005,
    /// [FR-AN-06]) — the minimum node id of the function's near-clone component,
    /// or `None` when it belongs to no near-clone group. The Uniqueness input
    /// ([FR-QM-13]): a function is near-cloned iff this is non-`None`.
    ///
    /// [FR-AN-06]: ../../../docs/specs/requirements/FR-AN-06.md
    /// [FR-QM-13]: ../../../docs/specs/requirements/FR-QM-13.md
    pub clone_group: Option<NodeId>,
    /// Whether the callable carries an implementation body (S-500, [FR-EX-11])
    /// — the `nodes.has_body` column. The Cohesion and Focus method sets read it
    /// (metric-semantics v7, [FR-QM-11], [FR-QM-12]): only a recorded
    /// `Some(false)` is bodyless. `None` — a callable indexed before migration 25
    /// and not re-extracted since — reads as **bodied** (not yet extracted, never
    /// bodyless), so an upgraded store scores exactly as it did until the
    /// re-extraction records the fact ([NFR-CC-04]).
    ///
    /// [FR-EX-11]: ../../../docs/specs/requirements/FR-EX-11.md
    /// [FR-QM-11]: ../../../docs/specs/requirements/FR-QM-11.md
    /// [FR-QM-12]: ../../../docs/specs/requirements/FR-QM-12.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub has_body: Option<bool>,
    /// The member names the callable accesses through its own receiver
    /// (`this.x`, [FR-EX-08]) that bound to no field — the unresolved
    /// `Accesses` rows it is the source of, sorted and distinct. Cohesion links
    /// two methods sharing one such name that their class does not declare, an
    /// inherited member such as React's `this.state` (metric-semantics v9,
    /// [FR-QM-11], [CR-252]).
    ///
    /// [FR-EX-08]: ../../../docs/specs/requirements/FR-EX-08.md
    /// [CR-252]: ../../../docs/requests/CR-252-findings-and-offenders-without-the-noise.md
    pub unbound_accesses: Vec<String>,
}

/// What a re-bless records beside the gate baseline ([FR-GV-16], migration
/// 39). Both `None` on an anchor that records no re-bless.
///
/// [FR-GV-16]: ../../../docs/specs/requirements/FR-GV-16.md
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BaselineRecord {
    /// The `gate --save --reason` text.
    pub reason: Option<String>,
    /// The serialized attribution of the blessed change ([FR-GV-26]).
    ///
    /// [FR-GV-26]: ../../../docs/specs/requirements/FR-GV-26.md
    pub attribution: Option<String>,
}

/// One persisted `metric_snapshots` row (S-018, [FR-QM-07]).
///
/// Raw + normalized per metric, the counts the run scored, the `empty` flag,
/// and the rounded aggregate signal — `None` is the [ADR-12] "n/a" sentinel
/// (empty graph), distinct from a real `0` (zero short-circuit).
///
/// [FR-QM-07]: ../../../docs/specs/requirements/FR-QM-07.md
/// [ADR-12]: ../../../docs/specs/architecture/decisions/ADR-12.md
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MetricSnapshotRow {
    /// The snapshot row id (append-only series order).
    pub id: i64,
    /// Unix-seconds timestamp the snapshot was recorded at.
    pub created_at: i64,
    /// Optional VCS commit pin for the snapshot.
    pub commit_sha: Option<String>,
    /// Vertices in the metric graph the run scored.
    pub node_count: i64,
    /// Edges in the metric graph the run scored.
    pub edge_count: i64,
    /// Production function/method nodes considered by Equality/Redundancy
    /// (`is_test=true` excluded, [FR-QM-08]).
    ///
    /// [FR-QM-08]: ../../../docs/specs/requirements/FR-QM-08.md
    pub function_count: i64,
    /// Test function/method nodes excluded from the production scope
    /// ([FR-QM-08], [FR-QM-07]): the persisted "N test functions excluded"
    /// count. `0` on snapshots written before migration 7.
    ///
    /// [FR-QM-07]: ../../../docs/specs/requirements/FR-QM-07.md
    pub test_function_count: i64,
    /// The metrics-semantics version the snapshot was scored under
    /// ([FR-GV-10]): the gate auto-re-baselines when this differs from the
    /// current [`METRIC_SEMANTICS_VERSION`](crate::metrics::METRIC_SEMANTICS_VERSION).
    /// `1` (test-inclusive) on snapshots written before migration 7.
    ///
    /// [FR-GV-10]: ../../../docs/specs/requirements/FR-GV-10.md
    pub metric_version: i64,
    /// The [ADR-12] empty-graph flag (`node_count == 0`).
    ///
    /// [ADR-12]: ../../../docs/specs/architecture/decisions/ADR-12.md
    pub empty: bool,
    /// Raw + normalized per-metric values, in canonical metric order:
    /// modularity, acyclicity, depth, equality, redundancy ([ADR-08]).
    ///
    /// [ADR-08]: ../../../docs/specs/architecture/decisions/ADR-08.md
    pub modularity_raw: f64,
    /// Normalized modularity `(Q+0.5)/1.5` clamped to [0,1] ([FR-QM-01]).
    ///
    /// [FR-QM-01]: ../../../docs/specs/requirements/FR-QM-01.md
    pub modularity_normalized: f64,
    /// `Some(false)` when Modularity was not applicable ([CR-156], [FR-QM-18]:
    /// fewer than five edges on a v6/v7 row, five cross-module edges since
    /// metric-semantics v8), `Some(true)` when it applied, `None` on a row
    /// persisted before migration 23 (read as applicable). The gate's regression detail
    /// and the evolution series read it so a dimension outside the signal is
    /// never reported as a movement of the signal.
    ///
    /// [CR-156]: ../../../docs/requests/CR-156-modularity-drops-out-of-a-too-small-graph.md
    /// [FR-QM-18]: ../../../docs/specs/requirements/FR-QM-18.md
    pub modularity_applicable: Option<bool>,
    /// Cycle count — multi-node SCCs (`len > 1`) only; self-recursion
    /// excluded ([FR-QM-02], metric-semantics v4).
    ///
    /// [FR-QM-02]: ../../../docs/specs/requirements/FR-QM-02.md
    pub acyclicity_raw: f64,
    /// Normalized acyclicity `1/(1+cycles)`.
    pub acyclicity_normalized: f64,
    /// `Some(false)` when Acyclicity was not applicable ([FR-QM-18]: fewer than
    /// five cross-module edges, metric-semantics v8), `Some(true)` when it
    /// applied, `None` on a row persisted before migration 38 (read as
    /// applicable). Read like [`modularity_applicable`](Self::modularity_applicable).
    ///
    /// [FR-QM-18]: ../../../docs/specs/requirements/FR-QM-18.md
    pub acyclicity_applicable: Option<bool>,
    /// Longest path (vertex count) over the condensation ([FR-QM-03]).
    ///
    /// [FR-QM-03]: ../../../docs/specs/requirements/FR-QM-03.md
    pub depth_raw: f64,
    /// Normalized depth `1/(1+depth/8)`.
    pub depth_normalized: f64,
    /// `Some(false)` when Depth was not applicable ([FR-QM-18]) — the condition
    /// [`acyclicity_applicable`](Self::acyclicity_applicable) records; `None`
    /// on a row persisted before migration 38 (read as applicable).
    ///
    /// [FR-QM-18]: ../../../docs/specs/requirements/FR-QM-18.md
    pub depth_applicable: Option<bool>,
    /// The production metric graph's cross-module edges — the count the three
    /// graph-dimension flags were decided on ([FR-QM-18]); `None` on a row
    /// persisted before migration 38.
    pub cross_module_edges: Option<i64>,
    /// Gini coefficient of per-function cyclomatic complexity ([FR-QM-04]).
    ///
    /// [FR-QM-04]: ../../../docs/specs/requirements/FR-QM-04.md
    pub equality_raw: f64,
    /// Normalized equality `1 − Gini`.
    pub equality_normalized: f64,
    /// Redundant-function ratio (dead or duplicate, counted once)
    /// ([FR-QM-05]).
    ///
    /// [FR-QM-05]: ../../../docs/specs/requirements/FR-QM-05.md
    pub redundancy_raw: f64,
    /// Normalized redundancy `1 − ratio`.
    pub redundancy_normalized: f64,
    /// The effective detection-threshold set hash the snapshot scored under
    /// (CR-005, [FR-QM-14], [BR-25]); `None` on a pre-v3 snapshot. The gate
    /// auto-re-baselines when this differs from the baseline ([FR-GV-10]), wired
    /// in S-045. The five new dimensions' raw+normalized pairs and applicability
    /// flags are persisted ([FR-QM-07]) but not surfaced on this read model — the
    /// fresh-compute [`MetricSnapshot`](crate::models::quality::MetricSnapshot)
    /// carries them for `scan`; this row backs the gate/evolution series.
    ///
    /// [FR-QM-14]: ../../../docs/specs/requirements/FR-QM-14.md
    /// [FR-GV-10]: ../../../docs/specs/requirements/FR-GV-10.md
    pub thresholds_hash: Option<String>,
    /// The rounded 0–10000 signal; `None` = "n/a" (empty graph, [ADR-12]).
    ///
    /// [ADR-12]: ../../../docs/specs/architecture/decisions/ADR-12.md
    pub aggregate_signal: Option<i64>,
}

/// The most-recent persisted `metric_snapshots` row with the **full** CR-005
/// dimension set — every persisted dimension (all ten) plus the five
/// applicability flags: Cohesion's and Focus's ([FR-QM-09]..[FR-QM-14]),
/// Modularity's (migration 23, CR-156), and Acyclicity's and Depth's with the
/// cross-module edge count they were decided on (migration 38, FR-QM-18).
///
/// Unlike [`MetricSnapshotRow`] — which deliberately surfaces only the original
/// five for the gate/evolution series — this read model carries the structural
/// dimensions so the read-only `latest_metrics` accessor ([ADR-28], [CR-018],
/// S-082) can return the last persisted snapshot's whole breakdown to the web
/// dashboard **without re-computing one**: a dashboard GET reflects the last
/// `scan`, it never triggers a new evaluate-and-persist. The `*_normalized`
/// pairs are `None` only on a pre-v3 snapshot (scored before the structural
/// dimensions existed) or — for `cohesion`/`focus` — when the dimension dropped
/// out of the mean (no applicable construct, the [`MetricSnapshot`] n/a sentinel).
///
/// [FR-QM-09]: ../../../docs/specs/requirements/FR-QM-09.md
/// [FR-QM-14]: ../../../docs/specs/requirements/FR-QM-14.md
/// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
/// [CR-018]: ../../../docs/requests/CR-018-web-dashboard-write-on-read.md
/// [`MetricSnapshot`]: crate::models::quality::MetricSnapshot
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LatestMetricSnapshot {
    /// The row's id — what a reader pairs anything else of this snapshot by,
    /// never "latest" again ([CR-135] §3.2). Not serialized.
    ///
    /// [CR-135]: ../../../docs/requests/CR-135-the-health-readout-is-internally-consistent-and-never-stale.md
    #[serde(skip)]
    pub id: i64,
    /// Vertices in the metric graph the run scored.
    pub node_count: i64,
    /// Edges in the metric graph the run scored.
    pub edge_count: i64,
    /// Production function/method nodes considered ([FR-QM-08]).
    pub function_count: i64,
    /// Test function/method nodes excluded from the production scope ([FR-QM-08]).
    pub test_function_count: i64,
    /// The [ADR-12] empty-graph flag (`node_count == 0`).
    pub empty: bool,
    /// The effective detection-threshold set hash the snapshot scored under
    /// ([FR-QM-14]); `None` on a pre-v3 snapshot.
    pub thresholds_hash: Option<String>,
    /// The rounded 0–10000 signal; `None` = "n/a" (empty graph, [ADR-12]).
    pub aggregate_signal: Option<i64>,
    pub modularity_raw: f64,
    pub modularity_normalized: f64,
    /// `Some(false)` when Modularity was not applicable ([CR-156], [FR-QM-18]:
    /// fewer than five edges on a v6/v7 row, five cross-module edges since
    /// metric-semantics v8) — its value columns are still populated; `Some(true)` when it
    /// applied; `None` on a row persisted before migration 23, which the read
    /// path takes as applicable.
    ///
    /// [CR-156]: ../../../docs/requests/CR-156-modularity-drops-out-of-a-too-small-graph.md
    /// [FR-QM-18]: ../../../docs/specs/requirements/FR-QM-18.md
    pub modularity_applicable: Option<bool>,
    pub acyclicity_raw: f64,
    pub acyclicity_normalized: f64,
    /// `Some(false)` when Acyclicity was not applicable ([FR-QM-18]); `None`
    /// before migration 38, read as applicable.
    ///
    /// [FR-QM-18]: ../../../docs/specs/requirements/FR-QM-18.md
    pub acyclicity_applicable: Option<bool>,
    pub depth_raw: f64,
    pub depth_normalized: f64,
    /// `Some(false)` when Depth was not applicable ([FR-QM-18]); `None` before
    /// migration 38, read as applicable.
    pub depth_applicable: Option<bool>,
    /// The cross-module edge count the three graph-dimension flags were decided
    /// on ([FR-QM-18]); `None` before migration 38.
    pub cross_module_edges: Option<i64>,
    pub equality_raw: f64,
    pub equality_normalized: f64,
    pub redundancy_raw: f64,
    pub redundancy_normalized: f64,
    /// Structural dimensions (CR-005). `None` on a pre-v3 snapshot.
    pub nesting_raw: Option<f64>,
    pub nesting_normalized: Option<f64>,
    pub conciseness_raw: Option<f64>,
    pub conciseness_normalized: Option<f64>,
    /// Cohesion / Focus value columns are `None` exactly when their
    /// `*_applicable` flag is `Some(false)` — the applicability drop-out.
    pub cohesion_raw: Option<f64>,
    pub cohesion_normalized: Option<f64>,
    pub cohesion_applicable: Option<bool>,
    pub focus_raw: Option<f64>,
    pub focus_normalized: Option<f64>,
    pub focus_applicable: Option<bool>,
    pub uniqueness_raw: Option<f64>,
    pub uniqueness_normalized: Option<f64>,
    /// The worst-offender lists persisted **with this snapshot** ([FR-QM-15]),
    /// read by its id. `recorded` is `false` — every list empty — on a snapshot
    /// written before migration 26, which recorded none; `values_recorded` is
    /// `false` on one written before migration 37 ([FR-QM-19]).
    ///
    /// [FR-QM-15]: ../../../docs/specs/requirements/FR-QM-15.md
    /// [FR-QM-19]: ../../../docs/specs/requirements/FR-QM-19.md
    pub worst_offenders: WorstOffenders,
    /// The "where to start" ranking and outlier labels persisted **with this
    /// snapshot** ([FR-QM-17]), read by its id. `recorded` is `false` — both
    /// lists empty — on a snapshot written before migration 36.
    ///
    /// [FR-QM-17]: ../../../docs/specs/requirements/FR-QM-17.md
    pub where_to_start: WhereToStart,
}

/// The fields needed to insert a `metric_snapshots` row (S-018, [FR-QM-07]).
///
/// Same shape as [`MetricSnapshotRow`] minus the assigned `id`. The table is
/// append-only: there is deliberately no update primitive.
///
/// [FR-QM-07]: ../../../docs/specs/requirements/FR-QM-07.md
#[derive(Debug, Clone)]
pub struct NewMetricSnapshot<'a> {
    /// Unix-seconds timestamp to record.
    pub created_at: i64,
    /// Optional VCS commit pin.
    pub commit_sha: Option<&'a str>,
    /// Vertices in the metric graph.
    pub node_count: i64,
    /// Edges in the metric graph.
    pub edge_count: i64,
    /// Production function/method nodes considered ([FR-QM-08]).
    ///
    /// [FR-QM-08]: ../../../docs/specs/requirements/FR-QM-08.md
    pub function_count: i64,
    /// Test function/method nodes excluded from the production scope
    /// ([FR-QM-08], [FR-QM-07]).
    ///
    /// [FR-QM-07]: ../../../docs/specs/requirements/FR-QM-07.md
    pub test_function_count: i64,
    /// The metrics-semantics version this snapshot was scored under
    /// ([FR-GV-10]) — the gate's re-baseline trigger.
    ///
    /// [FR-GV-10]: ../../../docs/specs/requirements/FR-GV-10.md
    pub metric_version: i64,
    /// The [ADR-12] empty-graph flag.
    ///
    /// [ADR-12]: ../../../docs/specs/architecture/decisions/ADR-12.md
    pub empty: bool,
    /// Raw Newman Q.
    pub modularity_raw: f64,
    /// Normalized modularity.
    pub modularity_normalized: f64,
    /// `Some(false)` when Modularity was not applicable ([CR-156], [FR-QM-18]:
    /// fewer than [`MIN_CROSS_MODULE_EDGES`](crate::metrics::MIN_CROSS_MODULE_EDGES)
    /// cross-module edges since metric-semantics v8) —
    /// the raw/normalized pair above is persisted either way; `Some(true)` when
    /// it applied. `None` persists `NULL`, the pre-migration-23 reading.
    ///
    /// [CR-156]: ../../../docs/requests/CR-156-modularity-drops-out-of-a-too-small-graph.md
    /// [FR-QM-18]: ../../../docs/specs/requirements/FR-QM-18.md
    pub modularity_applicable: Option<bool>,
    /// Raw cycle count.
    pub acyclicity_raw: f64,
    /// Normalized acyclicity.
    pub acyclicity_normalized: f64,
    /// `Some(false)` when Acyclicity was not applicable ([FR-QM-18]: fewer than
    /// [`MIN_CROSS_MODULE_EDGES`](crate::metrics::MIN_CROSS_MODULE_EDGES)
    /// cross-module edges) — the pair above is persisted either way. `None`
    /// persists `NULL`, the pre-migration-38 reading.
    ///
    /// [FR-QM-18]: ../../../docs/specs/requirements/FR-QM-18.md
    pub acyclicity_applicable: Option<bool>,
    /// Raw condensed longest-path depth.
    pub depth_raw: f64,
    /// Normalized depth.
    pub depth_normalized: f64,
    /// `Some(false)` when Depth was not applicable ([FR-QM-18]), as for
    /// [`acyclicity_applicable`](Self::acyclicity_applicable).
    pub depth_applicable: Option<bool>,
    /// The cross-module edge count the three graph-dimension flags were decided
    /// on ([FR-QM-18]); `None` persists `NULL`.
    pub cross_module_edges: Option<i64>,
    /// Raw Gini coefficient.
    pub equality_raw: f64,
    /// Normalized equality.
    pub equality_normalized: f64,
    /// Raw redundant-function ratio.
    pub redundancy_raw: f64,
    /// Normalized redundancy.
    pub redundancy_normalized: f64,
    /// Raw deep-nesting ratio (CR-005, [FR-QM-09]); `None` persists `NULL` on a
    /// pre-v3 snapshot that scored only the original five.
    ///
    /// [FR-QM-09]: ../../../docs/specs/requirements/FR-QM-09.md
    pub nesting_raw: Option<f64>,
    /// Normalized Nesting `(1 − ratio)` floored at 0.01 ([FR-QM-09]).
    pub nesting_normalized: Option<f64>,
    /// Raw brain-method ratio (CR-005, [FR-QM-10]).
    ///
    /// [FR-QM-10]: ../../../docs/specs/requirements/FR-QM-10.md
    pub conciseness_raw: Option<f64>,
    /// Normalized Conciseness `(1 − ratio)` floored at 0.01 ([FR-QM-10]).
    pub conciseness_normalized: Option<f64>,
    /// Raw mean `1/LCOM4` over production classes (CR-005, [FR-QM-11]); `None`
    /// when Cohesion dropped out of the mean (no applicable classes) or on a
    /// pre-v3 snapshot.
    ///
    /// [FR-QM-11]: ../../../docs/specs/requirements/FR-QM-11.md
    pub cohesion_raw: Option<f64>,
    /// Normalized Cohesion (the mean `1/LCOM4`) floored at 0.01 ([FR-QM-11]);
    /// `None` exactly when [`cohesion_applicable`](Self::cohesion_applicable) is
    /// `Some(false)`.
    pub cohesion_normalized: Option<f64>,
    /// `Some(true)` when Cohesion applied to the mean, `Some(false)` when it
    /// dropped out (no applicable classes, [FR-QM-11]/[FR-QM-14]); `None` on a
    /// pre-v3 snapshot.
    pub cohesion_applicable: Option<bool>,
    /// Raw god-container ratio (CR-005, [FR-QM-12]); `None` when Focus dropped
    /// out (no class-like containers) or on a pre-v3 snapshot.
    ///
    /// [FR-QM-12]: ../../../docs/specs/requirements/FR-QM-12.md
    pub focus_raw: Option<f64>,
    /// Normalized Focus `(1 − ratio)` floored at 0.01 ([FR-QM-12]); `None`
    /// exactly when [`focus_applicable`](Self::focus_applicable) is `Some(false)`.
    pub focus_normalized: Option<f64>,
    /// `Some(true)` when Focus applied, `Some(false)` when it dropped out (no
    /// class-like containers, [FR-QM-12]/[FR-QM-14]); `None` on a pre-v3 snapshot.
    pub focus_applicable: Option<bool>,
    /// Raw near-clone ratio (CR-005, [FR-QM-13]).
    ///
    /// [FR-QM-13]: ../../../docs/specs/requirements/FR-QM-13.md
    pub uniqueness_raw: Option<f64>,
    /// Normalized Uniqueness `(1 − ratio)` floored at 0.01 ([FR-QM-13]).
    pub uniqueness_normalized: Option<f64>,
    /// The effective detection-threshold set hash this run scored under (CR-005,
    /// [FR-QM-14], [BR-25]); `None` on a pre-v3 snapshot. A baseline mismatch
    /// triggers the announced auto-re-baseline ([FR-GV-10]).
    ///
    /// [FR-QM-14]: ../../../docs/specs/requirements/FR-QM-14.md
    pub thresholds_hash: Option<&'a str>,
    /// The rounded signal; `None` persists SQL `NULL` (the "n/a" sentinel).
    pub aggregate_signal: Option<i64>,
}

/// One function/method node's constraint inputs for the governance evaluator
/// (S-020, [FR-GV-02]): the per-function point-query slice behind `max_cc`
/// and `max_fn_lines`, carrying the name/file/line needed for an actionable
/// violation message.
///
/// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionConstraintRow {
    /// The storage-local rowid handle.
    pub id: NodeId,
    /// The human-facing name.
    pub name: String,
    /// The defining file's path, if the node is bound to one.
    pub file_path: Option<String>,
    /// 1-based start line of the declaration, when recorded.
    pub start_line: Option<i64>,
    /// Per-function cyclomatic complexity from Pass 1; `None` = not computed.
    pub cyclomatic_complexity: Option<i64>,
    /// Per-function line count from Pass 1; `None` = not computed.
    pub line_count: Option<i64>,
}

/// The store-integrity slice behind the `health` tool (S-020): the schema
/// version, readable from the RO pool. The FTS coherence half lives on
/// [`BatchWriter::fts_integrity_check`] — FTS5's `'integrity-check'` command
/// is an INSERT, so it needs the writer connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreHealth {
    /// The `PRAGMA user_version` ([FR-DB-04]).
    ///
    /// [FR-DB-04]: ../../../docs/specs/requirements/FR-DB-04.md
    pub schema_version: i64,
}

/// The fast **structural-integrity** census behind `doctor` ([FR-GV-18],
/// [NFR-RA-13], [ADR-46]): the invariant `node_count == distinct(symbol_id)`
/// plus the three orphan-row counters (dangling `file_id`, dangling edge
/// endpoints, orphan shingles).
///
/// Computed by [`structural_report`] in O(a handful of indexed queries) — every
/// counter is a `COUNT` over a primary-key anti-join or the `symbol_id`
/// distinct, so it never scans row content and stays inside the point-query
/// latency budget ([NFR-PE-01]). A healthy graph reports every counter zero
/// ([`StructuralReport::is_ok`]); any non-zero counter is a structural fault the
/// gate hard-fails on, independent of the metric signal ([FR-GV-02],
/// [FR-GV-05]).
///
/// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
/// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
/// [FR-GV-05]: ../../../docs/specs/requirements/FR-GV-05.md
/// [NFR-RA-13]: ../../../docs/specs/requirements/NFR-RA-13.md
/// [NFR-PE-01]: ../../../docs/specs/requirements/NFR-PE-01.md
/// [ADR-46]: ../../../docs/specs/architecture/decisions/ADR-46.md
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct StructuralReport {
    /// `COUNT(*) FROM nodes` — total graph vertices.
    pub node_count: u64,
    /// `COUNT(DISTINCT symbol_id) FROM nodes` — the one-node-per-symbol target.
    pub distinct_symbol_ids: u64,
    /// `node_count − distinct_symbol_ids`: nodes leaked past the
    /// one-per-`symbol_id` invariant (Channel A, [ADR-46]). Zero on a healthy
    /// graph — the `UNIQUE(symbol_id)` schema constraint makes it structurally
    /// unreachable at write time; a non-zero value is a regression that bypassed
    /// the constraint.
    pub duplicate_symbol_nodes: u64,
    /// Nodes whose non-NULL `file_id` references a `files` row that is gone
    /// (the `ON DELETE SET NULL` FK should nullify these — a survivor is drift).
    pub dangling_file_refs: u64,
    /// Edges whose `source` or `target` references a missing `nodes` row (the
    /// `ON DELETE CASCADE` FK should reap these — a survivor is drift).
    pub dangling_edge_endpoints: u64,
    /// Shingles whose `node_id` references a missing `nodes` row (the
    /// `ON DELETE CASCADE` FK should reap these — a survivor is drift).
    pub orphan_shingles: u64,
}

impl StructuralReport {
    /// `true` when the graph holds exactly one node per `symbol_id` and carries
    /// no orphan rows — the [NFR-RA-13] invariant.
    ///
    /// [NFR-RA-13]: ../../../docs/specs/requirements/NFR-RA-13.md
    pub fn is_ok(&self) -> bool {
        self.duplicate_symbol_nodes == 0
            && self.dangling_file_refs == 0
            && self.dangling_edge_endpoints == 0
            && self.orphan_shingles == 0
    }

    /// One human-readable fault line per non-zero counter, in a deterministic
    /// order ([NFR-RA-06]) — empty when [`is_ok`](Self::is_ok). Each names the
    /// structural fault so the gate can report *which* invariant broke, even
    /// when the metric signal is unchanged ([FR-GV-18]).
    ///
    /// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    pub fn faults(&self) -> Vec<String> {
        let mut faults = Vec::new();
        if self.duplicate_symbol_nodes > 0 {
            faults.push(format!(
                "{} duplicate-symbol node(s): {} nodes for {} distinct symbol_id \
                 (expected one node per symbol_id, NFR-RA-13)",
                self.duplicate_symbol_nodes, self.node_count, self.distinct_symbol_ids
            ));
        }
        if self.dangling_file_refs > 0 {
            faults.push(format!(
                "{} node(s) with a dangling file_id (references a missing files row)",
                self.dangling_file_refs
            ));
        }
        if self.dangling_edge_endpoints > 0 {
            faults.push(format!(
                "{} edge(s) with a dangling endpoint (source or target references a missing node)",
                self.dangling_edge_endpoints
            ));
        }
        if self.orphan_shingles > 0 {
            faults.push(format!(
                "{} orphan shingle(s) (node_id references a missing node)",
                self.orphan_shingles
            ));
        }
        faults
    }
}

/// Run the fast structural-integrity census ([`StructuralReport`], [FR-GV-18],
/// [NFR-RA-13]) against `conn`.
///
/// Four indexed reads: the `node_count`/`distinct(symbol_id)` pair in one row,
/// then three primary-key anti-joins (dangling `file_id`, dangling edge
/// endpoints, orphan shingles). No table-content scan — it stays inside the
/// point-query budget ([NFR-PE-01]) and is cheap enough to assert in debug
/// builds after every `index`/`sync`.
///
/// Free-standing over `&Connection` so both the read seam
/// ([`GraphStore::structural_check`]) and the writer connection can call it, and
/// so a hand-built dirty fixture can be checked directly in tests.
///
/// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
/// [NFR-RA-13]: ../../../docs/specs/requirements/NFR-RA-13.md
/// [NFR-PE-01]: ../../../docs/specs/requirements/NFR-PE-01.md
pub(crate) fn structural_report(conn: &Connection) -> Result<StructuralReport> {
    let (node_count, distinct_symbol_ids) = conn
        .query_row(
            "SELECT COUNT(*), COUNT(DISTINCT symbol_id) FROM nodes",
            [],
            |row| Ok((row.get::<_, i64>(0)? as u64, row.get::<_, i64>(1)? as u64)),
        )
        .context("counting nodes vs distinct symbol_id")?;

    // Anti-joins keyed on primary keys (files.id, nodes.id): SQLite serves each
    // NOT EXISTS as an indexed lookup, so these never scan row content.
    let dangling_file_refs = conn
        .query_row(
            "SELECT COUNT(*) FROM nodes n \
             WHERE n.file_id IS NOT NULL \
               AND NOT EXISTS (SELECT 1 FROM files f WHERE f.id = n.file_id)",
            [],
            |row| row.get::<_, i64>(0),
        )
        .context("counting dangling file_id references")? as u64;

    let dangling_edge_endpoints = conn
        .query_row(
            "SELECT COUNT(*) FROM edges e \
             WHERE NOT EXISTS (SELECT 1 FROM nodes n WHERE n.id = e.source) \
                OR NOT EXISTS (SELECT 1 FROM nodes n WHERE n.id = e.target)",
            [],
            |row| row.get::<_, i64>(0),
        )
        .context("counting dangling edge endpoints")? as u64;

    let orphan_shingles = conn
        .query_row(
            "SELECT COUNT(*) FROM shingles sh \
             WHERE NOT EXISTS (SELECT 1 FROM nodes n WHERE n.id = sh.node_id)",
            [],
            |row| row.get::<_, i64>(0),
        )
        .context("counting orphan shingles")? as u64;

    Ok(StructuralReport {
        node_count,
        distinct_symbol_ids,
        duplicate_symbol_nodes: node_count.saturating_sub(distinct_symbol_ids),
        dangling_file_refs,
        dangling_edge_endpoints,
        orphan_shingles,
    })
}

/// The singleton `rules_cache` row ([FR-GV-01], S-020): the blake3 hash of
/// `rules.toml` and its parsed JSON, so an unchanged contract skips the TOML
/// parse + validation on re-run.
///
/// [FR-GV-01]: ../../../docs/specs/requirements/FR-GV-01.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RulesCacheRow {
    /// blake3 hex hash of the `rules.toml` content the cache was built from.
    pub rules_hash: String,
    /// The parsed [`Rules`](crate::config::Rules) as canonical JSON.
    pub parsed_json: String,
}

/// One persisted `violations` row (S-020, [FR-GV-02], SRS §5.1).
///
/// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ViolationRow {
    /// The row id (insertion order within the run).
    pub id: i64,
    /// The metric snapshot persisted by the same run (`scan`), if any.
    pub snapshot_id: Option<i64>,
    /// `"constraint"`, `"layer"`, or `"boundary"`.
    pub rule_type: String,
    /// The rule key (e.g. `max_cc`, `layer-ordering`, `boundary:a->b`).
    pub rule_key: String,
    /// The offending node, when the violation points at one.
    pub node_id: Option<i64>,
    /// The offending file, when the violation points at one.
    pub file: Option<String>,
    pub message: String,
    /// `"error"` or `"warning"` ([FR-GV-03]).
    ///
    /// [FR-GV-03]: ../../../docs/specs/requirements/FR-GV-03.md
    pub severity: String,
    /// Unix-seconds timestamp of the run that wrote this row — the same value
    /// for every row of one run, and the same value as that run's
    /// [`CheckRunRow::ran_at`] ([FR-GV-21]: one run, one time).
    ///
    /// Stored `NOT NULL` since S-020 and, until [CR-096], never selected back:
    /// exposing it is what lets a store written **before** the marker
    /// migration still date its existing findings.
    ///
    /// [CR-096]: ../../../docs/requests/CR-096-recorded-check-marker.md
    /// [FR-GV-21]: ../../../docs/specs/requirements/FR-GV-21.md
    pub created_at: i64,
}

/// The singleton `check_run` marker ([FR-GV-21], S-313, [CR-096]): the record
/// that a [`check_rules`](crate::Engine::check_rules) run **happened**.
///
/// `violations` is replaced wholesale per run ([FR-GV-02] idempotence), so a
/// run that finds nothing leaves a table byte-identical to a project that was
/// never checked. This row is the only place that distinction lives — its
/// **absence** means no run has happened, never a clean result.
///
/// Upserted, never appended (BR-40): it records the last run, not a history.
/// [FR-GV-06](../../../docs/specs/requirements/FR-GV-06.md) already owns the
/// quality time series.
///
/// [CR-096]: ../../../docs/requests/CR-096-recorded-check-marker.md
/// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
/// [FR-GV-21]: ../../../docs/specs/requirements/FR-GV-21.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckRunRow {
    /// Unix-seconds timestamp the run happened at — identical to the
    /// `created_at` of every [`ViolationRow`] the same transaction wrote.
    pub ran_at: i64,
    /// **What `HEAD` was when this ran, and nothing more.**
    ///
    /// It supports "has the tree moved since this ran". It does **not** mean
    /// "these findings were introduced by that commit": the run scored
    /// whatever was in the working tree, which may never have been committed
    /// and is not compared against here. The same field on `metric_snapshots`
    /// was once read the second way and produced a confidently wrong
    /// attribution ([CR-095] review, finding 12, retracted), which is why the
    /// meaning is documented on the field rather than only in the CR.
    ///
    /// `None` when `HEAD` does not resolve (no git, no commits). A consumer
    /// then omits the tree comparison rather than substituting a placeholder.
    ///
    /// [CR-095]: ../../../docs/requests/CR-095-session-start-quality-readout.md
    pub commit_sha: Option<String>,
    /// How many `violations` rows the same transaction wrote. `0` with no rows
    /// is a **recorded clean run** — the case the empty table cannot express.
    pub violation_count: i64,
    /// How many rules the run evaluated — [`violation_count`]'s **denominator**
    /// (S-437, [CR-140] §3.1, [NFR-CC-04]).
    ///
    /// `Some(0)` is a run that evaluated nothing, which is not a clean result
    /// however few violations it found ([FR-GV-03]: *"'Clean' means a contract
    /// was evaluated and held; it never means nothing was evaluated"*).
    ///
    /// `None` means the marker was written **before** migration 21 and records
    /// no evaluated set — *unknown*, never zero. A consumer renders the
    /// distinction rather than collapsing it, which is the misreading this
    /// field exists to remove.
    ///
    /// [`violation_count`]: CheckRunRow::violation_count
    /// [CR-140]: ../../../docs/requests/CR-140-the-recorded-check-marker-carries-what-it-evaluated.md
    /// [FR-GV-03]: ../../../docs/specs/requirements/FR-GV-03.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub checked_rules: Option<i64>,
    /// Whether a rules contract was authored at all when the run happened
    /// (S-437, [CR-140] §3.1).
    ///
    /// Not derivable from [`checked_rules`]: a **present** contract authoring
    /// **zero** rules is what [`logos init`](crate::init)'s default template
    /// produces, and it is a different state from no contract at all — the
    /// first is a configured project, the second an unconfigured one. Both
    /// record `checked_rules = Some(0)`.
    ///
    /// `None` means the marker predates migration 21, as for [`checked_rules`].
    ///
    /// [`checked_rules`]: CheckRunRow::checked_rules
    /// [CR-140]: ../../../docs/requests/CR-140-the-recorded-check-marker-carries-what-it-evaluated.md
    pub rules_present: Option<bool>,
    /// Which operation wrote this marker: [`CHECK_RUN_OP_CHECK`] or
    /// [`CHECK_RUN_OP_SCAN`] (S-437, [CR-140] §3.1).
    ///
    /// [`scan`](crate::Engine::scan) shares the write site with
    /// [`check_rules`](crate::Engine::check_rules) — it replaces the violation
    /// set wholesale too — so the marker alone could not say which run it
    /// describes, and a consumer that attributed it to one of them was
    /// guessing.
    ///
    /// `None` means the marker predates migration 21, as for [`checked_rules`].
    ///
    /// [`checked_rules`]: CheckRunRow::checked_rules
    /// [CR-140]: ../../../docs/requests/CR-140-the-recorded-check-marker-carries-what-it-evaluated.md
    pub operation: Option<String>,
}

/// [`CheckRunRow::operation`] for a [`check_rules`](crate::Engine::check_rules)
/// run — the enforce leg ([FR-GV-02]).
///
/// The spelling is pinned by migration 21's `CHECK (operation IN
/// ('check','scan'))`, so a value outside this pair fails the write rather
/// than storing something no reader knows.
///
/// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
pub const CHECK_RUN_OP_CHECK: &str = "check";

/// [`CheckRunRow::operation`] for a [`scan`](crate::Engine::scan) run — the
/// report leg, which replaces the violation set wholesale and so writes the
/// marker too ([FR-GV-09]).
///
/// [FR-GV-09]: ../../../docs/specs/requirements/FR-GV-09.md
pub const CHECK_RUN_OP_SCAN: &str = "scan";

/// The marker fields one [`replace_violations`] call records — the run, not
/// its findings ([FR-GV-21], S-437, [CR-140] §3.1).
///
/// Grouped into a struct rather than passed as five positional arguments for
/// the reason [`NewViolation`] is: `commit_sha` and `operation` are both
/// string-shaped and `ran_at` and `checked_rules` are both integer-shaped, so
/// a transposed pair would type-check. Every field describes **one** run,
/// which is what makes them one parameter.
///
/// [`replace_violations`]: BatchWriter::replace_violations
/// [CR-140]: ../../../docs/requests/CR-140-the-recorded-check-marker-carries-what-it-evaluated.md
/// [FR-GV-21]: ../../../docs/specs/requirements/FR-GV-21.md
#[derive(Debug, Clone, Copy)]
pub struct NewCheckRun<'a> {
    /// Unix-seconds timestamp of the run, stamped on every violation row it
    /// writes as that row's `created_at` ([FR-GV-21]: one run, one time).
    pub ran_at: i64,
    /// `HEAD` at write time, or `None` when it does not resolve. See
    /// [`CheckRunRow::commit_sha`] for what this does and does not mean.
    pub commit_sha: Option<&'a str>,
    /// How many rules the run evaluated — the denominator of the violation
    /// count. See [`CheckRunRow::checked_rules`].
    pub checked_rules: u32,
    /// Whether a rules contract was authored at all. See
    /// [`CheckRunRow::rules_present`].
    pub rules_present: bool,
    /// [`CHECK_RUN_OP_CHECK`] or [`CHECK_RUN_OP_SCAN`]. Any other value is
    /// refused by migration 21's `CHECK`.
    pub operation: &'a str,
}

/// The fields needed to insert a `violations` row (S-020, [FR-GV-02]).
///
/// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
#[derive(Debug, Clone)]
pub struct NewViolation<'a> {
    /// The metric snapshot persisted by the same run, if any.
    pub snapshot_id: Option<i64>,
    /// `"constraint"`, `"layer"`, or `"boundary"`.
    pub rule_type: &'a str,
    /// The rule key.
    pub rule_key: &'a str,
    /// The offending node, when known.
    pub node_id: Option<i64>,
    /// The offending file, when known.
    pub file: Option<&'a str>,
    pub message: &'a str,
    /// `"error"` or `"warning"`.
    pub severity: &'a str,
}

/// A file's committed-configuration contribution, as the store records it
/// (S-380, [FR-WS-19]): the profile the source declares and its canonical
/// key → value pairs.
///
/// The graph-store-local row shape, following `NewNode`/`NewUnresolvedRef`. The
/// extraction engine produces a `ConfigSourceFact`; [`pipeline`](crate::pipeline)
/// adapts it to this on the way in, so the store depends on `model` alone and
/// the documented extraction → pipeline → store direction is preserved.
///
/// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
#[derive(Debug, Clone, Copy)]
pub struct NewConfigSource<'a> {
    /// The `application-<profile>` profile, or `None` for the unprofiled source.
    pub profile: Option<&'a str>,
    /// Canonical key → value pairs. A key a multi-document source defines twice
    /// appears once per distinct value.
    pub values: &'a [(&'a str, &'a str)],
}

/// One build manifest and the artifact facts it yields, as the store records
/// them (S-462, [CR-148] §3.2 A, migration 22).
///
/// The store-local row shape, following [`NewConfigSource`]: the extraction
/// engine produces a
/// [`ManifestFacts`](crate::extract::build_manifest::ManifestFacts) and
/// [`pipeline`](crate::pipeline) adapts it to this on the way in, so the store
/// depends on no extraction type. The string fields carry migration 22's
/// vocabulary, which its CHECKs enforce.
///
/// [CR-148]: ../../../docs/requests/CR-148-build-manifests-yield-a-build-dependency-relation.md
#[derive(Debug, Clone)]
pub struct NewBuildManifest<'a> {
    /// Project-relative path of the manifest.
    pub path: &'a str,
    /// `maven` or `gradle`.
    pub format: &'a str,
    /// blake3 of the manifest text; `None` only when it could not be read.
    pub content_hash: Option<&'a str>,
    /// `read`, `malformed` or `unreadable`.
    pub status: &'a str,
    /// Why it yielded no facts; `None` exactly when `status` is `read`.
    pub detail: Option<&'a str>,
    /// What it produces and references.
    pub artifacts: Vec<NewBuildArtifact<'a>>,
}

/// One produced or referenced artifact of a [`NewBuildManifest`].
#[derive(Debug, Clone, Copy)]
pub struct NewBuildArtifact<'a> {
    /// `produced` or `referenced`.
    pub role: &'a str,
    /// `parent`, `dependency`, `managed` or `bom-import`; `None` when produced.
    pub kind: Option<&'a str>,
    pub group_id: Option<&'a str>,
    pub artifact_id: Option<&'a str>,
    pub version: Option<&'a str>,
    pub scope: Option<&'a str>,
    pub project_path: Option<&'a str>,
    /// `resolved`, `version-refused` or `refused`.
    pub resolution: &'a str,
    /// Why it did not resolve; `None` exactly when `resolution` is `resolved`.
    pub reason: Option<&'a str>,
}

/// One persisted build manifest with its artifact facts (S-462, migration 22) —
/// the member-local fact base the workspace build relation joins ([ADR-69]
/// point 2).
///
/// [ADR-69]: ../../../docs/specs/architecture/decisions/ADR-69.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuildManifestRow {
    pub path: String,
    pub format: String,
    pub content_hash: Option<String>,
    pub status: String,
    pub detail: Option<String>,
    /// In insertion order: the produced fact first, then references grouped by
    /// kind (Maven `<parent>`, `<dependencies>`, `<dependencyManagement>`, each
    /// in document order; Gradle in document order).
    pub artifacts: Vec<BuildArtifactRow>,
}

/// One persisted artifact fact of a [`BuildManifestRow`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuildArtifactRow {
    pub role: String,
    pub kind: Option<String>,
    pub group_id: Option<String>,
    pub artifact_id: Option<String>,
    pub version: Option<String>,
    pub scope: Option<String>,
    pub project_path: Option<String>,
    pub resolution: String,
    pub reason: Option<String>,
}

/// One declared type as the store records it (S-472, migration 24) — the
/// store-local row shape, following [`NewBuildArtifact`]: the extraction engine
/// produces a
/// [`SourceType`](crate::extract::declared_types::SourceType) or an
/// [`AvroType`](crate::extract::declared_types::AvroType) and
/// [`pipeline`](crate::pipeline) adapts it here, so the store depends on no
/// extraction type. The string fields carry migration 24's vocabulary, which
/// its CHECKs enforce.
#[derive(Debug, Clone, Copy)]
pub struct NewDeclaredType<'a> {
    /// The type's own name, as declared.
    pub name: &'a str,
    /// The dotted fully-qualified name; `None` exactly when refused.
    pub fqn: Option<&'a str>,
    /// `class`, `interface`, `enum` or `record`.
    pub kind: &'a str,
    /// The declaring node's symbol — a source fact only.
    pub symbol: Option<&'a str>,
    /// `main` or `test` — a source fact only.
    pub tree: Option<&'a str>,
    /// Why no name is recorded; `None` exactly when `fqn` is `Some`.
    pub reason: Option<&'a str>,
}

/// One Avro schema file and the types it declares, as the store records them
/// (S-472, migration 24).
#[derive(Debug, Clone)]
pub struct NewAvroSchema<'a> {
    /// Project-relative path of the schema.
    pub path: &'a str,
    /// blake3 of the schema text; `None` only when it could not be read.
    pub content_hash: Option<&'a str>,
    /// `read`, `malformed` or `unreadable`.
    pub status: &'a str,
    /// Why it yielded no type; `None` exactly when `status` is `read`.
    pub detail: Option<&'a str>,
    /// Its records and enums, in document order.
    pub types: Vec<NewDeclaredType<'a>>,
}

/// One persisted declared type with its provenance (S-472, migration 24) — the
/// member-local fact the workspace type index is built from ([ADR-70] point 2).
///
/// [ADR-70]: ../../../docs/specs/architecture/decisions/ADR-70.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeclaredTypeRow {
    /// `source` (a Java/Kotlin file) or `avro` (an `.avsc` schema).
    pub origin: String,
    /// Project-relative path of the declaring file or schema — the provenance.
    pub path: String,
    pub name: String,
    /// The dotted fully-qualified name; `None` exactly when refused.
    pub fqn: Option<String>,
    pub kind: String,
    /// The declaring node's symbol; `None` for an Avro type.
    pub symbol: Option<String>,
    /// `main` or `test`; `None` for an Avro type.
    pub tree: Option<String>,
    /// `resolved` or `refused`.
    pub resolution: String,
    pub reason: Option<String>,
}

/// One still-unresolved `Imports`/`TypeUses` ledger row of a Java or Kotlin
/// file, with the file's path (S-473) — what the workspace's cross-member type
/// overlay matches against every member's declared types ([ADR-70] point 2).
///
/// [ADR-70]: ../../../docs/specs/architecture/decisions/ADR-70.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TypeRefRow {
    /// The referencing declaration's stable symbol string.
    pub source_symbol: String,
    /// Project-relative path of the importing file.
    pub path: String,
    /// 1-based source line of the reference, when known.
    pub line: Option<i64>,
    /// The target as the ledger holds it (`com::x::Svc` for an import).
    pub target: String,
    /// [`EdgeKind::Imports`] or [`EdgeKind::TypeUses`].
    pub kind: EdgeKind,
}

/// One persisted Avro schema, read or not (S-472, migration 24) — the
/// "schemas read, of schemas found" denominator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AvroSchemaRow {
    pub path: String,
    pub content_hash: Option<String>,
    pub status: String,
    pub detail: Option<String>,
}

/// One node's arity facts as [`GraphStore::node_arities`] reads them (S-591):
/// the node, its parameter range and whether it takes `self`, each `None`
/// where unknown.
pub type NodeArity = (NodeId, Option<ParamRange>, Option<bool>);

/// One node's associated-item facts as [`GraphStore::node_item_facts`] reads
/// them (S-606, [FR-EX-34]): each `None`/`false` where not recorded.
///
/// [FR-EX-34]: ../../../docs/specs/requirements/FR-EX-34.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeItemFacts {
    /// The node.
    pub id: NodeId,
    /// How the callable writes its receiver (`nodes.receiver_mode`).
    pub receiver_mode: Option<ReceiverMode>,
    /// An enum's variant names, space-joined (`nodes.variants`).
    pub variants: Option<String>,
    /// Whether the node is a required signature (`nodes.signature`).
    pub signature: bool,
}

/// The fields needed to record one impl block's header (S-606, [CR-202] F1;
/// the `impl_blocks` table, migration 34).
///
/// [CR-202]: ../../../docs/requests/CR-202-one-rust-associated-item-lookup.md
#[derive(Debug, Clone, Copy)]
pub struct NewImplBlock<'a> {
    /// 1-based first line of the block.
    pub start_line: u32,
    /// 1-based last line of the block.
    pub end_line: u32,
    /// The self type as written, generics stripped; a reference's referent.
    pub self_type: &'a str,
    /// Whether the self type is a reference (`&T`, `&mut T`).
    pub self_ref: bool,
    /// The trait a trait impl implements; `None` for an inherent impl.
    pub trait_path: Option<&'a str>,
    /// An `impl Deref`'s `type Target`; `None` otherwise.
    pub deref_target: Option<&'a str>,
}

/// One recorded impl block as [`GraphStore::impl_blocks`] reads it (S-606,
/// [CR-202] F1): the [`NewImplBlock`] fields and the declaring file's path.
///
/// [CR-202]: ../../../docs/requests/CR-202-one-rust-associated-item-lookup.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImplBlockRow {
    /// The declaring file's project-relative path.
    pub file_path: String,
    /// 1-based first line of the block.
    pub start_line: u32,
    /// 1-based last line of the block.
    pub end_line: u32,
    /// The self type as written, generics stripped; a reference's referent.
    pub self_type: String,
    /// Whether the self type is a reference (`&T`, `&mut T`).
    pub self_ref: bool,
    /// The trait a trait impl implements; `None` for an inherent impl.
    pub trait_path: Option<String>,
    /// An `impl Deref`'s `type Target`; `None` otherwise.
    pub deref_target: Option<String>,
}

/// The fields needed to record one local binding (S-593, [FR-EX-33]; the
/// `local_bindings` table, migration 41) — an
/// [`extract::LocalBindingFact`](crate::extract::LocalBindingFact).
///
/// [FR-EX-33]: ../../../docs/specs/requirements/FR-EX-33.md
#[derive(Debug, Clone, Copy)]
pub struct NewLocalBinding<'a> {
    /// The bound name as written.
    pub name: &'a str,
    /// What binds it.
    pub kind: LocalBindingKind,
    /// 1-based line of the name.
    pub line: u32,
    /// 0-based byte column of the name.
    pub column: u32,
    /// 1-based first line of the scope it binds in.
    pub scope_start_line: u32,
    /// 0-based byte column the scope starts at.
    pub scope_start_column: u32,
    /// 1-based last line of the scope.
    pub scope_end_line: u32,
    /// 0-based byte column the scope ends at (exclusive).
    pub scope_end_column: u32,
    /// Whether it is in force throughout its scope.
    pub hoisted: bool,
}

/// One recorded local binding as [`GraphStore::local_bindings`] reads it
/// (S-593, [FR-EX-33]): the [`NewLocalBinding`] fields and the declaring
/// file's path.
///
/// [FR-EX-33]: ../../../docs/specs/requirements/FR-EX-33.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalBindingRow {
    /// The declaring file's project-relative path.
    pub file_path: String,
    /// The bound name as written.
    pub name: String,
    /// What binds it.
    pub kind: LocalBindingKind,
    /// 1-based line of the name.
    pub line: u32,
    /// 0-based byte column of the name.
    pub column: u32,
    /// 1-based first line of the scope it binds in.
    pub scope_start_line: u32,
    /// 0-based byte column the scope starts at.
    pub scope_start_column: u32,
    /// 1-based last line of the scope.
    pub scope_end_line: u32,
    /// 0-based byte column the scope ends at (exclusive).
    pub scope_end_column: u32,
    /// Whether it is in force throughout its scope.
    pub hoisted: bool,
}

/// One framework detector as the incremental footprint probe
/// ([`GraphStore::has_framework_footprint`]) tests the ledger against: the
/// canonical `::`-joined prefix, and whether only an **import** reference
/// matches it (S-541) — `true` only when every loaded language declaring the
/// prefix says so, so the probe never answers narrower than the framework
/// pass's own candidacy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectorPrefix {
    /// The canonical `::`-joined prefix.
    pub prefix: String,
    /// Whether only an `Imports` reference matches it.
    pub import_only: bool,
}

/// The receiver parameters the incremental footprint probe
/// ([`GraphStore::has_framework_footprint`]) looks for (S-541): every name a
/// loaded language declares in its `[framework_candidacy] receiver_parameters`,
/// bound in a file of one of those languages' `extensions`. Scoped to the
/// declaring languages because `app` and `router` are ordinary parameter names
/// in every language: a Python `def init_app(app)` must not put a
/// framework-free repository on the whole-graph pass at every sync.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReceiverParameters {
    /// The parameter names, deduplicated.
    pub names: Vec<String>,
    /// The file extensions (no dot) of the languages declaring any of them.
    pub extensions: Vec<String>,
}

/// The fields needed to insert a reference-ledger row (S-011).
///
/// Insertion is idempotent over `(source_symbol, target, form, kind, payload,
/// receiver, alias, peeled, arg_count)` — the ledger's UNIQUE rule absorbs a
/// function calling the same path twice, and keeps one row per local name an
/// import is aliased by, per wrapper a proven receiver was peeled of and per
/// argument count a call passes.
#[derive(Debug, Clone, Copy)]
pub struct NewUnresolvedRef<'a> {
    /// FK into `files(id)` — the file whose extraction produced the ref.
    pub file_id: Option<i64>,
    /// The referencing declaration's stable symbol string.
    pub source_symbol: &'a str,
    /// The reference target text, interpreted per `form`.
    pub target: &'a str,
    /// The in-scope name an import binds; `None` for non-import forms.
    pub alias: Option<&'a str>,
    /// How `target` is to be interpreted.
    pub form: RefForm,
    /// The edge kind a successful binding produces.
    pub kind: EdgeKind,
    /// 1-based source line of the reference, when known.
    pub line: Option<i64>,
    /// The cross-artifact relation class ([`ArtifactRelation`](crate::model::ArtifactRelation)
    /// wire token) for an `ArtifactRef`/`ArtifactBinding` ref (CR-011); `None` for
    /// every code/doc/access ref.
    pub payload: Option<&'a str>,
    /// The receiver shape of a Method-form call (S-514), and of a Path-form
    /// call retyped from its proven receiver (S-587); `None` for every other
    /// row.
    pub receiver: Option<ReceiverShape>,
    /// The wrappers a proven receiver's declared type was peeled of (S-587,
    /// the `peeled` column, migration 32); `None` for every other row.
    pub peeled: Option<&'a str>,
    /// How many arguments a call passes (S-591, the `arg_count` column,
    /// migration 33); `None` when it cannot be counted and for every non-call
    /// row but a partial type part's own `Extends` row, whose type-parameter
    /// count it is (S-557), and a type-arity language's supertype or `new` row,
    /// whose written type-argument count it is (S-602).
    pub arg_count: Option<u32>,
    /// Whether an import row's declaration is a re-export (S-606, the
    /// `exported` column, migration 34); `None` for every other row. Outside
    /// the identity: a duplicate keeps the first row's mark.
    pub exported: Option<bool>,
}

/// The point-query read interface over the code graph.
///
/// Implemented by [`SqliteGraphStore`]; this is the seam navigation/resolution
/// components depend on rather than the concrete store ([graph-store
/// component]).
///
/// [graph-store component]: ../../../docs/specs/architecture/components/graph-store.md
pub trait GraphStore {
    /// Fetch a single node by id, or `None` if no such node exists.
    fn node(&self, id: NodeId) -> Result<Option<NodeRow>>;

    /// All nodes that *call* `id` — the sources of inbound `calls` edges
    /// (backed by `idx_edges_target_kind`).
    fn callers(&self, id: NodeId) -> Result<Vec<NodeRow>>;

    /// All nodes that `id` *calls* — the targets of outbound `calls` edges
    /// (backed by `idx_edges_source_kind`).
    fn callees(&self, id: NodeId) -> Result<Vec<NodeRow>>;

    /// Full-text search over node names, optionally filtered by `kind`,
    /// returning up to `limit` rows ranked best-match-first.
    ///
    /// `query` is bound as an FTS5 MATCH parameter — never interpolated. An
    /// empty or whitespace-only `query` returns no rows (`Ok(vec![])`).
    fn search(&self, query: &str, kind: Option<NodeKind>, limit: i64) -> Result<Vec<NodeRow>>;

    /// Fetch the node carrying the exact canonical `symbol` string, or `None`.
    ///
    /// The navigation symbol-argument resolver's first strategy ([FR-NV-04]):
    /// an agent that already holds a canonical [`LogosSymbol`] (from a prior
    /// result) round-trips it exactly. Deterministic when a symbol ever maps
    /// to more than one node (lowest `id` wins, same rule as
    /// [`BatchWriter::node_id_for_symbol`]).
    ///
    /// [FR-NV-04]: ../../../docs/specs/requirements/FR-NV-04.md
    fn node_by_symbol(&self, symbol: &str) -> Result<Option<NodeRow>>;

    /// All nodes whose human-facing `name` equals `name` exactly, ordered by
    /// `id`.
    ///
    /// The navigation symbol-argument resolver's second strategy: `callers
    /// parse_config` should hit the function named `parse_config` without the
    /// caller knowing canonical symbol syntax ([FR-NV-05]).
    ///
    /// [FR-NV-05]: ../../../docs/specs/requirements/FR-NV-05.md
    fn nodes_by_name(&self, name: &str) -> Result<Vec<NodeRow>>;

    /// Every node `name` defined in the file at `path` (empty when the file is
    /// absent or carries no nodes).
    ///
    /// The CR-015 incremental resolver reads this for each changed or removed
    /// file *before* persist replaces it, building the set of symbol names a
    /// sync adds or removes. Those "dirty" names decide which otherwise-untouched
    /// reference rows must be re-bound; the rest provably keep their binding, so
    /// the sync skips them instead of re-resolving the whole ledger — same result
    /// as a full re-bind ([FR-RS-03]), a fraction of the work. It must run
    /// pre-persist because the re-extract deletes the very nodes it returns.
    ///
    /// [FR-RS-03]: ../../../docs/specs/requirements/FR-RS-03.md
    fn node_names_for_path(&self, path: &str) -> Result<Vec<String>>;

    /// The local names the file at `path` imports — the `alias` of each of its
    /// `Imports` ledger rows (S-588).
    ///
    /// The incremental resolver's twin of
    /// [`node_names_for_path`](GraphStore::node_names_for_path), read for each
    /// changed file before persist replaces its ledger rows: a proven Rust
    /// receiver's type is read through another file's `use` — a crate root's
    /// `pub use`, a parent module's import a `use super::*` brings in — so a
    /// sync that retargets one moves a row in an untouched file that spells
    /// only the imported name.
    fn import_aliases_for_path(&self, path: &str) -> Result<Vec<String>>;

    /// Whether the file at `path` records a **global** namespace wildcard — a
    /// C# `global using N;`, an `Imports` `Glob` row carrying
    /// `resolve::GLOBAL_WILDCARD_ALIAS` (S-518).
    ///
    /// The incremental resolver's twin of
    /// [`node_names_for_path`](GraphStore::node_names_for_path), read for each
    /// changed file before persist replaces its ledger rows: such a row brings
    /// names into view in files the change does not touch, under names it does
    /// not spell, so a sync that adds or removes one re-binds every
    /// package-shaped row (`resolve::Delta::global_imports_moved`).
    fn declares_global_import(&self, path: &str) -> Result<bool>;

    /// The namespace recorded for the file at `path` (S-518, `files.namespace`)
    /// — `None` when the file is absent or keyed by its path. Read for each
    /// changed file before persist replaces it: a file that changes the
    /// namespace it declares renames no node, so the namespace's own segments
    /// are what select the rows binding through it.
    fn file_namespace(&self, path: &str) -> Result<Option<String>>;

    /// Every committed definition of one canonical configuration key: the file
    /// that proves it, the profile that file declares, and the value (S-380,
    /// [CR-121], [FR-WS-19]).
    ///
    /// `key` is matched as stored — a canonical key
    /// ([`canonical_key`](crate::extract::config::corpus::canonical_key)) — so a
    /// caller holding a source spelling (`api.uri-get-mailbox`, `uriGetMailbox`)
    /// must canonicalise it first; this is a lookup, not a binder.
    ///
    /// Ordered by `(path, value)` so the answer is stable across runs. Every
    /// definition is returned, including two that disagree: disagreement is
    /// represented for the caller to judge, never averaged and never refused
    /// here ([FR-WS-19]).
    ///
    /// [CR-121]: ../../../docs/requests/CR-121-caller-to-callee-and-producer-to-consumer-across-services.md
    /// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
    fn config_definitions(&self, key: &str) -> Result<Vec<ConfigDefinition>>;

    /// Every build manifest this member's last index or sync recorded, with the
    /// artifacts each produces and references (S-462, [CR-148] §3.2 A).
    ///
    /// Ordered by path; each manifest's artifacts in the order they were
    /// recorded. Refused facts are returned too — a caller joining on
    /// coordinates filters on `resolution`, and a census counts both. Empty for
    /// a member with no build manifest.
    ///
    /// [CR-148]: ../../../docs/requests/CR-148-build-manifests-yield-a-build-dependency-relation.md
    fn build_manifests(&self) -> Result<Vec<BuildManifestRow>>;

    /// **The read API over one member's declared-type facts** (S-472,
    /// [CR-152] §3.2 B): every type this member's last index or sync recorded,
    /// from source and from Avro, each with its provenance path.
    ///
    /// Ordered by provenance path, then in the order each file's or schema's
    /// facts were recorded (source node order, schema document order). Refused
    /// facts are returned too — a caller indexing names filters on
    /// `resolution`, and a census counts both. Empty for a member with no
    /// Java/Kotlin/Avro file; read [`declared_types_extracted`] to tell that
    /// member from one whose facts were never extracted.
    ///
    /// [`declared_types_extracted`]: GraphStore::declared_types_extracted
    /// [CR-152]: ../../../docs/requests/CR-152-cross-member-type-references-overlay.md
    fn declared_types(&self) -> Result<Vec<DeclaredTypeRow>>;

    /// Every Avro schema this member's last index or sync found, read or not,
    /// ordered by path (S-472) — the schema denominator beside
    /// [`declared_types`](GraphStore::declared_types).
    fn avro_schemas(&self) -> Result<Vec<AvroSchemaRow>>;

    /// Every inbound edge of **any** kind: `(edge kind, source node)` pairs,
    /// ordered by `(kind, source id)`.
    ///
    /// The immediate-edges half of the `node` read-model ([FR-NV-04]) —
    /// [`callers`](GraphStore::callers) is the `Calls`-only special case.
    ///
    /// [FR-NV-04]: ../../../docs/specs/requirements/FR-NV-04.md
    fn neighbours_in(&self, id: NodeId) -> Result<Vec<(EdgeKind, NodeRow)>>;

    /// Every outbound edge of **any** kind: `(edge kind, target node)` pairs,
    /// ordered by `(kind, target id)`.
    fn neighbours_out(&self, id: NodeId) -> Result<Vec<(EdgeKind, NodeRow)>>;

    /// Whole-store row counts for the `status` health snapshot ([FR-NV-07]).
    /// The ledger figures leave out capture-before-delete (`Symbol`-form) rows,
    /// so a synced store and a cold reindex report one ratio (S-598).
    ///
    /// [FR-NV-07]: ../../../docs/specs/requirements/FR-NV-07.md
    fn counts(&self) -> Result<StoreCounts>;

    /// The per-project **language composition** ([FR-UI-10]): one entry per
    /// language **present** in the graph, with its node count and the number of
    /// distinct files that contributed those nodes, ordered deterministically
    /// (node count descending, then language ascending — [NFR-RA-06]).
    ///
    /// Presence is read from the `nodes ⋈ files` join, so a language appears
    /// **iff** it has at least one indexed node: a registered-but-unused grammar
    /// is absent, a file tagged with a language but carrying no nodes never
    /// fabricates an entry, and a node whose `file_id` was nulled (its file
    /// deleted) contributes to no language. Every count is therefore a graph
    /// fact ([NFR-RA-05]). An empty graph yields an empty vector.
    ///
    /// [FR-UI-10]: ../../../docs/specs/requirements/FR-UI-10.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    fn language_composition(&self) -> Result<Vec<LanguageCount>>;

    /// The **production callables** per language — non-derived, non-test
    /// `Function`/`Method` nodes, keyed by their file's `files.language` —
    /// in language order ([NFR-RA-06]). The share each language holds of the
    /// production code is read from this, the population Equality and
    /// Redundancy count ([FR-QM-08]); the coverage warning reads it to decide
    /// which languages are large enough to name ([FR-QM-18]). A callable with
    /// no file, or a file with no language, belongs to no row.
    ///
    /// [FR-QM-08]: ../../../docs/specs/requirements/FR-QM-08.md
    /// [FR-QM-18]: ../../../docs/specs/requirements/FR-QM-18.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    fn production_callables_by_language(&self) -> Result<Vec<(String, u64)>>;

    /// The paths of the files whose parse held an ERROR or MISSING node
    /// (`files.parse_damaged = 1`, S-571, [FR-IX-14]) — what flags a
    /// worst-offender or "where to start" entry `partial_parse` when a snapshot
    /// is computed; the flag is persisted with the entry. A file indexed before
    /// migration 44 records none and is not among them.
    ///
    /// [FR-IX-14]: ../../../docs/specs/requirements/FR-IX-14.md
    fn parse_damaged_paths(&self) -> Result<HashSet<String>>;

    /// Per-language resolution counts for the `Calls` and `Imports` classes
    /// ([FR-RS-09], [S-441]): one entry per language tagged on an indexed file,
    /// ordered by language name ascending ([NFR-RA-06]).
    ///
    /// Presence is read from the **`files`** table, not the `nodes ⋈ files`
    /// join [`language_composition`](Self::language_composition) uses: a
    /// language in the index must never be an absent row here, even one whose
    /// files contributed no node. The ledger half groups `unresolved_refs` by
    /// its owning file's language, leaving out capture-before-delete
    /// (`Symbol`-form) rows — each duplicates a reference its source file's own
    /// row records (S-598); the edge half groups edges by their
    /// **source** node's file language and splits them on whether the target
    /// node lies in the same file. An edge whose source or target lies in no
    /// indexed file is attributable to no language and no locality, and is
    /// counted nowhere. An empty store yields an empty vector.
    ///
    /// A pure read: three aggregate `SELECT`s in one read transaction, so the
    /// row set and both count halves describe one graph generation; no write
    /// ([ADR-28]).
    ///
    /// [FR-RS-09]: ../../../docs/specs/requirements/FR-RS-09.md
    /// [S-441]: ../../../docs/planning/journal.md#s-441-resolution-coverage-is-reported-per-language-with-its-denominator
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    /// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
    fn resolution_by_language(&self) -> Result<Vec<LanguageRefCounts>>;

    /// The reported reference ledger as the reference classes read it
    /// ([FR-RS-49], S-635): see [`ReferencePopulation`]. Capture-before-delete
    /// (`Symbol`-form) rows are left out, as [`counts`](Self::counts) leaves
    /// them out (S-598).
    ///
    /// A pure read: one grouped scan of the ledger and the name reads, in one
    /// read transaction, so the counts, the groups and the names describe one
    /// graph generation — [`resolution_by_language`](Self::resolution_by_language)'s
    /// snapshot discipline. No write ([ADR-28]).
    ///
    /// [FR-RS-49]: ../../../docs/specs/requirements/FR-RS-49.md
    /// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
    fn reference_population(&self) -> Result<ReferencePopulation>;

    /// Best-effort "did you mean" name suggestions for `text` ([FR-NV-09]).
    ///
    /// Tries an FTS5 prefix match on the first search token, then falls back
    /// to a substring `LIKE` over node names. Returns distinct names, at most
    /// `limit`. Never errors on odd input — an unparsable FTS query simply
    /// yields the fallback path ([FR-NV-09] graceful contract).
    ///
    /// [FR-NV-09]: ../../../docs/specs/requirements/FR-NV-09.md
    fn suggest(&self, text: &str, limit: i64) -> Result<Vec<String>>;

    /// Stream **every** node in the graph, ordered by `id`.
    ///
    /// This is the whole-graph read [graph-hydration] uses to materialise a
    /// petgraph view ([ADR-05]); the point queries above answer single-vertex
    /// questions. The deterministic `id` ordering is what lets hydration assign
    /// stable vertex indices so graph algorithms are reproducible ([NFR-RA-06]).
    ///
    /// [graph-hydration]: ../../../docs/specs/architecture/components/graph-hydration.md
    /// [ADR-05]: ../../../docs/specs/architecture/decisions/ADR-05.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    fn all_nodes(&self) -> Result<Vec<NodeRow>>;

    /// The `(symbol, body)` of every `ProtoService` node carrying rpc-method body
    /// text (S-253 provider enrichment, [FR-WS-09]).
    ///
    /// The federation bridge expands each `ProtoService` into one gRPC provider
    /// key per rpc method (`package.Service/Method`); the method names ride the
    /// node `body`, which [`all_nodes`](GraphStore::all_nodes)'s [`NodeRow`] does
    /// not carry, so this is the narrow companion read that supplies them. The
    /// default is empty — only the SQLite store implements it — so a non-SQLite or
    /// test store contributes no gRPC providers rather than failing to compile.
    ///
    /// [FR-WS-09]: ../../../docs/specs/requirements/FR-WS-09.md
    fn proto_service_bodies(&self) -> Result<Vec<(LogosSymbol, String)>> {
        Ok(Vec::new())
    }

    /// The `(node, self type)` of every node carrying a recorded self type
    /// (S-493, [FR-RS-11]) — the `nodes.self_type` column, ordered by node id.
    ///
    /// The binder's companion read beside [`all_nodes`](GraphStore::all_nodes),
    /// whose [`NodeRow`] does not carry the column — the
    /// [`proto_service_bodies`](GraphStore::proto_service_bodies) shape. The
    /// default is empty — only the SQLite store implements it — so a non-SQLite
    /// or test store binds no `self.m()` call through a self type rather than
    /// failing to compile.
    ///
    /// [FR-RS-11]: ../../../docs/specs/requirements/FR-RS-11.md
    fn node_self_types(&self) -> Result<Vec<(NodeId, String)>> {
        Ok(Vec::new())
    }

    /// The `(node, parameter range, takes self)` of every node carrying either
    /// fact (S-591, [FR-EX-32], [CR-200]) — the `nodes.param_min`,
    /// `nodes.param_max` and `nodes.takes_self` columns, ordered by node id. A
    /// node with neither fact is absent; one with only one carries `None` for
    /// the other.
    ///
    /// The [`node_self_types`](GraphStore::node_self_types) shape: the
    /// companion read beside [`all_nodes`](GraphStore::all_nodes) for the
    /// binder's candidate filters (S-604, S-592), empty by default — only the SQLite store implements it — so a non-SQLite or test
    /// store records every fact unknown, and an unknown fact never filters.
    ///
    /// [FR-EX-32]: ../../../docs/specs/requirements/FR-EX-32.md
    /// [CR-200]: ../../../docs/requests/CR-200-a-rust-method-call-binds-only-a-callable-that-takes-self.md
    fn node_arities(&self) -> Result<Vec<NodeArity>> {
        Ok(Vec::new())
    }

    /// The associated-item facts of every node carrying any (S-606,
    /// [FR-EX-34]) — the `nodes.receiver_mode`, `nodes.variants` and
    /// `nodes.signature` columns, ordered by node id. The
    /// [`node_arities`](GraphStore::node_arities) shape, empty by default — only
    /// the SQLite store implements it — so a non-SQLite or test store records
    /// none.
    ///
    /// [FR-EX-34]: ../../../docs/specs/requirements/FR-EX-34.md
    fn node_item_facts(&self) -> Result<Vec<NodeItemFacts>> {
        Ok(Vec::new())
    }

    /// The callables a type implementing their container never inherits a
    /// body from (S-609, [FR-RS-48]): every node recorded with no body
    /// (`nodes.has_body` 0) or recorded `nodes.uninherited` (migration 35),
    /// ordered by id. What a supertype walk never binds at an interface level.
    /// The [`node_arities`](GraphStore::node_arities) shape, empty by default —
    /// only the SQLite store implements it — so a non-SQLite or test store
    /// records none.
    ///
    /// [FR-RS-48]: ../../../docs/specs/requirements/FR-RS-48.md
    fn uninherited_members(&self) -> Result<Vec<NodeId>> {
        Ok(Vec::new())
    }

    /// The unexported functions (S-584, [FR-RS-40]): every `Function` node,
    /// of any language, whose plugin's export rule left it unexported
    /// (`nodes.exported` 0), ordered by id. The binder's link-unit rung reads
    /// only a link-unit file's — a C `static`, its own file's alone — and never
    /// offers one to another file. `None` by default — only the SQLite store
    /// implements it — so a non-SQLite or test store records no linkage, and
    /// the rung, given none, binds nothing.
    ///
    /// [FR-RS-40]: ../../../docs/specs/requirements/FR-RS-40.md
    fn unexported_functions(&self) -> Result<Option<Vec<NodeId>>> {
        Ok(None)
    }

    /// Every type's generic arity (S-602, [CR-199]) — the `nodes.type_arity`
    /// column (migration 42), ordered by node id: what the binder's namespace
    /// rungs match a written type's arity against. A node with none recorded is
    /// absent, and an absent arity admits any. The
    /// [`node_arities`](GraphStore::node_arities) shape, empty by default —
    /// only the SQLite store implements it — so a non-SQLite or test store
    /// records none.
    ///
    /// [CR-199]: ../../../docs/requests/CR-199-csharp-resolution-honours-type-arity-and-using-scope.md
    fn type_arities(&self) -> Result<Vec<(NodeId, u32)>> {
        Ok(Vec::new())
    }

    /// Every link from a later part of a partial type to its first-declared
    /// part (S-557, [FR-RS-29]) as `(part, first part)`: the `Extends` edges
    /// whose payload is [`PARTIAL_TYPE_ALIAS`](crate::resolve::PARTIAL_TYPE_ALIAS),
    /// ordered by part. What Focus and Cohesion unite a type's parts by — no
    /// other `Extends` edge carries that payload, so a supertype that shares
    /// its subtype's name is never mistaken for one. The
    /// [`uninherited_members`](GraphStore::uninherited_members) shape, empty
    /// by default — only the SQLite store implements it — so a non-SQLite or
    /// test store records none.
    ///
    /// [FR-RS-29]: ../../../docs/specs/requirements/FR-RS-29.md
    fn partial_type_links(&self) -> Result<Vec<(NodeId, NodeId)>> {
        Ok(Vec::new())
    }

    /// Every `Calls` edge a trait fan-out reached by dispatch (S-712,
    /// [CR-226] BR-76) as `(caller, implementation, trait method symbol)`:
    /// the edges whose payload is a [`dispatch`](crate::model::dispatch)
    /// marker, ordered by caller then implementation. The
    /// [`partial_type_links`](GraphStore::partial_type_links) shape, empty by
    /// default — only the SQLite store implements it.
    ///
    /// [CR-226]: ../../../docs/requests/CR-226-dispatch-is-not-a-dependency.md
    fn dispatch_links(&self) -> Result<Vec<(NodeId, NodeId, String)>> {
        Ok(Vec::new())
    }

    /// The trait method symbol of each `(caller, callee)` pair of `pairs`
    /// whose `Calls` edge is marked dispatch ([`dispatch_links`]
    /// (GraphStore::dispatch_links)'s marker), keyed by the pair; a pair with
    /// no edge, or an unmarked one, is absent. What a navigation row's `via`
    /// label is read from. Empty by default, like `dispatch_links`.
    fn dispatched_via(&self, pairs: &[(NodeId, NodeId)]) -> Result<HashMap<(NodeId, NodeId), String>> {
        let _ = pairs;
        Ok(HashMap::new())
    }

    /// Every recorded impl block (S-606, [CR-202] F1), ordered by file path
    /// and line — empty by default, as [`node_item_facts`](GraphStore::node_item_facts).
    ///
    /// [CR-202]: ../../../docs/requests/CR-202-one-rust-associated-item-lookup.md
    fn impl_blocks(&self) -> Result<Vec<ImplBlockRow>> {
        Ok(Vec::new())
    }

    /// Every recorded local binding (S-593, [FR-EX-33]), ordered by file path
    /// and position — empty by default, as [`impl_blocks`](GraphStore::impl_blocks).
    ///
    /// [FR-EX-33]: ../../../docs/specs/requirements/FR-EX-33.md
    fn local_bindings(&self) -> Result<Vec<LocalBindingRow>> {
        Ok(Vec::new())
    }

    /// The recorded local bindings (S-593, [FR-EX-33]) of the files whose
    /// extension is one of `extensions` (normalised: lower-case, no dot),
    /// ordered as [`local_bindings`](GraphStore::local_bindings) — what the
    /// binder's link-unit rung reads to leave a call through a parameter or a
    /// local unbound (S-584). A targeted read rather than `local_bindings`,
    /// which materialises every language's. Empty for no extensions, and by
    /// default.
    ///
    /// [FR-EX-33]: ../../../docs/specs/requirements/FR-EX-33.md
    fn local_bindings_with_extensions(&self, extensions: &[String]) -> Result<Vec<LocalBindingRow>> {
        let _ = extensions;
        Ok(Vec::new())
    }

    /// The distinct `(file_id, name)` of every recorded **parameter** binding
    /// whose name is one of `names` (S-541), ordered — the framework pass's
    /// receiver-parameter candidacy (an Express router module receives the
    /// `app` it registers on). A targeted read rather than
    /// [`local_bindings`](GraphStore::local_bindings), which materialises every
    /// binding of the graph to answer a question about a handful of names.
    /// Empty for no names, and by default, as `local_bindings`.
    fn files_binding_parameters(&self, names: &[String]) -> Result<Vec<(i64, String)>> {
        let _ = names;
        Ok(Vec::new())
    }

    /// The `(path, namespace)` of every file carrying a recorded declared
    /// namespace (S-518, [FR-RS-13]) — the `files.namespace` column, ordered by
    /// path. The binder's module key reads it through
    /// [`PackageLayout::with_declared_namespaces`](crate::resolve::package_key::PackageLayout::with_declared_namespaces).
    ///
    /// The [`node_self_types`](GraphStore::node_self_types) shape: the default
    /// is empty — only the SQLite store implements it — so a non-SQLite or test
    /// store keys every file by the path, as before.
    ///
    /// [FR-RS-13]: ../../../docs/specs/requirements/FR-RS-13.md
    fn file_namespaces(&self) -> Result<Vec<(String, String)>> {
        Ok(Vec::new())
    }

    /// Stream **every** edge in the graph, ordered by `(source, target, kind)`.
    ///
    /// The relationship half of the hydration read: combined with
    /// [`all_nodes`](GraphStore::all_nodes) it is everything petgraph needs to
    /// build a view. Hydration filters out [`EdgeKind::Contains`] for the
    /// dependency views ([FR-DB-06]); the store returns every kind so the caller
    /// decides. The deterministic ordering keeps the built graph reproducible
    /// ([NFR-RA-06]).
    ///
    /// [FR-DB-06]: ../../../docs/specs/requirements/FR-DB-06.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    fn all_edges(&self) -> Result<Vec<EdgeRow>>;

    /// Every edge keyed by the canonical symbols of its endpoints, with each
    /// endpoint's defining file — the identity the deep `verify` diff compares
    /// across the live store and a shadow reindex ([FR-GV-19], [NFR-RA-06]),
    /// where storage rowids do not line up.
    ///
    /// Ordered by `(kind, source symbol, target symbol)`, so the result is
    /// deterministic ([NFR-RA-06]). Read-only; one joined query.
    ///
    /// [FR-GV-19]: ../../../docs/specs/requirements/FR-GV-19.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    fn symbol_edges(&self) -> Result<Vec<SymbolEdgeRow>>;

    /// The source node of every **dispatch live-root marker** — a `RoutesTo`
    /// self-edge (`source == target`) planted by the dispatch pass ([CR-043],
    /// [`crate::resolve::dispatch`]).
    ///
    /// A targeted read so the dispatch pass stays change-proportional on the
    /// sync hot path ([NFR-PE-03]): markers are a tiny set (one per
    /// framework-dispatched method), so this is O(markers), never the
    /// whole-graph [`all_edges`](GraphStore::all_edges) scan. No genuine
    /// framework route is a self-edge, so the `source == target` filter selects
    /// exactly the markers.
    ///
    /// [CR-043]: ../../../docs/requests/CR-043-dead-code-detector-precision.md
    /// [NFR-PE-03]: ../../../docs/specs/requirements/NFR-PE-03.md
    fn dispatch_markers(&self) -> Result<Vec<NodeId>>;

    /// The `Function`/`Method` nodes defined in the given project-relative file
    /// `paths`, ordered by `id` — a required signature (S-606, `nodes.signature`)
    /// excepted: it holds no code to dispatch to.
    ///
    /// The change-proportional node read the dispatch pass uses on a sync
    /// ([NFR-PE-03], [`crate::resolve::dispatch`]): only the changed files' nodes
    /// are materialised, never the whole-graph [`all_nodes`](GraphStore::all_nodes)
    /// scan. An empty `paths` yields an empty result.
    ///
    /// [NFR-PE-03]: ../../../docs/specs/requirements/NFR-PE-03.md
    fn callable_nodes_in_files(&self, paths: &[String]) -> Result<Vec<NodeRow>>;

    /// Every **span-bearing** node defined in the given project-relative file
    /// `paths`, ordered by path then start line then id.
    ///
    /// The line-range counterpart to
    /// [`callable_nodes_in_files`](GraphStore::callable_nodes_in_files): where
    /// that one narrows to `Function`/`Method` for the dispatch pass, this
    /// returns *every* kind that recorded a `start_line`, because mapping a git
    /// hunk back to the symbol that encloses it ([FR-NV-13],
    /// [`crate::navigate::branch`]) must be able to land on a struct, a const or
    /// a module as readily as on a function. Nodes with no recorded start line
    /// are excluded: a hunk cannot be attributed to a span that does not exist.
    ///
    /// Change-proportional like its sibling — only the changed files' nodes are
    /// materialised, never the whole-graph [`all_nodes`](GraphStore::all_nodes)
    /// scan. An empty `paths` yields an empty result.
    ///
    /// [FR-NV-13]: ../../../docs/specs/requirements/FR-NV-13.md
    fn span_nodes_in_files(&self, paths: &[String]) -> Result<Vec<NodeRow>>;

    /// The language each of the given project-relative file `paths` is tagged
    /// with (`files.language`), keyed by path ([FR-NV-14], [S-442]).
    ///
    /// The anchor half of a relational answer's resolution denominator: the
    /// answer names which files it is anchored in, and this says which
    /// per-language row of [`resolution_by_language`](GraphStore::resolution_by_language)
    /// each one selects. A path the index does not hold is **absent** from the
    /// map; a held path whose file records no language maps to `None`. Like
    /// its span-reading sibling it reads only the named files. An empty `paths`
    /// yields an empty map.
    ///
    /// [FR-NV-14]: ../../../docs/specs/requirements/FR-NV-14.md
    /// [S-442]: ../../../docs/planning/journal.md#s-442-a-relational-answer-states-the-resolution-denominator-it-was-computed-over
    fn file_languages(
        &self,
        paths: &[String],
    ) -> Result<std::collections::BTreeMap<String, Option<String>>>;

    /// The subset of `node_ids` that carry a **dispatch live-root marker** — a
    /// `RoutesTo` self-edge (`source == target`, [`crate::resolve::dispatch`]).
    ///
    /// Index-served via `idx_edges_source_kind` (the `source IN (…)` + `kind`
    /// predicate), so the dispatch pass can read its existing markers
    /// change-proportionally on a sync — only for the changed files' nodes,
    /// never the whole-graph scan [`dispatch_markers`](GraphStore::dispatch_markers)
    /// does on a full index ([NFR-PE-03]). An empty `node_ids` yields nothing.
    ///
    /// [NFR-PE-03]: ../../../docs/specs/requirements/NFR-PE-03.md
    fn markers_for_nodes(&self, node_ids: &[NodeId]) -> Result<Vec<NodeId>>;

    /// Every indexed file with the content hash recorded at its last index.
    ///
    /// The pipeline reads this once at the start of a sync to dirty-detect the
    /// candidate set ([FR-SY-03]) and to decide whether an auto-index is needed
    /// (an empty result means the graph has never been indexed, [FR-IX-07]).
    ///
    /// [FR-SY-03]: ../../../docs/specs/requirements/FR-SY-03.md
    /// [FR-IX-07]: ../../../docs/specs/requirements/FR-IX-07.md
    fn indexed_files(&self) -> Result<Vec<FileRecord>>;

    /// Every file whose latest persistence attempt failed, ordered by path
    /// (S-513, [FR-EH-05]) — the record `status` and `scan` read out.
    ///
    /// Empty on a store where nothing failed, and on a store upgraded across
    /// migration 27, which recorded nothing because nothing could be recorded.
    ///
    /// [FR-EH-05]: ../../../docs/specs/requirements/FR-EH-05.md
    fn persist_failures(&self) -> Result<Vec<PersistFailureRow>>;

    /// Stream the **entire** reference ledger, ordered by `id` (S-011).
    ///
    /// The resolution pass reads this as part of its immutable snapshot: every
    /// row is re-evaluated on every index/sync ([FR-RS-03] retry contract),
    /// and import rows — bound or not — are the durable per-file scope facts
    /// (alias/glob maps) name binding builds on. The `id` ordering keeps the
    /// pass deterministic ([NFR-RA-06]).
    ///
    /// [FR-RS-03]: ../../../docs/specs/requirements/FR-RS-03.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    fn unresolved_refs(&self) -> Result<Vec<UnresolvedRefRow>>;

    /// The still-unresolved `Imports` and `TypeUses` rows of this member's
    /// `.java` and `.kt` files, each with its file's path, ordered by path and
    /// then ledger id (S-473).
    ///
    /// A targeted read for the workspace type overlay, which reads it on the
    /// `workspace status` freshness walk: the whole ledger
    /// ([`unresolved_refs`](GraphStore::unresolved_refs)) would cost every
    /// member a full ledger read for the few rows a JVM file holds. A row whose
    /// file is no longer indexed is not returned.
    fn unresolved_type_refs(&self) -> Result<Vec<TypeRefRow>>;

    /// Stream every node with its **annotation-pass** columns, ordered by `id`
    /// (S-014).
    ///
    /// The whole-graph read the annotation engine snapshots before computing
    /// dead-code, duplicates, and layer membership ([FR-AN-01..03]); the
    /// deterministic ordering keeps the pass reproducible ([NFR-RA-06]).
    ///
    /// [FR-AN-01..03]: ../../../docs/specs/requirements/FR-AN-01.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    fn annotation_nodes(&self) -> Result<Vec<AnnotationNodeRow>>;

    /// Stream the inverted near-clone shingle index ([FR-EX-09], the migration-10
    /// `shingles` table) as `(node_id, hash)` rows in deterministic
    /// `(node_id, hash)` order (S-043).
    ///
    /// The id-ordered iteration the near-clone clustering sub-pass requires
    /// ([FR-AN-06]): a node's shingle rows are contiguous and ascending, so the
    /// pass reads each function's set in one pass and builds the inverted index
    /// deterministically ([NFR-RA-06]). Each `hash` is the platform-independent
    /// `u64` fingerprint read back from its lossless signed-`i64` storage with
    /// the inverse cast.
    ///
    /// [FR-AN-06]: ../../../docs/specs/requirements/FR-AN-06.md
    /// [FR-EX-09]: ../../../docs/specs/requirements/FR-EX-09.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    fn shingle_index(&self) -> Result<Vec<(NodeId, u64)>>;

    /// Cheap existence check: does the graph carry **any** framework footprint —
    /// a promoted `route`/`component` node, an `unresolved_refs` target naming
    /// one of `detectors` (canonical `::`-joined detector strings, the
    /// per-plugin `framework_detectors`; an import-only one matched by an
    /// `Imports` row alone, S-541), or a parameter binding one of `receivers`'
    /// names in a file of one of its extensions (S-541)?
    ///
    /// The incremental [`framework`](crate::resolve::framework) gate (S-024-HF):
    /// the framework-promotion pass is a pure, idempotent function of the
    /// candidate files (those whose ledger names a detector, or that bind a
    /// receiver parameter their language declares) and the bound graph. When the
    /// post-resolve graph has no promoted node, no detector ref and no such
    /// binding, **no** promotion is possible, so an incremental `sync` skips the
    /// whole-graph framework snapshot entirely — the framework-free fast path the
    /// [NFR-PE-03] single-file-sync budget needs. A full [`index`](crate::pipeline::index)
    /// never calls this (it always runs the whole-graph pass). The match mirrors
    /// [`matches_detector`](crate::resolve::framework): a target equals a detector
    /// or extends it by whole `::` segments. The receiver clause pairs every
    /// declared name with every declaring language's extension where the pass
    /// pairs each language with its own names — a superset, so the probe can
    /// cost a needless pass but never skip a due one. Returns `false` for empty
    /// lists with no promoted nodes.
    ///
    /// [NFR-PE-03]: ../../../docs/specs/requirements/NFR-PE-03.md
    fn has_framework_footprint(
        &self,
        detectors: &[DetectorPrefix],
        receivers: &ReceiverParameters,
    ) -> Result<bool>;

    /// Whether this graph carries any **broker footprint** — a promoted
    /// `topic`/`producer`/`consumer` node, or a broker-arm reference in the ledger
    /// (S-256, [FR-WS-11]).
    ///
    /// The incremental gate of the broker-topic promotion pass
    /// ([`crate::resolve::topics`]), and the exact sibling of
    /// [`has_framework_footprint`](GraphStore::has_framework_footprint): that pass
    /// is a pure, idempotent function of the broker ledger and the bound graph, so
    /// a graph with neither a promoted broker node nor a broker ref can promote
    /// nothing and has nothing to demote. An incremental `sync` therefore skips the
    /// whole-graph snapshot entirely, which is what keeps a repo that indexes **no
    /// broker topics** byte-for-byte unaffected ([FR-WS-11], [NFR-RA-06]) and
    /// inside the [NFR-PE-03] single-file-sync budget. A full `index` never calls
    /// this.
    ///
    /// [FR-WS-11]: ../../../docs/specs/requirements/FR-WS-11.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    /// [NFR-PE-03]: ../../../docs/specs/requirements/NFR-PE-03.md
    fn has_broker_footprint(&self) -> Result<bool>;

    /// The **promoted broker subgraph** alone: the `topic`/`producer`/`consumer`
    /// nodes and the `publishes`/`subscribes` edges joining them (S-256,
    /// [FR-WS-11]).
    ///
    /// A targeted read, for the same reason
    /// [`dispatch_markers`](GraphStore::dispatch_markers) is one: the workspace topic
    /// inventory ([`crate::federation::topics`]) is served on every `workspace status`
    /// request, per member, and its answer is O(topics) — typically **zero**. Reading
    /// it via the whole-graph [`all_nodes`](GraphStore::all_nodes) +
    /// [`all_edges`](GraphStore::all_edges) would make a read-model that usually
    /// returns nothing cost a full graph materialisation of every member ([NFR-PE-10]).
    /// A repo with no broker coupling answers with two empty vectors after an index
    /// scan.
    ///
    /// Ordered (`id`, then `source, target, kind`) so the projection built from it is
    /// deterministic ([NFR-RA-06]).
    ///
    /// [FR-WS-11]: ../../../docs/specs/requirements/FR-WS-11.md
    /// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    fn broker_subgraph(&self) -> Result<(Vec<NodeRow>, Vec<EdgeRow>)>;

    /// The ids of every node the annotation pass marked `is_test = 1`, ordered
    /// by `id` ([FR-AN-05]).
    ///
    /// The single source of truth the `[[require_tested]]` contract ([FR-GV-13])
    /// reads to seed its reachability BFS — by reading the persisted column
    /// rather than re-deriving test status, the contract and the annotation can
    /// never disagree about what a test is (CR-001 CRA-01). The `id` ordering keeps
    /// the consumer deterministic ([NFR-RA-06]).
    ///
    /// [FR-AN-05]: ../../../docs/specs/requirements/FR-AN-05.md
    /// [FR-GV-13]: ../../../docs/specs/requirements/FR-GV-13.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    fn test_node_ids(&self) -> Result<Vec<NodeId>>;

    /// Stream every **non-derived** function/method node's metric inputs,
    /// ordered by `id` (S-018).
    ///
    /// The per-function slice the metrics engine's Equality and Redundancy
    /// metrics read ([FR-QM-04], [FR-QM-05]); derived policy nodes are never
    /// functions but are excluded defensively. The `id` ordering keeps the
    /// metric reduction canonical ([ADR-08], [NFR-RA-06]).
    ///
    /// [FR-QM-04]: ../../../docs/specs/requirements/FR-QM-04.md
    /// [FR-QM-05]: ../../../docs/specs/requirements/FR-QM-05.md
    /// [ADR-08]: ../../../docs/specs/architecture/decisions/ADR-08.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    fn function_metrics(&self) -> Result<Vec<FunctionMetricRow>>;

    /// The **exact-duplicate groups** ([FR-AN-02]): the non-derived
    /// functions/methods the last annotation run marked `is_duplicate`, grouped
    /// by the shared AST-shape fingerprint that made them duplicates. Each group
    /// is id-ascending and the groups are ordered by their lowest id
    /// ([NFR-RA-06]); test-scope members are included — the reader scopes them.
    /// The unit the "where to start" ranking ranks for Redundancy ([FR-QM-17]).
    ///
    /// [FR-AN-02]: ../../../docs/specs/requirements/FR-AN-02.md
    /// [FR-QM-17]: ../../../docs/specs/requirements/FR-QM-17.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    fn duplicate_groups(&self) -> Result<Vec<Vec<NodeId>>>;

    /// Stream every persisted metric snapshot, ordered by `id` (S-018).
    ///
    /// The append-only series behind [FR-QM-07] verification and the
    /// `evolution` reader (S-020).
    ///
    /// [FR-QM-07]: ../../../docs/specs/requirements/FR-QM-07.md
    fn metric_snapshots(&self) -> Result<Vec<MetricSnapshotRow>>;

    /// [`metric_snapshots`](Self::metric_snapshots) up to and including the
    /// snapshot `id`, ordered by `id` — the series a verdict over that
    /// snapshot reads, which one recorded after it never enters ([FR-GV-27]).
    ///
    /// [FR-GV-27]: ../../../docs/specs/requirements/FR-GV-27.md
    fn metric_snapshots_through(&self, id: i64) -> Result<Vec<MetricSnapshotRow>>;

    /// The metric snapshot the `baseline` row for `scope` points at, or
    /// `None` when no baseline has been saved (S-020, [FR-GV-04]).
    ///
    /// [FR-GV-04]: ../../../docs/specs/requirements/FR-GV-04.md
    fn baseline_snapshot(&self, scope: &str) -> Result<Option<MetricSnapshotRow>>;

    /// What the re-bless that set the `scope` baseline recorded ([FR-GV-16],
    /// migration 39), or `None` when no baseline is saved.
    ///
    /// [FR-GV-16]: ../../../docs/specs/requirements/FR-GV-16.md
    fn baseline_record(&self, scope: &str) -> Result<Option<BaselineRecord>>;

    /// The most-recent persisted snapshot's **full** dimension breakdown, or
    /// `None` when no snapshot has ever been recorded (a never-`scan`-ned store).
    ///
    /// A pure read of the last `metric_snapshots` row — it computes and persists
    /// nothing. Backs the read-only `latest_metrics` accessor the web dashboard
    /// reads through so a GET reflects the last `scan` and never writes ([ADR-28],
    /// [CR-018], S-082). Distinct from [`metric_snapshots`](Self::metric_snapshots)
    /// (the whole gate/evolution series, original five dimensions only).
    ///
    /// Carries the worst-offender lists persisted with that row ([FR-QM-15]),
    /// read by the row's own id so the pair never mixes two snapshots.
    ///
    /// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
    /// [CR-018]: ../../../docs/requests/CR-018-web-dashboard-write-on-read.md
    /// [FR-QM-15]: ../../../docs/specs/requirements/FR-QM-15.md
    fn latest_metric_snapshot(&self) -> Result<Option<LatestMetricSnapshot>>;

    /// The `metric_snapshots` row persisted under `snapshot_id`, read as
    /// [`latest_metric_snapshot`](Self::latest_metric_snapshot) reads the last
    /// one, or `None` when no such row exists. The attribution of a change
    /// against the gate baseline reads the baseline's row through it — every
    /// dimension, not the original five the series carries ([FR-GV-26]).
    ///
    /// [FR-GV-26]: ../../../docs/specs/requirements/FR-GV-26.md
    fn metric_snapshot(&self, snapshot_id: i64) -> Result<Option<LatestMetricSnapshot>>;

    /// The **uncapped** offender lists of the most-recent snapshot
    /// ([FR-QM-19]), or `None` when no snapshot has ever been recorded. A pure
    /// read, one statement pair under the row's own id. On a snapshot written
    /// before migration 37 the lists are empty and
    /// [`values_recorded`](WorstOffenders::values_recorded) is `false`: that
    /// snapshot never persisted them, and `[]` must not read as clean.
    ///
    /// [FR-QM-19]: ../../../docs/specs/requirements/FR-QM-19.md
    fn latest_uncapped_offenders(&self) -> Result<Option<WorstOffenders>>;

    /// The **uncapped** offender lists of the snapshot persisted under
    /// `snapshot_id` ([FR-QM-19]), or `None` when no such row exists — the
    /// by-id twin of [`latest_uncapped_offenders`](Self::latest_uncapped_offenders),
    /// with the same "not recorded" reading of a pre-migration-37 row. The
    /// branch review reads the merge-base commit's own snapshot through it
    /// ([ADR-82], S-677).
    ///
    /// [FR-QM-19]: ../../../docs/specs/requirements/FR-QM-19.md
    /// [ADR-82]: ../../../docs/specs/architecture/decisions/ADR-82.md
    fn uncapped_offenders_of(&self, snapshot_id: i64) -> Result<Option<WorstOffenders>>;

    /// The singleton `rules_cache` row, or `None` when no `rules.toml` parse
    /// has been cached yet (S-020, [FR-GV-01]).
    ///
    /// [FR-GV-01]: ../../../docs/specs/requirements/FR-GV-01.md
    fn rules_cache(&self) -> Result<Option<RulesCacheRow>>;

    /// Stream every **non-derived** function/method node's constraint inputs,
    /// ordered by `id` (S-020, [FR-GV-02]): the per-function point queries
    /// behind `max_cc` / `max_fn_lines`, with name/file/line for actionable
    /// violation messages.
    ///
    /// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
    fn function_constraint_rows(&self) -> Result<Vec<FunctionConstraintRow>>;

    /// The persisted `violations` of the last `check_rules` run, ordered by
    /// `id` (S-020, [FR-GV-02] idempotence verification).
    ///
    /// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
    fn violations(&self) -> Result<Vec<ViolationRow>>;

    /// The singleton `check_run` marker, or `None` when no `check_rules` run
    /// has ever been recorded (S-313, [FR-GV-21]).
    ///
    /// `None` means **no run**, never a clean one. A pure read: it never
    /// creates the row a missing marker would need ([ADR-49]'s report leg must
    /// not write).
    ///
    /// [ADR-49]: ../../../docs/specs/architecture/decisions/ADR-49.md
    /// [FR-GV-21]: ../../../docs/specs/requirements/FR-GV-21.md
    fn check_run(&self) -> Result<Option<CheckRunRow>>;

    /// The store-integrity slice behind the `health` tool (S-020): schema
    /// version + FTS5 coherence in one read.
    fn store_health(&self) -> Result<StoreHealth>;

    /// The fast **structural-integrity** census behind `doctor` ([FR-GV-18],
    /// [NFR-RA-13], [ADR-46]): asserts `node_count == distinct(symbol_id)` and
    /// zero orphan rows in O(a handful of indexed queries). Readable from the RO
    /// pool, so `session_end` / `check_rules` fold the verdict in via
    /// [`Runtime::submit_read`](crate::runtime::Runtime::submit_read) and
    /// hard-fail on drift.
    ///
    /// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
    /// [NFR-RA-13]: ../../../docs/specs/requirements/NFR-RA-13.md
    /// [ADR-46]: ../../../docs/specs/architecture/decisions/ADR-46.md
    fn structural_check(&self) -> Result<StructuralReport>;

    /// Read a durable `project_metadata` value by key, or `None` when the key
    /// has never been written (CR-004, [ADR-20], the migration-15 kv table).
    ///
    /// The pipeline reads [`CONFIG_FINGERPRINT_KEY`] here to detect that the
    /// admission-relevant configuration changed since the last reconciliation
    /// and purge now-unadmitted files exactly once per change ([FR-SY-07]). A
    /// `None` (fresh DB, or a store upgraded across migration 15) reads as "no
    /// fingerprint recorded yet" and arms the first reconciliation.
    ///
    /// [ADR-20]: ../../../docs/specs/architecture/decisions/ADR-20.md
    /// [FR-SY-07]: ../../../docs/specs/requirements/FR-SY-07.md
    fn project_metadata(&self, key: &str) -> Result<Option<String>>;

    /// The current persisted monotonic graph revision (CR-027, [ADR-32],
    /// [FR-SY-09]), or `0` when no `index`/`sync` has advanced it yet.
    ///
    /// The durable, cross-process successor to the in-memory
    /// [`SyncStamp`](crate::hydrate::SyncStamp): a second process opening the same
    /// `logos.db` reads the identical value, and the status read-model
    /// ([RP-02]) exposes it. A fresh database (or one upgraded across migration
    /// 15 before the first advance) reads as `0` — "no graph yet". A stored value
    /// that fails to parse is likewise treated as `0` rather than erroring: the
    /// revision is a freshness hint, never a correctness gate.
    ///
    /// [ADR-32]: ../../../docs/specs/architecture/decisions/ADR-32.md
    /// [FR-SY-09]: ../../../docs/specs/requirements/FR-SY-09.md
    /// [RP-02]: ../../../docs/specs/requirements/RP-02.md
    fn graph_revision(&self) -> Result<u64> {
        Ok(self
            .project_metadata(GRAPH_REVISION_KEY)?
            .and_then(|raw| raw.parse::<u64>().ok())
            .unwrap_or(0))
    }

    /// Whether a full-walk build-manifest pass has ever run over this store —
    /// the presence of [`BUILD_FACTS_EXTRACTED_KEY`] (S-462 task 2).
    ///
    /// `false` on a store upgraded across migration 22 until its first full
    /// index or full-walk reconcile, and on a store never indexed: in both,
    /// [`build_manifests`](GraphStore::build_manifests) being empty says
    /// nothing about the member's manifests.
    fn build_facts_extracted(&self) -> Result<bool> {
        Ok(self.project_metadata(BUILD_FACTS_EXTRACTED_KEY)?.is_some())
    }

    /// Whether this member's declared-type facts are complete — the presence of
    /// [`DECLARED_TYPES_EXTRACTED_KEY`] (S-472).
    ///
    /// `false` on a store upgraded across migration 24 until its first full
    /// index or full-walk reconcile, and on a store never indexed: in both,
    /// [`declared_types`](GraphStore::declared_types) being empty says nothing
    /// about the member's types.
    fn declared_types_extracted(&self) -> Result<bool> {
        Ok(self.project_metadata(DECLARED_TYPES_EXTRACTED_KEY)?.is_some())
    }

    /// The coordinate this store's symbols carry — [`SYMBOL_COORDINATE_KEY`]
    /// (S-703) — or `None` when none is recorded: a coordinate-less store, whose
    /// symbols render the empty package slot, or one that holds no file yet.
    fn symbol_coordinate(&self) -> Result<Option<String>> {
        self.project_metadata(SYMBOL_COORDINATE_KEY)
    }

    /// Whether the store holds at least one `symbols` row — anything a
    /// coordinate migration (S-704) could rewrite.
    fn holds_symbols(&self) -> Result<bool>;
}

/// The canonical SQLite-backed graph store — sole writer of `logos.db`.
pub struct SqliteGraphStore {
    conn: Connection,
}

/// Per-phase timings for [`SqliteGraphStore::open_with_timings`] ([CR-116],
/// [NFR-PE-05]).
///
/// [CR-116]: ../../../docs/requests/CR-116-cold-start-budget-and-its-guard-disagree.md
/// [NFR-PE-05]: ../../../docs/specs/requirements/NFR-PE-05.md
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct StoreOpenTimings {
    /// Opening the file and applying the per-connection pragma contract.
    pub connect: Duration,
    /// Running the forward-only schema migrations.
    pub migrate: Duration,
}

impl SqliteGraphStore {
    /// Open (creating if absent) the database at `path`, apply the connection
    /// contract, and run any pending migrations.
    ///
    /// The result is a single, copyable `logos.db` file ([NFR-DM-03]): WAL
    /// sidecars (`-wal`/`-shm`) are checkpointed back into it on close.
    ///
    /// # Errors
    /// Returns an error if the file cannot be opened, the pragmas cannot be
    /// set, or a migration fails.
    ///
    /// [NFR-DM-03]: ../../../docs/specs/requirements/NFR-DM-03.md
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let conn = Connection::open(path)
            .with_context(|| format!("opening graph store at {}", path.display()))?;
        Self::from_connection(conn)
    }

    /// Open an ephemeral in-memory database (tests, transient analyses).
    ///
    /// The pragma contract is still applied; `journal_mode` reports `memory`
    /// rather than `wal` since WAL is meaningful only for on-disk files.
    ///
    /// # Errors
    /// Returns an error if the connection cannot be configured or migrated.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("opening in-memory graph store")?;
        Self::from_connection(conn)
    }

    /// Open a **read-only** connection over an already-migrated `logos.db`.
    ///
    /// This is the connection the WAL reader pool ([execution-runtime], [ADR-02])
    /// hands out: under WAL, a read-only connection serves a consistent snapshot
    /// and is **never blocked by the single writer** ([NFR-PE-01], [NFR-RA-10]).
    /// Unlike [`open`](Self::open) it does **not** migrate — a read-only
    /// connection cannot write — so the writer must have opened (and migrated)
    /// the database first. The connection contract is enforced read-side via
    /// `query_only = ON`, which rejects any accidental write at the SQL layer.
    ///
    /// # Threading
    /// The connection is opened with `SQLITE_OPEN_NO_MUTEX`, so it must be
    /// accessed by **at most one thread at a time**. The
    /// [`ReaderPool`](crate::runtime) is the supported way to get concurrent
    /// read-only access — it hands each connection to one thread at a time. Do
    /// not share a single `open_readonly` store across threads directly.
    ///
    /// # Errors
    /// Returns an error if the file cannot be opened read-only, the read-side
    /// pragmas cannot be set, or the database is not yet migrated to the latest
    /// schema version (open the writer first).
    ///
    /// [execution-runtime]: ../../../docs/specs/architecture/components/execution-runtime.md
    /// [ADR-02]: ../../../docs/specs/architecture/decisions/ADR-02.md
    /// [NFR-PE-01]: ../../../docs/specs/requirements/NFR-PE-01.md
    pub fn open_readonly(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        // READ_ONLY (never create), NO_MUTEX (each connection is used by exactly
        // one thread at a time — the pool moves it, never shares it), URI off to
        // mirror a plain file open.
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("opening read-only graph store at {}", path.display()))?;
        configure_readonly_connection(&conn)?;

        // A read-only connection cannot migrate; assert the writer already did.
        let version = migrate::current_version(&conn)?;
        let latest = migrate::latest_version();
        if version != latest {
            return Err(anyhow!(
                "read-only graph store at {} is at schema v{version}, expected v{latest} — \
                 the writer must open (and migrate) the database before the reader pool attaches",
                path.display()
            ));
        }
        Ok(Self { conn })
    }

    /// Apply the per-connection contract then migrate to the latest schema.
    fn from_connection(mut conn: Connection) -> Result<Self> {
        configure_connection(&conn)?;
        migrate::apply_migrations(&mut conn)?;
        Ok(Self { conn })
    }

    /// [`open`](Self::open), timed per phase ([CR-116], [NFR-PE-05]): splits
    /// the connection-open-plus-pragma-contract phase from schema migration,
    /// which `open` (via [`from_connection`](Self::from_connection)) times as
    /// one indivisible call. `open` is untouched, so this diagnostic path's own
    /// cost never lands on the production cold-start path — used only by
    /// [`Engine::start_with_phase_report`](crate::Engine::start_with_phase_report).
    ///
    /// # Errors
    /// As [`open`](Self::open).
    ///
    /// [CR-116]: ../../../docs/requests/CR-116-cold-start-budget-and-its-guard-disagree.md
    /// [NFR-PE-05]: ../../../docs/specs/requirements/NFR-PE-05.md
    pub(crate) fn open_with_timings(path: impl AsRef<Path>) -> Result<(Self, StoreOpenTimings)> {
        let path = path.as_ref();

        let t = Instant::now();
        let mut conn = Connection::open(path)
            .with_context(|| format!("opening graph store at {}", path.display()))?;
        configure_connection(&conn)?;
        let connect = t.elapsed();

        let t = Instant::now();
        migrate::apply_migrations(&mut conn)?;
        let migrate = t.elapsed();

        Ok((Self { conn }, StoreOpenTimings { connect, migrate }))
    }

    /// The schema version recorded in `PRAGMA user_version`.
    ///
    /// # Errors
    /// Returns an error if the pragma cannot be read.
    pub fn schema_version(&self) -> Result<i64> {
        migrate::current_version(&self.conn)
    }

    /// Verify the **writer** connection pragma contract is in force ([FR-DB-02]).
    ///
    /// A cheap health check for a store opened through [`open`](Self::open) /
    /// [`open_in_memory`](Self::open_in_memory) (the writer path): confirms
    /// `foreign_keys` is ON, `synchronous` is NORMAL, `journal_mode` is WAL (or
    /// `memory` for in-memory stores), and the writer bulk-load pragmas (CR-057)
    /// are set — `cache_size`, `temp_store = MEMORY`, and, for an on-disk store, a
    /// non-zero `mmap_size` (memory-mapped I/O is inert on `:memory:`, so it reads
    /// back `0` there). It is **not** valid against a read-only pool connection
    /// ([`open_readonly`](Self::open_readonly)), which deliberately omits the
    /// bulk-load pragmas — it would report them missing.
    ///
    /// # Errors
    /// Returns an error naming the first pragma found out of contract.
    ///
    /// [FR-DB-02]: ../../../docs/specs/requirements/FR-DB-02.md
    pub fn verify_connection_contract(&self) -> Result<()> {
        let foreign_keys: i64 = self
            .conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .context("reading PRAGMA foreign_keys")?;
        if foreign_keys != 1 {
            return Err(anyhow!(
                "foreign_keys is {foreign_keys}, expected 1 (ON) — FR-DB-02 violated"
            ));
        }

        let synchronous: i64 = self
            .conn
            .query_row("PRAGMA synchronous", [], |r| r.get(0))
            .context("reading PRAGMA synchronous")?;
        if synchronous != 1 {
            return Err(anyhow!(
                "synchronous is {synchronous}, expected 1 (NORMAL) — FR-DB-02 violated"
            ));
        }

        let journal_mode: String = self
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .context("reading PRAGMA journal_mode")?;
        if journal_mode != "wal" && journal_mode != "memory" {
            return Err(anyhow!(
                "journal_mode is {journal_mode:?}, expected \"wal\" — FR-DB-02 violated"
            ));
        }

        // Writer bulk-load pragmas (FR-DB-02, CR-057). `cache_size` reads back
        // the negative KiB value we set; `temp_store` reads back MEMORY (2).
        let cache_size: i64 = self
            .conn
            .query_row("PRAGMA cache_size", [], |r| r.get(0))
            .context("reading PRAGMA cache_size")?;
        if cache_size != WRITER_CACHE_SIZE_KIB {
            return Err(anyhow!(
                "cache_size is {cache_size}, expected {WRITER_CACHE_SIZE_KIB} — \
                 the bulk-load contract (FR-DB-02, CR-057) is not in force"
            ));
        }
        let temp_store: i64 = self
            .conn
            .query_row("PRAGMA temp_store", [], |r| r.get(0))
            .context("reading PRAGMA temp_store")?;
        if temp_store != TEMP_STORE_MEMORY {
            return Err(anyhow!(
                "temp_store is {temp_store}, expected {TEMP_STORE_MEMORY} (MEMORY) — \
                 the bulk-load contract (FR-DB-02, CR-057) is not in force"
            ));
        }
        // mmap I/O is meaningful only for an on-disk file; `:memory:` reports 0.
        if journal_mode == "wal" {
            let mmap_size: i64 = self
                .conn
                .query_row("PRAGMA mmap_size", [], |r| r.get(0))
                .context("reading PRAGMA mmap_size")?;
            if mmap_size != WRITER_MMAP_SIZE_BYTES {
                return Err(anyhow!(
                    "mmap_size is {mmap_size}, expected {WRITER_MMAP_SIZE_BYTES} — \
                     the bulk-load contract (FR-DB-02, CR-057) is not in force"
                ));
            }
        }

        Ok(())
    }

    /// Run the FTS5 external-content integrity check ([FR-DB-03], [NFR-RA-09]).
    ///
    /// Asserts the `nodes_fts` inverted index is consistent with the `nodes`
    /// content table. A desync (e.g. a missed `'delete'` trigger row) makes
    /// SQLite return `SQLITE_CORRUPT_VTAB`, surfaced here as an error.
    ///
    /// # Errors
    /// Returns an error if the FTS index has desynced.
    ///
    /// [FR-DB-03]: ../../../docs/specs/requirements/FR-DB-03.md
    /// [NFR-RA-09]: ../../../docs/specs/requirements/NFR-RA-09.md
    pub fn fts_integrity_check(&self) -> Result<()> {
        self.conn
            .execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES('integrity-check');")
            .context("FTS5 integrity check failed — index desynced (NFR-RA-09)")
    }

    /// Checkpoint the WAL back into the main database file.
    ///
    /// Folds `-wal` frames into `logos.db` (TRUNCATE mode empties the WAL), so
    /// the single file is self-contained and copyable ([NFR-DM-03]). A no-op
    /// for in-memory stores.
    ///
    /// # Errors
    /// Returns an error if the checkpoint statement fails.
    pub fn checkpoint(&self) -> Result<()> {
        self.conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .context("checkpointing WAL into the main database file")
    }

    // ── Writers (autocommit) ─────────────────────────────────────────────────

    /// Insert a file row, returning its id.
    ///
    /// # Errors
    /// Returns an error on a constraint violation (e.g. duplicate `path`) or
    /// I/O failure.
    pub fn insert_file(
        &self,
        path: &str,
        language: Option<&str>,
        content_hash: Option<&str>,
    ) -> Result<i64> {
        insert_file(&self.conn, path, language, content_hash)
    }

    /// Insert the symbol if new (idempotent), returning its id.
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn upsert_symbol(&self, symbol: &LogosSymbol) -> Result<i64> {
        upsert_symbol(&self.conn, symbol)
    }

    /// Insert a node, or — if a node for `node.symbol_id` already exists —
    /// update it in place ([FR-SY-10], [ADR-46]). Idempotent over
    /// `symbol_id`: re-extracting the same symbol never accumulates a second
    /// row, it folds into the existing one.
    ///
    /// # Errors
    /// Returns an error if `kind` is out of range (the `CHECK` constraint
    /// fires), a foreign key is unsatisfied, or I/O fails.
    ///
    /// [FR-SY-10]: ../../../docs/specs/requirements/FR-SY-10.md
    /// [ADR-46]: ../../../docs/specs/architecture/decisions/ADR-46.md
    pub fn insert_node(&self, node: &NewNode<'_>) -> Result<NodeId> {
        insert_node(&self.conn, node)
    }

    /// Insert an edge.
    ///
    /// # Errors
    /// Returns an error if `(source, target, kind)` already exists (the
    /// uniqueness rule rejects it), an endpoint node is missing, or I/O fails.
    pub fn insert_edge(&self, source: NodeId, target: NodeId, kind: EdgeKind) -> Result<()> {
        insert_edge(&self.conn, source, target, kind)
    }

    /// Delete a node by id (cascading to its edges and firing the FTS
    /// `'delete'` trigger). Returns the number of rows removed (0 or 1).
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn delete_node(&self, id: NodeId) -> Result<usize> {
        delete_node(&self.conn, id)
    }

    // ── Atomic batch ─────────────────────────────────────────────────────────

    /// Run `f` inside a single transaction, committing only if it returns `Ok`.
    ///
    /// This is the crash-safety primitive ([NFR-RA-07]): the closure receives a
    /// [`BatchWriter`] and performs many writes; if any step returns `Err`, the
    /// transaction is dropped un-committed and SQLite rolls the whole batch back
    /// — no partial index/sync state survives an interruption.
    ///
    /// # Errors
    /// Returns the closure's error (after rollback), or an error if the
    /// transaction cannot be opened or committed.
    ///
    /// [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
    pub fn write_batch<T>(&mut self, f: impl FnOnce(&BatchWriter<'_>) -> Result<T>) -> Result<T> {
        let tx = self.conn.transaction().context("opening write batch")?;
        let out = {
            let writer = BatchWriter { conn: &tx };
            f(&writer)?
        };
        tx.commit().context("committing write batch")?;
        Ok(out)
    }
}

/// Decode the rows of a local-binding read — [`GraphStore::local_bindings`]
/// and [`GraphStore::local_bindings_with_extensions`] select the same columns.
fn local_binding_rows(
    stmt: &mut rusqlite::Statement<'_>,
    params: impl rusqlite::Params,
) -> Result<Vec<LocalBindingRow>> {
    let raws = stmt
        .query_map(params, |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                [
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                ],
                row.get::<_, i64>(9)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("collecting local bindings")?;
    raws.into_iter()
        .map(|(file_path, name, kind, at, hoisted)| {
            let corrupt = |what: &str, v: i64| {
                anyhow!("corrupt local-binding {what} {v} in {file_path}: rebuild advised (NFR-RA-08)")
            };
            let kind = i32::try_from(kind)
                .ok()
                .and_then(|k| LocalBindingKind::try_from(k).ok())
                .ok_or_else(|| corrupt("kind", kind))?;
            let mut pos = [0u32; 6];
            for (slot, v) in pos.iter_mut().zip(at) {
                *slot = u32::try_from(v).map_err(|_| corrupt("position", v))?;
            }
            let [line, column, scope_start_line, scope_start_column, scope_end_line, scope_end_column] = pos;
            Ok(LocalBindingRow {
                file_path: file_path.clone(),
                name,
                kind,
                line,
                column,
                scope_start_line,
                scope_start_column,
                scope_end_line,
                scope_end_column,
                hoisted: hoisted != 0,
            })
        })
        .collect()
}

impl GraphStore for SqliteGraphStore {
    fn node(&self, id: NodeId) -> Result<Option<NodeRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT n.id, s.symbol, n.kind, n.name, f.path, n.start_line, n.end_line \
             FROM nodes n \
             JOIN symbols s ON s.id = n.symbol_id \
             LEFT JOIN files f ON f.id = n.file_id \
             WHERE n.id = ?1",
        )?;
        let raw = stmt
            .query_row([id.get()], map_raw_row)
            .optional()
            .context("querying node by id")?;
        raw.map(raw_to_node).transpose()
    }

    fn callers(&self, id: NodeId) -> Result<Vec<NodeRow>> {
        self.adjacent(id, Direction::Callers)
    }

    fn callees(&self, id: NodeId) -> Result<Vec<NodeRow>> {
        self.adjacent(id, Direction::Callees)
    }

    fn search(&self, query: &str, kind: Option<NodeKind>, limit: i64) -> Result<Vec<NodeRow>> {
        // An empty/whitespace query is a well-defined no-op rather than an
        // opaque FTS5 `syntax error` (which a blank search box would otherwise
        // hit). `query` and the kind filter are BOUND parameters — the FTS
        // MATCH text never enters the SQL string (NFR-SE-02 injection boundary).
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare_cached(
            "SELECT n.id, s.symbol, n.kind, n.name, f.path, n.start_line, n.end_line \
             FROM nodes_fts \
             JOIN nodes n ON n.id = nodes_fts.rowid \
             JOIN symbols s ON s.id = n.symbol_id \
             LEFT JOIN files f ON f.id = n.file_id \
             WHERE nodes_fts MATCH ?1 \
               AND (?2 IS NULL OR n.kind = ?2) \
             ORDER BY rank \
             LIMIT ?3",
        )?;
        let kind_filter: Option<i32> = kind.map(NodeKind::as_i32);
        let raws = stmt
            .query_map(rusqlite::params![query, kind_filter, limit], map_raw_row)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting search results")?;
        raws.into_iter().map(raw_to_node).collect()
    }

    fn node_by_symbol(&self, symbol: &str) -> Result<Option<NodeRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT n.id, s.symbol, n.kind, n.name, f.path, n.start_line, n.end_line \
             FROM nodes n \
             JOIN symbols s ON s.id = n.symbol_id \
             LEFT JOIN files f ON f.id = n.file_id \
             WHERE s.symbol = ?1 \
             ORDER BY n.id \
             LIMIT 1",
        )?;
        let raw = stmt
            .query_row([symbol], map_raw_row)
            .optional()
            .context("querying node by canonical symbol")?;
        raw.map(raw_to_node).transpose()
    }

    fn nodes_by_name(&self, name: &str) -> Result<Vec<NodeRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT n.id, s.symbol, n.kind, n.name, f.path, n.start_line, n.end_line \
             FROM nodes n \
             JOIN symbols s ON s.id = n.symbol_id \
             LEFT JOIN files f ON f.id = n.file_id \
             WHERE n.name = ?1 \
             ORDER BY n.id",
        )?;
        let raws = stmt
            .query_map([name], map_raw_row)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting nodes by name")?;
        raws.into_iter().map(raw_to_node).collect()
    }

    fn node_names_for_path(&self, path: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT n.name FROM nodes n \
             JOIN files f ON f.id = n.file_id \
             WHERE f.path = ?1",
        )?;
        let names = stmt
            .query_map([path], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting node names for path")?;
        Ok(names)
    }

    fn import_aliases_for_path(&self, path: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT r.alias FROM unresolved_refs r \
             JOIN files f ON f.id = r.file_id \
             WHERE f.path = ?1 AND r.kind = ?2 AND r.alias IS NOT NULL",
        )?;
        let aliases = stmt
            .query_map(rusqlite::params![path, EdgeKind::Imports as i64], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting import aliases for path")?;
        Ok(aliases)
    }

    fn file_namespace(&self, path: &str) -> Result<Option<String>> {
        self.conn
            .query_row("SELECT namespace FROM files WHERE path = ?1", [path], |row| {
                row.get::<_, Option<String>>(0)
            })
            .optional()
            .map(Option::flatten)
            .context("reading a file's declared namespace")
    }

    fn declares_global_import(&self, path: &str) -> Result<bool> {
        self.conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM unresolved_refs r \
                 JOIN files f ON f.id = r.file_id \
                 WHERE f.path = ?1 AND r.kind = ?2 AND r.form = ?3 AND r.alias = ?4)",
                rusqlite::params![
                    path,
                    EdgeKind::Imports as i64,
                    RefForm::Glob as i64,
                    crate::resolve::GLOBAL_WILDCARD_ALIAS
                ],
                |row| row.get::<_, bool>(0),
            )
            .context("reading whether a file declares a global import")
    }

    fn config_definitions(&self, key: &str) -> Result<Vec<ConfigDefinition>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT f.path, s.profile, v.value \
             FROM config_values v \
             JOIN config_sources s ON s.id = v.source_id \
             JOIN files f ON f.id = s.file_id \
             WHERE v.key = ?1 \
             ORDER BY f.path, v.value",
        )?;
        let rows = stmt
            .query_map([key], |row| {
                Ok(ConfigDefinition {
                    path: row.get(0)?,
                    profile: row.get(1)?,
                    value: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting configuration definitions for key")?;
        Ok(rows)
    }

    fn build_manifests(&self) -> Result<Vec<BuildManifestRow>> {
        let mut manifests: Vec<(i64, BuildManifestRow)> = self
            .conn
            .prepare_cached(
                "SELECT id, path, format, content_hash, status, detail \
                 FROM build_manifests ORDER BY path",
            )?
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    BuildManifestRow {
                        path: row.get(1)?,
                        format: row.get(2)?,
                        content_hash: row.get(3)?,
                        status: row.get(4)?,
                        detail: row.get(5)?,
                        artifacts: Vec::new(),
                    },
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting build manifests")?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT role, kind, group_id, artifact_id, version, scope, project_path, \
                    resolution, reason \
             FROM build_artifacts WHERE manifest_id = ?1 ORDER BY id",
        )?;
        for (id, manifest) in &mut manifests {
            manifest.artifacts = stmt
                .query_map([*id], |row| {
                    Ok(BuildArtifactRow {
                        role: row.get(0)?,
                        kind: row.get(1)?,
                        group_id: row.get(2)?,
                        artifact_id: row.get(3)?,
                        version: row.get(4)?,
                        scope: row.get(5)?,
                        project_path: row.get(6)?,
                        resolution: row.get(7)?,
                        reason: row.get(8)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
                .context("collecting build artifacts for manifest")?;
        }
        Ok(manifests.into_iter().map(|(_, m)| m).collect())
    }

    fn declared_types(&self) -> Result<Vec<DeclaredTypeRow>> {
        self.conn
            .prepare_cached(
                "SELECT CASE WHEN d.file_id IS NULL THEN 'avro' ELSE 'source' END, \
                        COALESCE(f.path, s.path), d.name, d.fqn, d.kind, d.symbol, d.tree, \
                        d.resolution, d.reason \
                 FROM declared_types d \
                 LEFT JOIN files f ON f.id = d.file_id \
                 LEFT JOIN avro_schemas s ON s.id = d.schema_id \
                 ORDER BY 2, d.id",
            )?
            .query_map([], |row| {
                Ok(DeclaredTypeRow {
                    origin: row.get(0)?,
                    path: row.get(1)?,
                    name: row.get(2)?,
                    fqn: row.get(3)?,
                    kind: row.get(4)?,
                    symbol: row.get(5)?,
                    tree: row.get(6)?,
                    resolution: row.get(7)?,
                    reason: row.get(8)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting declared types")
    }

    fn avro_schemas(&self) -> Result<Vec<AvroSchemaRow>> {
        self.conn
            .prepare_cached(
                "SELECT path, content_hash, status, detail FROM avro_schemas ORDER BY path",
            )?
            .query_map([], |row| {
                Ok(AvroSchemaRow {
                    path: row.get(0)?,
                    content_hash: row.get(1)?,
                    status: row.get(2)?,
                    detail: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting Avro schemas")
    }

    fn neighbours_in(&self, id: NodeId) -> Result<Vec<(EdgeKind, NodeRow)>> {
        self.neighbours(id, SQL_NEIGHBOURS_IN)
    }

    fn neighbours_out(&self, id: NodeId) -> Result<Vec<(EdgeKind, NodeRow)>> {
        self.neighbours(id, SQL_NEIGHBOURS_OUT)
    }

    fn counts(&self) -> Result<StoreCounts> {
        // The reference ledger is counted without its capture-before-delete
        // rows (`form <> ?1`, [`RefForm::Symbol`]): each duplicates a reference
        // its source file's own row records, so counting it made a synced store
        // report figures a cold reindex does not (S-598, CR-195, [NFR-RA-06]).
        let mut stmt = self.conn.prepare_cached(
            "SELECT (SELECT COUNT(*) FROM files), \
                    (SELECT COUNT(*) FROM nodes), \
                    (SELECT COUNT(*) FROM edges), \
                    (SELECT COUNT(*) FROM unresolved_refs WHERE form <> ?1), \
                    (SELECT COUNT(*) FROM unresolved_refs WHERE resolved = 1 AND form <> ?1)",
        )?;
        stmt.query_row([RefForm::Symbol.as_i32()], |row| {
            Ok(StoreCounts {
                files: row.get::<_, i64>(0)? as u64,
                nodes: row.get::<_, i64>(1)? as u64,
                edges: row.get::<_, i64>(2)? as u64,
                refs_total: row.get::<_, i64>(3)? as u64,
                refs_resolved: row.get::<_, i64>(4)? as u64,
            })
        })
        .context("reading store counts")
    }

    fn language_composition(&self) -> Result<Vec<LanguageCount>> {
        // Drive presence off the node⋈file join (not the `files` table): a
        // language appears iff it has ≥1 indexed node ([FR-UI-10]). The inner
        // JOIN drops nodes whose `file_id` is NULL (file deleted, FK SET NULL)
        // and files with no nodes; `f.language IS NOT NULL` drops un-tagged
        // files. `COUNT(DISTINCT n.file_id)` is the contributing-file count.
        let mut stmt = self.conn.prepare_cached(
            "SELECT f.language, COUNT(n.id), COUNT(DISTINCT n.file_id) \
             FROM nodes n JOIN files f ON f.id = n.file_id \
             WHERE f.language IS NOT NULL \
             GROUP BY f.language",
        )?;
        let mut composition = stmt
            .query_map([], |row| {
                Ok(LanguageCount {
                    language: row.get::<_, String>(0)?,
                    nodes: row.get::<_, i64>(1)? as u64,
                    files: row.get::<_, i64>(2)? as u64,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("reading language composition")?;
        // Deterministic order, sorted in Rust so it is independent of SQLite's
        // collation and identical across the four target platforms ([NFR-RA-06]):
        // node count descending (the Dashboard sizes the Languages card by
        // magnitude), language name ascending as the stable tie-break.
        composition.sort_by(|a, b| {
            b.nodes
                .cmp(&a.nodes)
                .then_with(|| a.language.cmp(&b.language))
        });
        Ok(composition)
    }

    fn parse_damaged_paths(&self) -> Result<HashSet<String>> {
        let mut stmt = self.conn.prepare_cached("SELECT path FROM files WHERE parse_damaged = 1")?;
        let paths = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<HashSet<_>>>()
            .context("reading the parse-damaged files")?;
        Ok(paths)
    }

    fn production_callables_by_language(&self) -> Result<Vec<(String, u64)>> {
        // Sorted in Rust, not by SQLite's collation, so the order is identical
        // on every platform ([NFR-RA-06]).
        let mut stmt = self.conn.prepare_cached(
            "SELECT f.language, COUNT(*) FROM nodes n JOIN files f ON f.id = n.file_id \
             WHERE n.kind IN (?1, ?2) AND n.derived = 0 AND n.is_test = 0 \
               AND f.language IS NOT NULL \
             GROUP BY f.language",
        )?;
        let mut rows = stmt
            .query_map([NodeKind::Function.as_i32(), NodeKind::Method.as_i32()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("reading production callables per language")?;
        rows.sort();
        Ok(rows)
    }

    fn resolution_by_language(&self) -> Result<Vec<LanguageRefCounts>> {
        let (calls, imports) = (EdgeKind::Calls as i64, EdgeKind::Imports as i64);
        // One snapshot for all three reads. Outside a transaction each
        // statement is its own autocommit read, so a sync committing between
        // them could seed the rows from one generation and count them from the
        // next — a language whose first file lands after the seed silently
        // dropped, or one row mixing a ledger `bound` with another
        // generation's edge split. A deferred transaction is permitted on the
        // `query_only` reader and pins the first read's snapshot for the rest.
        // Opened only in autocommit, so a caller already inside a transaction
        // keeps its own snapshot rather than failing on a nested `BEGIN`.
        let snapshot = if self.conn.is_autocommit() {
            Some(self.conn.unchecked_transaction()?)
        } else {
            None
        };
        // Keyed by language in a `BTreeMap`, so the order is the map's rather
        // than SQLite's collation and is identical on every platform
        // ([NFR-RA-06]). Seeded from `files`, so every language in the index
        // earns a row before either count half is read.
        let mut rows: std::collections::BTreeMap<String, LanguageRefCounts> =
            std::collections::BTreeMap::new();
        let mut files = self.conn.prepare_cached(
            "SELECT language, COUNT(*), COALESCE(SUM(path LIKE '%.h'), 0), \
                    COALESCE(SUM(parse_damaged = 1), 0), COALESCE(SUM(parse_damaged IS NULL), 0) \
             FROM files WHERE language IS NOT NULL GROUP BY language",
        )?;
        for row in files.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)? as u64,
                row.get::<_, i64>(2)? as u64,
                row.get::<_, i64>(3)? as u64,
                row.get::<_, i64>(4)? as u64,
            ))
        })? {
            let (language, count, headers, damaged, unrecorded) =
                row.context("reading per-language file counts")?;
            rows.insert(
                language.clone(),
                LanguageRefCounts {
                    language,
                    files: count,
                    header_files: headers,
                    damaged_files: damaged,
                    unrecorded_files: unrecorded,
                    ..LanguageRefCounts::default()
                },
            );
        }
        // The ledger half: the numerator over its denominator. A
        // capture-before-delete row (`RefForm::Symbol`) sits under its *target*
        // file and duplicates a reference its source's own row records, so it is
        // no reference of this readout (S-598, CR-195, [NFR-RA-06]).
        let mut ledger = self.conn.prepare_cached(
            "SELECT f.language, r.kind, COUNT(*), COALESCE(SUM(r.resolved), 0) \
             FROM unresolved_refs r JOIN files f ON f.id = r.file_id \
             WHERE f.language IS NOT NULL AND r.kind IN (?1, ?2) AND r.form <> ?3 \
             GROUP BY f.language, r.kind",
        )?;
        let ledger_rows = ledger
            .query_map([calls, imports, i64::from(RefForm::Symbol.as_i32())], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)? as u64,
                    row.get::<_, i64>(3)? as u64,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("reading the per-language reference ledger")?;
        for (language, kind, references, bound) in ledger_rows {
            if let Some(counts) = rows
                .get_mut(&language)
                .and_then(|r| relation_class(r, kind))
            {
                counts.references = references;
                counts.bound = bound;
            }
        }

        // The edge half: the resolved edge set, split on whether the target
        // shares the source's file. Both endpoints must lie in an indexed file
        // (the inner joins), or the edge has no locality to report.
        let mut edges = self.conn.prepare_cached(
            "SELECT fs.language, e.kind, \
                    COALESCE(SUM(ns.file_id = nt.file_id), 0), \
                    COALESCE(SUM(ns.file_id <> nt.file_id), 0) \
             FROM edges e \
             JOIN nodes ns ON ns.id = e.source \
             JOIN nodes nt ON nt.id = e.target \
             JOIN files fs ON fs.id = ns.file_id \
             JOIN files ft ON ft.id = nt.file_id \
             WHERE fs.language IS NOT NULL AND e.kind IN (?1, ?2) \
             GROUP BY fs.language, e.kind",
        )?;
        let edge_rows = edges
            .query_map([calls, imports], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)? as u64,
                    row.get::<_, i64>(3)? as u64,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("reading the per-language resolved edge set")?;
        for (language, kind, same_file, cross_file) in edge_rows {
            if let Some(counts) = rows
                .get_mut(&language)
                .and_then(|r| relation_class(r, kind))
            {
                counts.same_file_edges = same_file;
                counts.cross_file_edges = cross_file;
            }
        }
        if let Some(snapshot) = snapshot {
            snapshot.commit()?;
        }
        Ok(rows.into_values().collect())
    }

    fn reference_population(&self) -> Result<ReferencePopulation> {
        let (doc, symbol) = (
            i64::from(EdgeKind::DocReference.as_i32()),
            i64::from(RefForm::Symbol.as_i32()),
        );
        // One snapshot for every read, for `resolution_by_language`'s reason: a
        // sync committing between two autocommit reads would take the ledger
        // and the names from two generations.
        let snapshot = if self.conn.is_autocommit() {
            Some(self.conn.unchecked_transaction()?)
        } else {
            None
        };
        let mut population = ReferencePopulation::default();
        // The files first: each one's path and namespace for the universe, and
        // its language, interned once so the ledger scan below keys a row by an
        // index rather than joining `files` per row.
        let mut languages: Vec<Option<String>> = vec![None];
        let mut language_of: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();
        let mut files = self
            .conn
            .prepare_cached("SELECT id, path, namespace, language FROM files")?;
        for file in files.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })? {
            let (id, path, namespace, language) = file.context("reading the indexed files")?;
            language_of.insert(id, intern(&mut languages, language));
            population.names.files.push((path, namespace));
        }
        // The whole reported ledger in ONE plain scan — no join and no
        // `GROUP BY`, whose sort cost ten times the scan on a 22,000-row
        // ledger: the halves' counts (every row), the unbound code targets
        // (a target repeats across call sites — `clone`, `map` — so each is
        // classified once per language and kind, and allocated once), and the
        // aliases with their targets. A capture-before-delete row
        // (`RefForm::Symbol`) is no reference of this readout (S-598).
        let mut ledger = self.conn.prepare_cached(
            "SELECT file_id, kind, resolved, target, alias, payload, source_symbol \
             FROM unresolved_refs WHERE form <> ?1",
        )?;
        let mut invocations: std::collections::BTreeMap<(String, String), u64> =
            std::collections::BTreeMap::new();
        let mut unbound: std::collections::HashMap<(usize, i64), std::collections::HashMap<String, u64>> =
            std::collections::HashMap::new();
        let mut aliases: std::collections::BTreeSet<(String, String)> =
            std::collections::BTreeSet::new();
        let mut rows = ledger.query([symbol])?;
        while let Some(row) = rows.next()? {
            let language = row
                .get::<_, Option<i64>>(0)?
                .and_then(|id| language_of.get(&id).copied())
                .unwrap_or(0);
            let kind: i64 = row.get(1)?;
            let resolved: bool = row.get(2)?;
            // Read lossily, as the alias below is: a target the ledger holds
            // as bytes that are not UTF-8 must not fail the whole `status`
            // this readout rides on — every other read of it would succeed.
            let target = String::from_utf8_lossy(row.get_ref(3)?.as_bytes()?);
            let target: &str = &target;
            let (references, bound) = if kind == doc {
                (&mut population.doc_references, &mut population.doc_resolved)
            } else {
                (&mut population.code_references, &mut population.code_resolved)
            };
            *references += 1;
            // A call the binder proved goes through a local value (S-594)
            // binds nothing by design and names no declaration, so no name
            // test can call it external: it stays an internal candidate, never
            // judged by whether its local's name is declared elsewhere.
            let local_value = row.get_ref(4)?.as_str().ok() == Some(crate::resolve::LOCAL_VALUE_ALIAS);
            if resolved {
                *bound += 1;
            } else if kind != doc && !local_value {
                tally(unbound.entry((language, kind)).or_default(), target);
                tally_invocation(&mut invocations, row, target)?;
            }
            if let rusqlite::types::ValueRef::Text(alias) = row.get_ref(4)? {
                aliases.insert((String::from_utf8_lossy(alias).into_owned(), target.to_string()));
            }
        }
        // Sorted in Rust, so the order is identical on every platform
        // ([NFR-RA-06]).
        population.unbound_code = unbound
            .into_iter()
            .flat_map(|((language, kind), targets)| {
                let language = &languages[language];
                targets.into_iter().map(move |(target, rows)| UnboundCodeTarget {
                    language: language.clone(),
                    kind,
                    target,
                    rows,
                })
            })
            .collect();
        population.unbound_code.sort_by(|a, b| {
            (&a.language, a.kind, &a.target).cmp(&(&b.language, b.kind, &b.target))
        });
        population.unbound_invocations = invocations
            .into_iter()
            .map(|((source_symbol, target), rows)| UnboundInvocation { source_symbol, target, rows })
            .collect();
        population.names.aliases = aliases.into_iter().collect();
        // Every node name, repeats included: the universe is a set, so a
        // `DISTINCT` would only add a sort.
        let mut names = self.conn.prepare_cached("SELECT name FROM nodes")?;
        population.names.names = names
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("reading the declared names")?;
        if let Some(snapshot) = snapshot {
            snapshot.commit()?;
        }
        Ok(population)
    }

    fn suggest(&self, text: &str, limit: i64) -> Result<Vec<String>> {
        if limit <= 0 {
            return Ok(Vec::new());
        }
        // Strategy 1: FTS5 prefix match on the first identifier-ish token.
        // The token is embedded in a quoted FTS string ("tok"*), itself a
        // BOUND parameter — user text never reaches the SQL string (NFR-SE-02).
        if let Some(token) = first_identifier_token(text) {
            let mut stmt = self.conn.prepare_cached(
                "SELECT DISTINCT n.name \
                 FROM nodes_fts \
                 JOIN nodes n ON n.id = nodes_fts.rowid \
                 WHERE nodes_fts MATCH ?1 \
                 ORDER BY rank \
                 LIMIT ?2",
            )?;
            let fts_query = format!("\"{}\"*", token.replace('"', "\"\""));
            // An FTS syntax error on hostile input is not a failure of the
            // graceful contract — fall through to the LIKE strategy.
            let names: Vec<String> = stmt
                .query_map(rusqlite::params![fts_query, limit], |r| {
                    r.get::<_, String>(0)
                })
                .map(|rows| rows.filter_map(std::result::Result::ok).collect())
                .unwrap_or_default();
            if !names.is_empty() {
                return Ok(names);
            }
        }
        // Strategy 2: substring LIKE over names, wildcards escaped so the
        // user text matches literally.
        let escaped = text
            .trim()
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        if escaped.is_empty() {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare_cached(
            "SELECT DISTINCT name FROM nodes \
             WHERE name LIKE '%' || ?1 || '%' ESCAPE '\\' \
             ORDER BY name \
             LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![escaped, limit], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting LIKE suggestions")?;
        Ok(rows)
    }

    fn all_nodes(&self) -> Result<Vec<NodeRow>> {
        // ORDER BY n.id is load-bearing, not cosmetic: hydration assigns petgraph
        // vertex indices in iteration order, so a stable order makes graph
        // algorithms reproducible (NFR-RA-06).
        let mut stmt = self.conn.prepare_cached(
            "SELECT n.id, s.symbol, n.kind, n.name, f.path, n.start_line, n.end_line \
             FROM nodes n \
             JOIN symbols s ON s.id = n.symbol_id \
             LEFT JOIN files f ON f.id = n.file_id \
             ORDER BY n.id",
        )?;
        let raws = stmt
            .query_map([], map_raw_row)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting all nodes for hydration")?;
        raws.into_iter().map(raw_to_node).collect()
    }

    fn node_self_types(&self) -> Result<Vec<(NodeId, String)>> {
        // ORDER BY id keeps the read deterministic ([NFR-RA-06]).
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, self_type FROM nodes WHERE self_type IS NOT NULL ORDER BY id",
        )?;
        let rows = stmt
            .query_map([], |row| Ok((NodeId(row.get(0)?), row.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting node self types for the binder")?;
        Ok(rows)
    }

    fn node_arities(&self) -> Result<Vec<NodeArity>> {
        // ORDER BY id keeps the read deterministic ([NFR-RA-06]).
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, param_min, param_max, takes_self FROM nodes \
             WHERE param_min IS NOT NULL OR takes_self IS NOT NULL ORDER BY id",
        )?;
        let raws = stmt
            .query_map([], |row| {
                Ok((
                    NodeId(row.get(0)?),
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting node arities for the binder")?;
        raws.into_iter()
            .map(|(id, min, max, takes_self)| {
                let count = |v: i64| {
                    u32::try_from(v)
                        .map_err(|e| anyhow!("corrupt parameter count {v} for node {}: {e}; rebuild advised (NFR-RA-08)", id.0))
                };
                let params = match min {
                    Some(min) => Some(ParamRange {
                        min: count(min)?,
                        max: max.map(count).transpose()?,
                    }),
                    None => None,
                };
                Ok((id, params, takes_self.map(|v| v != 0)))
            })
            .collect()
    }

    fn node_item_facts(&self) -> Result<Vec<NodeItemFacts>> {
        // ORDER BY id keeps the read deterministic ([NFR-RA-06]).
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, receiver_mode, variants, signature FROM nodes \
             WHERE receiver_mode IS NOT NULL OR variants IS NOT NULL OR signature IS NOT NULL ORDER BY id",
        )?;
        let raws = stmt
            .query_map([], |row| {
                Ok((
                    NodeId(row.get(0)?),
                    row.get::<_, Option<i32>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting node item facts")?;
        raws.into_iter()
            .map(|(id, mode, variants, signature)| {
                let receiver_mode = mode.map(ReceiverMode::try_from).transpose().map_err(|e| {
                    anyhow!("corrupt receiver mode for node {}: {e}; rebuild advised (NFR-RA-08)", id.0)
                })?;
                Ok(NodeItemFacts {
                    id,
                    receiver_mode,
                    variants,
                    signature: signature.is_some(),
                })
            })
            .collect()
    }

    fn uninherited_members(&self) -> Result<Vec<NodeId>> {
        // ORDER BY id keeps the read deterministic ([NFR-RA-06]).
        let mut stmt = self.conn.prepare_cached(
            "SELECT id FROM nodes WHERE has_body = 0 OR uninherited IS NOT NULL ORDER BY id",
        )?;
        let ids = stmt
            .query_map([], |row| Ok(NodeId(row.get(0)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting uninherited members for the binder")?;
        Ok(ids)
    }

    fn unexported_functions(&self) -> Result<Option<Vec<NodeId>>> {
        // ORDER BY id keeps the read deterministic ([NFR-RA-06]).
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id FROM nodes WHERE kind = ?1 AND exported = 0 ORDER BY id")?;
        let ids = stmt
            .query_map([NodeKind::Function.as_i32()], |row| Ok(NodeId(row.get(0)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting unexported functions for the binder")?;
        Ok(Some(ids))
    }

    fn type_arities(&self) -> Result<Vec<(NodeId, u32)>> {
        // ORDER BY id keeps the read deterministic ([NFR-RA-06]).
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id, type_arity FROM nodes WHERE type_arity IS NOT NULL ORDER BY id")?;
        let raws = stmt
            .query_map([], |row| Ok((NodeId(row.get(0)?), row.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting type arities for the binder")?;
        raws.into_iter()
            .map(|(id, arity)| {
                let arity = u32::try_from(arity)
                    .map_err(|e| anyhow!("corrupt type arity {arity} for node {}: {e}; rebuild advised (NFR-RA-08)", id.0))?;
                Ok((id, arity))
            })
            .collect()
    }

    fn partial_type_links(&self) -> Result<Vec<(NodeId, NodeId)>> {
        // ORDER BY source keeps the read deterministic ([NFR-RA-06]).
        let mut stmt = self
            .conn
            .prepare_cached("SELECT source, target FROM edges WHERE kind = ?1 AND payload = ?2 ORDER BY source, target")?;
        let links = stmt
            .query_map(
                rusqlite::params![EdgeKind::Extends.as_i32(), crate::resolve::PARTIAL_TYPE_ALIAS],
                |row| Ok((NodeId(row.get(0)?), NodeId(row.get(1)?))),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting partial type links for the metrics")?;
        Ok(links)
    }

    fn dispatch_links(&self) -> Result<Vec<(NodeId, NodeId, String)>> {
        // ORDER BY source keeps the read deterministic ([NFR-RA-06]).
        let mut stmt = self.conn.prepare_cached(
            "SELECT source, target, payload FROM edges WHERE kind = ?1 AND payload GLOB ?2 ORDER BY source, target",
        )?;
        let links = stmt
            .query_map(
                rusqlite::params![EdgeKind::Calls.as_i32(), crate::model::dispatch::DISPATCH_GLOB],
                |row| Ok((NodeId(row.get(0)?), NodeId(row.get(1)?), row.get::<_, String>(2)?)),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting dispatch links")?;
        Ok(links
            .into_iter()
            .filter_map(|(source, target, payload)| {
                crate::model::dispatch::dispatched_method(&payload).map(|m| (source, target, m.to_string()))
            })
            .collect())
    }

    fn dispatched_via(&self, pairs: &[(NodeId, NodeId)]) -> Result<HashMap<(NodeId, NodeId), String>> {
        // One indexed point read per pair: `UNIQUE(source, target, kind)`.
        let mut stmt = self
            .conn
            .prepare_cached("SELECT payload FROM edges WHERE source = ?1 AND target = ?2 AND kind = ?3 AND payload GLOB ?4")?;
        let mut via = HashMap::new();
        for &(source, target) in pairs {
            let payload: Option<String> = stmt
                .query_row(
                    rusqlite::params![
                        source.get(),
                        target.get(),
                        EdgeKind::Calls.as_i32(),
                        crate::model::dispatch::DISPATCH_GLOB
                    ],
                    |row| row.get(0),
                )
                .optional()
                .context("reading a dispatch marker")?;
            if let Some(method) = payload.as_deref().and_then(crate::model::dispatch::dispatched_method) {
                via.insert((source, target), method.to_string());
            }
        }
        Ok(via)
    }

    fn impl_blocks(&self) -> Result<Vec<ImplBlockRow>> {
        // ORDER BY path, line keeps the read deterministic ([NFR-RA-06]).
        let mut stmt = self.conn.prepare_cached(
            "SELECT f.path, b.start_line, b.end_line, b.self_type, b.self_ref, b.trait_path, b.deref_target \
             FROM impl_blocks b JOIN files f ON f.id = b.file_id \
             ORDER BY f.path, b.start_line, b.end_line, b.self_type, b.id",
        )?;
        let raws = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting impl blocks")?;
        raws.into_iter()
            .map(|(file_path, start, end, self_type, self_ref, trait_path, deref_target)| {
                let line = |v: i64| {
                    u32::try_from(v)
                        .map_err(|e| anyhow!("corrupt impl-block line {v} in {file_path}: {e}; rebuild advised (NFR-RA-08)"))
                };
                Ok(ImplBlockRow {
                    start_line: line(start)?,
                    end_line: line(end)?,
                    file_path: file_path.clone(),
                    self_type,
                    self_ref: self_ref != 0,
                    trait_path,
                    deref_target,
                })
            })
            .collect()
    }

    fn local_bindings(&self) -> Result<Vec<LocalBindingRow>> {
        // ORDER BY path, position keeps the read deterministic ([NFR-RA-06]).
        let mut stmt = self.conn.prepare_cached(
            "SELECT f.path, b.name, b.kind, b.line, b.col, b.scope_start_line, b.scope_start_col, \
                    b.scope_end_line, b.scope_end_col, b.hoisted \
             FROM local_bindings b JOIN files f ON f.id = b.file_id \
             ORDER BY f.path, b.line, b.col, b.name, b.kind, b.id",
        )?;
        local_binding_rows(&mut stmt, [])
    }

    fn local_bindings_with_extensions(&self, extensions: &[String]) -> Result<Vec<LocalBindingRow>> {
        if extensions.is_empty() {
            return Ok(Vec::new());
        }
        // The extensions are bound parameters, never spliced into the text;
        // ORDER BY path, position keeps the read deterministic ([NFR-RA-06]).
        let matches = vec!["lower(f.path) LIKE ?"; extensions.len()].join(" OR ");
        let sql = format!(
            "SELECT f.path, b.name, b.kind, b.line, b.col, b.scope_start_line, b.scope_start_col, \
                    b.scope_end_line, b.scope_end_col, b.hoisted \
             FROM local_bindings b JOIN files f ON f.id = b.file_id \
             WHERE {matches} \
             ORDER BY f.path, b.line, b.col, b.name, b.kind, b.id"
        );
        let patterns: Vec<String> = extensions.iter().map(|ext| format!("%.{ext}")).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        local_binding_rows(&mut stmt, rusqlite::params_from_iter(patterns))
    }

    fn files_binding_parameters(&self, names: &[String]) -> Result<Vec<(i64, String)>> {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        // ORDER BY keeps the read deterministic ([NFR-RA-06]); the names are
        // bound parameters, never spliced into the text.
        let placeholders = vec!["?"; names.len()].join(", ");
        let sql = format!(
            "SELECT DISTINCT file_id, name FROM local_bindings \
             WHERE kind = ? AND name IN ({placeholders}) ORDER BY file_id, name"
        );
        let parameter = i64::from(LocalBindingKind::Parameter.as_i32());
        let params = std::iter::once(rusqlite::types::Value::Integer(parameter))
            .chain(names.iter().map(|n| rusqlite::types::Value::Text(n.clone())));
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(params), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting framework receiver-parameter bindings")?;
        Ok(rows)
    }

    fn file_namespaces(&self) -> Result<Vec<(String, String)>> {
        // ORDER BY path keeps the read deterministic ([NFR-RA-06]).
        let mut stmt = self.conn.prepare_cached(
            "SELECT path, namespace FROM files WHERE namespace IS NOT NULL ORDER BY path",
        )?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting file namespaces for the binder")?;
        Ok(rows)
    }

    fn proto_service_bodies(&self) -> Result<Vec<(LogosSymbol, String)>> {
        // Only `ProtoService` nodes carry the S-253 rpc-method body; ORDER BY id
        // keeps the read deterministic ([NFR-RA-06]).
        let mut stmt = self.conn.prepare_cached(
            "SELECT s.symbol, n.body \
             FROM nodes n \
             JOIN symbols s ON s.id = n.symbol_id \
             WHERE n.kind = ?1 AND n.body IS NOT NULL \
             ORDER BY n.id",
        )?;
        let rows = stmt
            .query_map([NodeKind::ProtoService.as_i32()], |row| {
                let symbol: String = row.get(0)?;
                let body: String = row.get(1)?;
                Ok((symbol, body))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting proto service bodies for the gRPC bridge")?;
        rows.into_iter()
            .map(|(symbol, body)| {
                let symbol = LogosSymbol::parse(&symbol).with_context(|| {
                    format!("corrupt symbol stored for proto service {symbol}; rebuild advised (NFR-RA-08)")
                })?;
                Ok((symbol, body))
            })
            .collect()
    }

    fn all_edges(&self) -> Result<Vec<EdgeRow>> {
        // Deterministic order for reproducible hydration (NFR-RA-06).
        // `IS 1` folds a NULL payload's NULL match to false.
        let mut stmt = self.conn.prepare_cached(
            "SELECT source, target, kind, (payload GLOB ?1) IS 1 FROM edges \
             ORDER BY source, target, kind",
        )?;
        let raws = stmt
            .query_map([crate::model::dispatch::DISPATCH_GLOB], |row| {
                let source: i64 = row.get(0)?;
                let target: i64 = row.get(1)?;
                let kind: i32 = row.get(2)?;
                let dispatch: bool = row.get(3)?;
                Ok((source, target, kind, dispatch))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting all edges for hydration")?;
        raws.into_iter()
            .map(|(source, target, kind, dispatch)| {
                let kind = EdgeKind::try_from(kind).map_err(|e| {
                    anyhow!("corrupt edge kind {kind} for edge {source}->{target}: {e}; rebuild advised (NFR-RA-08)")
                })?;
                Ok(EdgeRow {
                    source: NodeId(source),
                    target: NodeId(target),
                    kind,
                    dispatch,
                })
            })
            .collect()
    }

    fn symbol_edges(&self) -> Result<Vec<SymbolEdgeRow>> {
        // ORDER BY (kind, symbols) is the deterministic order (NFR-RA-06); the
        // LEFT JOINs keep an edge whose endpoint is unbound to a file.
        let mut stmt = self.conn.prepare_cached(
            "SELECT e.kind, ss.symbol, ts.symbol, sf.path, tf.path \
             FROM edges e \
             JOIN nodes sn ON sn.id = e.source \
             JOIN symbols ss ON ss.id = sn.symbol_id \
             LEFT JOIN files sf ON sf.id = sn.file_id \
             JOIN nodes tn ON tn.id = e.target \
             JOIN symbols ts ON ts.id = tn.symbol_id \
             LEFT JOIN files tf ON tf.id = tn.file_id \
             ORDER BY e.kind, ss.symbol, ts.symbol",
        )?;
        let raws = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i32>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting symbol-keyed edges for verify")?;
        raws.into_iter()
            .map(|(kind, source, target, source_file, target_file)| {
                let kind = EdgeKind::try_from(kind).map_err(|e| {
                    anyhow!("corrupt edge kind {kind} for edge {source}->{target}: {e}; rebuild advised (NFR-RA-08)")
                })?;
                Ok(SymbolEdgeRow {
                    kind,
                    source,
                    target,
                    source_file,
                    target_file,
                })
            })
            .collect()
    }

    fn dispatch_markers(&self) -> Result<Vec<NodeId>> {
        // Only the dispatch live-root markers (RoutesTo self-edges) — a tiny set,
        // so the dispatch pass never pays the whole-graph `all_edges` scan on the
        // sync hot path (NFR-PE-03). `source` order keeps the result deterministic
        // (NFR-RA-06).
        let kind = EdgeKind::RoutesTo.as_i32();
        let mut stmt = self.conn.prepare_cached(
            "SELECT source FROM edges WHERE kind = ?1 AND source = target ORDER BY source",
        )?;
        let ids = stmt
            .query_map([kind], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting dispatch live-root markers")?;
        Ok(ids.into_iter().map(NodeId).collect())
    }

    fn callable_nodes_in_files(&self, paths: &[String]) -> Result<Vec<NodeRow>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        // Dynamic `IN (?,?,…)` over the (small) changed-file set; `prepare` not
        // `prepare_cached` because the placeholder count varies. `kind IN (7,8)`
        // is Function/Method (the only nodes that carry a dispatch marker); a
        // required signature (S-606) carries none and is left out.
        let placeholders = vec!["?"; paths.len()].join(",");
        let func = NodeKind::Function.as_i32();
        let method = NodeKind::Method.as_i32();
        let sql = format!(
            "SELECT n.id, s.symbol, n.kind, n.name, f.path, n.start_line, n.end_line \
             FROM nodes n \
             JOIN symbols s ON s.id = n.symbol_id \
             JOIN files f ON f.id = n.file_id \
             WHERE f.path IN ({placeholders}) AND n.kind IN ({func}, {method}) \
               AND n.signature IS NULL \
             ORDER BY n.id"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let raws = stmt
            .query_map(rusqlite::params_from_iter(paths.iter()), map_raw_row)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting callable nodes in files")?;
        raws.into_iter().map(raw_to_node).collect()
    }

    fn span_nodes_in_files(&self, paths: &[String]) -> Result<Vec<NodeRow>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        // Dynamic `IN (?,?,…)` over the (small) changed-file set; `prepare` not
        // `prepare_cached` because the placeholder count varies. No kind filter:
        // a git hunk can land on any declaration that recorded a span, and
        // `start_line IS NOT NULL` is exactly the "has a span" predicate.
        let placeholders = vec!["?"; paths.len()].join(",");
        let sql = format!(
            "SELECT n.id, s.symbol, n.kind, n.name, f.path, n.start_line, n.end_line \
             FROM nodes n \
             JOIN symbols s ON s.id = n.symbol_id \
             JOIN files f ON f.id = n.file_id \
             WHERE f.path IN ({placeholders}) AND n.start_line IS NOT NULL \
             ORDER BY f.path, n.start_line, n.id"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let raws = stmt
            .query_map(rusqlite::params_from_iter(paths.iter()), map_raw_row)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting span-bearing nodes in files")?;
        raws.into_iter().map(raw_to_node).collect()
    }

    fn file_languages(
        &self,
        paths: &[String],
    ) -> Result<std::collections::BTreeMap<String, Option<String>>> {
        if paths.is_empty() {
            return Ok(std::collections::BTreeMap::new());
        }
        // Dynamic `IN (?,?,…)` over the answer's (small) anchor set, as
        // `span_nodes_in_files`; served by the `files.path` UNIQUE index.
        let placeholders = vec!["?"; paths.len()].join(",");
        let sql = format!("SELECT path, language FROM files WHERE path IN ({placeholders})");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(paths.iter()), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })?
            .collect::<rusqlite::Result<std::collections::BTreeMap<_, _>>>()
            .context("reading the languages of the named files")?;
        Ok(rows)
    }

    fn markers_for_nodes(&self, node_ids: &[NodeId]) -> Result<Vec<NodeId>> {
        if node_ids.is_empty() {
            return Ok(Vec::new());
        }
        // `source IN (…) AND kind = … AND source = target`: the `source IN` +
        // kind predicate is served by `idx_edges_source_kind`, so this is
        // O(changed nodes), not the whole-edge scan (NFR-PE-03). `prepare` (not
        // cached) because the placeholder count varies with the changed set.
        let kind = EdgeKind::RoutesTo.as_i32();
        let placeholders = vec!["?"; node_ids.len()].join(",");
        let sql = format!(
            "SELECT source FROM edges \
             WHERE kind = {kind} AND source = target AND source IN ({placeholders}) \
             ORDER BY source"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let ids = stmt
            .query_map(rusqlite::params_from_iter(node_ids.iter().map(|n| n.0)), |row| {
                row.get::<_, i64>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting dispatch markers for nodes")?;
        Ok(ids.into_iter().map(NodeId).collect())
    }

    fn indexed_files(&self) -> Result<Vec<FileRecord>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id, path, content_hash FROM files ORDER BY path")?;
        let rows = stmt
            .query_map([], |row| {
                Ok(FileRecord {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    content_hash: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("listing indexed files")?;
        Ok(rows)
    }

    fn persist_failures(&self) -> Result<Vec<PersistFailureRow>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT path, reason, stale FROM persist_failures ORDER BY path")?;
        let rows = stmt
            .query_map([], |row| {
                Ok(PersistFailureRow {
                    path: row.get(0)?,
                    reason: row.get(1)?,
                    stale: row.get::<_, i64>(2)? != 0,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("listing persist failures")?;
        Ok(rows)
    }

    fn project_metadata(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM project_metadata WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()
            .context("reading project_metadata value")
    }

    fn holds_symbols(&self) -> Result<bool> {
        self.conn
            .query_row("SELECT EXISTS (SELECT 1 FROM symbols)", [], |row| row.get(0))
            .context("probing for symbol rows")
    }

    fn unresolved_refs(&self) -> Result<Vec<UnresolvedRefRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, file_id, source_symbol, target, alias, form, kind, line, resolved, payload, \
                    receiver, peeled, arg_count, exported \
             FROM unresolved_refs ORDER BY id",
        )?;
        let raws = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i32>(5)?,
                    row.get::<_, i32>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<i32>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<i64>>(12)?,
                    row.get::<_, Option<i64>>(13)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting the reference ledger")?;
        raws.into_iter()
            .map(
                |(id, file_id, source_symbol, target, alias, form, kind, line, resolved, payload, receiver, peeled, arg_count, exported)| {
                    let form = RefForm::try_from(form).map_err(|e| {
                        anyhow!("corrupt ref form {form} for ref {id}: {e}; rebuild advised (NFR-RA-08)")
                    })?;
                    let kind = EdgeKind::try_from(kind).map_err(|e| {
                        anyhow!("corrupt ref kind {kind} for ref {id}: {e}; rebuild advised (NFR-RA-08)")
                    })?;
                    let receiver = receiver
                        .map(ReceiverShape::try_from)
                        .transpose()
                        .map_err(|e| {
                            anyhow!("corrupt receiver shape for ref {id}: {e}; rebuild advised (NFR-RA-08)")
                        })?;
                    let arg_count = arg_count
                        .map(u32::try_from)
                        .transpose()
                        .map_err(|e| anyhow!("corrupt argument count for ref {id}: {e}; rebuild advised (NFR-RA-08)"))?;
                    Ok(UnresolvedRefRow {
                        id,
                        file_id,
                        source_symbol,
                        target,
                        alias,
                        form,
                        kind,
                        line,
                        resolved: resolved != 0,
                        payload,
                        receiver,
                        peeled,
                        arg_count,
                        exported: exported.map(|v| v != 0),
                    })
                },
            )
            .collect()
    }

    fn unresolved_type_refs(&self) -> Result<Vec<TypeRefRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT r.source_symbol, f.path, r.line, r.target, r.kind \
             FROM unresolved_refs r JOIN files f ON f.id = r.file_id \
             WHERE r.resolved = 0 AND r.kind IN (?1, ?2) \
               AND (f.path GLOB '*.java' OR f.path GLOB '*.kt') \
             ORDER BY f.path, r.id",
        )?;
        let raws = stmt
            .query_map(
                rusqlite::params![EdgeKind::Imports.as_i32(), EdgeKind::TypeUses.as_i32()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i32>(4)?,
                    ))
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting the unresolved type references")?;
        raws.into_iter()
            .map(|(source_symbol, path, line, target, kind)| {
                let kind = EdgeKind::try_from(kind).map_err(|e| {
                    anyhow!("corrupt ref kind {kind} in {path}: {e}; rebuild advised (NFR-RA-08)")
                })?;
                Ok(TypeRefRow { source_symbol, path, line, target, kind })
            })
            .collect()
    }

    fn annotation_nodes(&self) -> Result<Vec<AnnotationNodeRow>> {
        // ORDER BY n.id keeps the annotation pass deterministic (NFR-RA-06).
        let mut stmt = self.conn.prepare_cached(
            "SELECT n.id, n.kind, n.name, n.exported, n.derived, n.fingerprint, \
                    n.test_evidence, n.file_id, f.path, n.is_dead, n.is_duplicate, \
                    n.is_test, n.layer_membership, n.clone_group, n.has_body, \
                    n.body_tokens \
             FROM nodes n \
             LEFT JOIN files f ON f.id = n.file_id \
             ORDER BY n.id",
        )?;
        let raws = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i32>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<i64>>(9)?,
                    row.get::<_, Option<i64>>(10)?,
                    row.get::<_, i64>(11)?,
                    row.get::<_, Option<String>>(12)?,
                    row.get::<_, Option<i64>>(13)?,
                    row.get::<_, Option<i64>>(14)?,
                    row.get::<_, Option<i64>>(15)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting nodes for the annotation pass")?;
        raws.into_iter()
            .map(
                |(id, kind, name, exported, derived, fingerprint, test_evidence, file_id, file_path, dead, dup, test, layer, clone_group, has_body, body_tokens)| {
                    let kind = NodeKind::try_from(kind).map_err(|e| {
                        anyhow!(
                            "corrupt node kind {kind} for node {id}: {e}; rebuild advised (NFR-RA-08)"
                        )
                    })?;
                    Ok(AnnotationNodeRow {
                        id: NodeId(id),
                        kind,
                        name,
                        exported: exported != 0,
                        derived: derived != 0,
                        fingerprint,
                        test_evidence: test_evidence != 0,
                        file_id,
                        file_path,
                        is_dead: dead.map(|v| v != 0),
                        is_duplicate: dup.map(|v| v != 0),
                        is_test: test != 0,
                        layer_membership: layer,
                        clone_group: clone_group.map(NodeId),
                        has_body: has_body.map(|v| v != 0),
                        body_tokens,
                    })
                },
            )
            .collect()
    }

    fn shingle_index(&self) -> Result<Vec<(NodeId, u64)>> {
        // ORDER BY (node_id, hash) is the deterministic id-ordered iteration the
        // near-clone clustering pass requires (FR-AN-06, NFR-RA-06): a function's
        // rows are contiguous and ascending. `hash as u64` is the inverse of the
        // lossless u64→i64 store cast (the shingles table holds the u64 as the
        // equivalent signed INTEGER).
        let mut stmt = self
            .conn
            .prepare_cached("SELECT node_id, hash FROM shingles ORDER BY node_id, hash")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((NodeId(row.get::<_, i64>(0)?), row.get::<_, i64>(1)? as u64))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting the shingle index for near-clone clustering")?;
        Ok(rows)
    }

    fn has_framework_footprint(
        &self,
        detectors: &[DetectorPrefix],
        receivers: &ReceiverParameters,
    ) -> Result<bool> {
        // 1) A promoted route/component node already in the graph? Its presence
        //    alone forces the full reconcile (a demotion may be due).
        let route = crate::model::NodeKind::Route.as_i32();
        let component = crate::model::NodeKind::Component.as_i32();
        let has_promoted: bool = self
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM nodes WHERE kind = ?1 OR kind = ?2)",
                rusqlite::params![route, component],
                |row| row.get::<_, i64>(0),
            )
            .context("checking for promoted framework nodes")?
            != 0;
        if has_promoted {
            return Ok(true);
        }

        // 2) A parameter binding a language names as a framework receiver (an
        //    Express router module's `app`, S-541), in a file of a declaring
        //    language? The pass's other candidacy rule, read from the same
        //    recorded bindings it reads, as an EXISTS that stops at the first hit.
        if !receivers.names.is_empty() && !receivers.extensions.is_empty() {
            let names = vec!["?"; receivers.names.len()].join(", ");
            let paths = vec!["f.path GLOB ?"; receivers.extensions.len()].join(" OR ");
            let sql = format!(
                "SELECT EXISTS(SELECT 1 FROM local_bindings b JOIN files f ON f.id = b.file_id \
                 WHERE b.kind = ? AND b.name IN ({names}) AND ({paths}))"
            );
            let parameter = i64::from(LocalBindingKind::Parameter.as_i32());
            let params = std::iter::once(rusqlite::types::Value::Integer(parameter))
                .chain(receivers.names.iter().map(|n| rusqlite::types::Value::Text(n.clone())))
                .chain(
                    receivers
                        .extensions
                        .iter()
                        .map(|e| rusqlite::types::Value::Text(format!("*.{e}"))),
                );
            let has_receiver: bool = self
                .conn
                .query_row(&sql, rusqlite::params_from_iter(params), |row| {
                    row.get::<_, i64>(0)
                })
                .context("checking for framework receiver-parameter bindings")?
                != 0;
            if has_receiver {
                return Ok(true);
            }
        }
        if detectors.is_empty() {
            return Ok(false);
        }

        // 3) Any **unresolved** ledger ref naming a detector? `target = prefix` or
        //    it extends the prefix by whole `::` segments — the `matches_detector`
        //    rule in SQL. Canonical detector strings carry no GLOB metacharacters
        //    (`* ? [`), so `prefix::*` matches exactly the "extends by a segment"
        //    case. The `resolved = 0` scope is both an optimisation and a faithful
        //    mirror of the framework pass's own candidacy fingerprint: a detector
        //    names an *external* package the graph never indexes, so a detector ref
        //    can never bind and always survives as `resolved = 0` (see the
        //    framework module's "Ledger-gated candidacy" — the surviving ledger
        //    *is* the framework fingerprint). The cheap integer `resolved = 0`
        //    filter restricts the per-row GLOB work to the small unresolved tail
        //    instead of the whole ledger. EXISTS returns on the first hit; a
        //    framework-free graph answers `false` after one scan. An import-only
        //    detector (S-541) is matched by an `Imports` row alone, as the pass
        //    matches it.
        let imports = crate::model::EdgeKind::Imports.as_i32();
        let mut clauses: Vec<&str> = Vec::with_capacity(detectors.len());
        let mut params: Vec<rusqlite::types::Value> = Vec::with_capacity(detectors.len() * 3);
        for DetectorPrefix { prefix, import_only } in detectors {
            if *import_only {
                clauses.push("(kind = ? AND (target = ? OR target GLOB ?))");
                params.push(rusqlite::types::Value::Integer(i64::from(imports)));
            } else {
                clauses.push("(target = ? OR target GLOB ?)");
            }
            params.push(rusqlite::types::Value::Text(prefix.clone()));
            params.push(rusqlite::types::Value::Text(format!("{prefix}::*")));
        }
        let sql = format!(
            "SELECT EXISTS(SELECT 1 FROM unresolved_refs WHERE resolved = 0 AND ({}))",
            clauses.join(" OR ")
        );
        let has_ref: bool = self
            .conn
            .query_row(&sql, rusqlite::params_from_iter(params), |row| {
                row.get::<_, i64>(0)
            })
            .context("checking for framework-detector references")?
            != 0;
        Ok(has_ref)
    }

    fn has_broker_footprint(&self) -> Result<bool> {
        // 1) A promoted broker node already in the graph? Its presence alone forces
        //    the full reconcile (a demotion may be due — the last publish may have
        //    just been deleted).
        let (topic, producer, consumer) = (
            crate::model::NodeKind::Topic.as_i32(),
            crate::model::NodeKind::Producer.as_i32(),
            crate::model::NodeKind::Consumer.as_i32(),
        );
        let has_promoted: bool = self
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM nodes WHERE kind IN (?1, ?2, ?3))",
                rusqlite::params![topic, producer, consumer],
                |row| row.get::<_, i64>(0),
            )
            .context("checking for promoted broker nodes")?
            != 0;
        if has_promoted {
            return Ok(true);
        }

        // 2) Any broker-arm reference in the ledger? Both arms ride the free
        //    `payload` relation token the S-254 capture wrote (MIGRATION_14). The
        //    `resolved` column is deliberately NOT filtered on: a broker ref binds
        //    to no local artifact kind (`ArtifactRelation::target_kind` is `None`
        //    and a topic is not a config kind), so it is unresolved by construction
        //    today — but the promotion pass owns every broker row regardless of how
        //    the resolution pass later classifies it, and a `resolved = 0` filter
        //    here would silently stop promoting the day that changed.
        let has_ref: bool = self
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM unresolved_refs WHERE payload IN (?1, ?2))",
                rusqlite::params![
                    crate::model::ArtifactRelation::BrokerPublish.as_str(),
                    crate::model::ArtifactRelation::BrokerSubscribe.as_str(),
                ],
                |row| row.get::<_, i64>(0),
            )
            .context("checking for broker-arm ledger references")?
            != 0;
        Ok(has_ref)
    }

    fn broker_subgraph(&self) -> Result<(Vec<NodeRow>, Vec<EdgeRow>)> {
        let (topic, producer, consumer) = (
            crate::model::NodeKind::Topic.as_i32(),
            crate::model::NodeKind::Producer.as_i32(),
            crate::model::NodeKind::Consumer.as_i32(),
        );
        let mut stmt = self.conn.prepare_cached(
            "SELECT n.id, s.symbol, n.kind, n.name, f.path, n.start_line, n.end_line \
             FROM nodes n \
             JOIN symbols s ON s.id = n.symbol_id \
             LEFT JOIN files f ON f.id = n.file_id \
             WHERE n.kind IN (?1, ?2, ?3) \
             ORDER BY n.id",
        )?;
        let raws = stmt
            .query_map(rusqlite::params![topic, producer, consumer], map_raw_row)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting the promoted broker nodes")?;
        let nodes: Vec<NodeRow> = raws
            .into_iter()
            .map(raw_to_node)
            .collect::<Result<Vec<_>>>()?;

        let (publishes, subscribes) = (
            crate::model::EdgeKind::Publishes.as_i32(),
            crate::model::EdgeKind::Subscribes.as_i32(),
        );
        let mut stmt = self.conn.prepare_cached(
            "SELECT source, target, kind FROM edges \
             WHERE kind IN (?1, ?2) \
             ORDER BY source, target, kind",
        )?;
        let edges = stmt
            .query_map(rusqlite::params![publishes, subscribes], |row| {
                Ok((
                    NodeId(row.get::<_, i64>(0)?),
                    NodeId(row.get::<_, i64>(1)?),
                    row.get::<_, i32>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting the promoted broker edges")?
            .into_iter()
            .map(|(source, target, kind)| {
                Ok(EdgeRow {
                    source,
                    target,
                    kind: EdgeKind::try_from(kind).with_context(|| {
                        format!("corrupt edge kind {kind} stored; rebuild advised (NFR-RA-08)")
                    })?,
                    dispatch: false, // a broker edge is never a calls edge
                })
            })
            .collect::<Result<Vec<_>>>()?;

        Ok((nodes, edges))
    }

    fn test_node_ids(&self) -> Result<Vec<NodeId>> {
        // ORDER BY id keeps the consumer (require_tested BFS) deterministic
        // (NFR-RA-06). Reading the persisted verdict — not re-deriving it —
        // is what guarantees reachability ≡ the annotation (CR-001 CRA-01).
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id FROM nodes WHERE is_test = 1 ORDER BY id")?;
        let ids = stmt
            .query_map([], |row| Ok(NodeId(row.get(0)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting test node ids")?;
        Ok(ids)
    }

    fn function_metrics(&self) -> Result<Vec<FunctionMetricRow>> {
        // ORDER BY id keeps the metric reduction canonical (ADR-08, NFR-RA-06).
        // The CR-005 dimensions add line_count (Conciseness), max_nesting_depth
        // (Nesting/Conciseness), and clone_group (Uniqueness) to the slice;
        // CR-163 adds has_body (the bodied Cohesion/Focus method sets).
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, cyclomatic_complexity, is_dead, is_duplicate, \
                    line_count, max_nesting_depth, clone_group, has_body \
             FROM nodes WHERE kind IN (?1, ?2) AND derived = 0 \
             ORDER BY id",
        )?;
        let mut rows = stmt
            .query_map(
                [NodeKind::Function.as_i32(), NodeKind::Method.as_i32()],
                |row| {
                    Ok(FunctionMetricRow {
                        id: NodeId(row.get(0)?),
                        cyclomatic_complexity: row.get(1)?,
                        is_dead: row.get::<_, Option<i64>>(2)?.map(|v| v != 0),
                        is_duplicate: row.get::<_, Option<i64>>(3)?.map(|v| v != 0),
                        line_count: row.get(4)?,
                        max_nesting_depth: row.get(5)?,
                        clone_group: row.get::<_, Option<i64>>(6)?.map(NodeId),
                        has_body: row.get::<_, Option<i64>>(7)?.map(|v| v != 0),
                        unbound_accesses: Vec::new(),
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting function rows for the metrics engine")?;
        // CR-252: each callable's unbound own-receiver accesses, joined back to
        // the node through its symbol; ordered so each list is sorted.
        let mut stmt = self.conn.prepare_cached(
            "SELECT DISTINCT n.id, r.target \
             FROM unresolved_refs r \
             JOIN symbols s ON s.symbol = r.source_symbol \
             JOIN nodes n ON n.symbol_id = s.id \
             WHERE r.kind = ?1 AND r.resolved = 0 \
               AND n.kind IN (?2, ?3) AND n.derived = 0 \
             ORDER BY n.id, r.target",
        )?;
        let unbound = stmt
            .query_map(
                [
                    EdgeKind::Accesses.as_i32(),
                    NodeKind::Function.as_i32(),
                    NodeKind::Method.as_i32(),
                ],
                |row| Ok((NodeId(row.get(0)?), row.get::<_, String>(1)?)),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting unbound member accesses for the metrics engine")?;
        // Both reads are id-ordered, so a binary search finds each row.
        for (id, name) in unbound {
            if let Ok(i) = rows.binary_search_by_key(&id, |r| r.id) {
                rows[i].unbound_accesses.push(name);
            }
        }
        Ok(rows)
    }

    fn duplicate_groups(&self) -> Result<Vec<Vec<NodeId>>> {
        // Grouped by fingerprint — the very key the annotation pass grouped the
        // eligible callables by (annotate::duplicate_set) — so a group here is
        // exactly the set of members that made each other duplicates.
        let mut stmt = self.conn.prepare_cached(
            "SELECT fingerprint, id FROM nodes \
             WHERE kind IN (?1, ?2) AND derived = 0 AND is_duplicate = 1 \
                   AND fingerprint IS NOT NULL \
             ORDER BY fingerprint, id",
        )?;
        let rows = stmt
            .query_map(
                [NodeKind::Function.as_i32(), NodeKind::Method.as_i32()],
                |row| Ok((row.get::<_, String>(0)?, NodeId(row.get(1)?))),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting the exact-duplicate groups")?;
        let mut groups: Vec<Vec<NodeId>> = Vec::new();
        let mut last: Option<String> = None;
        for (fingerprint, id) in rows {
            if last.as_deref() == Some(fingerprint.as_str()) {
                groups.last_mut().expect("a group was opened").push(id);
            } else {
                groups.push(vec![id]);
                last = Some(fingerprint);
            }
        }
        groups.sort_by_key(|g| g[0]);
        Ok(groups)
    }

    fn metric_snapshots(&self) -> Result<Vec<MetricSnapshotRow>> {
        self.metric_snapshots_up_to(i64::MAX)
    }

    fn metric_snapshots_through(&self, id: i64) -> Result<Vec<MetricSnapshotRow>> {
        self.metric_snapshots_up_to(id)
    }

    fn baseline_record(&self, scope: &str) -> Result<Option<BaselineRecord>> {
        self.conn
            .prepare_cached("SELECT reason, attribution FROM baseline WHERE scope = ?1")?
            .query_row([scope], |row| {
                Ok(BaselineRecord {
                    reason: row.get(0)?,
                    attribution: row.get(1)?,
                })
            })
            .optional()
            .context("querying the gate baseline's record")
    }

    fn baseline_snapshot(&self, scope: &str) -> Result<Option<MetricSnapshotRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT m.id, m.created_at, m.commit_sha, m.node_count, m.edge_count, \
                    m.function_count, m.test_function_count, m.metric_version, m.empty, \
                    m.modularity_raw, m.modularity_normalized, \
                    m.acyclicity_raw, m.acyclicity_normalized, \
                    m.depth_raw, m.depth_normalized, \
                    m.equality_raw, m.equality_normalized, \
                    m.redundancy_raw, m.redundancy_normalized, \
                    m.thresholds_hash, m.aggregate_signal, m.modularity_applicable, \
                    m.acyclicity_applicable, m.depth_applicable, m.cross_module_edges \
             FROM baseline b \
             JOIN metric_snapshots m ON m.id = b.snapshot_id \
             WHERE b.scope = ?1",
        )?;
        stmt.query_row([scope], |row| {
            Ok(MetricSnapshotRow {
                id: row.get(0)?,
                created_at: row.get(1)?,
                commit_sha: row.get(2)?,
                node_count: row.get(3)?,
                edge_count: row.get(4)?,
                function_count: row.get(5)?,
                test_function_count: row.get(6)?,
                metric_version: row.get(7)?,
                empty: row.get::<_, i64>(8)? != 0,
                modularity_raw: row.get(9)?,
                modularity_normalized: row.get(10)?,
                acyclicity_raw: row.get(11)?,
                acyclicity_normalized: row.get(12)?,
                depth_raw: row.get(13)?,
                depth_normalized: row.get(14)?,
                equality_raw: row.get(15)?,
                equality_normalized: row.get(16)?,
                redundancy_raw: row.get(17)?,
                redundancy_normalized: row.get(18)?,
                thresholds_hash: row.get(19)?,
                aggregate_signal: row.get(20)?,
                modularity_applicable: row.get::<_, Option<i64>>(21)?.map(|n| n != 0),
                acyclicity_applicable: row.get::<_, Option<i64>>(22)?.map(|n| n != 0),
                depth_applicable: row.get::<_, Option<i64>>(23)?.map(|n| n != 0),
                cross_module_edges: row.get(24)?,
            })
        })
        .optional()
        .context("querying the gate baseline")
    }

    fn latest_metric_snapshot(&self) -> Result<Option<LatestMetricSnapshot>> {
        read_metric_snapshot(&self.conn, "ORDER BY id DESC LIMIT 1", [])
    }

    fn metric_snapshot(&self, snapshot_id: i64) -> Result<Option<LatestMetricSnapshot>> {
        read_metric_snapshot(&self.conn, "WHERE id = ?1", [snapshot_id])
    }

    fn latest_uncapped_offenders(&self) -> Result<Option<WorstOffenders>> {
        read_uncapped_offenders(&self.conn, "ORDER BY id DESC LIMIT 1", [])
    }

    fn uncapped_offenders_of(&self, snapshot_id: i64) -> Result<Option<WorstOffenders>> {
        read_uncapped_offenders(&self.conn, "WHERE id = ?1", [snapshot_id])
    }

    fn rules_cache(&self) -> Result<Option<RulesCacheRow>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT rules_hash, parsed_json FROM rules_cache WHERE id = 1")?;
        stmt.query_row([], |row| {
            Ok(RulesCacheRow {
                rules_hash: row.get(0)?,
                parsed_json: row.get(1)?,
            })
        })
        .optional()
        .context("querying the rules cache")
    }

    fn function_constraint_rows(&self) -> Result<Vec<FunctionConstraintRow>> {
        // ORDER BY id keeps violation order deterministic (NFR-RA-06).
        let mut stmt = self.conn.prepare_cached(
            "SELECT n.id, n.name, f.path, n.start_line, \
                    n.cyclomatic_complexity, n.line_count \
             FROM nodes n \
             LEFT JOIN files f ON f.id = n.file_id \
             WHERE n.kind IN (?1, ?2) AND n.derived = 0 \
             ORDER BY n.id",
        )?;
        let rows = stmt
            .query_map(
                [NodeKind::Function.as_i32(), NodeKind::Method.as_i32()],
                |row| {
                    Ok(FunctionConstraintRow {
                        id: NodeId(row.get(0)?),
                        name: row.get(1)?,
                        file_path: row.get(2)?,
                        start_line: row.get(3)?,
                        cyclomatic_complexity: row.get(4)?,
                        line_count: row.get(5)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting function rows for the rules evaluator")?;
        Ok(rows)
    }

    fn violations(&self) -> Result<Vec<ViolationRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, snapshot_id, rule_type, rule_key, node_id, file, message, severity, \
                    created_at \
             FROM violations ORDER BY id",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ViolationRow {
                    id: row.get(0)?,
                    snapshot_id: row.get(1)?,
                    rule_type: row.get(2)?,
                    rule_key: row.get(3)?,
                    node_id: row.get(4)?,
                    file: row.get(5)?,
                    message: row.get(6)?,
                    severity: row.get(7)?,
                    created_at: row.get(8)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting persisted violations")?;
        Ok(rows)
    }

    fn check_run(&self) -> Result<Option<CheckRunRow>> {
        // The three evaluated-set columns are read as Option: NULL is a marker
        // written before migration 21, and it stays distinguishable from a
        // recorded zero all the way to the surface (S-437, CR-140 CRA-05).
        let mut stmt = self.conn.prepare_cached(
            "SELECT ran_at, commit_sha, violation_count, checked_rules, rules_present, operation \
             FROM check_run WHERE id = 1",
        )?;
        stmt.query_row([], |row| {
            Ok(CheckRunRow {
                ran_at: row.get(0)?,
                commit_sha: row.get(1)?,
                violation_count: row.get(2)?,
                checked_rules: row.get(3)?,
                rules_present: row.get(4)?,
                operation: row.get(5)?,
            })
        })
        .optional()
        .context("querying the check-run marker")
    }

    fn store_health(&self) -> Result<StoreHealth> {
        Ok(StoreHealth {
            schema_version: self.schema_version()?,
        })
    }

    fn structural_check(&self) -> Result<StructuralReport> {
        structural_report(&self.conn)
    }
}

/// Which side of a `calls` edge to pivot on for an adjacency query.
///
/// A closed enum (not a `&str`) so the SQL is selected from two fixed `const`
/// statements — the type system makes string interpolation of an endpoint
/// impossible by construction, keeping the injection boundary structural rather
/// than conventional ([NFR-SE-02]).
#[derive(Debug, Clone, Copy)]
enum Direction {
    /// Inbound `calls` edges: match on `target`, project the `source` node.
    Callers,
    /// Outbound `calls` edges: match on `source`, project the `target` node.
    Callees,
}

/// `callers` of a node: pivot on `e.target`, return the `e.source` node.
const SQL_CALLERS: &str =
    "SELECT n.id, s.symbol, n.kind, n.name, f.path, n.start_line, n.end_line \
     FROM edges e \
     JOIN nodes n ON n.id = e.source \
     JOIN symbols s ON s.id = n.symbol_id \
     LEFT JOIN files f ON f.id = n.file_id \
     WHERE e.target = ?1 AND e.kind = ?2 \
     ORDER BY n.id";

/// `callees` of a node: pivot on `e.source`, return the `e.target` node.
const SQL_CALLEES: &str =
    "SELECT n.id, s.symbol, n.kind, n.name, f.path, n.start_line, n.end_line \
     FROM edges e \
     JOIN nodes n ON n.id = e.target \
     JOIN symbols s ON s.id = n.symbol_id \
     LEFT JOIN files f ON f.id = n.file_id \
     WHERE e.source = ?1 AND e.kind = ?2 \
     ORDER BY n.id";

/// `neighbours_in`: every inbound edge of any kind, projecting the source node
/// plus the edge kind (first column).
const SQL_NEIGHBOURS_IN: &str =
    "SELECT e.kind, n.id, s.symbol, n.kind, n.name, f.path, n.start_line, n.end_line \
     FROM edges e \
     JOIN nodes n ON n.id = e.source \
     JOIN symbols s ON s.id = n.symbol_id \
     LEFT JOIN files f ON f.id = n.file_id \
     WHERE e.target = ?1 \
     ORDER BY e.kind, n.id";

/// `neighbours_out`: every outbound edge of any kind, projecting the target
/// node plus the edge kind (first column).
const SQL_NEIGHBOURS_OUT: &str =
    "SELECT e.kind, n.id, s.symbol, n.kind, n.name, f.path, n.start_line, n.end_line \
     FROM edges e \
     JOIN nodes n ON n.id = e.target \
     JOIN symbols s ON s.id = n.symbol_id \
     LEFT JOIN files f ON f.id = n.file_id \
     WHERE e.source = ?1 \
     ORDER BY e.kind, n.id";

impl SqliteGraphStore {
    /// Shared body of [`GraphStore::metric_snapshots`] /
    /// [`GraphStore::metric_snapshots_through`]: every snapshot whose id is at
    /// most `id`, ordered by `id`. `id` is a bound parameter.
    fn metric_snapshots_up_to(&self, id: i64) -> Result<Vec<MetricSnapshotRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, created_at, commit_sha, node_count, edge_count, function_count, \
                    test_function_count, metric_version, empty, \
                    modularity_raw, modularity_normalized, \
                    acyclicity_raw, acyclicity_normalized, \
                    depth_raw, depth_normalized, \
                    equality_raw, equality_normalized, \
                    redundancy_raw, redundancy_normalized, \
                    thresholds_hash, aggregate_signal, modularity_applicable, \
                    acyclicity_applicable, depth_applicable, cross_module_edges \
             FROM metric_snapshots WHERE id <= ?1 ORDER BY id",
        )?;
        let rows = stmt
            .query_map([id], |row| {
                Ok(MetricSnapshotRow {
                    id: row.get(0)?,
                    created_at: row.get(1)?,
                    commit_sha: row.get(2)?,
                    node_count: row.get(3)?,
                    edge_count: row.get(4)?,
                    function_count: row.get(5)?,
                    test_function_count: row.get(6)?,
                    metric_version: row.get(7)?,
                    empty: row.get::<_, i64>(8)? != 0,
                    modularity_raw: row.get(9)?,
                    modularity_normalized: row.get(10)?,
                    acyclicity_raw: row.get(11)?,
                    acyclicity_normalized: row.get(12)?,
                    depth_raw: row.get(13)?,
                    depth_normalized: row.get(14)?,
                    equality_raw: row.get(15)?,
                    equality_normalized: row.get(16)?,
                    redundancy_raw: row.get(17)?,
                    redundancy_normalized: row.get(18)?,
                    thresholds_hash: row.get(19)?,
                    aggregate_signal: row.get(20)?,
                    modularity_applicable: row.get::<_, Option<i64>>(21)?.map(|n| n != 0),
                    acyclicity_applicable: row.get::<_, Option<i64>>(22)?.map(|n| n != 0),
                    depth_applicable: row.get::<_, Option<i64>>(23)?.map(|n| n != 0),
                    cross_module_edges: row.get(24)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting metric snapshots")?;
        Ok(rows)
    }

    /// Shared body of [`GraphStore::callers`] / [`GraphStore::callees`].
    ///
    /// Selects one of two fixed `const` statements by [`Direction`]; `id` and
    /// the `calls` discriminant are bound parameters. No SQL is built at
    /// runtime — there is no interpolation surface.
    fn adjacent(&self, id: NodeId, direction: Direction) -> Result<Vec<NodeRow>> {
        let sql = match direction {
            Direction::Callers => SQL_CALLERS,
            Direction::Callees => SQL_CALLEES,
        };
        let mut stmt = self.conn.prepare_cached(sql)?;
        let raws = stmt
            .query_map(
                rusqlite::params![id.get(), EdgeKind::Calls.as_i32()],
                map_raw_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting adjacency results")?;
        raws.into_iter().map(raw_to_node).collect()
    }

    /// Shared body of [`GraphStore::neighbours_in`] /
    /// [`GraphStore::neighbours_out`]: one of two fixed `const` statements,
    /// `id` bound — same no-interpolation posture as [`adjacent`](Self::adjacent).
    fn neighbours(&self, id: NodeId, sql: &'static str) -> Result<Vec<(EdgeKind, NodeRow)>> {
        let mut stmt = self.conn.prepare_cached(sql)?;
        let raws = stmt
            .query_map([id.get()], |row| {
                let edge_kind: i32 = row.get(0)?;
                Ok((
                    edge_kind,
                    (
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ),
                ))
            })?
            .collect::<rusqlite::Result<Vec<(i32, RawNodeRow)>>>()
            .context("collecting neighbour edges")?;
        raws.into_iter()
            .map(|(edge_kind, raw)| {
                let kind = EdgeKind::try_from(edge_kind).map_err(|e| {
                    anyhow!("corrupt edge kind {edge_kind} on a neighbour edge: {e}; rebuild advised (NFR-RA-08)")
                })?;
                Ok((kind, raw_to_node(raw)?))
            })
            .collect()
    }
}

/// Map a persisted vocabulary name back to its `'static` spelling, or `None`
/// for a name outside `vocabulary` — the one rule every snapshot read-model
/// reader applies to a column that carries no `CHECK` list: skip, never invent.
fn pinned(vocabulary: &[&'static str], name: &str) -> Option<&'static str> {
    vocabulary.iter().copied().find(|v| *v == name)
}

/// The capped display offender rows ([FR-QM-15]).
const CAPPED_OFFENDERS: &str = "metric_snapshot_offenders";

/// The uncapped offender rows ([FR-QM-19], migration 37).
///
/// [FR-QM-19]: ../../../docs/specs/requirements/FR-QM-19.md
const UNCAPPED_OFFENDERS: &str = "metric_snapshot_offenders_uncapped";

/// One `metric_snapshots` row with the capped lists and ranking persisted with
/// it, picked by `selector` — the shared body of
/// [`GraphStore::latest_metric_snapshot`] and its by-id twin
/// [`GraphStore::metric_snapshot`], as [`read_uncapped_offenders`] is of theirs.
fn read_metric_snapshot(
    conn: &Connection,
    selector: &'static str,
    params: impl rusqlite::Params,
) -> Result<Option<LatestMetricSnapshot>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT node_count, edge_count, function_count, test_function_count, empty, \
                thresholds_hash, aggregate_signal, \
                modularity_raw, modularity_normalized, \
                acyclicity_raw, acyclicity_normalized, \
                depth_raw, depth_normalized, \
                equality_raw, equality_normalized, \
                redundancy_raw, redundancy_normalized, \
                nesting_raw, nesting_normalized, \
                conciseness_raw, conciseness_normalized, \
                cohesion_raw, cohesion_normalized, cohesion_applicable, \
                focus_raw, focus_normalized, focus_applicable, \
                uniqueness_raw, uniqueness_normalized, modularity_applicable, \
                id, offenders_recorded, insight_recorded, offender_values_recorded, \
                acyclicity_applicable, depth_applicable, cross_module_edges \
         FROM metric_snapshots {selector}"
    ))?;
    let latest = stmt.query_row(params, |row| {
        // `*_applicable` is stored as INTEGER 0/1/NULL → Option<bool>.
        let opt_bool = |idx: usize| -> rusqlite::Result<Option<bool>> {
            Ok(row.get::<_, Option<i64>>(idx)?.map(|n| n != 0))
        };
        let id: i64 = row.get(30)?;
        let recorded = row.get::<_, Option<i64>>(31)?.is_some();
        let insight_recorded = row.get::<_, Option<i64>>(32)?.is_some();
        // Only a recorded snapshot can vouch for its entries' values.
        let values_recorded = recorded && row.get::<_, Option<i64>>(33)?.is_some();
        let snapshot = LatestMetricSnapshot {
            id,
            node_count: row.get(0)?,
            edge_count: row.get(1)?,
            function_count: row.get(2)?,
            test_function_count: row.get(3)?,
            empty: row.get::<_, i64>(4)? != 0,
            thresholds_hash: row.get(5)?,
            aggregate_signal: row.get(6)?,
            modularity_raw: row.get(7)?,
            modularity_normalized: row.get(8)?,
            acyclicity_raw: row.get(9)?,
            acyclicity_normalized: row.get(10)?,
            acyclicity_applicable: opt_bool(34)?,
            depth_raw: row.get(11)?,
            depth_normalized: row.get(12)?,
            depth_applicable: opt_bool(35)?,
            cross_module_edges: row.get(36)?,
            equality_raw: row.get(13)?,
            equality_normalized: row.get(14)?,
            redundancy_raw: row.get(15)?,
            redundancy_normalized: row.get(16)?,
            nesting_raw: row.get(17)?,
            nesting_normalized: row.get(18)?,
            conciseness_raw: row.get(19)?,
            conciseness_normalized: row.get(20)?,
            cohesion_raw: row.get(21)?,
            cohesion_normalized: row.get(22)?,
            cohesion_applicable: opt_bool(23)?,
            focus_raw: row.get(24)?,
            focus_normalized: row.get(25)?,
            focus_applicable: opt_bool(26)?,
            uniqueness_raw: row.get(27)?,
            uniqueness_normalized: row.get(28)?,
            modularity_applicable: opt_bool(29)?,
            worst_offenders: WorstOffenders {
                recorded,
                values_recorded,
                ..WorstOffenders::default()
            },
            where_to_start: WhereToStart::default(),
        };
        Ok((id, insight_recorded, snapshot))
    });
    let Some((id, insight_recorded, mut snapshot)) = latest
        .optional()
        .context("querying a metric snapshot")?
    else {
        return Ok(None);
    };
    // The lists are read by the id the row above was read under, never by
    // "latest" again. A snapshot and its offender rows are written in one
    // transaction and never mutated, so this pairs the row with exactly its
    // own lists even when a `scan` commits a newer snapshot in between —
    // the single-read property the Health bundle relies on (CR-135 §3.2).
    if snapshot.worst_offenders.recorded {
        read_snapshot_offenders(conn, CAPPED_OFFENDERS, id, &mut snapshot.worst_offenders)?;
    }
    // The ranking likewise, by the same id (FR-QM-17).
    snapshot.where_to_start = read_where_to_start(conn, id, insight_recorded)?;
    Ok(Some(snapshot))
}

/// The **uncapped** offender lists ([FR-QM-19]) of the one `metric_snapshots`
/// row `selector` picks (`ORDER BY id DESC LIMIT 1`, or `WHERE id = ?1`), or
/// `None` when it picks none. The one body behind
/// [`GraphStore::latest_uncapped_offenders`] and
/// [`GraphStore::uncapped_offenders_of`]: the row's flags are read first and
/// the lists by the id they were read under, so the pair never mixes two
/// snapshots. Before migration 37 there is no uncapped list to read, and the
/// lists stay empty under `values_recorded: false`.
///
/// [FR-QM-19]: ../../../docs/specs/requirements/FR-QM-19.md
fn read_uncapped_offenders(
    conn: &Connection,
    selector: &'static str,
    params: impl rusqlite::Params,
) -> Result<Option<WorstOffenders>> {
    let picked = conn
        .prepare_cached(&format!(
            "SELECT id, offenders_recorded, offender_values_recorded \
             FROM metric_snapshots {selector}"
        ))?
        .query_row(params, |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?.is_some(),
                row.get::<_, Option<i64>>(2)?.is_some(),
            ))
        })
        .optional()
        .context("querying a metric snapshot's offender flags")?;
    let Some((id, recorded, values_flag)) = picked else {
        return Ok(None);
    };
    let values_recorded = recorded && values_flag;
    let mut offenders = WorstOffenders {
        recorded,
        values_recorded,
        ..WorstOffenders::default()
    };
    if values_recorded {
        read_snapshot_offenders(conn, UNCAPPED_OFFENDERS, id, &mut offenders)?;
    }
    Ok(Some(offenders))
}

/// Fill `into` with the offender rows persisted under `snapshot_id` in `table`
/// ([FR-QM-15]), each list in rank order — the order it was computed in. A row
/// whose dimension is outside
/// [`WorstOffenders::DIMENSIONS`] is skipped: migration 26 leaves the column
/// open for further lists, and none of them would belong to these nine. (None
/// has used it yet: [CR-164]'s ranking has its own migration-36 tables.)
///
/// `table` is [`CAPPED_OFFENDERS`] or [`UNCAPPED_OFFENDERS`] — the same row
/// shape since migration 37 ([FR-QM-19]). The `node_id`, `measure`, `value`
/// and `threshold` columns read `NULL` on a pre-migration row, so its entries
/// carry `None` for all four; the snapshot's `values_recorded` flag says that
/// means "not recorded". A `measure` outside [`Offender::MEASURES`] reads as
/// `None`, never invented.
///
/// [FR-QM-15]: ../../../docs/specs/requirements/FR-QM-15.md
/// [FR-QM-19]: ../../../docs/specs/requirements/FR-QM-19.md
/// [CR-164]: ../../../docs/requests/CR-164-insight-layer-ranks-what-to-fix-first.md
fn read_snapshot_offenders(
    conn: &Connection,
    table: &'static str,
    snapshot_id: i64,
    into: &mut WorstOffenders,
) -> Result<()> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT dimension, name, file, line, detail, node_id, measure, value, threshold, partial_parse \
         FROM {table} WHERE snapshot_id = ?1 ORDER BY dimension, rank"
    ))?;
    let rows = stmt.query_map([snapshot_id], |row| {
        let measure = row.get::<_, Option<String>>(6)?;
        Ok((
            row.get::<_, String>(0)?,
            Offender {
                name: row.get(1)?,
                file: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                line: row.get(3)?,
                detail: row.get(4)?,
                node_id: row.get(5)?,
                measure: measure.and_then(|m| pinned(&Offender::MEASURES, &m)),
                value: row.get(7)?,
                threshold: row.get(8)?,
                partial_parse: row.get::<_, Option<bool>>(9)?.unwrap_or(false),
            },
        ))
    })?;
    for row in rows {
        let (dimension, offender) = row.context("reading a snapshot offender row")?;
        if let Some(list) = into.list_mut(&dimension) {
            list.push(offender);
        }
    }
    Ok(())
}

/// The "where to start" set of the snapshot persisted under `snapshot_id`
/// ([FR-QM-17]): its rows ([`read_snapshot_insight`]) when the snapshot's
/// `insight_recorded` marker is set, otherwise the **not recorded** default
/// ([NFR-CC-04]) — a snapshot written before migration 36.
///
/// [FR-QM-17]: ../../../docs/specs/requirements/FR-QM-17.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
fn read_where_to_start(
    conn: &Connection,
    snapshot_id: i64,
    recorded: bool,
) -> Result<WhereToStart> {
    if !recorded {
        return Ok(WhereToStart::default());
    }
    let mut where_to_start = WhereToStart::recorded();
    read_snapshot_insight(conn, snapshot_id, &mut where_to_start)?;
    Ok(where_to_start)
}

/// Fill `into` with the "where to start" rows persisted under `snapshot_id`
/// ([FR-QM-17]): the fix units in rank order, then each outlier measure in
/// [`OutlierMeasure::MEASURES`] order with its labels in rank order — exactly
/// as computed. A row naming a kind, dimension or measure outside the pinned
/// vocabulary is skipped rather than invented, as [`read_snapshot_offenders`]
/// does: the columns carry no `CHECK` list (migration 36), and a later list
/// that reuses them does not belong to this read-model. A unit or label
/// written before migration 40 reads `node_id: None` — its storage id was
/// cleared, never reread as an identity.
///
/// [FR-QM-17]: ../../../docs/specs/requirements/FR-QM-17.md
fn read_snapshot_insight(conn: &Connection, snapshot_id: i64, into: &mut WhereToStart) -> Result<()> {
    // A comma-joined list column back to its names; `None` if any is unknown.
    fn pinned_list(vocabulary: &[&'static str], joined: &str) -> Option<Vec<&'static str>> {
        if joined.is_empty() {
            return Some(Vec::new());
        }
        joined.split(',').map(|n| pinned(vocabulary, n)).collect()
    }

    let mut stmt = conn.prepare_cached(
        "SELECT kind, node_id, name, file, line, dimensions, points, detail, outliers, partial_parse \
         FROM metric_snapshot_fix_units WHERE snapshot_id = ?1 ORDER BY rank",
    )?;
    let rows = stmt.query_map([snapshot_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, String>(7)?,
            row.get::<_, String>(8)?,
            row.get::<_, Option<bool>>(9)?.unwrap_or(false),
        ))
    })?;
    for row in rows {
        let (kind, node_id, name, file, line, dimensions, points, detail, outliers, partial_parse) =
            row.context("reading a snapshot fix-unit row")?;
        let (Some(kind), Some(dimensions), Some(outliers)) = (
            FixUnitKind::parse(&kind),
            pinned_list(&FixUnit::DIMENSIONS, &dimensions),
            pinned_list(&OutlierMeasure::MEASURES, &outliers),
        ) else {
            continue;
        };
        into.units.push(FixUnit {
            kind,
            node_id,
            name,
            file: file.unwrap_or_default(),
            line,
            dimensions,
            points,
            detail,
            outliers,
            partial_parse,
        });
    }

    let mut measures = conn.prepare_cached(
        "SELECT measure, units, fence, threshold, labelled FROM metric_snapshot_outlier_measures \
         WHERE snapshot_id = ?1",
    )?;
    let mut read: Vec<OutlierMeasure> = measures
        .query_map([snapshot_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<f64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("reading the snapshot outlier measures")?
        .into_iter()
        .filter_map(|(measure, units, fence, threshold, labelled)| {
            Some(OutlierMeasure {
                measure: pinned(&OutlierMeasure::MEASURES, &measure)?,
                units: units as u64,
                fence,
                threshold,
                labelled: labelled as u64,
                labels: Vec::new(),
            })
        })
        .collect();
    read.sort_by_key(|m| OutlierMeasure::MEASURES.iter().position(|v| *v == m.measure));

    let mut labels = conn.prepare_cached(
        "SELECT measure, node_id, name, file, line, value FROM metric_snapshot_outlier_labels \
         WHERE snapshot_id = ?1 ORDER BY measure, rank",
    )?;
    let rows = labels.query_map([snapshot_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            OutlierLabel {
                node_id: row.get(1)?,
                name: row.get(2)?,
                file: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                line: row.get(4)?,
                value: row.get(5)?,
            },
        ))
    })?;
    for row in rows {
        let (measure, label) = row.context("reading a snapshot outlier label row")?;
        if let Some(m) = read.iter_mut().find(|m| m.measure == measure) {
            m.labels.push(label);
        }
    }
    into.outliers = read;
    Ok(())
}

/// The first identifier-ish token of a free-text query — the FTS5 prefix-match
/// seed for [`GraphStore::suggest`]. `None` when the text holds no
/// alphanumeric/underscore run.
fn first_identifier_token(text: &str) -> Option<&str> {
    let start = text.find(|c: char| c.is_alphanumeric() || c == '_')?;
    let rest = &text[start..];
    let end = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

/// Migration 24's `resolution` token for a declared type: `refused` exactly
/// when it carries a reason.
fn resolution_token(t: &NewDeclaredType<'_>) -> &'static str {
    if t.reason.is_some() {
        "refused"
    } else {
        "resolved"
    }
}

/// A writer scoped to an open transaction (see [`SqliteGraphStore::write_batch`]).
///
/// Exposes the same insert primitives as the store, but every call participates
/// in the surrounding transaction, so the whole batch commits or rolls back as
/// a unit ([NFR-RA-07]).
pub struct BatchWriter<'a> {
    conn: &'a Connection,
}

impl BatchWriter<'_> {
    /// Read an integer `PRAGMA` value from the **live writer connection** this
    /// batch runs on ([FR-DB-02]).
    ///
    /// The writer's RW connection lives inside the single-writer actor thread
    /// ([ADR-02]) and is never handed out, so this is the only seam that can
    /// inspect it directly — the CR-057 bulk-load-pragma verification submits a
    /// no-op write batch and reads `cache_size` / `mmap_size` / `temp_store`
    /// through here. Intended for **value-read** pragmas only: `name` must be a
    /// trusted literal in query form (`PRAGMA name`, no `= value`) — it is
    /// interpolated into SQL (pragma names cannot be bound as parameters), and
    /// this method never appends a value, so it does not itself write. `pub(crate)`
    /// because it is a verification seam, not part of the public write API.
    ///
    /// # Errors
    /// Returns an error if the pragma name is unknown or the read fails.
    ///
    /// [FR-DB-02]: ../../../docs/specs/requirements/FR-DB-02.md
    /// [ADR-02]: ../../../docs/specs/architecture/decisions/ADR-02.md
    #[cfg(test)]
    pub(crate) fn pragma_i64(&self, name: &str) -> Result<i64> {
        self.conn
            .query_row(&format!("PRAGMA {name}"), [], |r| r.get(0))
            .with_context(|| format!("reading PRAGMA {name} from the writer connection"))
    }

    /// Insert a file row within the batch; see [`SqliteGraphStore::insert_file`].
    ///
    /// # Errors
    /// Returns an error on a constraint violation or I/O failure.
    pub fn insert_file(
        &self,
        path: &str,
        language: Option<&str>,
        content_hash: Option<&str>,
    ) -> Result<i64> {
        insert_file(self.conn, path, language, content_hash)
    }

    /// Insert the symbol if new within the batch; see
    /// [`SqliteGraphStore::upsert_symbol`].
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn upsert_symbol(&self, symbol: &LogosSymbol) -> Result<i64> {
        upsert_symbol(self.conn, symbol)
    }

    /// Insert or upsert a node within the batch; see
    /// [`SqliteGraphStore::insert_node`].
    ///
    /// # Errors
    /// Returns an error if a constraint fires or I/O fails.
    pub fn insert_node(&self, node: &NewNode<'_>) -> Result<NodeId> {
        insert_node(self.conn, node)
    }

    /// Insert a node whose symbol no node in the store holds yet, or `None`
    /// when one already does — the extracted-fact insert ([BR-74], S-703).
    ///
    /// Unlike [`insert_node`](Self::insert_node), it never folds into an
    /// existing row: an equal symbol is a duplicate the caller refuses, not
    /// the same node.
    ///
    /// # Errors
    /// Returns an error if a constraint other than the symbol's uniqueness
    /// fires, or I/O fails.
    ///
    /// [BR-74]: ../../../docs/specs/software-spec.md#35-extraction--symbol-ids
    pub fn insert_new_node(&self, node: &NewNode<'_>) -> Result<Option<NodeId>> {
        insert_new_node(self.conn, node)
    }

    /// Insert an edge within the batch; see [`SqliteGraphStore::insert_edge`].
    ///
    /// # Errors
    /// Returns an error if a constraint fires or I/O fails.
    pub fn insert_edge(&self, source: NodeId, target: NodeId, kind: EdgeKind) -> Result<()> {
        insert_edge(self.conn, source, target, kind)
    }

    /// Persist a function node's winnowed near-clone shingle set (CR-005,
    /// [FR-EX-09]) into the inverted `shingles` index. Idempotent over the
    /// `(node_id, hash)` primary key, so a re-emitted hash is a no-op; an empty
    /// set (a body below the token floor) writes nothing.
    ///
    /// The `u64` fingerprints are bit-cast to the equivalent signed `i64` for
    /// SQLite's INTEGER column — a total, lossless round-trip
    /// ([`i64::from_ne_bytes`]/[`u64::to_ne_bytes`] would also serve; the cast is
    /// the same reinterpretation) read back by the clustering pass with the
    /// inverse cast (S-043).
    ///
    /// # Errors
    /// Returns an error if a constraint fires or I/O fails.
    ///
    /// [FR-EX-09]: ../../../docs/specs/requirements/FR-EX-09.md
    pub fn insert_shingles(&self, node_id: NodeId, hashes: &[u64]) -> Result<()> {
        if hashes.is_empty() {
            return Ok(());
        }
        let mut stmt = self.conn.prepare_cached(
            "INSERT INTO shingles (node_id, hash) VALUES (?1, ?2) \
             ON CONFLICT(node_id, hash) DO NOTHING",
        )?;
        for &hash in hashes {
            stmt.execute(rusqlite::params![node_id.get(), hash as i64])
                .context("inserting shingle fingerprint")?;
        }
        Ok(())
    }

    // ── Incremental-sync primitives ([ADR-10]) ───────────────────────────────

    /// The id of the file row for `path`, or `None` if the file is not indexed.
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn file_id(&self, path: &str) -> Result<Option<i64>> {
        self.conn
            .query_row("SELECT id FROM files WHERE path = ?1", [path], |r| r.get(0))
            .optional()
            .context("looking up file id by path")
    }

    /// Write an existing file row in place, keeping its id stable so the
    /// row's identity survives a re-extract: its `language` and `content_hash`,
    /// and what the extraction read about the file itself — the namespace it
    /// declares (S-518, [FR-RS-13]; `None` for a file keyed by its path) and
    /// whether its parse held an ERROR or MISSING node (S-571, [FR-IX-14]).
    /// Written on every persist, so a file that stops declaring a namespace
    /// has its old one cleared, and a file whose damage is repaired reads
    /// clean again.
    ///
    /// [FR-RS-13]: ../../../docs/specs/requirements/FR-RS-13.md
    /// [FR-IX-14]: ../../../docs/specs/requirements/FR-IX-14.md
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn update_file(
        &self,
        file_id: i64,
        language: Option<&str>,
        content_hash: Option<&str>,
        namespace: Option<&str>,
        parse_damaged: bool,
    ) -> Result<()> {
        self.write_one(
            "UPDATE files SET language = ?2, content_hash = ?3, namespace = ?4, parse_damaged = ?5 WHERE id = ?1",
            rusqlite::params![file_id, language, content_hash, namespace, parse_damaged],
            "updating file row",
        )
    }

    /// Run one write statement, naming what it does in its error — the body
    /// the one-statement writers share.
    fn write_one(&self, sql: &str, params: impl rusqlite::Params, what: &'static str) -> Result<()> {
        self.conn.execute(sql, params).context(what)?;
        Ok(())
    }

    /// The namespace recorded for the file `file_id` (S-518) — `None` when it is
    /// keyed by its path. Read by persist before it records the new one.
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn file_namespace_of(&self, file_id: i64) -> Result<Option<String>> {
        self.conn
            .query_row("SELECT namespace FROM files WHERE id = ?1", [file_id], |row| {
                row.get::<_, Option<String>>(0)
            })
            .optional()
            .map(Option::flatten)
            .context("reading the file's declared namespace")
    }

    /// Upsert a durable `project_metadata` key/value pair (CR-004, [ADR-20], the
    /// migration-15 kv table).
    ///
    /// Used to record [`CONFIG_FINGERPRINT_KEY`] after a config-narrowing purge,
    /// so the purge runs at most once per config change ([FR-SY-07]). The
    /// `ON CONFLICT … DO UPDATE` makes a re-record idempotent — the key is the
    /// primary key, so each project carries one row per fact.
    ///
    /// [ADR-20]: ../../../docs/specs/architecture/decisions/ADR-20.md
    /// [FR-SY-07]: ../../../docs/specs/requirements/FR-SY-07.md
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn set_project_metadata(&self, key: &str, value: &str) -> Result<()> {
        self.write_one(
            "INSERT INTO project_metadata (key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, value],
            "recording project_metadata value",
        )
    }

    /// Remove a durable `project_metadata` key, if present ([CR-130]).
    ///
    /// The deleting twin of [`set_project_metadata`](Self::set_project_metadata),
    /// for a fact that can stop being true of a graph. Its one caller is the
    /// index pipeline's [`LAST_FULL_INDEX_AT_KEY`] stamp: a full index that
    /// leaves the store empty has no graph to date, and a timestamp left behind
    /// would date a graph that no longer exists — the readout-honesty failure
    /// [CR-130] removes ([NFR-CC-04]). Idempotent: deleting an absent key is a
    /// no-op, not an error, which is what lets that caller clear unconditionally
    /// without first probing for the row.
    ///
    /// [CR-130]: ../../../docs/requests/CR-130-a-readout-names-a-remediation-that-cannot-apply.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn clear_project_metadata(&self, key: &str) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM project_metadata WHERE key = ?1",
                rusqlite::params![key],
            )
            .context("clearing project_metadata value")?;
        Ok(())
    }

    /// Move every symbol this store holds to `coordinate` ([FR-EX-35], S-704):
    /// remove the `symbols` rows no node references, then rewrite the package
    /// slot of every symbol-bearing value — `symbols.symbol`,
    /// `unresolved_refs.source_symbol`, a capture-before-delete row's `target`
    /// and `declared_types.symbol` — and clear every `files.content_hash`, so
    /// the next reconcile re-extracts each file under the coordinate.
    ///
    /// Every value whose coordinate differs is rewritten, not only the
    /// coordinate-less ones: a file a full index could not load keeps an older
    /// coordinate while the store moves on (S-703). Each value goes through
    /// [`LogosSymbol::with_coordinate`], so a local symbol and the descriptor
    /// tail come through byte for byte. Node and edge rows, snapshot history
    /// and the gate baseline are not touched: a node references its symbol by
    /// id.
    ///
    /// It writes no metadata; the caller records the coordinate in the same
    /// batch, so the record and the symbols never disagree.
    ///
    /// # Errors
    /// A value that is not a symbol (a corrupt store), two nodes whose symbols
    /// would become one, or an I/O failure. The batch then rolls back whole.
    ///
    /// [FR-EX-35]: ../../../docs/specs/requirements/FR-EX-35.md
    pub fn migrate_symbol_coordinate(&self, coordinate: &str) -> Result<CoordinateMigration> {
        let orphan_symbols_removed = self
            .conn
            .execute(
                "DELETE FROM symbols \
                 WHERE NOT EXISTS (SELECT 1 FROM nodes n WHERE n.symbol_id = symbols.id)",
                [],
            )
            .context("removing orphan symbol rows")? as u64;
        let symbols_rewritten =
            rewrite_symbol_column(self.conn, "symbols", "symbol", "", coordinate)?;
        let ref_symbols_rewritten =
            rewrite_symbol_column(self.conn, "unresolved_refs", "source_symbol", "", coordinate)?
                + rewrite_symbol_column(
                    self.conn,
                    "unresolved_refs",
                    "target",
                    &format!("AND form = {}", RefForm::Symbol.as_i32()),
                    coordinate,
                )?;
        let declared_type_symbols_rewritten =
            rewrite_symbol_column(self.conn, "declared_types", "symbol", "", coordinate)?;
        let files_invalidated = self
            .conn
            .execute(
                "UPDATE files SET content_hash = NULL WHERE content_hash IS NOT NULL",
                [],
            )
            .context("clearing file content hashes")? as u64;
        Ok(CoordinateMigration {
            orphan_symbols_removed,
            symbols_rewritten,
            ref_symbols_rewritten,
            declared_type_symbols_rewritten,
            files_invalidated,
        })
    }

    /// Run `f` under a **savepoint** nested in the open batch, so its writes
    /// commit or roll back as one unit **without** ending the batch (S-513,
    /// [FR-EH-05]).
    ///
    /// This is what lets one file of a chunked write batch fail alone: the
    /// outer `Result` is a fault of the savepoint machinery itself — the batch
    /// is then unusable and the caller must abort it — while the inner one is
    /// `f`'s own outcome, its writes already undone when it is an `Err`. The
    /// surrounding transaction stays open and healthy either way, so the files
    /// before and after this one still commit with it ([NFR-RA-07]).
    ///
    /// # Errors
    /// The outer `Err` when the savepoint cannot be opened, released or rolled
    /// back.
    ///
    /// [FR-EH-05]: ../../../docs/specs/requirements/FR-EH-05.md
    /// [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
    pub fn isolated<T>(
        &self,
        f: impl FnOnce(&BatchWriter<'_>) -> Result<T>,
    ) -> Result<Result<T>> {
        self.conn
            .execute_batch("SAVEPOINT logos_isolated")
            .context("opening an isolation savepoint")?;
        match f(self) {
            Ok(value) => {
                self.conn
                    .execute_batch("RELEASE logos_isolated")
                    .context("releasing an isolation savepoint")?;
                Ok(Ok(value))
            }
            Err(err) => {
                self.conn
                    .execute_batch("ROLLBACK TO logos_isolated; RELEASE logos_isolated")
                    .context("rolling back an isolation savepoint")?;
                Ok(Err(err))
            }
        }
    }

    /// Record that `path`'s latest persistence attempt failed (S-513,
    /// [FR-EH-05]), replacing any earlier record of the same file — see
    /// [`PersistFailureRow`] for what `stale` means.
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    ///
    /// [FR-EH-05]: ../../../docs/specs/requirements/FR-EH-05.md
    pub fn record_persist_failure(&self, path: &str, reason: &str, stale: bool) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO persist_failures (path, reason, stale) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(path) DO UPDATE SET reason = excluded.reason, stale = excluded.stale",
                rusqlite::params![path, reason, i64::from(stale)],
            )
            .context("recording a persist failure")?;
        Ok(())
    }

    /// Clear `path`'s persist-failure record, if any — the file persisted, or
    /// left the graph (S-513). Idempotent.
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn clear_persist_failure(&self, path: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM persist_failures WHERE path = ?1", [path])
            .context("clearing a persist failure")?;
        Ok(())
    }

    /// Clear every persist-failure record — a full index rewrites the record
    /// to exactly its own failures (S-513).
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn clear_persist_failures(&self) -> Result<()> {
        self.conn
            .execute("DELETE FROM persist_failures", [])
            .context("clearing the persist-failure record")?;
        Ok(())
    }

    /// Advance the persisted monotonic graph revision by one and return the new
    /// value (CR-027, [ADR-32], [FR-SY-09]).
    ///
    /// Read-modify-write **within the surrounding transaction**: the current
    /// revision is read from `project_metadata` and re-written incremented in the
    /// same batch ([NFR-RA-07]). Because every write rides the single-writer
    /// actor ([NFR-RA-10]), no concurrent writer can interleave between the read
    /// and the upsert, so the counter is strictly monotonic without a separate
    /// lock. A never-written revision reads as `0`, so the first advance returns
    /// `1`; an unparseable stored value is likewise treated as `0` (the read
    /// counterpart in [`GraphStore::graph_revision`] makes the same choice).
    ///
    /// Persisting the revision is the single-row write [NFR-PE-03] budgets for on
    /// the existing post-`sync` write path. The caller advances it once per
    /// completed `index` and once per `sync` that mutated the graph (the
    /// pipeline gates the call), and once inside the batch of a coordinate
    /// migration, which moves every symbol (S-704) — never on a no-op `sync`
    /// or a read-only read.
    ///
    /// [ADR-32]: ../../../docs/specs/architecture/decisions/ADR-32.md
    /// [FR-SY-09]: ../../../docs/specs/requirements/FR-SY-09.md
    /// [NFR-PE-03]: ../../../docs/specs/requirements/NFR-PE-03.md
    /// [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
    /// [NFR-RA-10]: ../../../docs/specs/requirements/NFR-RA-10.md
    ///
    /// # Errors
    /// Returns an error on I/O failure reading or writing the revision row.
    pub fn advance_graph_revision(&self) -> Result<u64> {
        let current: u64 = self
            .conn
            .query_row(
                "SELECT value FROM project_metadata WHERE key = ?1",
                [GRAPH_REVISION_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .context("reading current graph revision")?
            .and_then(|raw| raw.parse::<u64>().ok())
            .unwrap_or(0);
        // saturating_add keeps the type total: a u64 counter advanced once per
        // pipeline run will never realistically overflow, but saturating beats a
        // debug panic and stays clippy-clean.
        let next = current.saturating_add(1);
        // Reuse the shared upsert rather than repeating its SQL: both run on the
        // same transaction connection, so atomicity is unchanged (the single-writer
        // actor forbids any interleaving between the read above and this write), and
        // the revision can never drift from how every other metadata key is written.
        self.set_project_metadata(GRAPH_REVISION_KEY, &next.to_string())?;
        Ok(next)
    }

    /// Delete every node bound to `file_id`, returning the count removed.
    ///
    /// Each deleted node cascades to its incident edges (both `source` and
    /// `target` are `ON DELETE CASCADE`) and fires the FTS `'delete'` trigger.
    /// This is the destructive half of an incremental re-extract — call
    /// [`inbound_cross_file_edges`](Self::inbound_cross_file_edges) **first** to
    /// preserve links pointing into this file ([ADR-10]).
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn delete_nodes_for_file(&self, file_id: i64) -> Result<usize> {
        self.conn
            .execute("DELETE FROM nodes WHERE file_id = ?1", [file_id])
            .context("deleting nodes for file")
    }

    /// Delete a file row by id, returning the count removed (0 or 1).
    ///
    /// Any nodes still bound to it have their `file_id` set to `NULL`
    /// (`ON DELETE SET NULL`); a re-extract deletes those nodes first via
    /// [`delete_nodes_for_file`](Self::delete_nodes_for_file), so this is used
    /// for true removals (a file deleted from disk).
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn delete_file(&self, file_id: i64) -> Result<usize> {
        self.conn
            .execute("DELETE FROM files WHERE id = ?1", [file_id])
            .context("deleting file row")
    }

    /// Capture the cross-file edges that point **into** `file_id` — edges whose
    /// target node lives in this file but whose source node does not ([ADR-10]).
    ///
    /// Endpoints are returned as symbol strings so the edges can be re-attached
    /// after the file is re-extracted (the target node's rowid changes, but its
    /// canonical symbol does not). Intra-file edges (source also in this file)
    /// are excluded — extraction re-emits those. A source node with a `NULL`
    /// `file_id` is treated as cross-file and therefore captured.
    ///
    /// **Derived edges are never captured** (`derived = 0` filter): the
    /// annotation pass owns and recomputes them every run (S-014, [FR-AN-03]),
    /// so capturing one would launder it into the ledger as a permanently
    /// retried, resolution-rebound `derived = 0` edge that `clear_derived`
    /// could never own — a governance flag outliving the rule that produced it.
    ///
    /// [FR-AN-03]: ../../../docs/specs/requirements/FR-AN-03.md
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn inbound_cross_file_edges(&self, file_id: i64) -> Result<Vec<CapturedEdge>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT ss.symbol, ts.symbol, e.kind \
             FROM edges e \
             JOIN nodes sn ON sn.id = e.source \
             JOIN nodes tn ON tn.id = e.target \
             JOIN symbols ss ON ss.id = sn.symbol_id \
             JOIN symbols ts ON ts.id = tn.symbol_id \
             WHERE tn.file_id = ?1 AND (sn.file_id IS NULL OR sn.file_id <> ?1) \
               AND e.derived = 0",
        )?;
        let rows = stmt
            .query_map([file_id], |row| {
                Ok(CapturedEdge {
                    source_symbol: row.get(0)?,
                    target_symbol: row.get(1)?,
                    kind: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("capturing inbound cross-file edges")?;
        Ok(rows)
    }

    // ── Reference-ledger primitives (S-011, [ADR-10]) ────────────────────────

    /// Insert a reference-ledger row, idempotently over the relation-,
    /// receiver-, alias-, wrapper- and argument-count-aware `(source_symbol,
    /// target, form, kind, COALESCE(payload, ''), COALESCE(receiver, 0),
    /// COALESCE(alias, ''), COALESCE(peeled, ''), COALESCE(arg_count, -1))`
    /// uniqueness rule (migration 18, [CR-080]; widened by the receiver shape in
    /// migration 29, S-514, by the import alias in migration 31, S-597 — two
    /// local names of one target are two rows, [FR-DB-07] — by a proven
    /// receiver's peeled wrappers in migration 32, S-587, and by a call's
    /// argument count in migration 33, S-591).
    ///
    /// A duplicate (the same function calling the same path twice, or a
    /// captured edge re-captured on a later sync) is a no-op — the first row's
    /// `line` is kept. Two rows that agree on the four shipped key columns but
    /// carry a **different relation** (`payload`) are distinct couplings and both
    /// survive: a broker *relay* keeps its publish and its subscribe rather than
    /// collapsing one onto the other. `payload` is normalised through `COALESCE`
    /// because SQLite treats each bare `NULL` as distinct in a UNIQUE key, so a
    /// raw-`payload` key would stop deduping ordinary (`payload = NULL`) code refs.
    ///
    /// [CR-080]: ../../../docs/requests/CR-080-broker-relay-ledger-dedup.md
    /// [FR-DB-07]: ../../../docs/specs/requirements/FR-DB-07.md
    ///
    /// # Errors
    /// Returns an error if a CHECK/FK constraint fires or I/O fails.
    pub fn insert_unresolved_ref(&self, r: &NewUnresolvedRef<'_>) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO unresolved_refs \
                 (file_id, source_symbol, target, alias, form, kind, line, payload, receiver, peeled, arg_count, exported) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) \
                 ON CONFLICT(source_symbol, target, form, kind, COALESCE(payload, ''), \
                             COALESCE(receiver, 0), COALESCE(alias, ''), COALESCE(peeled, ''), \
                             COALESCE(arg_count, -1)) DO NOTHING",
                rusqlite::params![
                    r.file_id,
                    r.source_symbol,
                    r.target,
                    r.alias,
                    r.form.as_i32(),
                    r.kind.as_i32(),
                    r.line,
                    r.payload,
                    r.receiver.map(ReceiverShape::as_i32),
                    r.peeled,
                    r.arg_count,
                    r.exported.map(i64::from),
                ],
            )
            .context("inserting unresolved ref")?;
        Ok(())
    }

    /// Replace a file's configuration-corpus contribution (S-380, [CR-121],
    /// [FR-WS-19]): delete the source row it previously had, then record the
    /// profile and canonical key → value pairs the fresh extract proved.
    ///
    /// The delete runs unconditionally, ahead of the `source` check, so a file
    /// that *stops* being a configuration source (renamed away from
    /// `application*.…`, or emptied) leaves no stale values behind — the same
    /// replace-wholesale contract
    /// [`delete_unresolved_refs_for_file`](Self::delete_unresolved_refs_for_file)
    /// gives the reference ledger. On a file that never was one the delete
    /// matches no row, so `None` is a no-op end to end: that is what keeps a
    /// member with no configuration corpus byte-for-byte unaffected.
    ///
    /// Deleting the source cascades its values away (migration 19's FK), so the
    /// two tables can never disagree about which file proved what.
    ///
    /// # Errors
    /// Returns an error if a constraint fires or I/O fails.
    ///
    /// [CR-121]: ../../../docs/requests/CR-121-caller-to-callee-and-producer-to-consumer-across-services.md
    /// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
    pub fn replace_config_source(
        &self,
        file_id: i64,
        source: Option<NewConfigSource<'_>>,
    ) -> Result<()> {
        self.conn
            .execute("DELETE FROM config_sources WHERE file_id = ?1", [file_id])
            .context("deleting the previous configuration source for file")?;
        let Some(source) = source else {
            return Ok(());
        };
        self.conn
            .execute(
                "INSERT INTO config_sources (file_id, profile) VALUES (?1, ?2)",
                rusqlite::params![file_id, source.profile],
            )
            .context("inserting configuration source")?;
        let source_id = self.conn.last_insert_rowid();
        let mut stmt = self.conn.prepare_cached(
            "INSERT INTO config_values (source_id, key, value) VALUES (?1, ?2, ?3) \
             ON CONFLICT(source_id, key, value) DO NOTHING",
        )?;
        for (key, value) in source.values {
            stmt.execute(rusqlite::params![source_id, key, value])
                .context("inserting configuration value")?;
        }
        Ok(())
    }

    /// Replace this member's build-manifest facts wholesale (S-462, [CR-148]
    /// §3.2 A): delete every recorded manifest, then record `manifests`.
    ///
    /// Wholesale because the facts are: a pom's inherited group and properties
    /// come from its in-member parent chain, so one manifest's change can move
    /// another's facts, and the pipeline re-derives the whole member whenever
    /// any manifest changed. Deleting a manifest cascades its artifacts away
    /// (migration 22's FK), so the two tables can never disagree.
    ///
    /// An incremental sync calls this only when a manifest was added, changed or
    /// removed; a full index calls it whenever the walk found a manifest or one
    /// is recorded. Neither calls it for a member with no manifest and none
    /// recorded, so a member without a build manifest writes nothing here.
    ///
    /// # Errors
    /// Returns an error if a constraint fires (a vocabulary token migration 22
    /// does not admit, a refusal without a reason) or I/O fails.
    ///
    /// [CR-148]: ../../../docs/requests/CR-148-build-manifests-yield-a-build-dependency-relation.md
    pub fn replace_build_manifests(&self, manifests: &[NewBuildManifest<'_>]) -> Result<()> {
        self.conn
            .execute("DELETE FROM build_manifests", [])
            .context("deleting the previous build manifests")?;
        let mut manifest_stmt = self.conn.prepare_cached(
            "INSERT INTO build_manifests (path, format, content_hash, status, detail) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        let mut artifact_stmt = self.conn.prepare_cached(
            "INSERT INTO build_artifacts (manifest_id, role, kind, group_id, artifact_id, \
                                          version, scope, project_path, resolution, reason) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )?;
        for manifest in manifests {
            manifest_stmt
                .execute(rusqlite::params![
                    manifest.path,
                    manifest.format,
                    manifest.content_hash,
                    manifest.status,
                    manifest.detail
                ])
                .with_context(|| format!("inserting build manifest {}", manifest.path))?;
            let manifest_id = self.conn.last_insert_rowid();
            for a in &manifest.artifacts {
                artifact_stmt
                    .execute(rusqlite::params![
                        manifest_id,
                        a.role,
                        a.kind,
                        a.group_id,
                        a.artifact_id,
                        a.version,
                        a.scope,
                        a.project_path,
                        a.resolution,
                        a.reason
                    ])
                    .with_context(|| format!("inserting a build artifact of {}", manifest.path))?;
            }
        }
        Ok(())
    }

    /// Replace the declared-type facts a **source file** proves (S-472,
    /// [CR-152] §3.2 B): delete the file's previous facts, then record `types`.
    ///
    /// Per file, the replace-wholesale contract
    /// [`replace_config_source`](Self::replace_config_source) gives the
    /// configuration corpus: a re-extract calls it for exactly the file it
    /// re-extracts, so a one-file change moves only that file's facts. On a file
    /// that never declared a type the delete matches no row and an empty
    /// `types` writes none — a member with no Java/Kotlin source is
    /// byte-for-byte unaffected. Removing the file cascades its facts away
    /// (migration 24's FK).
    ///
    /// # Errors
    /// Returns an error if a constraint fires (a token migration 24 does not
    /// admit, a refusal without a reason, a source fact without a symbol) or
    /// I/O fails.
    ///
    /// [CR-152]: ../../../docs/requests/CR-152-cross-member-type-references-overlay.md
    pub fn replace_file_declared_types(&self, file_id: i64, types: &[NewDeclaredType<'_>]) -> Result<()> {
        self.conn
            .execute("DELETE FROM declared_types WHERE file_id = ?1", [file_id])
            .context("deleting the previous declared types of a file")?;
        let mut stmt = self.conn.prepare_cached(
            "INSERT INTO declared_types (file_id, name, fqn, kind, symbol, tree, resolution, reason) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for t in types {
            stmt.execute(rusqlite::params![
                file_id,
                t.name,
                t.fqn,
                t.kind,
                t.symbol,
                t.tree,
                resolution_token(t),
                t.reason
            ])
            .with_context(|| format!("inserting declared type {}", t.name))?;
        }
        Ok(())
    }

    /// Replace the impl blocks a source file declares (S-606, [CR-202] F1):
    /// delete the file's previous blocks, then record `blocks` — the
    /// [`replace_file_declared_types`](Self::replace_file_declared_types)
    /// per-file contract, so a re-extract moves only that file's blocks, and a
    /// file declaring none writes none. Removing the file cascades its blocks
    /// away (migration 34's FK).
    ///
    /// # Errors
    /// Returns an error if a constraint fires (an empty self type, a `Deref`
    /// target without a trait) or I/O fails.
    ///
    /// [CR-202]: ../../../docs/requests/CR-202-one-rust-associated-item-lookup.md
    pub fn replace_file_impl_blocks(&self, file_id: i64, blocks: &[NewImplBlock<'_>]) -> Result<()> {
        self.conn
            .execute("DELETE FROM impl_blocks WHERE file_id = ?1", [file_id])
            .context("deleting the previous impl blocks of a file")?;
        let mut stmt = self.conn.prepare_cached(
            "INSERT INTO impl_blocks (file_id, start_line, end_line, self_type, self_ref, trait_path, deref_target) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for b in blocks {
            stmt.execute(rusqlite::params![
                file_id,
                b.start_line,
                b.end_line,
                b.self_type,
                i64::from(b.self_ref),
                b.trait_path,
                b.deref_target,
            ])
            .with_context(|| format!("inserting the impl block of {} at line {}", b.self_type, b.start_line))?;
        }
        Ok(())
    }

    /// Replace the local bindings a source file records (S-593, [FR-EX-33]):
    /// delete the file's previous bindings, then record `bindings` — the
    /// [`replace_file_impl_blocks`](Self::replace_file_impl_blocks) per-file
    /// contract, so a re-extract moves only that file's bindings, and a file
    /// recording none writes none. Removing the file cascades its bindings away
    /// (migration 41's FK).
    ///
    /// # Errors
    /// Returns an error if a constraint fires (an empty name, a scope ending
    /// before it starts) or I/O fails.
    ///
    /// [FR-EX-33]: ../../../docs/specs/requirements/FR-EX-33.md
    pub fn replace_file_local_bindings(&self, file_id: i64, bindings: &[NewLocalBinding<'_>]) -> Result<()> {
        self.conn
            .execute("DELETE FROM local_bindings WHERE file_id = ?1", [file_id])
            .context("deleting the previous local bindings of a file")?;
        let mut stmt = self.conn.prepare_cached(
            "INSERT INTO local_bindings (file_id, name, kind, line, col, scope_start_line, scope_start_col, \
                                         scope_end_line, scope_end_col, hoisted) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )?;
        for b in bindings {
            stmt.execute(rusqlite::params![
                file_id,
                b.name,
                b.kind.as_i32(),
                b.line,
                b.column,
                b.scope_start_line,
                b.scope_start_column,
                b.scope_end_line,
                b.scope_end_column,
                i64::from(b.hoisted),
            ])
            .with_context(|| format!("inserting the local binding {} at line {}", b.name, b.line))?;
        }
        Ok(())
    }

    /// Record one Avro schema and its types, replacing whatever was recorded
    /// under its path (S-472): the schema row is deleted — cascading its types
    /// away — and re-inserted. Every other schema's facts are untouched, so a
    /// one-schema change moves only that schema's facts.
    ///
    /// # Errors
    /// Returns an error if a constraint fires or I/O fails.
    pub fn replace_avro_schema(&self, schema: &NewAvroSchema<'_>) -> Result<()> {
        self.delete_avro_schema(schema.path)?;
        self.conn
            .execute(
                "INSERT INTO avro_schemas (path, content_hash, status, detail) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![schema.path, schema.content_hash, schema.status, schema.detail],
            )
            .with_context(|| format!("inserting Avro schema {}", schema.path))?;
        let schema_id = self.conn.last_insert_rowid();
        let mut stmt = self.conn.prepare_cached(
            "INSERT INTO declared_types (schema_id, name, fqn, kind, resolution, reason) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for t in &schema.types {
            stmt.execute(rusqlite::params![
                schema_id,
                t.name,
                t.fqn,
                t.kind,
                resolution_token(t),
                t.reason
            ])
            .with_context(|| format!("inserting a declared type of {}", schema.path))?;
        }
        Ok(())
    }

    /// Forget the Avro schema recorded under `path`, with its types (the FK
    /// cascades). A path never recorded matches no row.
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn delete_avro_schema(&self, path: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM avro_schemas WHERE path = ?1", [path])
            .with_context(|| format!("deleting Avro schema {path}"))?;
        Ok(())
    }

    /// Delete every reference-ledger row produced by `file_id`, returning the
    /// count removed.
    ///
    /// The replace-wholesale half of a re-extract: stale refs from the file's
    /// previous version never linger, fresh extraction re-inserts the current
    /// set.
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn delete_unresolved_refs_for_file(&self, file_id: i64) -> Result<usize> {
        self.conn
            .execute("DELETE FROM unresolved_refs WHERE file_id = ?1", [file_id])
            .context("deleting unresolved refs for file")
    }

    /// Record a reference-ledger row's binding state after a resolution run.
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn mark_ref_resolved(&self, ref_id: i64, resolved: bool) -> Result<()> {
        self.conn
            .execute(
                "UPDATE unresolved_refs SET resolved = ?2 WHERE id = ?1",
                rusqlite::params![ref_id, i64::from(resolved)],
            )
            .context("updating ref resolved state")?;
        Ok(())
    }

    /// Delete one reference-ledger row by id, returning the count removed (0
    /// or 1).
    ///
    /// The resolution pass's end of a capture-before-delete row's life
    /// ([ADR-10], CR-187): once it is spent, the ledger drops it, so a synced
    /// ledger equals a fresh index's ([FR-SY-10]).
    ///
    /// [ADR-10]: ../../../docs/specs/architecture/decisions/ADR-10.md
    /// [FR-SY-10]: ../../../docs/specs/requirements/FR-SY-10.md
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn delete_unresolved_ref(&self, ref_id: i64) -> Result<usize> {
        self.conn
            .execute("DELETE FROM unresolved_refs WHERE id = ?1", [ref_id])
            .context("deleting a spent ledger row")
    }

    /// Insert an edge unless `(source, target, kind)` already exists; returns
    /// `true` when a row was actually inserted.
    ///
    /// The resolution pass uses this (rather than [`insert_edge`]) because two
    /// distinct ledger rows can legitimately bind to the same edge — e.g. a
    /// captured exact-symbol ref and the re-extracted textual ref for the same
    /// call ([ADR-10]).
    ///
    /// [`insert_edge`]: Self::insert_edge
    /// [ADR-10]: ../../../docs/specs/architecture/decisions/ADR-10.md
    ///
    /// # Errors
    /// Returns an error if an endpoint node is missing or I/O fails.
    pub fn insert_edge_if_absent(
        &self,
        source: NodeId,
        target: NodeId,
        kind: EdgeKind,
    ) -> Result<bool> {
        self.insert_edge_with_payload_if_absent(source, target, kind, None)
    }

    /// Insert an edge with a relation `payload` unless `(source, target, kind)`
    /// already exists; returns `true` when a row was actually inserted.
    ///
    /// The payload variant of [`insert_edge_if_absent`](Self::insert_edge_if_absent):
    /// the resolution pass writes the cross-artifact relation class
    /// ([`ArtifactRelation`](crate::model::ArtifactRelation) wire token) onto an
    /// `ArtifactRef`/`ArtifactBinding` edge so navigation can surface *which*
    /// relation a binding expresses (CR-011, [FR-CG-07], [FR-CG-11]); a code/doc/
    /// access binding passes `None`. `payload` is **not** part of the UNIQUE key,
    /// so a re-resolved edge keeps its first-written payload — idempotent like the
    /// plain insert.
    ///
    /// [FR-CG-07]: ../../../docs/specs/requirements/FR-CG-07.md
    /// [FR-CG-11]: ../../../docs/specs/requirements/FR-CG-11.md
    ///
    /// # Errors
    /// Returns an error if an endpoint node is missing or I/O fails.
    pub fn insert_edge_with_payload_if_absent(
        &self,
        source: NodeId,
        target: NodeId,
        kind: EdgeKind,
        payload: Option<&str>,
    ) -> Result<bool> {
        let inserted = self
            .conn
            .execute(
                "INSERT INTO edges (source, target, kind, payload) VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(source, target, kind) DO NOTHING",
                rusqlite::params![source.get(), target.get(), kind.as_i32(), payload],
            )
            .context("inserting edge with payload if absent")?;
        Ok(inserted > 0)
    }

    /// [`insert_edge_with_payload_if_absent`](Self::insert_edge_with_payload_if_absent)
    /// for an edge computed off a snapshot: `Ok(None)` when an endpoint node no
    /// longer exists, instead of an error.
    ///
    /// The resolution pass binds against a read snapshot and commits in a later
    /// write; a write serialized between the two (a config apply's purge, a
    /// watcher sync) can delete an endpoint. Its foreign-key failure is
    /// statement-level, so the enclosing transaction carries on and the caller
    /// leaves the reference for the next pass to re-bind (ADR-02).
    ///
    /// **A dispatch marker yields** (S-712, [CR-226] BR-76): when the edge is
    /// already present marked `dispatch:` ([`crate::model::dispatch`]), an
    /// unmarked write clears the marker — a direct call to an implementation
    /// is a dependency, even where the same caller also reaches it by dispatch
    /// — and of two markers the smaller stays. So the committed payload is the
    /// same whichever of the edge's rows is written first ([NFR-RA-06]). Any
    /// other payload is kept as the first write left it. The upgrade is no
    /// insertion: `Some(false)`. It decides between this run's writes and
    /// what is stored; an edge out of a source the run re-bound whole is
    /// then set to the run's own verdict
    /// ([`set_dispatch_marker`](Self::set_dispatch_marker)).
    ///
    /// # Errors
    /// Returns an error on any other failure.
    ///
    /// [CR-226]: ../../../docs/requests/CR-226-dispatch-is-not-a-dependency.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    pub fn insert_edge_with_payload_if_live(
        &self,
        source: NodeId,
        target: NodeId,
        kind: EdgeKind,
        payload: Option<&str>,
    ) -> Result<Option<bool>> {
        match self.conn.execute(
            "INSERT INTO edges (source, target, kind, payload) VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(source, target, kind) DO NOTHING",
            rusqlite::params![source.get(), target.get(), kind.as_i32(), payload],
        ) {
            Ok(0) => {
                self.yield_dispatch_marker(source, target, kind, payload)?;
                Ok(Some(false))
            }
            Ok(_) => Ok(Some(true)),
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY =>
            {
                Ok(None)
            }
            Err(e) => Err(e).context("inserting edge with payload if live"),
        }
    }

    /// Set the dispatch marker of the `Calls` edge `source → target` to
    /// `marker` — `None` unmarks it — where the edge is unmarked or marked
    /// dispatch (S-712): what a resolution run decided for an edge out of a
    /// source whose every row it re-bound, overriding whatever an earlier run
    /// left on it. A no-op for an absent edge, and for one carrying any other
    /// payload.
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn set_dispatch_marker(&self, source: NodeId, target: NodeId, marker: Option<&str>) -> Result<()> {
        self.conn
            .execute(
                "UPDATE edges SET payload = ?3 \
                 WHERE source = ?1 AND target = ?2 AND kind = ?4 \
                   AND (payload IS NULL OR payload GLOB ?5) AND payload IS NOT ?3",
                rusqlite::params![
                    source.get(),
                    target.get(),
                    marker,
                    EdgeKind::Calls.as_i32(),
                    crate::model::dispatch::DISPATCH_GLOB
                ],
            )
            .context("setting a dispatch marker")?;
        Ok(())
    }

    /// The upgrade half of
    /// [`insert_edge_with_payload_if_live`](Self::insert_edge_with_payload_if_live):
    /// the present edge's dispatch marker gives way to an unmarked `payload`,
    /// or to a smaller marker. A no-op for every other payload, on either side.
    fn yield_dispatch_marker(&self, source: NodeId, target: NodeId, kind: EdgeKind, payload: Option<&str>) -> Result<()> {
        use crate::model::dispatch::{dispatched_method, DISPATCH_GLOB};
        if payload.is_some_and(|p| dispatched_method(p).is_none()) {
            return Ok(());
        }
        self.conn
            .execute(
                "UPDATE edges SET payload = ?4 \
                 WHERE source = ?1 AND target = ?2 AND kind = ?3 \
                   AND payload GLOB ?5 AND (?4 IS NULL OR ?4 < payload)",
                rusqlite::params![source.get(), target.get(), kind.as_i32(), payload, DISPATCH_GLOB],
            )
            .context("yielding a dispatch marker to a direct bind")?;
        Ok(())
    }

    // ── Annotation primitives (S-014, [FR-AN-01..04]) ────────────────────────

    /// Clear every derived policy node and derived edge, returning
    /// `(nodes_removed, edges_removed)`.
    ///
    /// The idempotency primitive of the annotation pass ([FR-AN-03]): each run
    /// starts from a clean slate and re-materialises the policy graph, so a
    /// re-run never accumulates stale `forbidden_dependency` flags or layer
    /// nodes. Derived nodes cascade their incident edges and fire the FTS
    /// `'delete'` trigger like any other node delete.
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    ///
    /// [FR-AN-03]: ../../../docs/specs/requirements/FR-AN-03.md
    pub fn clear_derived(&self) -> Result<(usize, usize)> {
        // Edges first: a standalone derived edge between two extracted nodes
        // (the forbidden_dependency flag) would not be cascaded by the node
        // delete below.
        let edges = self
            .conn
            .execute("DELETE FROM edges WHERE derived = 1", [])
            .context("clearing derived edges")?;
        let nodes = self
            .conn
            .execute("DELETE FROM nodes WHERE derived = 1", [])
            .context("clearing derived nodes")?;
        Ok((nodes, edges))
    }

    /// Write one node's Pass-3 annotation results to its native columns
    /// ([FR-AN-04]): `is_dead`, `is_duplicate`, `is_test`, `layer_membership`,
    /// and the near-clone `clone_group` ([FR-AN-06], CR-005).
    ///
    /// `None` records the honest "not applicable / not computed" `NULL`,
    /// distinct from `false`, for the tri-state verdicts. `is_test` is a
    /// definite boolean ([FR-AN-05]) — positive-evidence classification, so it
    /// is always `0` or `1`, never `NULL`. `clone_group` is the stable clone-group
    /// identifier (the minimum node id of the function's near-clone component) or
    /// `None` when the function is in no near-clone group ([FR-AN-06]); writing it
    /// for every annotated node — `None` included — clears stale membership from a
    /// prior run, keeping the pass idempotent ([NFR-RA-06]).
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    ///
    /// [FR-AN-04]: ../../../docs/specs/requirements/FR-AN-04.md
    /// [FR-AN-05]: ../../../docs/specs/requirements/FR-AN-05.md
    /// [FR-AN-06]: ../../../docs/specs/requirements/FR-AN-06.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    pub fn set_node_annotations(
        &self,
        id: NodeId,
        is_dead: Option<bool>,
        is_duplicate: Option<bool>,
        is_test: bool,
        layer_membership: Option<&str>,
        clone_group: Option<NodeId>,
    ) -> Result<()> {
        let mut stmt = self.conn.prepare_cached(
            "UPDATE nodes SET is_dead = ?2, is_duplicate = ?3, is_test = ?4, \
                              layer_membership = ?5, clone_group = ?6 \
             WHERE id = ?1",
        )?;
        stmt.execute(rusqlite::params![
            id.get(),
            is_dead.map(i64::from),
            is_duplicate.map(i64::from),
            i64::from(is_test),
            layer_membership,
            clone_group.map(NodeId::get),
        ])
        .context("writing node annotations")?;
        Ok(())
    }

    /// Insert a **derived** edge (`derived = 1`) unless `(source, target,
    /// kind)` already exists; returns `true` when a row was inserted.
    ///
    /// The materialisation primitive for `forbidden_dependency` flags
    /// ([FR-AN-03]). Conflict-tolerant because two boundaries can legitimately
    /// condemn the same underlying dependency. The conflict arm **upgrades**
    /// `derived` to `1` rather than doing nothing — defence in depth: should a
    /// same-`(source, target, kind)` row ever exist un-flagged (e.g. a
    /// historical capture from before
    /// [`inbound_cross_file_edges`](Self::inbound_cross_file_edges) excluded
    /// derived edges), the flag is repaired so `clear_derived` owns it again.
    ///
    /// # Errors
    /// Returns an error if an endpoint node is missing or I/O fails.
    ///
    /// [FR-AN-03]: ../../../docs/specs/requirements/FR-AN-03.md
    pub fn insert_derived_edge(
        &self,
        source: NodeId,
        target: NodeId,
        kind: EdgeKind,
    ) -> Result<bool> {
        // `WHERE edges.derived = 0` keeps the duplicate-materialisation case a
        // true no-op (affected-rows 0) while still repairing a stranded flag.
        let inserted = self
            .conn
            .execute(
                "INSERT INTO edges (source, target, kind, derived) VALUES (?1, ?2, ?3, 1) \
                 ON CONFLICT(source, target, kind) DO UPDATE SET derived = 1 \
                 WHERE edges.derived = 0",
                rusqlite::params![source.get(), target.get(), kind.as_i32()],
            )
            .context("inserting derived edge")?;
        Ok(inserted > 0)
    }

    /// Record a file's `[[layers]]` band ([FR-AN-03]); `None` clears it (the
    /// file matches no layer glob under the current `rules.toml`).
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    ///
    /// [FR-AN-03]: ../../../docs/specs/requirements/FR-AN-03.md
    pub fn set_file_layer(&self, file_id: i64, layer: Option<&str>) -> Result<()> {
        let mut stmt = self
            .conn
            .prepare_cached("UPDATE files SET layer = ?2 WHERE id = ?1")?;
        stmt.execute(rusqlite::params![file_id, layer])
            .context("writing file layer")?;
        Ok(())
    }

    /// Resolve a symbol string to the id of the node that carries it, or `None`.
    ///
    /// Used to rebind a captured cross-file edge: the source endpoint persists
    /// across the sync, and the target endpoint is looked up by its (stable)
    /// symbol after re-extraction.
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn node_id_for_symbol(&self, symbol: &str) -> Result<Option<NodeId>> {
        self.conn
            .query_row(
                // `ORDER BY n.id` makes the pick deterministic if a symbol ever maps
                // to more than one node (symbols are unique per node by construction
                // today, ADR-07, but the ordering removes any reliance on that).
                "SELECT n.id FROM nodes n \
                 JOIN symbols s ON s.id = n.symbol_id \
                 WHERE s.symbol = ?1 \
                 ORDER BY n.id \
                 LIMIT 1",
                [symbol],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .context("resolving node id by symbol")
            .map(|opt| opt.map(NodeId))
    }

    // ── Framework-promotion primitives (S-012, [FR-FW-01]..[FR-FW-04]) ───────

    /// Delete a node by id within the batch (cascading to its incident edges
    /// and firing the FTS `'delete'` trigger); see
    /// [`SqliteGraphStore::delete_node`].
    ///
    /// The framework pass uses this to retire a promoted `route`/`component`
    /// node whose source pattern no longer exists (the reconcile sweep).
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn delete_node(&self, id: NodeId) -> Result<usize> {
        delete_node(self.conn, id)
    }

    /// Delete one `(source, target, kind)` edge, returning the count removed
    /// (0 or 1).
    ///
    /// The framework pass uses this to retire a stale edge incident to a
    /// promoted node (e.g. a route whose handler wiring changed) without
    /// churning the node ids themselves.
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn delete_edge(&self, source: NodeId, target: NodeId, kind: EdgeKind) -> Result<usize> {
        self.conn
            .execute(
                "DELETE FROM edges WHERE source = ?1 AND target = ?2 AND kind = ?3",
                rusqlite::params![source.get(), target.get(), kind.as_i32()],
            )
            .context("deleting edge")
    }

    // ── Metric-snapshot primitives (S-018, [FR-QM-07]) ───────────────────────

    /// Append one `metric_snapshots` row together with the worst-offender lists
    /// the same computation produced, returning the row's id.
    ///
    /// The single write the metrics engine performs ([FR-QM-07]). The table is
    /// append-only by contract — past snapshots are never mutated, so this is
    /// deliberately the only snapshot primitive. The lists are a **required**
    /// argument ([FR-QM-15]): there is no way to append a snapshot without
    /// saying what it found, so every caller — `scan`, `gate`, `session_start`,
    /// `session_end` — persists its lists, and they land in the caller's batch
    /// transaction with the row (one `submit_write`, [NFR-RA-07]). The row is
    /// stamped `offenders_recorded = 1`, even when every list is empty: that is
    /// a recorded-empty result, not an unrecorded one.
    ///
    /// The "where to start" ranking ([FR-QM-17]) is required on the same terms
    /// and lands in the same batch, stamping `insight_recorded = 1`.
    ///
    /// `uncapped` is every offender, the lists `offenders` is the head of
    /// ([FR-QM-19]); it lands in the same batch and the row is stamped
    /// `offender_values_recorded = 1`, which also vouches for the `node_id`,
    /// `measure`, `value` and `threshold` of the capped rows.
    ///
    /// # Errors
    /// Returns an error if `offenders`, `uncapped` or `insight` is not a
    /// recorded set (the `default()` "not recorded" value would otherwise
    /// persist as a recorded-empty result), if either offender set was not
    /// computed with its values, if a CHECK constraint fires, or if I/O fails.
    ///
    /// [FR-QM-17]: ../../../docs/specs/requirements/FR-QM-17.md
    /// [FR-QM-19]: ../../../docs/specs/requirements/FR-QM-19.md
    ///
    /// [FR-QM-07]: ../../../docs/specs/requirements/FR-QM-07.md
    /// [FR-QM-15]: ../../../docs/specs/requirements/FR-QM-15.md
    /// [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
    pub fn insert_metric_snapshot(
        &self,
        snapshot: &NewMetricSnapshot<'_>,
        offenders: &WorstOffenders,
        uncapped: &WorstOffenders,
        insight: &WhereToStart,
    ) -> Result<i64> {
        anyhow::ensure!(
            offenders.recorded && uncapped.recorded,
            "refusing to persist offender lists that were never computed as a recorded set"
        );
        anyhow::ensure!(
            offenders.values_recorded && uncapped.values_recorded,
            "refusing to persist offender lists computed without their values"
        );
        anyhow::ensure!(
            insight.recorded,
            "refusing to persist a where-to-start ranking that was never computed as a recorded set"
        );
        self.conn
            .execute(
                "INSERT INTO metric_snapshots \
                 (created_at, commit_sha, node_count, edge_count, function_count, \
                  test_function_count, metric_version, empty, \
                  modularity_raw, modularity_normalized, \
                  acyclicity_raw, acyclicity_normalized, \
                  depth_raw, depth_normalized, \
                  equality_raw, equality_normalized, \
                  redundancy_raw, redundancy_normalized, \
                  nesting_raw, nesting_normalized, \
                  conciseness_raw, conciseness_normalized, \
                  cohesion_raw, cohesion_normalized, cohesion_applicable, \
                  focus_raw, focus_normalized, focus_applicable, \
                  uniqueness_raw, uniqueness_normalized, \
                  thresholds_hash, aggregate_signal, modularity_applicable, \
                  offenders_recorded, insight_recorded, offender_values_recorded, \
                  acyclicity_applicable, depth_applicable, cross_module_edges) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, \
                         ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, \
                         ?30, ?31, ?32, ?33, 1, 1, 1, ?34, ?35, ?36)",
                rusqlite::params![
                    snapshot.created_at,
                    snapshot.commit_sha,
                    snapshot.node_count,
                    snapshot.edge_count,
                    snapshot.function_count,
                    snapshot.test_function_count,
                    snapshot.metric_version,
                    i64::from(snapshot.empty),
                    snapshot.modularity_raw,
                    snapshot.modularity_normalized,
                    snapshot.acyclicity_raw,
                    snapshot.acyclicity_normalized,
                    snapshot.depth_raw,
                    snapshot.depth_normalized,
                    snapshot.equality_raw,
                    snapshot.equality_normalized,
                    snapshot.redundancy_raw,
                    snapshot.redundancy_normalized,
                    snapshot.nesting_raw,
                    snapshot.nesting_normalized,
                    snapshot.conciseness_raw,
                    snapshot.conciseness_normalized,
                    snapshot.cohesion_raw,
                    snapshot.cohesion_normalized,
                    snapshot.cohesion_applicable.map(i64::from),
                    snapshot.focus_raw,
                    snapshot.focus_normalized,
                    snapshot.focus_applicable.map(i64::from),
                    snapshot.uniqueness_raw,
                    snapshot.uniqueness_normalized,
                    snapshot.thresholds_hash,
                    snapshot.aggregate_signal,
                    snapshot.modularity_applicable.map(i64::from),
                    snapshot.acyclicity_applicable.map(i64::from),
                    snapshot.depth_applicable.map(i64::from),
                    snapshot.cross_module_edges,
                ],
            )
            .context("inserting metric snapshot")?;
        let id = self.conn.last_insert_rowid();
        self.insert_snapshot_offenders(CAPPED_OFFENDERS, id, offenders)?;
        self.insert_snapshot_offenders(UNCAPPED_OFFENDERS, id, uncapped)?;
        self.insert_snapshot_insight(id, insight)?;
        Ok(id)
    }

    /// Append the offender rows of snapshot `id` to `table` ([FR-QM-15],
    /// [FR-QM-19]), each list in computed order: `rank` is the 1-based list
    /// position, so the read's `ORDER BY rank` returns the list exactly as
    /// computed.
    ///
    /// [FR-QM-15]: ../../../docs/specs/requirements/FR-QM-15.md
    /// [FR-QM-19]: ../../../docs/specs/requirements/FR-QM-19.md
    fn insert_snapshot_offenders(&self, table: &'static str, id: i64, offenders: &WorstOffenders) -> Result<()> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "INSERT INTO {table} \
             (snapshot_id, dimension, rank, name, file, line, detail, node_id, measure, value, threshold, partial_parse) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
        ))?;
        for (dimension, list) in offenders.lists() {
            for (rank, o) in (1_i64..).zip(list) {
                stmt.execute(rusqlite::params![
                    id,
                    dimension,
                    rank,
                    o.name,
                    (!o.file.is_empty()).then_some(o.file.as_str()),
                    o.line,
                    o.detail,
                    o.node_id,
                    o.measure,
                    o.value,
                    o.threshold,
                    o.partial_parse,
                ])
                .context("inserting a snapshot offender row")?;
            }
        }
        Ok(())
    }

    /// Append the "where to start" rows of snapshot `id` ([FR-QM-17]), each in
    /// computed order: `rank` is the 1-based position, so the read's `ORDER BY
    /// rank` returns the lists exactly as computed. A list column is
    /// comma-joined in its vocabulary's order, `''` when empty.
    ///
    /// [FR-QM-17]: ../../../docs/specs/requirements/FR-QM-17.md
    fn insert_snapshot_insight(&self, id: i64, insight: &WhereToStart) -> Result<()> {
        let mut units = self.conn.prepare_cached(
            "INSERT INTO metric_snapshot_fix_units \
             (snapshot_id, rank, kind, node_id, name, file, line, dimensions, points, detail, outliers, partial_parse) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        )?;
        for (rank, u) in (1_i64..).zip(&insight.units) {
            units
                .execute(rusqlite::params![
                    id,
                    rank,
                    u.kind.as_str(),
                    u.node_id,
                    u.name,
                    (!u.file.is_empty()).then_some(u.file.as_str()),
                    u.line,
                    u.dimensions.join(","),
                    u.points,
                    u.detail,
                    u.outliers.join(","),
                    u.partial_parse,
                ])
                .context("inserting a snapshot fix-unit row")?;
        }
        let mut measures = self.conn.prepare_cached(
            "INSERT INTO metric_snapshot_outlier_measures \
             (snapshot_id, measure, units, fence, threshold, labelled) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        let mut labels = self.conn.prepare_cached(
            "INSERT INTO metric_snapshot_outlier_labels \
             (snapshot_id, measure, rank, node_id, name, file, line, value) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for m in &insight.outliers {
            measures
                .execute(rusqlite::params![
                    id,
                    m.measure,
                    m.units as i64,
                    m.fence,
                    m.threshold,
                    m.labelled as i64,
                ])
                .context("inserting a snapshot outlier measure row")?;
            for (rank, l) in (1_i64..).zip(&m.labels) {
                labels
                    .execute(rusqlite::params![
                        id,
                        m.measure,
                        rank,
                        l.node_id,
                        l.name,
                        (!l.file.is_empty()).then_some(l.file.as_str()),
                        l.line,
                        l.value,
                    ])
                    .context("inserting a snapshot outlier label row")?;
            }
        }
        Ok(())
    }

    // ── Governance primitives (S-020, [FR-GV-01..05]) ────────────────────────

    /// Upsert the gate baseline for `scope` ([FR-GV-04]) — a first-run
    /// `session_start` and the gate's announced auto-re-baseline write it. It
    /// overwrites the previous anchor and records no re-bless: a `reason` or
    /// `attribution` the previous one carried is cleared with it
    /// ([`record_baseline`](Self::record_baseline)).
    ///
    /// # Errors
    /// Returns an error if `snapshot_id` does not exist or I/O fails.
    ///
    /// [FR-GV-04]: ../../../docs/specs/requirements/FR-GV-04.md
    pub fn upsert_baseline(&self, scope: &str, snapshot_id: i64, created_at: i64) -> Result<()> {
        self.record_baseline(scope, snapshot_id, created_at, &BaselineRecord::default())
    }

    /// Upsert the gate baseline for `scope` with what its re-bless records
    /// ([FR-GV-16], migration 39): the `--reason` and the attribution of the
    /// change it blesses, each `NULL` when absent. Both are replaced with the
    /// anchor, never kept from the one before.
    ///
    /// # Errors
    /// Returns an error if `snapshot_id` does not exist or I/O fails.
    ///
    /// [FR-GV-16]: ../../../docs/specs/requirements/FR-GV-16.md
    pub fn record_baseline(
        &self,
        scope: &str,
        snapshot_id: i64,
        created_at: i64,
        record: &BaselineRecord,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO baseline (scope, snapshot_id, created_at, reason, attribution) \
                 VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(scope) DO UPDATE SET snapshot_id = ?2, created_at = ?3, \
                 reason = ?4, attribution = ?5",
                rusqlite::params![scope, snapshot_id, created_at, record.reason, record.attribution],
            )
            .context("upserting the gate baseline")?;
        Ok(())
    }

    /// Replace the persisted violation set wholesale **and** record the run
    /// marker that describes it ([FR-GV-02], SRS §5.1 "written per check_rules
    /// run"; [FR-GV-21]) — the same clear-then-rematerialise idempotence as the
    /// derived policy graph (BR-12).
    ///
    /// The two writes are one call, not two, because they must describe the
    /// **same run**: `ran_at` is stamped on every row as its `created_at` and
    /// `violation_count` is counted from the rows actually written, so neither
    /// can drift from the other. A [`BatchWriter`] is already scoped to one
    /// transaction ([`SqliteGraphStore::write_batch`]), so an interrupted run
    /// leaves neither a marker without its rows nor rows without a marker
    /// ([NFR-RA-07]).
    ///
    /// `run.commit_sha` is `HEAD` **at write time** — it answers "has the tree
    /// moved since", not "which commit introduced these findings". `None` when
    /// `HEAD` does not resolve; a placeholder would be a lie a consumer cannot
    /// detect. See [`CheckRunRow::commit_sha`].
    ///
    /// Since S-437 ([CR-140] §3.1) the marker also carries the **evaluated
    /// set** — how many rules ran, whether a contract was authored, and which
    /// operation wrote the row. Those fields ride the same single upsert as
    /// the count rather than a follow-up `UPDATE`, and [`NewCheckRun`] makes
    /// them mandatory: there is no way to record a run without recording what
    /// it evaluated, which is the partially-populated marker [CR-140] CRA-07
    /// forbids. (The rollback half of CRA-07 is the enclosing write job's —
    /// one job is one transaction, [NFR-RA-07] — and holds for any number of
    /// statements; what one statement plus a mandatory parameter adds is that
    /// no code path can populate the marker by halves in the first place.)
    ///
    /// # Errors
    /// Returns an error on a constraint violation or I/O failure. A
    /// `run.operation` outside [`CHECK_RUN_OP_CHECK`] / [`CHECK_RUN_OP_SCAN`]
    /// is one such constraint violation, refused by migration 21's `CHECK`.
    ///
    /// [CR-140]: ../../../docs/requests/CR-140-the-recorded-check-marker-carries-what-it-evaluated.md
    /// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
    /// [FR-GV-21]: ../../../docs/specs/requirements/FR-GV-21.md
    /// [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
    pub fn replace_violations(
        &self,
        violations: &[NewViolation<'_>],
        run: NewCheckRun<'_>,
    ) -> Result<()> {
        self.conn
            .execute("DELETE FROM violations", [])
            .context("clearing the previous violation set")?;
        let mut stmt = self.conn.prepare_cached(
            "INSERT INTO violations \
             (snapshot_id, rule_type, rule_key, node_id, file, message, severity, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        let mut written: i64 = 0;
        for v in violations {
            stmt.execute(rusqlite::params![
                v.snapshot_id,
                v.rule_type,
                v.rule_key,
                v.node_id,
                v.file,
                v.message,
                v.severity,
                run.ran_at,
            ])
            .context("inserting a violation row")?;
            written += 1;
        }
        // Upserted, never appended (BR-40): the singleton records the last run.
        // `CHECK (id = 1)` in migration 20 makes that structural — this ON
        // CONFLICT is how the row is kept current, not how it is kept single.
        //
        // One statement, six values. The evaluated set (S-437) is written here
        // rather than by a follow-up UPDATE precisely so there is no window in
        // which the row carries a count without its denominator (CR-140
        // CRA-07); every field of the marker arrives or none of it does.
        self.conn
            .execute(
                "INSERT INTO check_run \
                 (id, ran_at, commit_sha, violation_count, checked_rules, rules_present, operation) \
                 VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6) \
                 ON CONFLICT(id) DO UPDATE SET ran_at = ?1, commit_sha = ?2, violation_count = ?3, \
                                               checked_rules = ?4, rules_present = ?5, operation = ?6",
                rusqlite::params![
                    run.ran_at,
                    run.commit_sha,
                    written,
                    run.checked_rules,
                    run.rules_present,
                    run.operation,
                ],
            )
            .context("recording the check-run marker")?;
        Ok(())
    }

    /// Run the FTS5 external-content integrity check on the writer
    /// connection (S-020 `health`): the `'integrity-check'` special command
    /// is an INSERT, so the read-only pool cannot issue it. A desync is the
    /// [NFR-RA-09] Correctness fault the `health` tool exists to surface.
    ///
    /// # Errors
    /// Returns an error when the FTS index disagrees with `nodes`.
    ///
    /// [NFR-RA-09]: ../../../docs/specs/requirements/NFR-RA-09.md
    pub fn fts_integrity_check(&self) -> Result<()> {
        self.conn
            .execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES('integrity-check');")
            .context("FTS5 integrity check failed — index desynced (NFR-RA-09)")
    }

    /// Write the singleton `rules_cache` row ([FR-GV-01]): called only when
    /// the `rules.toml` content hash changed.
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    ///
    /// [FR-GV-01]: ../../../docs/specs/requirements/FR-GV-01.md
    pub fn set_rules_cache(
        &self,
        rules_hash: &str,
        parsed_json: &str,
        updated_at: i64,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO rules_cache (id, rules_hash, parsed_json, updated_at) \
                 VALUES (1, ?1, ?2, ?3) \
                 ON CONFLICT(id) DO UPDATE SET rules_hash = ?1, parsed_json = ?2, updated_at = ?3",
                rusqlite::params![rules_hash, parsed_json, updated_at],
            )
            .context("writing the rules cache")?;
        Ok(())
    }
}

// ── Row-writer primitives (work on any &Connection: a store conn or a tx) ─────

fn insert_file(
    conn: &Connection,
    path: &str,
    language: Option<&str>,
    content_hash: Option<&str>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO files (path, language, content_hash) VALUES (?1, ?2, ?3)",
        rusqlite::params![path, language, content_hash],
    )
    .context("inserting file")?;
    Ok(conn.last_insert_rowid())
}

fn upsert_symbol(conn: &Connection, symbol: &LogosSymbol) -> Result<i64> {
    // Idempotent: the UNIQUE(symbol) index makes the second insert a no-op, then
    // we read the (existing or freshly inserted) id back.
    conn.execute(
        "INSERT INTO symbols (symbol) VALUES (?1) ON CONFLICT(symbol) DO NOTHING",
        [symbol.as_str()],
    )
    .context("upserting symbol")?;
    conn.query_row(
        "SELECT id FROM symbols WHERE symbol = ?1",
        [symbol.as_str()],
        |row| row.get(0),
    )
    .context("reading symbol id")
}

/// The `INSERT INTO nodes` statement over every extraction-owned column,
/// completed by an `ON CONFLICT` clause — one column list for
/// [`insert_node`]'s upsert and [`insert_new_node`]'s refusal, so the two can
/// never write different columns.
macro_rules! node_insert_sql {
    ($on_conflict:literal) => {
        concat!(
            "INSERT INTO nodes (symbol_id, kind, name, file_id, start_line, end_line, \
                                derived, exported, cyclomatic_complexity, line_count, fingerprint, \
                                test_evidence, body, max_nesting_depth, has_body, body_tokens, \
                                self_type, param_min, param_max, takes_self, receiver_mode, variants, signature, \
                                uninherited, type_arity) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25) ",
            $on_conflict
        )
    };
}

/// Run a [`node_insert_sql!`] statement for `node`, returning the rows it
/// inserted or updated.
fn execute_node_insert(conn: &Connection, node: &NewNode<'_>, sql: &str) -> rusqlite::Result<usize> {
    conn.execute(
        sql,
        rusqlite::params![
            node.symbol_id,
            node.kind.as_i32(),
            node.name,
            node.file_id,
            node.start_line,
            node.end_line,
            i64::from(node.derived),
            i64::from(node.exported),
            node.cyclomatic_complexity,
            node.line_count,
            node.fingerprint,
            i64::from(node.test_evidence),
            node.body,
            node.max_nesting_depth,
            node.has_body.map(i64::from),
            node.body_tokens,
            node.self_type,
            node.params.map(|p| p.min),
            node.params.and_then(|p| p.max),
            node.takes_self.map(i64::from),
            node.receiver_mode.map(ReceiverMode::as_i32),
            node.variants,
            node.signature.then_some(1i64),
            node.uninherited.then_some(1i64),
            node.type_arity,
        ],
    )
}

/// Insert a node only if no node holds its symbol ([BR-74], S-703): `None`
/// when one does, leaving that node untouched. `last_insert_rowid()` is exact
/// here because the statement either inserted this row or changed nothing.
///
/// [BR-74]: ../../../docs/specs/software-spec.md#35-extraction--symbol-ids
fn insert_new_node(conn: &Connection, node: &NewNode<'_>) -> Result<Option<NodeId>> {
    let inserted = execute_node_insert(conn, node, node_insert_sql!("ON CONFLICT(symbol_id) DO NOTHING"))
        .context("inserting node")?;
    Ok((inserted > 0).then(|| NodeId(conn.last_insert_rowid())))
}

/// Idempotent over `symbol_id` ([FR-SY-10], [NFR-RA-13], [ADR-46]): the
/// `UNIQUE(symbol_id)` constraint (S-201) makes a second node for an
/// already-present symbol a conflict rather than a duplicate row, and the
/// `DO UPDATE` folds the fresh extraction fields into the existing row in
/// place. The per-file delete-then-insert ([FR-SY-01]) is the primary
/// replace-in-place mechanism for a re-extracted file; this upsert is the
/// backstop that keeps `insert_node` itself safe to call again for a symbol
/// that was never deleted (a caller that re-materialises a node it owns
/// without a prior delete). A file's extracted facts never take this path:
/// they go through [`insert_new_node`], which refuses a symbol another node
/// holds instead of folding into it ([BR-74], S-703).
///
/// `last_insert_rowid()` only reflects the most recent INSERT, not an UPDATE,
/// so it cannot be trusted on the `DO UPDATE` path (SQLite docs). The id is
/// instead read back explicitly via `symbol_id`, mirroring the
/// [`upsert_symbol`] idiom.
///
/// The `SET` list covers exactly [`NewNode`]'s extraction-owned columns.
/// Annotation-owned columns (`is_dead`, `is_duplicate`, `layer_membership`,
/// `is_test`, `clone_group`) are deliberately absent, so a re-extraction never
/// wipes a prior annotation pass's verdict out from under it.
///
/// [FR-SY-01]: ../../../docs/specs/requirements/FR-SY-01.md
/// [FR-SY-10]: ../../../docs/specs/requirements/FR-SY-10.md
/// [NFR-RA-13]: ../../../docs/specs/requirements/NFR-RA-13.md
/// [ADR-46]: ../../../docs/specs/architecture/decisions/ADR-46.md
/// [BR-74]: ../../../docs/specs/software-spec.md#35-extraction--symbol-ids
fn insert_node(conn: &Connection, node: &NewNode<'_>) -> Result<NodeId> {
    execute_node_insert(
        conn,
        node,
        node_insert_sql!(
            "ON CONFLICT(symbol_id) DO UPDATE SET \
             kind = excluded.kind, \
             name = excluded.name, \
             file_id = excluded.file_id, \
             start_line = excluded.start_line, \
             end_line = excluded.end_line, \
             derived = excluded.derived, \
             exported = excluded.exported, \
             cyclomatic_complexity = excluded.cyclomatic_complexity, \
             line_count = excluded.line_count, \
             fingerprint = excluded.fingerprint, \
             test_evidence = excluded.test_evidence, \
             body = excluded.body, \
             max_nesting_depth = excluded.max_nesting_depth, \
             has_body = excluded.has_body, \
             body_tokens = excluded.body_tokens, \
             self_type = excluded.self_type, \
             param_min = excluded.param_min, \
             param_max = excluded.param_max, \
             takes_self = excluded.takes_self, \
             receiver_mode = excluded.receiver_mode, \
             variants = excluded.variants, \
             signature = excluded.signature, \
             uninherited = excluded.uninherited, \
             type_arity = excluded.type_arity"
        ),
    )
    .context("upserting node")?;
    conn.query_row(
        "SELECT id FROM nodes WHERE symbol_id = ?1",
        [node.symbol_id],
        |row| row.get(0),
    )
    .map(NodeId)
    .context("reading node id back by symbol_id")
}

fn insert_edge(conn: &Connection, source: NodeId, target: NodeId, kind: EdgeKind) -> Result<()> {
    conn.execute(
        "INSERT INTO edges (source, target, kind) VALUES (?1, ?2, ?3)",
        rusqlite::params![source.get(), target.get(), kind.as_i32()],
    )
    .context("inserting edge")?;
    Ok(())
}

fn delete_node(conn: &Connection, id: NodeId) -> Result<usize> {
    conn.execute("DELETE FROM nodes WHERE id = ?1", [id.get()])
        .context("deleting node")
}

/// Rewrite every non-NULL symbol in `table.column` (rows narrowed by the
/// `filter` SQL fragment, `""` for all) to `coordinate`, returning how many
/// values changed — the per-column step of
/// [`BatchWriter::migrate_symbol_coordinate`]. `table`, `column` and `filter`
/// are this module's own literals, never input.
fn rewrite_symbol_column(
    conn: &Connection,
    table: &str,
    column: &str,
    filter: &str,
    coordinate: &str,
) -> Result<u64> {
    let rows: Vec<(i64, String)> = conn
        .prepare(&format!("SELECT id, {column} FROM {table} WHERE {column} IS NOT NULL {filter}"))?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()
        .with_context(|| format!("reading {table}.{column}"))?;
    let mut update = conn.prepare(&format!("UPDATE {table} SET {column} = ?2 WHERE id = ?1"))?;
    let mut rewritten = 0;
    for (id, raw) in rows {
        let moved = LogosSymbol::parse(&raw)
            .and_then(|symbol| symbol.with_coordinate(Some(coordinate)))
            .with_context(|| format!("{table}.{column} row {id} holds {raw:?}, which is not a symbol"))?;
        if moved.as_str() != raw {
            update.execute(rusqlite::params![id, moved.as_str()]).with_context(|| {
                format!("rewriting {table}.{column} row {id} to {moved}: another row already holds it")
            })?;
            rewritten += 1;
        }
    }
    Ok(rewritten)
}

/// The raw column tuple read from a node-shaped row, before model conversion.
type RawNodeRow = (
    i64,
    String,
    i32,
    String,
    Option<String>,
    Option<i64>,
    Option<i64>,
);

/// Map a SQL row to the raw column tuple. Total over the SQL types, so it stays
/// inside `rusqlite::Result`; the fallible model conversion happens in
/// [`raw_to_node`].
fn map_raw_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawNodeRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}

/// Convert a raw row to a [`NodeRow`], validating the stored symbol and kind.
///
/// A discriminant or symbol that fails to map back is an integrity error — the
/// store this binary wrote should never contain one — surfaced with guidance to
/// rebuild ([NFR-RA-08]).
fn raw_to_node(
    (id, symbol, kind, name, file_path, start_line, end_line): RawNodeRow,
) -> Result<NodeRow> {
    let symbol = LogosSymbol::parse(&symbol).with_context(|| {
        format!("corrupt symbol stored for node {id}; rebuild advised (NFR-RA-08)")
    })?;
    let kind = NodeKind::try_from(kind).map_err(|e| {
        anyhow!("corrupt node kind {kind} for node {id}: {e}; rebuild advised (NFR-RA-08)")
    })?;
    Ok(NodeRow {
        id: NodeId(id),
        symbol,
        kind,
        name,
        file_path,
        start_line,
        end_line,
    })
}

/// Writer page cache: `-65536` = 64 MiB (negative = KiB, SQLite convention).
/// Larger than the 2 MiB default so the insert-heavy Pass-1 persist and the
/// whole-graph reads across passes 2–5 spill to disk far less; bounded well
/// under the ≤1 GB indexing ceiling ([NFR-PE-06]) — one writer connection, so
/// this 64 MiB is paid once, not per reader (CR-057).
const WRITER_CACHE_SIZE_KIB: i64 = -65_536;

/// Memory-mapped I/O window for the writer: 256 MiB. Serves whole-graph page
/// reads/inserts straight from the mapped file instead of `read`/`write`
/// syscalls + buffer copies, cutting page churn ([FR-DB-02]). The mapping is
/// file-backed and reclaimable (it does not pin 256 MiB of anonymous RSS), and
/// the on-disk store stays far below this window within the ~100k-LOC envelope,
/// so it holds under [NFR-PE-06]. Inert on `:memory:` stores (no file to map).
const WRITER_MMAP_SIZE_BYTES: i64 = 268_435_456;

/// `temp_store = MEMORY` (2): SQLite keeps transient b-trees (index builds,
/// sorters) in RAM rather than spilling temp files during the bulk load.
const TEMP_STORE_MEMORY: i64 = 2;

/// Apply the per-connection pragma contract ([FR-DB-02], [NFR-RA-10]).
///
/// `execute_batch` runs the base pragmas and discards the row `journal_mode`
/// returns. `foreign_keys` must be set here, on every connection, before any
/// transaction — it defaults OFF (the [ADR-05] footgun).
///
/// This is the **writer** connection's configuration path ([`open`](SqliteGraphStore::open)
/// / [`open_in_memory`](SqliteGraphStore::open_in_memory) → [`from_connection`](SqliteGraphStore::from_connection);
/// readers use [`configure_readonly_connection`]). The writer additionally sets
/// the bulk-load pragmas ([FR-DB-02], CR-057) — a larger page cache, a
/// memory-mapped I/O window, and in-memory temp storage — to cut page churn on
/// the cold-index write path and the whole-graph reads, bounded by the ≤1 GB
/// indexing RSS ceiling ([NFR-PE-06]). They are graph-output-neutral
/// ([NFR-RA-06]): pragmas change how bytes move to/from disk, never what is
/// stored or read back.
fn configure_connection(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;\n\
         PRAGMA foreign_keys = ON;\n\
         PRAGMA synchronous = NORMAL;",
    )
    .context("applying the per-connection pragma contract (FR-DB-02)")?;
    // Bulk-load pragmas on the writer connection (FR-DB-02, CR-057), each
    // bounded under NFR-PE-06 (see the constants' docs). Applied via
    // `pragma_update` (not `execute_batch`) so a bad value surfaces as an error
    // rather than a silently-ignored statement.
    conn.pragma_update(None, "cache_size", WRITER_CACHE_SIZE_KIB)
        .context("setting the writer cache_size bulk-load pragma (FR-DB-02)")?;
    conn.pragma_update(None, "mmap_size", WRITER_MMAP_SIZE_BYTES)
        .context("setting the writer mmap_size bulk-load pragma (FR-DB-02)")?;
    conn.pragma_update(None, "temp_store", TEMP_STORE_MEMORY)
        .context("setting the writer temp_store bulk-load pragma (FR-DB-02)")?;
    // Writers serialize via this timeout, then fail gracefully rather than
    // blocking readers (NFR-RA-10).
    conn.busy_timeout(Duration::from_secs(5))
        .context("setting busy_timeout")?;
    Ok(())
}

/// Apply the **read-only** per-connection contract for a WAL reader ([ADR-02]).
///
/// Deliberately omits `PRAGMA journal_mode = WAL`: switching journal mode takes
/// a write lock a read-only connection does not hold, and the file is already in
/// WAL (the writer set it on first open). `foreign_keys`/`synchronous` are
/// connection-level settings that touch no data, and `query_only = ON` makes the
/// read-only contract structural — any stray write on a pool connection fails at
/// the SQL layer rather than silently contending with the writer.
///
/// The bulk-load pragmas (CR-057) are **not** applied here: [FR-DB-02] scopes
/// them to the writer connection, and the [S-225] cold-index benchmark showed a
/// read-side memory-map window yields no measurable pass-2–5 read-time reduction
/// — those reads hit just-persisted, OS-page-cache-hot pages and are dominated
/// by CPU-side row deserialization, which `mmap` does not touch. Keeping the
/// readers on the base contract holds the pool strictly within [NFR-PE-06] (no
/// per-reader heap cache) and keeps the code aligned with the requirement's
/// writer-only wording.
///
/// [ADR-02]: ../../../docs/specs/architecture/decisions/ADR-02.md
/// [FR-DB-02]: ../../../docs/specs/requirements/FR-DB-02.md
/// [NFR-PE-06]: ../../../docs/specs/requirements/NFR-PE-06.md
/// [S-225]: ../../../docs/planning/journal.md#s-225-per-phase-index-instrumentation-and-repeatable-cold-index-benchmark
fn configure_readonly_connection(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;\n\
         PRAGMA synchronous = NORMAL;\n\
         PRAGMA query_only = ON;",
    )
    .context("applying the read-only connection contract (FR-DB-02)")?;
    // Match the writer's grace window so a checkpoint-induced brief contention
    // resolves rather than erroring instantly (NFR-RA-10).
    conn.busy_timeout(Duration::from_secs(5))
        .context("setting busy_timeout")?;
    Ok(())
}

// ── Worktree seeding (S-021, ADR-15) ────────────────────────────────────────

/// Copy a primary checkout's canonical store to `dst_db` as a worktree seed —
/// the graph-store "seed/copy" library seam consumed by the api-facade
/// ([ADR-15], [FR-WT-03]).
///
/// Copies the single-file DB (the [NFR-DM-03] copyability contract) **plus**
/// its `-wal` sidecar when present, so commits a live primary has not yet
/// checkpointed replay when the copy first opens. The `-shm` index is never
/// copied (SQLite rebuilds it), and nothing else in the source `.logos/`
/// travels — derived sibling state like `telemetry.db` stays put
/// ([NFR-DM-04]).
///
/// Best-effort against a live writer: the db+wal pair is copied at one moment;
/// a checkpoint racing the copy can at worst lose the very latest commits,
/// which the caller's diff-reconcile / reconcile backstop recovers ([ADR-11]).
///
/// # Errors
/// Returns an error if the destination directory cannot be created or either
/// file copy fails. On error the destination db+wal pair is removed — a failed
/// seed leaves NO partial artifacts a later fresh store could mispair with.
/// The caller treats a failed seed as "no seed" and falls back to a fresh
/// store + full index.
///
/// [ADR-11]: ../../../docs/specs/architecture/decisions/ADR-11.md
/// [ADR-15]: ../../../docs/specs/architecture/decisions/ADR-15.md
/// [FR-WT-03]: ../../../docs/specs/requirements/FR-WT-03.md
/// [NFR-DM-03]: ../../../docs/specs/requirements/NFR-DM-03.md
/// [NFR-DM-04]: ../../../docs/specs/requirements/NFR-DM-04.md
pub fn seed_copy(src_db: &Path, dst_db: &Path) -> Result<()> {
    let result = try_seed_copy(src_db, dst_db);
    if result.is_err() {
        // The pair is written together and cleaned together: stranding a
        // partial `logos.db` OR an orphan `-wal` would let a later fresh
        // store pair with foreign state.
        let _ = std::fs::remove_file(dst_db);
        let _ = std::fs::remove_file(wal_sidecar(dst_db));
    }
    result
}

/// Fallible body of [`seed_copy`]; the wrapper owns failure cleanup.
fn try_seed_copy(src_db: &Path, dst_db: &Path) -> Result<()> {
    if let Some(parent) = dst_db.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating the seed destination {}", parent.display()))?;
    }
    std::fs::copy(src_db, dst_db).with_context(|| {
        format!(
            "seeding {} from the primary store {}",
            dst_db.display(),
            src_db.display()
        )
    })?;

    let src_wal = wal_sidecar(src_db);
    let dst_wal = wal_sidecar(dst_db);
    if src_wal.is_file() {
        std::fs::copy(&src_wal, &dst_wal)
            .with_context(|| format!("copying the WAL sidecar {}", src_wal.display()))?;
    } else {
        // Never pair the fresh copy with a stale leftover WAL.
        let _ = std::fs::remove_file(&dst_wal);
    }
    Ok(())
}

/// The `-wal` sidecar path for a database file (`logos.db` → `logos.db-wal`).
fn wal_sidecar(db: &Path) -> std::path::PathBuf {
    let mut os = db.as_os_str().to_os_string();
    os.push("-wal");
    std::path::PathBuf::from(os)
}

#[cfg(test)]
mod tests;
