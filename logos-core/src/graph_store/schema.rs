//! Embedded, forward-only schema migrations ([FR-DB-04], [NFR-MA-06]).
//!
//! Migrations are a `&[(version, &str)]` slice baked into the binary — never
//! read from disk ([FR-DB-04]). The migration runner ([`super::migrate`])
//! applies, in one transaction each, every entry whose `version` is greater
//! than the database's current `PRAGMA user_version`, recording each in the
//! `schema_versions` audit table.
//!
//! # The frozen-string rule
//!
//! Once a `(version, sql)` pair has shipped it is **immutable**: editing the
//! SQL of an already-applied migration would silently diverge fresh databases
//! from upgraded ones. Schema changes are *new* tuples with the next version,
//! never edits to old ones. This is the whole point of forward-only migrations
//! ([NFR-MA-06]).
//!
//! # Discriminant contract
//!
//! The `nodes.kind` / `edges.kind` `CHECK` lists below are the on-disk half of
//! the discriminant contract frozen in [`crate::model`] ([NodeKind] 1..=34,
//! [EdgeKind] 1..=15). The `schema_check_matches_model_ontology` test in
//! [`super`] asserts these lists equal `NodeKind::ALL` / `EdgeKind::ALL`, so the
//! schema can never silently drift from the model it guards ([FR-DB-01]).
//!
//! [FR-DB-01]: ../../../../docs/specs/requirements/FR-DB-01.md
//! [FR-DB-04]: ../../../../docs/specs/requirements/FR-DB-04.md
//! [NFR-MA-06]: ../../../../docs/specs/requirements/NFR-MA-06.md
//! [NodeKind]: crate::model::NodeKind
//! [EdgeKind]: crate::model::EdgeKind

/// The forward-only migration ledger: `(version, sql)` applied in order.
///
/// `version` is dense and 1-based; `v1 = migration 1` ([FR-DB-04]). Append new
/// migrations here with the next integer — never edit a shipped entry.
pub(crate) const MIGRATIONS: &[(i64, &str)] = &[
    (1, MIGRATION_1),
    (2, MIGRATION_2),
    (3, MIGRATION_3),
    (4, MIGRATION_4),
    (5, MIGRATION_5),
    (6, MIGRATION_6),
    (7, MIGRATION_7),
    (8, MIGRATION_8),
    (9, MIGRATION_9),
    (10, MIGRATION_10),
    (11, MIGRATION_11),
    (12, MIGRATION_12),
    (13, MIGRATION_13),
    (14, MIGRATION_14),
    (15, MIGRATION_15),
    (16, MIGRATION_16),
];

/// Migration 1 — the canonical graph-store schema ([FR-DB-01]).
///
/// Establishes the system-of-record tables (`files`, `symbols`, `nodes`,
/// `edges`), the FTS5 external-content search index (`nodes_fts`) with its
/// sync triggers ([FR-DB-03]), the hot-path edge indexes, and the
/// `schema_versions` audit table. Analytics/governance tables
/// (`annotations` — owned by the annotation-engine work, its columns defined by
/// [FR-AN-04]; `metric_snapshots`, `baseline`, `violations`, `rules_cache`,
/// `unresolved_refs`, `project_metadata`) are deferred to their owning stories
/// — they have no Sprint-1 consumer and their columns are defined by the
/// metrics/governance work, so a later forward-only migration adds them.
///
/// [FR-AN-04]: ../../../../docs/specs/requirements/FR-AN-04.md
const MIGRATION_1: &str = "\
-- schema_versions: the migration audit trail. PRAGMA user_version is the
-- authoritative gate the runner checks on open; this table records the
-- human-auditable history (FR-DB-04).
CREATE TABLE schema_versions (
    version    INTEGER PRIMARY KEY,
    applied_at INTEGER NOT NULL
) STRICT;

-- files: one row per indexed source file.
CREATE TABLE files (
    id           INTEGER PRIMARY KEY,
    path         TEXT NOT NULL UNIQUE,
    language     TEXT,
    content_hash TEXT
) STRICT;

-- symbols: canonical SCIP symbol identities (the LogosSymbol vocabulary).
-- One row per distinct symbol string; nodes reference it by id.
CREATE TABLE symbols (
    id     INTEGER PRIMARY KEY,
    symbol TEXT NOT NULL UNIQUE
) STRICT;

-- nodes: the graph vertices. `kind` is constrained to the 15 frozen NodeKind
-- discriminants (FR-DB-01); 0 is intentionally never valid so a defaulted
-- column can never masquerade as a real kind.
CREATE TABLE nodes (
    id         INTEGER PRIMARY KEY,
    symbol_id  INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
    kind       INTEGER NOT NULL CHECK (kind IN (1,2,3,4,5,6,7,8,9,10,11,12,13,14,15)),
    name       TEXT NOT NULL,
    file_id    INTEGER REFERENCES files(id) ON DELETE SET NULL,
    start_line INTEGER,
    end_line   INTEGER
) STRICT;

CREATE INDEX idx_nodes_symbol_id ON nodes(symbol_id);
CREATE INDEX idx_nodes_kind      ON nodes(kind);

-- edges: the graph relationships. `kind` is constrained to the 10 frozen
-- EdgeKind discriminants. The (source,target,kind) uniqueness rule forbids
-- duplicate parallel edges of the same kind (FR-DB-01). Both endpoints cascade
-- on delete so removing a node never leaves a dangling edge.
CREATE TABLE edges (
    id     INTEGER PRIMARY KEY,
    source INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    target INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    kind   INTEGER NOT NULL CHECK (kind IN (1,2,3,4,5,6,7,8,9,10)),
    UNIQUE (source, target, kind)
) STRICT;

-- Hot-path indexes backing callers/callees/impact point queries
-- (graph-store component: idx_edges_source_kind / idx_edges_target_kind).
CREATE INDEX idx_edges_source_kind ON edges(source, kind);
CREATE INDEX idx_edges_target_kind ON edges(target, kind);

-- nodes_fts: FTS5 external-content index over nodes.name. content='nodes'
-- means no duplicate copy of the text is stored — the index reads names back
-- from `nodes` by rowid. Sync is OUR responsibility, via the triggers below
-- (FR-DB-03, NFR-RA-09). Symbols are exact-lookup (UNIQUE index), not
-- full-text, so only the human-facing `name` is indexed here.
CREATE VIRTUAL TABLE nodes_fts USING fts5(
    name,
    content='nodes',
    content_rowid='id'
);

-- INSERT: mirror the new row into the index.
CREATE TRIGGER nodes_fts_ai AFTER INSERT ON nodes BEGIN
    INSERT INTO nodes_fts(rowid, name) VALUES (new.id, new.name);
END;

-- DELETE: the CRITICAL 'delete' command row. A plain DELETE would leave the
-- inverted index pointing at a stale rowid and the external-content index
-- would silently desync (SRS §7.2/§16.8 trap, NFR-RA-09). The special
-- 'delete' row tells FTS5 to retract the old term postings.
CREATE TRIGGER nodes_fts_ad AFTER DELETE ON nodes BEGIN
    INSERT INTO nodes_fts(nodes_fts, rowid, name) VALUES ('delete', old.id, old.name);
END;

-- UPDATE: retract the old postings (the 'delete' row), then index the new.
CREATE TRIGGER nodes_fts_au AFTER UPDATE ON nodes BEGIN
    INSERT INTO nodes_fts(nodes_fts, rowid, name) VALUES ('delete', old.id, old.name);
    INSERT INTO nodes_fts(rowid, name) VALUES (new.id, new.name);
END;
";

/// Migration 2 — the `unresolved_refs` reference ledger (S-011, [ADR-10],
/// [FR-RS-03]).
///
/// One row per reference extracted from source (a call path, a method-receiver
/// call, an import, a glob import) or captured before a sync delete
/// ([ADR-10] capture-before-delete). The resolution pass (Pass 2) re-evaluates
/// the ledger on every index/sync: a row it can bind to **exactly one**
/// existing node becomes an edge and is flagged `resolved = 1`; everything
/// else stays `resolved = 0` and is retried on the next sync — never
/// fabricated ([NFR-RA-05]). Import rows double as durable per-file scope
/// facts (alias/glob maps), which is why bound rows are flagged rather than
/// deleted; the flag split also yields the exact bound-ratio [FR-RS-04] asks
/// for.
///
/// `form` is the on-disk half of the [`RefForm`] discriminant contract
/// (`crate::model::RefForm`, 1..=4); `kind` reuses the [EdgeKind] list. The
/// `migration_2_form_check_matches_ref_form_ontology` test below guards both
/// against drift.
///
/// `source_symbol` is a stable symbol *string* (not a node FK): node rowids
/// churn on re-extract, canonical symbols don't ([ADR-07]).
///
/// [ADR-10]: ../../../../docs/specs/architecture/decisions/ADR-10.md
/// [ADR-07]: ../../../../docs/specs/architecture/decisions/ADR-07.md
/// [FR-RS-03]: ../../../../docs/specs/requirements/FR-RS-03.md
/// [FR-RS-04]: ../../../../docs/specs/requirements/FR-RS-04.md
/// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
/// [`RefForm`]: crate::model::RefForm
const MIGRATION_2: &str = "\
-- unresolved_refs: the first-class, retried reference ledger (ADR-10) — not an
-- error log. file_id is the file whose (re-)extraction produced the row, so a
-- file removal cascades its refs away and a re-extract replaces them wholesale.
CREATE TABLE unresolved_refs (
    id            INTEGER PRIMARY KEY,
    file_id       INTEGER REFERENCES files(id) ON DELETE CASCADE,
    source_symbol TEXT NOT NULL,
    target        TEXT NOT NULL,
    alias         TEXT,
    form          INTEGER NOT NULL CHECK (form IN (1,2,3,4)),
    kind          INTEGER NOT NULL CHECK (kind IN (1,2,3,4,5,6,7,8,9,10)),
    line          INTEGER,
    resolved      INTEGER NOT NULL DEFAULT 0 CHECK (resolved IN (0,1)),
    UNIQUE (source_symbol, target, form, kind)
) STRICT;

-- Per-file replace on re-extract; resolved-state scans for coverage (FR-RS-04).
CREATE INDEX idx_unresolved_refs_file     ON unresolved_refs(file_id);
CREATE INDEX idx_unresolved_refs_resolved ON unresolved_refs(resolved);
";

/// Migration 3 — native annotation columns, the policy-kind CHECK widening, and
/// the `annotations` view (S-014, [FR-AN-04], [FR-AN-03]).
///
/// The annotation engine (Pass 3) writes its results to **native columns on
/// `nodes`** — no sidecar table ([FR-AN-04]). SQLite cannot widen a `CHECK`
/// constraint in place, so `nodes` is rebuilt copy-style. Row ids are
/// preserved, which keeps the FTS5 external-content index (whose postings key
/// on rowid) aligned without a rebuild. The three FTS sync triggers are
/// dropped first (so the copy cannot double-index) and recreated verbatim
/// afterwards.
///
/// **Why no `ALTER TABLE … RENAME` of `nodes`:** since SQLite 3.25 a rename
/// rewrites every other object's references to follow it — `ALTER TABLE nodes
/// RENAME TO nodes_old` would re-point the `edges` FK clauses at `nodes_old`,
/// and dropping `nodes_old` would then cascade-delete every edge. The
/// `legacy_alter_table` escape hatch is not reliable either: an omitted
/// PRAGMA is silently ignored, so a bundled build without it would corrupt
/// the store with no error. Instead the rebuild never renames a referenced
/// table: edge rows are stashed in a plain holder table, **both** `edges` and
/// `nodes` are dropped (children first, so nothing cascades), the new tables
/// are created under their final names, and the rows are copied back.
/// (`PRAGMA foreign_keys` cannot be toggled here — it is a no-op inside the
/// migration runner's open transaction — so the procedure is designed to be
/// correct under live FK enforcement.)
///
/// New columns:
/// - `nodes.derived` / `edges.derived` — `1` marks a policy node
///   ([`Layer`]/[`Boundary`]) or a derived `forbidden_dependency` edge the
///   annotation engine clears and re-materialises each run ([FR-AN-03]).
/// - `nodes.exported` — declaration visibility captured by Pass 1; the
///   exported-is-live dead-code root set ([FR-AN-01]).
/// - `nodes.cyclomatic_complexity` / `nodes.line_count` — per-function metrics
///   captured by Pass 1 ([FR-AN-04] queryable columns).
/// - `nodes.fingerprint` — the normalised AST-shape fingerprint duplicate
///   detection groups by ([FR-AN-02]).
/// - `nodes.is_dead` / `nodes.is_duplicate` / `nodes.layer_membership` — the
///   Pass-3 annotation results; `NULL` means "not yet annotated", distinct
///   from an honest `0` ([FR-AN-01..03]).
/// - `files.layer` — the file's `[[layers]]` band (first-glob-wins,
///   [FR-AN-03]).
///
/// The `annotations` **view** exposes exactly the queryable shape [FR-AN-04]
/// names — `annotations(node_id, cyclomatic_complexity, line_count, is_dead,
/// is_duplicate, layer_membership)` — while the storage stays native on
/// `nodes` (a view is not a sidecar table; there is nothing to keep in sync).
///
/// Rows indexed before this migration carry `exported = 0` / `fingerprint =
/// NULL` until their next re-extraction; a full `logos index` refreshes them.
///
/// [FR-AN-01]: ../../../../docs/specs/requirements/FR-AN-01.md
/// [FR-AN-02]: ../../../../docs/specs/requirements/FR-AN-02.md
/// [FR-AN-03]: ../../../../docs/specs/requirements/FR-AN-03.md
/// [FR-AN-04]: ../../../../docs/specs/requirements/FR-AN-04.md
/// [`Layer`]: crate::model::NodeKind::Layer
/// [`Boundary`]: crate::model::NodeKind::Boundary
const MIGRATION_3: &str = "\
-- Drop the FTS sync triggers around the rebuild: the copy below must not
-- double-index, and the triggers are recreated verbatim afterwards.
DROP TRIGGER nodes_fts_ai;
DROP TRIGGER nodes_fts_ad;
DROP TRIGGER nodes_fts_au;

-- Stash both tables' rows in plain holders (CTAS: no FKs, no constraints) so
-- the originals can be dropped — child before parent — without losing data.
CREATE TABLE edges_stash AS SELECT id, source, target, kind FROM edges;
CREATE TABLE nodes_stash AS
    SELECT id, symbol_id, kind, name, file_id, start_line, end_line FROM nodes;

-- Children first: dropping edges removes the only FK references to nodes, so
-- the nodes drop below cascades nothing. Both drops are clean under live FK
-- enforcement.
DROP TABLE edges;
DROP TABLE nodes;

-- The rebuilt nodes table: the migration-1 shape plus the widened kind CHECK
-- (policy kinds 16/17) and the native annotation columns (FR-AN-04).
CREATE TABLE nodes (
    id                    INTEGER PRIMARY KEY,
    symbol_id             INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
    kind                  INTEGER NOT NULL CHECK (kind IN (1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17)),
    name                  TEXT NOT NULL,
    file_id               INTEGER REFERENCES files(id) ON DELETE SET NULL,
    start_line            INTEGER,
    end_line              INTEGER,
    derived               INTEGER NOT NULL DEFAULT 0 CHECK (derived IN (0,1)),
    exported              INTEGER NOT NULL DEFAULT 0 CHECK (exported IN (0,1)),
    cyclomatic_complexity INTEGER,
    line_count            INTEGER,
    fingerprint           TEXT,
    is_dead               INTEGER CHECK (is_dead IN (0,1)),
    is_duplicate          INTEGER CHECK (is_duplicate IN (0,1)),
    layer_membership      TEXT
) STRICT;

-- The rebuilt edges table: the migration-1 shape plus the derived flag — 1
-- marks an annotation-materialised edge (the forbidden_dependency flags,
-- FR-AN-03) that is cleared and rebuilt each annotation run.
CREATE TABLE edges (
    id      INTEGER PRIMARY KEY,
    source  INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    target  INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    kind    INTEGER NOT NULL CHECK (kind IN (1,2,3,4,5,6,7,8,9,10)),
    derived INTEGER NOT NULL DEFAULT 0 CHECK (derived IN (0,1)),
    UNIQUE (source, target, kind)
) STRICT;

-- Copy nodes back with identical rowids so the restored edges' endpoints and
-- the FTS postings stay valid; the new columns take their defaults
-- (un-annotated state). Parents before children so FK checks hold throughout.
INSERT INTO nodes (id, symbol_id, kind, name, file_id, start_line, end_line)
    SELECT id, symbol_id, kind, name, file_id, start_line, end_line FROM nodes_stash;
INSERT INTO edges (id, source, target, kind)
    SELECT id, source, target, kind FROM edges_stash;

DROP TABLE nodes_stash;
DROP TABLE edges_stash;

-- The migration-1 indexes were dropped with the old tables; recreate them.
CREATE INDEX idx_nodes_symbol_id   ON nodes(symbol_id);
CREATE INDEX idx_nodes_kind        ON nodes(kind);
CREATE INDEX idx_edges_source_kind ON edges(source, kind);
CREATE INDEX idx_edges_target_kind ON edges(target, kind);

-- Recreate the FTS sync triggers verbatim (migration 1, FR-DB-03, NFR-RA-09).
CREATE TRIGGER nodes_fts_ai AFTER INSERT ON nodes BEGIN
    INSERT INTO nodes_fts(rowid, name) VALUES (new.id, new.name);
END;
CREATE TRIGGER nodes_fts_ad AFTER DELETE ON nodes BEGIN
    INSERT INTO nodes_fts(nodes_fts, rowid, name) VALUES ('delete', old.id, old.name);
END;
CREATE TRIGGER nodes_fts_au AFTER UPDATE ON nodes BEGIN
    INSERT INTO nodes_fts(nodes_fts, rowid, name) VALUES ('delete', old.id, old.name);
    INSERT INTO nodes_fts(rowid, name) VALUES (new.id, new.name);
END;

-- Layer membership of a file under the rules.toml [[layers]] globs (FR-AN-03).
ALTER TABLE files ADD COLUMN layer TEXT;

-- The FR-AN-04 queryable shape over the native columns. A view, not a table:
-- annotations live on nodes, there is no sidecar to keep in sync.
CREATE VIEW annotations AS
    SELECT id AS node_id,
           cyclomatic_complexity,
           line_count,
           is_dead,
           is_duplicate,
           layer_membership
    FROM nodes;
";

/// Migration 4 — the `metric_snapshots` quality-signal ledger (S-018,
/// [FR-QM-07], [ADR-12]).
///
/// One row per aggregate metrics run, **append-only** (the metrics-engine
/// component owns this table and never mutates a past snapshot — evolution
/// reads the series). Each row persists the raw + normalized value of all five
/// metrics, the graph counts the run scored, the `empty` honesty flag, and the
/// rounded 0–10000 aggregate signal ([ADR-08]):
///
/// - `*_raw` — the pre-normalization quantity (Newman Q, cycle count, condensed
///   longest-path depth, Gini coefficient, redundant-function ratio), so a
///   signal regression is explainable per dimension ([FR-QM-07]).
/// - `*_normalized` — the [0,1] value entering the geometric mean.
/// - `aggregate_signal` — the rounded integer signal; **`NULL` when `empty = 1`**
///   (the [ADR-12] empty-graph sentinel: an empty graph is "n/a", never a
///   misleading ~8033). A `0` here is the real zero short-circuit, distinct
///   from `NULL`.
/// - `commit_sha` — optional VCS pin for the snapshot ([FR-QM-07]).
///
/// REAL columns store IEEE-754 doubles exactly, so a re-read returns the bytes
/// the canonical-order reduction produced ([NFR-RA-06]); golden tests assert
/// the rounded `aggregate_signal`, not intermediate floats ([ADR-08]).
///
/// [FR-QM-07]: ../../../../docs/specs/requirements/FR-QM-07.md
/// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
/// [ADR-08]: ../../../../docs/specs/architecture/decisions/ADR-08.md
/// [ADR-12]: ../../../../docs/specs/architecture/decisions/ADR-12.md
const MIGRATION_4: &str = "\
-- metric_snapshots: the append-only quality-signal ledger (FR-QM-07, ADR-12).
-- Owned by the metrics-engine; a past snapshot is never mutated.
CREATE TABLE metric_snapshots (
    id                    INTEGER PRIMARY KEY,
    created_at            INTEGER NOT NULL,
    commit_sha            TEXT,
    node_count            INTEGER NOT NULL,
    edge_count            INTEGER NOT NULL,
    function_count        INTEGER NOT NULL,
    empty                 INTEGER NOT NULL DEFAULT 0 CHECK (empty IN (0,1)),
    modularity_raw        REAL NOT NULL,
    modularity_normalized REAL NOT NULL,
    acyclicity_raw        REAL NOT NULL,
    acyclicity_normalized REAL NOT NULL,
    depth_raw             REAL NOT NULL,
    depth_normalized      REAL NOT NULL,
    equality_raw          REAL NOT NULL,
    equality_normalized   REAL NOT NULL,
    redundancy_raw        REAL NOT NULL,
    redundancy_normalized REAL NOT NULL,
    -- NULL = the empty-graph sentinel ('n/a', ADR-12); 0 = zero short-circuit.
    aggregate_signal      INTEGER CHECK (aggregate_signal BETWEEN 0 AND 10000)
) STRICT;

-- Evolution reads the series in time order (FR-QM-07 consumer, S-020).
CREATE INDEX idx_metric_snapshots_created ON metric_snapshots(created_at);
";

/// Migration 5 — the governance tables (S-020, SRS §5.1).
///
/// `baseline` anchors the session/CI gate (FR-GV-04/05): one row per scope
/// (v1 has exactly one scope, the project), upserted by `session_start` /
/// `gate --save`, pointing at the snapshot the next gate compares against.
/// `violations` records the outcome of each `check_rules` run (replaced
/// wholesale per run — the same idempotence posture as the derived policy
/// graph, BR-12). `rules_cache` is the FR-GV-01 parse cache: the singleton
/// row holds the blake3 hash of `rules.toml` and its parsed JSON, so an
/// unchanged contract skips the TOML parse + validation on re-run.
const MIGRATION_5: &str = "\
-- baseline: the gate's comparison anchor (FR-GV-04, BR-10). One per scope.
CREATE TABLE baseline (
    scope       TEXT PRIMARY KEY,
    snapshot_id INTEGER NOT NULL REFERENCES metric_snapshots(id),
    created_at  INTEGER NOT NULL
) STRICT;

-- violations: the outcome of the last check_rules run (FR-GV-02, SRS §5.1).
-- Replaced wholesale per run; snapshot_id ties a scan's violations to the
-- metric snapshot the same run persisted (NULL for a bare check_rules).
CREATE TABLE violations (
    id          INTEGER PRIMARY KEY,
    snapshot_id INTEGER REFERENCES metric_snapshots(id),
    rule_type   TEXT NOT NULL CHECK (rule_type IN ('constraint','layer','boundary')),
    rule_key    TEXT NOT NULL,
    node_id     INTEGER,
    file        TEXT,
    message     TEXT NOT NULL,
    severity    TEXT NOT NULL CHECK (severity IN ('error','warning')),
    created_at  INTEGER NOT NULL
) STRICT;

-- rules_cache: the FR-GV-01 singleton parse cache, keyed by content hash.
CREATE TABLE rules_cache (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    rules_hash  TEXT NOT NULL,
    parsed_json TEXT NOT NULL,
    updated_at  INTEGER NOT NULL
) STRICT;
";

/// Migration 6 — the unified test annotation (S-028, [CR-001], [FR-AN-05],
/// [FR-EX-06]).
///
/// Two **additive** columns on `nodes` and a one-line view widening — no table
/// rebuild, so an existing database upgrades in place without a re-extract
/// ([FR-AN-04], [NFR-MA-06]). Both default `0`, so rows indexed before this
/// migration read as honest non-tests until their next annotation run (the
/// safe under-marking direction, [ADR-18]).
///
/// - `nodes.test_evidence` — the extraction-time test-marker signal captured by
///   Pass 1 ([FR-EX-06], S-027): `1` exactly when the function/method carries a
///   language-native test marker (Rust `#[test]`/`#[cfg(test)]` module, Python
///   `test_*`/`unittest.TestCase`, JS/TS `it`/`test`/`describe`, Go
///   `TestXxx`/`BenchmarkXxx`/`FuzzXxx` in `*_test.go`, Java `@Test`-family).
///   This is the persisted **input** the unified annotation reads — distinct
///   from the computed verdict, exactly as `exported` (Pass-1 input) is distinct
///   from `is_dead` (Pass-3 verdict).
/// - `nodes.is_test` — the Pass-3 verdict ([FR-AN-05]): `test_evidence` ∨ test
///   path conventions ∨ a `[semantics].test_markers` match. Positive evidence
///   only, never call-graph inference ([ADR-18]). The single source of truth
///   the metrics scope filter ([FR-QM-08]), `test_gaps` ([FR-GV-08]), and the
///   dead-code live roots ([FR-AN-01]) all consume — no detector re-derives it.
///
/// Unlike `is_dead`/`is_duplicate` (tri-state `NULL` = "not computed"),
/// `is_test` is `NOT NULL DEFAULT 0`: test classification is positive-evidence,
/// so the absence of evidence is an honest `false`, never "unknown".
///
/// The `annotations` view ([FR-AN-04]) is recreated to project `is_test`
/// alongside the existing verdict columns; `test_evidence` stays off the view —
/// it is the engine's internal input, not part of the queryable annotation
/// shape. SQLite cannot edit a view in place, so it is dropped and recreated
/// (a view carries no data — there is nothing to migrate).
///
/// [CR-001]: ../../../../docs/requests/CR-001-test-aware-quality-metrics.md
/// [ADR-18]: ../../../../docs/specs/architecture/decisions/ADR-18.md
/// [FR-AN-01]: ../../../../docs/specs/requirements/FR-AN-01.md
/// [FR-AN-04]: ../../../../docs/specs/requirements/FR-AN-04.md
/// [FR-AN-05]: ../../../../docs/specs/requirements/FR-AN-05.md
/// [FR-EX-06]: ../../../../docs/specs/requirements/FR-EX-06.md
/// [FR-GV-08]: ../../../../docs/specs/requirements/FR-GV-08.md
/// [FR-QM-08]: ../../../../docs/specs/requirements/FR-QM-08.md
/// [NFR-MA-06]: ../../../../docs/specs/requirements/NFR-MA-06.md
const MIGRATION_6: &str = "\
-- The Pass-1 extraction signal (FR-EX-06, S-027): 1 iff the node carries a
-- language-native test marker. The persisted INPUT to the is_test verdict.
ALTER TABLE nodes ADD COLUMN test_evidence INTEGER NOT NULL DEFAULT 0
    CHECK (test_evidence IN (0,1));

-- The Pass-3 unified verdict (FR-AN-05): evidence OR path convention OR marker.
-- NOT NULL DEFAULT 0 — positive-evidence classification, so absence is an
-- honest non-test, never the tri-state NULL is_dead/is_duplicate carry.
ALTER TABLE nodes ADD COLUMN is_test INTEGER NOT NULL DEFAULT 0
    CHECK (is_test IN (0,1));

-- Recreate the FR-AN-04 queryable view to project is_test alongside the other
-- verdicts. test_evidence is the engine's internal input and stays off the view.
DROP VIEW annotations;
CREATE VIEW annotations AS
    SELECT id AS node_id,
           cyclomatic_complexity,
           line_count,
           is_dead,
           is_duplicate,
           is_test,
           layer_membership
    FROM nodes;
";

/// Migration 7 — production-scope metric counts and the versioned gate baseline
/// (S-029, [CR-001], [FR-QM-08], [FR-QM-07], [FR-GV-10]).
///
/// Two **additive** columns on the append-only `metric_snapshots` ledger — no
/// table rebuild, so an existing database upgrades in place ([NFR-MA-06]):
///
/// - `metric_snapshots.test_function_count` — the count of `is_test`
///   function/method nodes excluded from the production scope ([FR-QM-08]),
///   persisted for transparency ([FR-QM-07], [NFR-CC-04]). `DEFAULT 0`: a
///   snapshot recorded before this migration scored under the test-inclusive
///   semantics and excluded nothing.
/// - `metric_snapshots.metric_version` — the metrics-semantics version the
///   snapshot was scored under ([FR-GV-10]). **`DEFAULT 1`** is load-bearing:
///   every pre-existing snapshot was computed under the test-inclusive scope
///   (v1), so the baseline that points at one is *incomparable* to a fresh
///   production-scope (v2) run. The gate reads this column and auto-re-baselines
///   on a mismatch ([UAT-GV-06]) instead of failing against an incomparable
///   anchor. New snapshots are written with the current
///   [`METRIC_SEMANTICS_VERSION`](crate::metrics::METRIC_SEMANTICS_VERSION).
///
/// [CR-001]: ../../../../docs/requests/CR-001-test-aware-quality-metrics.md
/// [FR-GV-10]: ../../../../docs/specs/requirements/FR-GV-10.md
/// [FR-QM-07]: ../../../../docs/specs/requirements/FR-QM-07.md
/// [FR-QM-08]: ../../../../docs/specs/requirements/FR-QM-08.md
/// [NFR-CC-04]: ../../../../docs/specs/requirements/NFR-CC-04.md
/// [NFR-MA-06]: ../../../../docs/specs/requirements/NFR-MA-06.md
const MIGRATION_7: &str = "\
-- The count of is_test functions excluded from the production scope (FR-QM-08),
-- persisted for transparency (FR-QM-07). 0 on pre-upgrade snapshots, which
-- scored test-inclusive and excluded nothing.
ALTER TABLE metric_snapshots ADD COLUMN test_function_count INTEGER NOT NULL DEFAULT 0;

-- The metrics-semantics version the snapshot was scored under (FR-GV-10).
-- DEFAULT 1 = the original test-inclusive scope: a baseline pointing at a
-- pre-upgrade snapshot is incomparable to a v2 production-scope run, so the
-- first post-upgrade gate auto-re-baselines (UAT-GV-06) instead of failing.
ALTER TABLE metric_snapshots ADD COLUMN metric_version INTEGER NOT NULL DEFAULT 1;
";

/// Migration 8 — the documentation-kind CHECK widening (S-033, [CR-003],
/// [ADR-19], [FR-DG-02], [FR-EX-05], [FR-DB-01]).
///
/// CR-003 admits documentation as new node/edge kinds in the **shared**
/// `nodes`/`edges` tables ([ADR-19] code-subgraph-scoping design). The frozen
/// discriminant contract grew accordingly: `NodeKind` 1..=17 → 1..=22 (the
/// `DocFile`/`DocSection` generic layer plus the typed `Requirement`/`Adr`/`Story`
/// enrichment) and `EdgeKind` 1..=10 → 1..=12 (the `doc_reference`/`traces_to`
/// doc edges). SQLite cannot widen a `CHECK` in place, so — exactly as migration
/// 3 did for the policy kinds — `nodes` and `edges` are rebuilt copy-style:
/// every row, every id, and the FTS postings (keyed on `nodes.id`) survive, so
/// an existing database upgrades forward **with no data loss** ([NFR-MA-06],
/// [NFR-RA-07], [FR-DB-01]).
///
/// The `unresolved_refs.kind` CHECK is widened the same way: [ADR-19] resolves
/// doc→doc and doc→code references "through the same matcher and `unresolved_refs`
/// ledger as code", so the ledger must accept the two doc edge kinds for S-035 to
/// bind them. Widening it here (in the foundational migration) keeps the
/// model⟷schema drift guard exact — every edge-kind CHECK equals `EdgeKind::ALL` —
/// and means the documentation-resolution story (S-035) needs no further schema
/// change.
///
/// The rebuild reuses migration 3's referenced-table-safe procedure (no `ALTER
/// TABLE … RENAME` of a table the `edges` FK points at): stash both tables in
/// plain holders, drop children-first so nothing cascades, recreate under the
/// final names with the **widened** CHECK and the *full* post-migration-6/7
/// column set, copy the rows back parents-first, then recreate the indexes and
/// FTS triggers verbatim. The `annotations` view (FR-AN-04) is dropped and
/// recreated unchanged so it never references a transiently-absent `nodes`.
///
/// This migration only widens what the `nodes`/`edges` tables *accept*; it
/// inserts no documentation rows. Documentation ingestion is the
/// [pipeline-orchestrator]'s concern (S-034), so a database that never indexes a
/// doc is byte-for-byte unaffected — the widened CHECK is harmless when unused
/// ([ADR-19] reversibility).
///
/// [CR-003]: ../../../../docs/requests/CR-003-documentation-graph-layer.md
/// [ADR-19]: ../../../../docs/specs/architecture/decisions/ADR-19.md
/// [FR-DG-02]: ../../../../docs/specs/requirements/FR-DG-02.md
/// [FR-EX-05]: ../../../../docs/specs/requirements/FR-EX-05.md
/// [NFR-MA-06]: ../../../../docs/specs/requirements/NFR-MA-06.md
/// [NFR-RA-07]: ../../../../docs/specs/requirements/NFR-RA-07.md
/// [pipeline-orchestrator]: ../../../../docs/specs/architecture/components/pipeline-orchestrator.md
const MIGRATION_8: &str = "\
-- Drop the FTS sync triggers around the rebuild: the copy below must not
-- double-index, and the triggers are recreated verbatim afterwards (as in
-- migration 3).
DROP TRIGGER nodes_fts_ai;
DROP TRIGGER nodes_fts_ad;
DROP TRIGGER nodes_fts_au;

-- The annotations view (FR-AN-04) reads native `nodes` columns; drop it so it
-- never references the transiently-dropped table, and recreate it unchanged
-- (migration-6 projection) once the rebuild completes.
DROP VIEW annotations;

-- Stash both tables' rows in plain holders (CTAS: no FKs/constraints) with the
-- FULL post-migration-6/7 column set, so the rebuild preserves real annotation
-- data — not just the migration-1 shape (this is a widening of populated
-- tables, unlike migration 3 which introduced the annotation columns fresh).
CREATE TABLE edges_stash AS SELECT id, source, target, kind, derived FROM edges;
CREATE TABLE nodes_stash AS
    SELECT id, symbol_id, kind, name, file_id, start_line, end_line,
           derived, exported, cyclomatic_complexity, line_count, fingerprint,
           is_dead, is_duplicate, layer_membership, test_evidence, is_test
    FROM nodes;

-- Children first: dropping edges removes the only FK references to nodes, so
-- the nodes drop cascades nothing. Clean under live FK enforcement.
DROP TABLE edges;
DROP TABLE nodes;

-- The rebuilt nodes table: the migration-6 shape with the widened kind CHECK
-- (the five documentation kinds 18..=22, CR-003/ADR-19).
CREATE TABLE nodes (
    id                    INTEGER PRIMARY KEY,
    symbol_id             INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
    kind                  INTEGER NOT NULL CHECK (kind IN (1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22)),
    name                  TEXT NOT NULL,
    file_id               INTEGER REFERENCES files(id) ON DELETE SET NULL,
    start_line            INTEGER,
    end_line              INTEGER,
    derived               INTEGER NOT NULL DEFAULT 0 CHECK (derived IN (0,1)),
    exported              INTEGER NOT NULL DEFAULT 0 CHECK (exported IN (0,1)),
    cyclomatic_complexity INTEGER,
    line_count            INTEGER,
    fingerprint           TEXT,
    is_dead               INTEGER CHECK (is_dead IN (0,1)),
    is_duplicate          INTEGER CHECK (is_duplicate IN (0,1)),
    layer_membership      TEXT,
    test_evidence         INTEGER NOT NULL DEFAULT 0 CHECK (test_evidence IN (0,1)),
    is_test               INTEGER NOT NULL DEFAULT 0 CHECK (is_test IN (0,1))
) STRICT;

-- The rebuilt edges table: the migration-3 shape with the widened kind CHECK
-- (the two documentation edges 11/12 — doc_reference, traces_to).
CREATE TABLE edges (
    id      INTEGER PRIMARY KEY,
    source  INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    target  INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    kind    INTEGER NOT NULL CHECK (kind IN (1,2,3,4,5,6,7,8,9,10,11,12)),
    derived INTEGER NOT NULL DEFAULT 0 CHECK (derived IN (0,1)),
    UNIQUE (source, target, kind)
) STRICT;

-- Copy rows back with identical ids (parents before children, so FK checks hold
-- and the FTS postings stay aligned). Every annotation column is carried over
-- verbatim — no row reverts to a default.
INSERT INTO nodes (id, symbol_id, kind, name, file_id, start_line, end_line,
                   derived, exported, cyclomatic_complexity, line_count, fingerprint,
                   is_dead, is_duplicate, layer_membership, test_evidence, is_test)
    SELECT id, symbol_id, kind, name, file_id, start_line, end_line,
           derived, exported, cyclomatic_complexity, line_count, fingerprint,
           is_dead, is_duplicate, layer_membership, test_evidence, is_test
    FROM nodes_stash;
INSERT INTO edges (id, source, target, kind, derived)
    SELECT id, source, target, kind, derived FROM edges_stash;

DROP TABLE nodes_stash;
DROP TABLE edges_stash;

-- The indexes were dropped with the old tables; recreate them (migration 1 + 3).
CREATE INDEX idx_nodes_symbol_id   ON nodes(symbol_id);
CREATE INDEX idx_nodes_kind        ON nodes(kind);
CREATE INDEX idx_edges_source_kind ON edges(source, kind);
CREATE INDEX idx_edges_target_kind ON edges(target, kind);

-- Recreate the FTS sync triggers verbatim (migration 1/3, FR-DB-03, NFR-RA-09).
CREATE TRIGGER nodes_fts_ai AFTER INSERT ON nodes BEGIN
    INSERT INTO nodes_fts(rowid, name) VALUES (new.id, new.name);
END;
CREATE TRIGGER nodes_fts_ad AFTER DELETE ON nodes BEGIN
    INSERT INTO nodes_fts(nodes_fts, rowid, name) VALUES ('delete', old.id, old.name);
END;
CREATE TRIGGER nodes_fts_au AFTER UPDATE ON nodes BEGIN
    INSERT INTO nodes_fts(nodes_fts, rowid, name) VALUES ('delete', old.id, old.name);
    INSERT INTO nodes_fts(rowid, name) VALUES (new.id, new.name);
END;

-- Recreate the FR-AN-04 queryable view exactly as migration 6 left it
-- (projecting is_test; test_evidence stays an internal input, off the view).
CREATE VIEW annotations AS
    SELECT id AS node_id,
           cyclomatic_complexity,
           line_count,
           is_dead,
           is_duplicate,
           is_test,
           layer_membership
    FROM nodes;

-- Widen unresolved_refs.kind to the doc edge kinds too (ADR-19: doc→doc and
-- doc→code references resolve through the SAME ledger as code, S-035). The
-- `form` CHECK is unchanged (RefForm is still 1..=4). unresolved_refs is
-- referenced by no other table, so the rebuild stashes, drops, recreates with
-- the widened kind CHECK, copies back, and recreates its two indexes.
CREATE TABLE unresolved_refs_stash AS
    SELECT id, file_id, source_symbol, target, alias, form, kind, line, resolved
    FROM unresolved_refs;
DROP TABLE unresolved_refs;
CREATE TABLE unresolved_refs (
    id            INTEGER PRIMARY KEY,
    file_id       INTEGER REFERENCES files(id) ON DELETE CASCADE,
    source_symbol TEXT NOT NULL,
    target        TEXT NOT NULL,
    alias         TEXT,
    form          INTEGER NOT NULL CHECK (form IN (1,2,3,4)),
    kind          INTEGER NOT NULL CHECK (kind IN (1,2,3,4,5,6,7,8,9,10,11,12)),
    line          INTEGER,
    resolved      INTEGER NOT NULL DEFAULT 0 CHECK (resolved IN (0,1)),
    UNIQUE (source_symbol, target, form, kind)
) STRICT;
INSERT INTO unresolved_refs
       (id, file_id, source_symbol, target, alias, form, kind, line, resolved)
    SELECT id, file_id, source_symbol, target, alias, form, kind, line, resolved
    FROM unresolved_refs_stash;
DROP TABLE unresolved_refs_stash;
CREATE INDEX idx_unresolved_refs_file     ON unresolved_refs(file_id);
CREATE INDEX idx_unresolved_refs_resolved ON unresolved_refs(resolved);
";

/// Migration 9 — extend the external-content FTS5 index to `DocSection` body
/// text (S-037, [CR-003], [ADR-19], [FR-DG-05], [FR-DB-03]).
///
/// [FR-DG-05] requires a phrase appearing only in a documentation *body* to be
/// found by `search` ([FR-NV-01]). The migration-1 `nodes_fts` index covers only
/// `nodes.name` — for a `DocSection` that is the heading text, not the prose
/// beneath it — so a body-only phrase is unsearchable until the body itself is
/// indexed. The [graph-store] architecture anticipated this exactly: the CR-003
/// doc migration "extends external-content FTS to `DocSection` body text"; the
/// foundational migration 8 (S-033) widened the kind CHECKs but **deferred** the
/// FTS body extension to the navigation story (this one), so it lands here.
///
/// Two changes, both forward-only and additive ([NFR-MA-06]):
///
/// 1. `nodes.body` — a nullable TEXT column added **in place** (`ALTER TABLE …
///    ADD COLUMN`, exactly as migrations 6/7 did — no table rebuild, every
///    existing row and id untouched). Only `DocSection` rows populate it (the
///    section's own prose, excluding nested sub-sections); every code node and
///    every other doc kind leaves it `NULL`, so the column is inert weight on a
///    non-doc graph ([ADR-19] reversibility).
/// 2. `nodes_fts(name, body)` — an FTS5 virtual table cannot gain a column in
///    place, so the index is dropped and recreated over both columns, then
///    repopulated from the content table with the `'rebuild'` command. It stays
///    **external-content** (`content='nodes'`), so the body text is stored once
///    in `nodes.body` — never duplicated into the index ([FR-DB-03]). The three
///    sync triggers are recreated carrying `body` alongside `name`, including the
///    critical `'delete'` command row so the index never silently desyncs
///    ([NFR-RA-09], SRS §7.2 trap).
///
/// A database that has never indexed a doc upgrades to an all-`NULL` `body`
/// column and a rebuilt index that finds exactly what it found before — the
/// extension is harmless when unused.
///
/// [CR-003]: ../../../../docs/requests/CR-003-documentation-graph-layer.md
/// [ADR-19]: ../../../../docs/specs/architecture/decisions/ADR-19.md
/// [FR-DG-05]: ../../../../docs/specs/requirements/FR-DG-05.md
/// [FR-DB-03]: ../../../../docs/specs/requirements/FR-DB-03.md
/// [FR-NV-01]: ../../../../docs/specs/requirements/FR-NV-01.md
/// [NFR-MA-06]: ../../../../docs/specs/requirements/NFR-MA-06.md
/// [NFR-RA-09]: ../../../../docs/specs/requirements/NFR-RA-09.md
/// [graph-store]: ../../../../docs/specs/architecture/components/graph-store.md
const MIGRATION_9: &str = "\
-- The DocSection body text (FR-DG-05): nullable, added in place, NULL on every
-- code node and on every doc node that is not a DocSection. No table rebuild —
-- existing rows and ids are untouched (NFR-MA-06).
ALTER TABLE nodes ADD COLUMN body TEXT;

-- The FTS5 vtable cannot ALTER in a column, so drop its sync triggers and the
-- index, recreate it over (name, body), and rebuild from the content table.
-- Still external-content (content='nodes'): body lives once in nodes.body.
DROP TRIGGER nodes_fts_ai;
DROP TRIGGER nodes_fts_ad;
DROP TRIGGER nodes_fts_au;
DROP TABLE nodes_fts;
CREATE VIRTUAL TABLE nodes_fts USING fts5(
    name,
    body,
    content='nodes',
    content_rowid='id'
);

-- Repopulate the inverted index from the existing nodes (bodies are NULL until
-- the next doc (re-)index writes them; code nodes never carry a body, FR-DG-05).
INSERT INTO nodes_fts(nodes_fts) VALUES('rebuild');

-- Recreate the sync triggers carrying body alongside name (FR-DB-03). The
-- 'delete' command rows retract the OLD postings so the external-content index
-- never silently desyncs (NFR-RA-09, SRS §7.2 trap).
CREATE TRIGGER nodes_fts_ai AFTER INSERT ON nodes BEGIN
    INSERT INTO nodes_fts(rowid, name, body) VALUES (new.id, new.name, new.body);
END;
CREATE TRIGGER nodes_fts_ad AFTER DELETE ON nodes BEGIN
    INSERT INTO nodes_fts(nodes_fts, rowid, name, body) VALUES ('delete', old.id, old.name, old.body);
END;
CREATE TRIGGER nodes_fts_au AFTER UPDATE ON nodes BEGIN
    INSERT INTO nodes_fts(nodes_fts, rowid, name, body) VALUES ('delete', old.id, old.name, old.body);
    INSERT INTO nodes_fts(rowid, name, body) VALUES (new.id, new.name, new.body);
END;
";

/// Migration 10 — the CR-005 structural extraction facts (S-042, [ADR-21],
/// [FR-EX-07], [FR-EX-08], [FR-EX-09]).
///
/// Three forward-only changes that persist the raw structural facts every
/// extended-metric dimension consumes, each harmless when unused
/// ([ADR-21] reversibility):
///
/// 1. **`nodes.max_nesting_depth`** — a nullable INTEGER added **in place**
///    (`ALTER TABLE … ADD COLUMN`, as migrations 6/7/9 did — no rebuild, every
///    existing row and id untouched). Populated for `Function`/`Method` nodes
///    only ([FR-EX-07]); `NULL` on every other node and on rows indexed before
///    this migration until their next re-extraction. The input to the Nesting
///    and Conciseness dimensions ([FR-QM-09]/[FR-QM-10]).
/// 2. **`shingles`** — winnowed near-clone fingerprint storage ([FR-EX-09]): one
///    row per `(node_id, hash)`, an inverted index the near-clone clustering
///    pass ([FR-AN-06], S-043) reads in id order. `ON DELETE CASCADE` ties a
///    function's shingles to its node, so a re-extract replaces them wholesale
///    like every other per-file fact. `idx_shingles_hash` backs the inverted
///    lookup S-043 clusters over.
/// 3. **The `Accesses` edge kind (13)** — the `edges.kind` CHECK is widened to
///    `1..=13` ([FR-EX-08], [FR-DB-01]). SQLite cannot widen a CHECK in place,
///    so `edges` is rebuilt copy-style (the migration-3/8 referenced-table-safe
///    procedure): stash the rows in a plain holder, drop and recreate `edges`
///    with the widened CHECK and its full column set, copy back with identical
///    ids, recreate the two hot-path indexes. `edges` is referenced by no other
///    table and carries no FTS triggers or views, so the rebuild is local — it
///    never touches `nodes`. `unresolved_refs.kind` is widened the same way,
///    because an ambiguous or unmatched member access persists in the ledger and
///    retries on sync ([NFR-RA-05]), so the ledger must accept kind 13; widening
///    it here keeps the model⟷schema drift guard exact (every edge-kind CHECK
///    equals `EdgeKind::ALL`).
///
/// `nodes` is **not** rebuilt: the only `nodes` change is the additive
/// `max_nesting_depth` column, so the FTS index, its triggers, and the
/// `annotations` view are all untouched. A database that indexes no code (or was
/// never re-extracted) carries an all-`NULL` column, an empty `shingles` table,
/// and no kind-13 edge — byte-for-byte unaffected.
///
/// [ADR-21]: ../../../../docs/specs/architecture/decisions/ADR-21.md
/// [FR-EX-07]: ../../../../docs/specs/requirements/FR-EX-07.md
/// [FR-EX-08]: ../../../../docs/specs/requirements/FR-EX-08.md
/// [FR-EX-09]: ../../../../docs/specs/requirements/FR-EX-09.md
/// [FR-DB-01]: ../../../../docs/specs/requirements/FR-DB-01.md
/// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
const MIGRATION_10: &str = "\
-- 1. Per-function max nesting depth (FR-EX-07): nullable, added in place. NULL
-- on every non-callable node and on rows indexed before this migration. No
-- table rebuild — existing rows and ids untouched (NFR-MA-06).
ALTER TABLE nodes ADD COLUMN max_nesting_depth INTEGER;

-- 2. Winnowed near-clone shingle fingerprints (FR-EX-09): one row per
-- (node_id, hash). The hash is a platform-independent u64 stored as the
-- equivalent signed INTEGER. ON DELETE CASCADE ties a function's shingles to
-- its node, so a re-extract replaces them wholesale. The hash index backs the
-- id-ordered inverted index the clustering pass reads (FR-AN-06, S-043).
CREATE TABLE shingles (
    node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    hash    INTEGER NOT NULL,
    PRIMARY KEY (node_id, hash)
) STRICT;
CREATE INDEX idx_shingles_hash ON shingles(hash);

-- 3. Widen edges.kind to the Accesses edge (13, FR-EX-08). SQLite cannot widen
-- a CHECK in place, so rebuild edges copy-style: stash (no FKs/constraints),
-- drop, recreate with the widened CHECK and full column set, copy back with
-- identical ids, recreate the indexes. edges is referenced by no other table
-- and carries no FTS triggers or view, so the rebuild never touches nodes.
CREATE TABLE edges_stash AS SELECT id, source, target, kind, derived FROM edges;
DROP TABLE edges;
CREATE TABLE edges (
    id      INTEGER PRIMARY KEY,
    source  INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    target  INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    kind    INTEGER NOT NULL CHECK (kind IN (1,2,3,4,5,6,7,8,9,10,11,12,13)),
    derived INTEGER NOT NULL DEFAULT 0 CHECK (derived IN (0,1)),
    UNIQUE (source, target, kind)
) STRICT;
INSERT INTO edges (id, source, target, kind, derived)
    SELECT id, source, target, kind, derived FROM edges_stash;
DROP TABLE edges_stash;
CREATE INDEX idx_edges_source_kind ON edges(source, kind);
CREATE INDEX idx_edges_target_kind ON edges(target, kind);

-- 4. Widen unresolved_refs.kind to 13 too: an ambiguous/unmatched member access
-- persists in the ledger and retries on sync (NFR-RA-05), so the ledger must
-- accept the Accesses kind. Same rebuild shape as migration 8; the form CHECK
-- is unchanged (RefForm is still 1..=4).
CREATE TABLE unresolved_refs_stash AS
    SELECT id, file_id, source_symbol, target, alias, form, kind, line, resolved
    FROM unresolved_refs;
DROP TABLE unresolved_refs;
CREATE TABLE unresolved_refs (
    id            INTEGER PRIMARY KEY,
    file_id       INTEGER REFERENCES files(id) ON DELETE CASCADE,
    source_symbol TEXT NOT NULL,
    target        TEXT NOT NULL,
    alias         TEXT,
    form          INTEGER NOT NULL CHECK (form IN (1,2,3,4)),
    kind          INTEGER NOT NULL CHECK (kind IN (1,2,3,4,5,6,7,8,9,10,11,12,13)),
    line          INTEGER,
    resolved      INTEGER NOT NULL DEFAULT 0 CHECK (resolved IN (0,1)),
    UNIQUE (source_symbol, target, form, kind)
) STRICT;
INSERT INTO unresolved_refs
       (id, file_id, source_symbol, target, alias, form, kind, line, resolved)
    SELECT id, file_id, source_symbol, target, alias, form, kind, line, resolved
    FROM unresolved_refs_stash;
DROP TABLE unresolved_refs_stash;
CREATE INDEX idx_unresolved_refs_file     ON unresolved_refs(file_id);
CREATE INDEX idx_unresolved_refs_resolved ON unresolved_refs(resolved);
";

/// Migration 11 — the CR-005 near-clone clustering annotation (S-043, [ADR-21],
/// [FR-AN-06]).
///
/// One forward-only, additive change persisting the clone-group membership the
/// near-clone clustering sub-pass ([annotation-engine], [FR-AN-06]) computes
/// from the migration-10 `shingles` index, and the Uniqueness dimension
/// consumes ([FR-QM-13], S-044):
///
/// 1. **`nodes.clone_group`** — a nullable INTEGER added **in place**
///    (`ALTER TABLE … ADD COLUMN`, exactly as migrations 6/7/9/10 did — no
///    table rebuild, every existing row and id untouched, [NFR-MA-06]). It is a
///    native annotation column ([FR-AN-04], no sidecar) holding the **stable
///    clone-group identifier** — the minimum node id of the function's
///    near-clone connected component — or `NULL` when the function belongs to no
///    near-clone group ([FR-AN-06]). The minimum-id representative makes the
///    persisted value a pure function of which functions are connected,
///    independent of clustering order, so the column is byte-identical across
///    runs ([NFR-RA-06]). Distinct from `is_duplicate`, which records the
///    exact-AST-shape verdict ([FR-AN-02]) the near-clone pass leaves untouched.
/// 2. **The `annotations` view** is dropped and recreated to project the new
///    column alongside the existing annotation columns, so clone-group
///    membership is queryable through the same [FR-AN-04] surface as the other
///    verdicts. The projection is otherwise the migration-8/10 shape, verbatim.
///
/// No table is rebuilt: the only `nodes` change is the additive column, so the
/// FTS index and its triggers are untouched. A database that indexes no code (or
/// was never re-annotated) carries an all-`NULL` column — byte-for-byte
/// unaffected ([ADR-21] reversibility: "the shingle columns are append-only and
/// harmless if unused", and the same holds for the clone-group column).
///
/// [annotation-engine]: ../../../../docs/specs/architecture/components/annotation-engine.md
/// [ADR-21]: ../../../../docs/specs/architecture/decisions/ADR-21.md
/// [FR-AN-02]: ../../../../docs/specs/requirements/FR-AN-02.md
/// [FR-AN-04]: ../../../../docs/specs/requirements/FR-AN-04.md
/// [FR-AN-06]: ../../../../docs/specs/requirements/FR-AN-06.md
/// [FR-QM-13]: ../../../../docs/specs/requirements/FR-QM-13.md
/// [NFR-MA-06]: ../../../../docs/specs/requirements/NFR-MA-06.md
/// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
const MIGRATION_11: &str = "\
-- Near-clone clustering membership (FR-AN-06): nullable, added in place. NULL on
-- every node not in a near-clone group and on rows annotated before this
-- migration. Holds the stable group id — the minimum node id of the function's
-- near-clone connected component — a pure, order-independent function of which
-- functions are connected (NFR-RA-06). A native annotation column (FR-AN-04); no
-- table rebuild — existing rows and ids untouched (NFR-MA-06).
ALTER TABLE nodes ADD COLUMN clone_group INTEGER;

-- Recreate the FR-AN-04 queryable view to project clone_group alongside the
-- existing annotation columns (otherwise the migration-8/10 projection verbatim).
-- A view cannot gain a column in place, so drop and recreate.
DROP VIEW annotations;
CREATE VIEW annotations AS
    SELECT id AS node_id,
           cyclomatic_complexity,
           line_count,
           is_dead,
           is_duplicate,
           is_test,
           layer_membership,
           clone_group
    FROM nodes;
";

/// Migration 12 — the CR-005 extended metric set on `metric_snapshots`
/// (S-044, [ADR-21], [ADR-12], [FR-QM-07], [FR-QM-09]..[FR-QM-14]).
///
/// One forward-only, additive widening of the append-only quality-signal ledger
/// so a snapshot records the full **ten-dimension** signal (metric-semantics
/// version 3): the five new structural dimensions' raw + normalized pairs, the
/// applicability flag of the two dimensions that can drop out of the mean, and
/// the effective-thresholds hash ([FR-QM-14], [BR-25]). Every column is added in
/// place (`ALTER TABLE … ADD COLUMN`, the migration-7 shape) — no rebuild, every
/// existing snapshot row and id untouched ([NFR-MA-06]):
///
/// - **`nesting_*` / `conciseness_*` / `uniqueness_*`** — raw + normalized for the
///   three always-applicable new dimensions ([FR-QM-09]/[FR-QM-10]/[FR-QM-13]).
///   Nullable: a pre-v3 snapshot scored only the original five, so it carries
///   `NULL` here — distinct from a real `0.0`, the same tri-state honesty the
///   `aggregate_signal` sentinel keeps ([NFR-CC-04]).
/// - **`cohesion_*` / `focus_*`** plus **`cohesion_applicable` / `focus_applicable`**
///   — the two dimensions that **drop out** of the mean when their construct is
///   absent ([FR-QM-11]/[FR-QM-12], [ADR-21]). The flag is `1` when the dimension
///   applied (classes / class-like containers exist) and `0` when it dropped
///   out; the value columns are `NULL` exactly when the flag is `0`, so the
///   snapshot is self-describing about which dimensions the mean spanned
///   ([UAT-QM-10], [NFR-CC-04]). `NULL` flag = a pre-v3 snapshot.
/// - **`thresholds_hash`** — the hash of the effective detection-threshold set
///   the run scored under ([FR-QM-14], [ADR-21]); a mismatch versus the baseline
///   triggers the announced auto-re-baseline ([FR-GV-10], [BR-25]) wired in
///   [S-045]. `NULL` on a pre-v3 snapshot.
///
/// REAL columns store IEEE-754 doubles exactly, so a re-read returns the bytes
/// the canonical-order reduction produced ([NFR-RA-06]); golden tests pin the
/// rounded `aggregate_signal`, not intermediate floats ([ADR-08]).
///
/// [S-045]: ../../../../docs/planning/journal.md#s-045-metric-thresholds-budgets-and-worst-offender-reporting
/// [ADR-12]: ../../../../docs/specs/architecture/decisions/ADR-12.md
/// [ADR-21]: ../../../../docs/specs/architecture/decisions/ADR-21.md
/// [FR-QM-07]: ../../../../docs/specs/requirements/FR-QM-07.md
/// [FR-QM-09]: ../../../../docs/specs/requirements/FR-QM-09.md
/// [FR-QM-10]: ../../../../docs/specs/requirements/FR-QM-10.md
/// [FR-QM-11]: ../../../../docs/specs/requirements/FR-QM-11.md
/// [FR-QM-12]: ../../../../docs/specs/requirements/FR-QM-12.md
/// [FR-QM-13]: ../../../../docs/specs/requirements/FR-QM-13.md
/// [FR-QM-14]: ../../../../docs/specs/requirements/FR-QM-14.md
/// [FR-GV-10]: ../../../../docs/specs/requirements/FR-GV-10.md
/// [NFR-CC-04]: ../../../../docs/specs/requirements/NFR-CC-04.md
/// [NFR-MA-06]: ../../../../docs/specs/requirements/NFR-MA-06.md
/// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
const MIGRATION_12: &str = "\
-- CR-005 extended metric set (FR-QM-09..14, ADR-21): five new structural
-- dimensions, two applicability flags, and the effective-thresholds hash, added
-- additively to the append-only ledger. NULL on pre-v3 snapshots (which scored
-- only the original five) — distinct from a real 0.0 (NFR-CC-04). No rebuild;
-- every existing row and id untouched (NFR-MA-06).
ALTER TABLE metric_snapshots ADD COLUMN nesting_raw            REAL;
ALTER TABLE metric_snapshots ADD COLUMN nesting_normalized     REAL;
ALTER TABLE metric_snapshots ADD COLUMN conciseness_raw        REAL;
ALTER TABLE metric_snapshots ADD COLUMN conciseness_normalized REAL;
-- Cohesion and Focus drop out of the mean when their construct is absent
-- (FR-QM-11/12): the *_applicable flag is 1 when scored, 0 when dropped out, and
-- the value columns are NULL exactly when the flag is 0 (self-describing
-- snapshot, UAT-QM-10).
ALTER TABLE metric_snapshots ADD COLUMN cohesion_raw           REAL;
ALTER TABLE metric_snapshots ADD COLUMN cohesion_normalized    REAL;
ALTER TABLE metric_snapshots ADD COLUMN cohesion_applicable    INTEGER CHECK (cohesion_applicable IN (0,1));
ALTER TABLE metric_snapshots ADD COLUMN focus_raw              REAL;
ALTER TABLE metric_snapshots ADD COLUMN focus_normalized       REAL;
ALTER TABLE metric_snapshots ADD COLUMN focus_applicable       INTEGER CHECK (focus_applicable IN (0,1));
ALTER TABLE metric_snapshots ADD COLUMN uniqueness_raw         REAL;
ALTER TABLE metric_snapshots ADD COLUMN uniqueness_normalized  REAL;
-- The effective detection-threshold set the run scored under (FR-QM-14, BR-25);
-- a baseline mismatch triggers the announced auto-re-baseline (FR-GV-10), wired
-- in S-045.
ALTER TABLE metric_snapshots ADD COLUMN thresholds_hash        TEXT;
";

/// Migration 13 — the config-kind CHECK widening (S-062, [CR-010], [ADR-25],
/// [FR-CG-02], [FR-EX-05], [FR-DB-01]).
///
/// CR-010 admits config & artifact formats as new **node** kinds in the shared
/// `nodes` table, following the documentation layer's pattern ([ADR-19]/migration
/// 8). The frozen discriminant contract grows accordingly: `NodeKind` 1..=22 →
/// 1..=34 (the generic `ConfigFile`/`ConfigSection` layer plus the ten typed
/// anchors of [FR-CG-03]). The layer is **`Contains`-only**: it introduces **no
/// new edge kinds**, so `EdgeKind` stays 1..=13 and `unresolved_refs.kind` is
/// untouched — the edge CHECK is recreated **byte-identical** to migration 10's
/// (`migration_13_widens_nodes_only_and_leaves_edges_byte_identical` guards this).
///
/// SQLite cannot widen a `CHECK` in place, so — exactly as migrations 3 and 8 did
/// — `nodes` is rebuilt copy-style. `edges` references `nodes(id)`, so the
/// referenced-table-safe procedure ([migration 3]/[migration 8]) drops children
/// first: stash both tables in plain holders, drop `edges` then `nodes` (so
/// nothing cascades), recreate `nodes` with the **widened** kind CHECK and the
/// *full* post-migration-11 column set (`body`, `max_nesting_depth`,
/// `clone_group` included), recreate `edges` with its kind CHECK **unchanged**
/// (1..=13), copy the rows back parents-first, then recreate the indexes, the
/// migration-9 FTS triggers (carrying `body`), and the migration-11 `annotations`
/// view. `unresolved_refs` is **not** rebuilt — no edge kind is added.
///
/// This migration only widens what `nodes` *accepts*; it inserts no config rows.
/// Config ingestion is the pipeline's concern (S-063+), so a database that never
/// indexes an artifact is byte-for-byte unaffected — the widened CHECK is
/// harmless when unused ([ADR-25] reversibility).
///
/// [CR-010]: ../../../../docs/requests/CR-010-config-artifact-graph-layer.md
/// [ADR-19]: ../../../../docs/specs/architecture/decisions/ADR-19.md
/// [ADR-25]: ../../../../docs/specs/architecture/decisions/ADR-25.md
/// [FR-CG-02]: ../../../../docs/specs/requirements/FR-CG-02.md
/// [FR-EX-05]: ../../../../docs/specs/requirements/FR-EX-05.md
/// [migration 3]: MIGRATION_3
/// [migration 8]: MIGRATION_8
const MIGRATION_13: &str = "\
-- Drop the FTS sync triggers around the rebuild: the copy below must not
-- double-index, and the triggers are recreated verbatim afterwards (migration 9
-- shape, carrying `body`).
DROP TRIGGER nodes_fts_ai;
DROP TRIGGER nodes_fts_ad;
DROP TRIGGER nodes_fts_au;

-- The annotations view (FR-AN-04) reads native `nodes` columns; drop it so it
-- never references the transiently-dropped table, and recreate it unchanged
-- (migration-11 projection) once the rebuild completes.
DROP VIEW annotations;

-- Stash both tables' rows in plain holders (CTAS: no FKs/constraints) with the
-- FULL post-migration-11 column set, so the rebuild preserves real annotation
-- data (this is a widening of populated tables, as in migration 8).
CREATE TABLE edges_stash AS SELECT id, source, target, kind, derived FROM edges;
CREATE TABLE nodes_stash AS
    SELECT id, symbol_id, kind, name, file_id, start_line, end_line,
           derived, exported, cyclomatic_complexity, line_count, fingerprint,
           is_dead, is_duplicate, layer_membership, test_evidence, is_test,
           body, max_nesting_depth, clone_group
    FROM nodes;

-- Children first: dropping edges removes the only FK references to nodes, so
-- the nodes drop cascades nothing. Clean under live FK enforcement.
DROP TABLE edges;
DROP TABLE nodes;

-- The rebuilt nodes table: the migration-8 shape carried forward through
-- migrations 9/10/11 (body, max_nesting_depth, clone_group) with the widened
-- kind CHECK — the twelve config & artifact kinds 23..=34 (CR-010/ADR-25).
CREATE TABLE nodes (
    id                    INTEGER PRIMARY KEY,
    symbol_id             INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
    kind                  INTEGER NOT NULL CHECK (kind IN (1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34)),
    name                  TEXT NOT NULL,
    file_id               INTEGER REFERENCES files(id) ON DELETE SET NULL,
    start_line            INTEGER,
    end_line              INTEGER,
    derived               INTEGER NOT NULL DEFAULT 0 CHECK (derived IN (0,1)),
    exported              INTEGER NOT NULL DEFAULT 0 CHECK (exported IN (0,1)),
    cyclomatic_complexity INTEGER,
    line_count            INTEGER,
    fingerprint           TEXT,
    is_dead               INTEGER CHECK (is_dead IN (0,1)),
    is_duplicate          INTEGER CHECK (is_duplicate IN (0,1)),
    layer_membership      TEXT,
    test_evidence         INTEGER NOT NULL DEFAULT 0 CHECK (test_evidence IN (0,1)),
    is_test               INTEGER NOT NULL DEFAULT 0 CHECK (is_test IN (0,1)),
    body                  TEXT,
    max_nesting_depth     INTEGER,
    clone_group           INTEGER
) STRICT;

-- The rebuilt edges table: the migration-10 shape with the kind CHECK
-- UNCHANGED (1..=13). CR-010 is a Contains-only layer — it burns no edge kind —
-- so this clause is byte-identical to migration 10's edges.kind CHECK.
CREATE TABLE edges (
    id      INTEGER PRIMARY KEY,
    source  INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    target  INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    kind    INTEGER NOT NULL CHECK (kind IN (1,2,3,4,5,6,7,8,9,10,11,12,13)),
    derived INTEGER NOT NULL DEFAULT 0 CHECK (derived IN (0,1)),
    UNIQUE (source, target, kind)
) STRICT;

-- Copy rows back with identical ids (parents before children, so FK checks hold
-- and the FTS postings stay aligned). Every annotation column is carried over
-- verbatim — no row reverts to a default.
INSERT INTO nodes (id, symbol_id, kind, name, file_id, start_line, end_line,
                   derived, exported, cyclomatic_complexity, line_count, fingerprint,
                   is_dead, is_duplicate, layer_membership, test_evidence, is_test,
                   body, max_nesting_depth, clone_group)
    SELECT id, symbol_id, kind, name, file_id, start_line, end_line,
           derived, exported, cyclomatic_complexity, line_count, fingerprint,
           is_dead, is_duplicate, layer_membership, test_evidence, is_test,
           body, max_nesting_depth, clone_group
    FROM nodes_stash;
INSERT INTO edges (id, source, target, kind, derived)
    SELECT id, source, target, kind, derived FROM edges_stash;

DROP TABLE nodes_stash;
DROP TABLE edges_stash;

-- The indexes were dropped with the old tables; recreate them (migration 1 + 3).
CREATE INDEX idx_nodes_symbol_id   ON nodes(symbol_id);
CREATE INDEX idx_nodes_kind        ON nodes(kind);
CREATE INDEX idx_edges_source_kind ON edges(source, kind);
CREATE INDEX idx_edges_target_kind ON edges(target, kind);

-- Recreate the FTS sync triggers carrying `body` alongside `name` (migration 9
-- shape, FR-DB-03). The 'delete' command rows retract the OLD postings so the
-- external-content index never silently desyncs (NFR-RA-09, SRS §7.2 trap).
CREATE TRIGGER nodes_fts_ai AFTER INSERT ON nodes BEGIN
    INSERT INTO nodes_fts(rowid, name, body) VALUES (new.id, new.name, new.body);
END;
CREATE TRIGGER nodes_fts_ad AFTER DELETE ON nodes BEGIN
    INSERT INTO nodes_fts(nodes_fts, rowid, name, body) VALUES ('delete', old.id, old.name, old.body);
END;
CREATE TRIGGER nodes_fts_au AFTER UPDATE ON nodes BEGIN
    INSERT INTO nodes_fts(nodes_fts, rowid, name, body) VALUES ('delete', old.id, old.name, old.body);
    INSERT INTO nodes_fts(rowid, name, body) VALUES (new.id, new.name, new.body);
END;

-- Recreate the FR-AN-04 queryable view exactly as migration 11 left it
-- (projecting clone_group alongside the other verdicts).
CREATE VIEW annotations AS
    SELECT id AS node_id,
           cyclomatic_complexity,
           line_count,
           is_dead,
           is_duplicate,
           is_test,
           layer_membership,
           clone_group
    FROM nodes;
";

/// Migration 14 — the CR-011 cross-artifact edge kinds and their payload column
/// ([ADR-26], [FR-CG-07], [FR-EX-05], [FR-DB-01]).
///
/// The first edge-ontology change since migration 10: CR-011 appends the two
/// payload-subtyped edge kinds `ArtifactRef` (14) and `ArtifactBinding` (15), so
/// both `edges.kind` and `unresolved_refs.kind` widen to 1..=15, and both tables
/// gain a nullable `payload` column carrying the relation class (`proto-import`,
/// `tf-module-call`, `route`, `type-name`, …) — payload subtyping, so each
/// future relation costs a string, not a discriminant.
///
/// SQLite cannot widen a CHECK or add a column to a `STRICT` table's frozen
/// shape in place, so both tables are rebuilt copy-style (the migration-10
/// pattern): stash the live columns, drop, recreate with the widened CHECK +
/// `payload`, copy the rows back with identical ids and a `NULL` payload, then
/// recreate the indexes. `nodes` is **not** touched — CR-011 adds no node kind —
/// so the FTS index, its triggers, and the `annotations` view are all untouched,
/// and `edges`/`unresolved_refs` carry no FTS triggers or views of their own.
///
/// The `UNIQUE` constraints are unchanged: `edges(source, target, kind)` and
/// `unresolved_refs(source_symbol, target, form, kind)`. `payload` is an attached
/// attribute, deliberately **not** part of either key — adding a nullable column
/// to a `UNIQUE` clause would defeat dedup (SQLite treats `NULL`s as distinct),
/// re-admitting duplicate code edges. First-payload-wins on the rare collision.
///
/// This migration only widens what the two tables *accept*; it inserts no rows.
/// A database that never indexes a cross-artifact reference carries an all-`NULL`
/// `payload` column and no kind-14/15 row — byte-for-byte unaffected ([ADR-26]
/// reversibility: dropping the binding clients reverts to the Contains-only
/// layer, and the gated path never read these edges).
///
/// [ADR-26]: ../../../../docs/specs/architecture/decisions/ADR-26.md
/// [FR-CG-07]: ../../../../docs/specs/requirements/FR-CG-07.md
/// [FR-EX-05]: ../../../../docs/specs/requirements/FR-EX-05.md
/// [FR-DB-01]: ../../../../docs/specs/requirements/FR-DB-01.md
const MIGRATION_14: &str = "\
-- 1. Widen edges.kind to the two CR-011 artifact edge kinds (14, 15) and add the
-- relation `payload`. SQLite cannot widen a CHECK in place, so rebuild edges
-- copy-style: stash (no FKs/constraints), drop, recreate with the widened CHECK
-- and the new column, copy back with identical ids (payload NULL on every
-- existing edge), recreate the indexes. edges is referenced by no other table
-- and carries no FTS triggers or view, so the rebuild never touches nodes.
CREATE TABLE edges_stash AS SELECT id, source, target, kind, derived FROM edges;
DROP TABLE edges;
CREATE TABLE edges (
    id      INTEGER PRIMARY KEY,
    source  INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    target  INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    kind    INTEGER NOT NULL CHECK (kind IN (1,2,3,4,5,6,7,8,9,10,11,12,13,14,15)),
    derived INTEGER NOT NULL DEFAULT 0 CHECK (derived IN (0,1)),
    payload TEXT,
    UNIQUE (source, target, kind)
) STRICT;
INSERT INTO edges (id, source, target, kind, derived)
    SELECT id, source, target, kind, derived FROM edges_stash;
DROP TABLE edges_stash;
CREATE INDEX idx_edges_source_kind ON edges(source, kind);
CREATE INDEX idx_edges_target_kind ON edges(target, kind);

-- 2. Widen unresolved_refs.kind to 15 too and add the matching `payload`: a
-- workspace-relative artifact reference whose target is not yet indexed persists
-- in the ledger and retries on sync (NFR-RA-05), so the ledger must accept the
-- two artifact kinds and carry the relation class for per-class coverage. Same
-- rebuild shape as migration 10; the form CHECK is unchanged (RefForm 1..=4).
CREATE TABLE unresolved_refs_stash AS
    SELECT id, file_id, source_symbol, target, alias, form, kind, line, resolved
    FROM unresolved_refs;
DROP TABLE unresolved_refs;
CREATE TABLE unresolved_refs (
    id            INTEGER PRIMARY KEY,
    file_id       INTEGER REFERENCES files(id) ON DELETE CASCADE,
    source_symbol TEXT NOT NULL,
    target        TEXT NOT NULL,
    alias         TEXT,
    form          INTEGER NOT NULL CHECK (form IN (1,2,3,4)),
    kind          INTEGER NOT NULL CHECK (kind IN (1,2,3,4,5,6,7,8,9,10,11,12,13,14,15)),
    line          INTEGER,
    resolved      INTEGER NOT NULL DEFAULT 0 CHECK (resolved IN (0,1)),
    payload       TEXT,
    UNIQUE (source_symbol, target, form, kind)
) STRICT;
INSERT INTO unresolved_refs
       (id, file_id, source_symbol, target, alias, form, kind, line, resolved)
    SELECT id, file_id, source_symbol, target, alias, form, kind, line, resolved
    FROM unresolved_refs_stash;
DROP TABLE unresolved_refs_stash;
CREATE INDEX idx_unresolved_refs_file     ON unresolved_refs(file_id);
CREATE INDEX idx_unresolved_refs_resolved ON unresolved_refs(resolved);
";

/// Migration 15 — the durable `project_metadata` key/value table ([CR-004],
/// [ADR-20], [FR-SY-07]).
///
/// A single additive table holding small, durable per-project facts as
/// `(key, value)` text pairs. Its first (and currently only) inhabitant is the
/// `config_fingerprint` row: a deterministic hash of the admission-relevant
/// configuration ([`crate::config::Config::admission_fingerprint`]) recorded at
/// the last full reconciliation, so the pipeline can detect that the admission
/// policy has changed and purge now-unadmitted files exactly once per change.
/// This is the durable record the in-memory `last_full_index_at` could never be
/// ([CR-004] §3.1). Forward-only and standalone: no data migration, no rebuild,
/// no FK to any existing table.
///
/// [CR-004]: ../../../../docs/requests/CR-004-config-change-reconciliation.md
/// [ADR-20]: ../../../../docs/specs/architecture/decisions/ADR-20.md
/// [FR-SY-07]: ../../../../docs/specs/requirements/FR-SY-07.md
const MIGRATION_15: &str = "\
CREATE TABLE project_metadata (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;
";

/// Migration 16 — enforce **one node per `symbol_id`** with a deduplicating
/// `UNIQUE(symbol_id)` rebuild (S-201, [CR-052], [ADR-46], [NFR-RA-13],
/// [FR-SY-10], [FR-DB-04]).
///
/// Incremental `sync` silently accumulated duplicate `symbol_id` rows
/// (Channel A): the `nodes` table carried no uniqueness constraint and node
/// insertion was an unconditional `INSERT`, so a re-extracted symbol whose prior
/// row was not deleted left a phantom duplicate the per-file delete never
/// reclaimed. A clean graph holds exactly one node per `symbol_id`, so this
/// migration makes that emergent convention a schema-enforced key ([ADR-46]).
/// The companion idempotent upsert (S-202) and the structural gate (S-204) build
/// on the constraint this migration establishes.
///
/// SQLite cannot add a table-level `UNIQUE` constraint in place, so `nodes` is
/// rebuilt copy-style — the same referenced-table-safe procedure migrations
/// 3/8/13 use (drop the FTS triggers and the `annotations` view, stash both
/// tables in plain holders, drop `edges` then `nodes` children-first so nothing
/// cascades, recreate under the final names, copy the rows back parents-first,
/// recreate the indexes/triggers/view). The kind CHECKs are recreated
/// **byte-identical** to their current authoritative form — `nodes.kind` 1..=34
/// (migration 13) and `edges.kind` 1..=15 (migration 14): CR-052 adds no node or
/// edge kind, so the model⟷schema drift guard
/// (`schema_check_matches_model_ontology`) still reads those frozen widenings.
///
/// # Deduplication (before the rebuild)
///
/// A drifted store can hold several rows for one `symbol_id`; copying them into a
/// `UNIQUE(symbol_id)` table would fail. So a dedup pass runs first, keeping the
/// **`MIN(id)` survivor** per `symbol_id` (the oldest row — the lowest, most
/// stable rowid, which keeps the FTS-aligned ids of the original insert):
///
/// 1. Build a `node_dedup_map(old_id → survivor_id)` over every node.
/// 2. Remap the losers' `shingles` onto their survivor (`INSERT OR IGNORE` on the
///    `(node_id, hash)` primary key — a survivor that already carries the hash
///    wins).
/// 3. Remap the losers' `edges` onto their survivors (`INSERT OR IGNORE` on the
///    `(source, target, kind)` UNIQUE key — a pre-existing parallel edge wins;
///    `derived`/`payload` follow the remapped row).
/// 4. Delete the loser nodes. Their now-redundant original edges/shingles cascade
///    away (children before the parent, via the `ON DELETE CASCADE` FKs).
///
/// After the pass exactly one node remains per `symbol_id`, so the copy-back into
/// the `UNIQUE(symbol_id)` table is total and loss-free. On a **clean** store the
/// map is the identity (every node is its own survivor), no row is remapped or
/// deleted, and the rebuild is a population-preserving copy — surviving rowids
/// unchanged, idempotent on re-run.
///
/// # FTS resync
///
/// Unlike migrations 3/8/13 (which preserve every rowid and so leave the
/// external-content `nodes_fts` index aligned without rebuilding it), this
/// migration *removes* loser rows. Their postings would orphan, so the triggers
/// are dropped for the whole operation and the index is repopulated with the
/// `'rebuild'` command after the copy-back ([NFR-RA-09]) — the same resync
/// migration 9 used when it changed the index shape.
///
/// The old non-unique `idx_nodes_symbol_id` is **not** recreated: the
/// `UNIQUE(symbol_id)` constraint provides its own covering index, so a separate
/// one would be redundant.
///
/// [CR-052]: ../../../../docs/requests/CR-052-graph-update-correctness.md
/// [ADR-46]: ../../../../docs/specs/architecture/decisions/ADR-46.md
/// [NFR-RA-13]: ../../../../docs/specs/requirements/NFR-RA-13.md
/// [FR-SY-10]: ../../../../docs/specs/requirements/FR-SY-10.md
/// [FR-DB-04]: ../../../../docs/specs/requirements/FR-DB-04.md
/// [NFR-RA-09]: ../../../../docs/specs/requirements/NFR-RA-09.md
const MIGRATION_16: &str = "\
-- Drop the FTS sync triggers around the rebuild: the dedup deletes and the copy
-- below must not touch the index, and it is repopulated with 'rebuild' once the
-- final population is in place.
DROP TRIGGER nodes_fts_ai;
DROP TRIGGER nodes_fts_ad;
DROP TRIGGER nodes_fts_au;

-- The annotations view (FR-AN-04) reads native `nodes` columns; drop it so it
-- never references the transiently-dropped table, and recreate it unchanged
-- (migration-13 projection) once the rebuild completes.
DROP VIEW annotations;

-- === Deduplicate to one node per symbol_id (Channel A, ADR-46) ===========
-- Map every node to the MIN(id) survivor of its symbol_id. A survivor maps to
-- itself; on a clean store this is the identity map, so nothing below changes a
-- row. A plain holder table (no FKs/constraints).
CREATE TABLE node_dedup_map AS
SELECT n.id AS old_id, m.survivor_id AS survivor_id
FROM nodes n
JOIN (SELECT symbol_id, MIN(id) AS survivor_id FROM nodes GROUP BY symbol_id) m
  ON m.symbol_id = n.symbol_id;

-- Remap the losers' shingles onto their survivor; the (node_id, hash) primary
-- key dedups (a survivor that already carries the hash wins).
INSERT OR IGNORE INTO shingles (node_id, hash)
SELECT d.survivor_id, sh.hash
FROM shingles sh
JOIN node_dedup_map d ON d.old_id = sh.node_id
WHERE d.old_id <> d.survivor_id;

-- Remap the losers' edges onto their survivors; the (source, target, kind)
-- UNIQUE key dedups (a pre-existing parallel edge wins). Only edges with a loser
-- endpoint are reinserted — survivor↔survivor edges are already correct.
-- The survivors all still exist (losers are deleted below), so the new rows
-- never violate the endpoint FKs.
INSERT OR IGNORE INTO edges (source, target, kind, derived, payload)
SELECT ds.survivor_id, dt.survivor_id, e.kind, e.derived, e.payload
FROM edges e
JOIN node_dedup_map ds ON ds.old_id = e.source
JOIN node_dedup_map dt ON dt.old_id = e.target
WHERE ds.old_id <> ds.survivor_id OR dt.old_id <> dt.survivor_id;

-- Delete the loser nodes. Their now-redundant original edges/shingles cascade
-- away (children before the parent, via ON DELETE CASCADE).
DELETE FROM nodes WHERE id IN (SELECT old_id FROM node_dedup_map WHERE old_id <> survivor_id);

DROP TABLE node_dedup_map;
-- === one node per symbol_id now holds; the UNIQUE rebuild copy is loss-free ==

-- Stash every table's rows in plain holders (CTAS: no FKs/constraints) with the
-- FULL post-migration-14 column set, so the rebuild preserves every annotation
-- column, every edge payload, and every (deduped) shingle — not just the shape.
-- `shingles` must be stashed too: dropping `nodes` below performs an implicit
-- DELETE that fires the shingles ON DELETE CASCADE, so an unstashed shingle store
-- would be wiped (and the dedup remap above lost with it).
CREATE TABLE edges_stash AS SELECT id, source, target, kind, derived, payload FROM edges;
CREATE TABLE shingles_stash AS SELECT node_id, hash FROM shingles;
CREATE TABLE nodes_stash AS
    SELECT id, symbol_id, kind, name, file_id, start_line, end_line,
           derived, exported, cyclomatic_complexity, line_count, fingerprint,
           is_dead, is_duplicate, layer_membership, test_evidence, is_test,
           body, max_nesting_depth, clone_group
    FROM nodes;

-- Children first: dropping edges and shingles removes every FK reference to
-- nodes, so the nodes drop cascades nothing. Clean under live FK enforcement.
DROP TABLE edges;
DROP TABLE shingles;
DROP TABLE nodes;

-- The rebuilt nodes table: the migration-13 column set and kind CHECK
-- (byte-identical, 1..=34 — CR-052 adds no node kind) plus the new
-- UNIQUE(symbol_id) constraint that makes the duplicate-symbol leak structurally
-- impossible (NFR-RA-13, ADR-46).
CREATE TABLE nodes (
    id                    INTEGER PRIMARY KEY,
    symbol_id             INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
    kind                  INTEGER NOT NULL CHECK (kind IN (1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34)),
    name                  TEXT NOT NULL,
    file_id               INTEGER REFERENCES files(id) ON DELETE SET NULL,
    start_line            INTEGER,
    end_line              INTEGER,
    derived               INTEGER NOT NULL DEFAULT 0 CHECK (derived IN (0,1)),
    exported              INTEGER NOT NULL DEFAULT 0 CHECK (exported IN (0,1)),
    cyclomatic_complexity INTEGER,
    line_count            INTEGER,
    fingerprint           TEXT,
    is_dead               INTEGER CHECK (is_dead IN (0,1)),
    is_duplicate          INTEGER CHECK (is_duplicate IN (0,1)),
    layer_membership      TEXT,
    test_evidence         INTEGER NOT NULL DEFAULT 0 CHECK (test_evidence IN (0,1)),
    is_test               INTEGER NOT NULL DEFAULT 0 CHECK (is_test IN (0,1)),
    body                  TEXT,
    max_nesting_depth     INTEGER,
    clone_group           INTEGER,
    UNIQUE (symbol_id)
) STRICT;

-- The rebuilt edges table: the migration-14 shape with the kind CHECK
-- UNCHANGED (1..=15) — edges is recreated only because it FK-references the
-- rebuilt nodes table.
CREATE TABLE edges (
    id      INTEGER PRIMARY KEY,
    source  INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    target  INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    kind    INTEGER NOT NULL CHECK (kind IN (1,2,3,4,5,6,7,8,9,10,11,12,13,14,15)),
    derived INTEGER NOT NULL DEFAULT 0 CHECK (derived IN (0,1)),
    payload TEXT,
    UNIQUE (source, target, kind)
) STRICT;

-- The rebuilt shingles store: the migration-10 shape verbatim, recreated only
-- because it FK-references the rebuilt nodes table.
CREATE TABLE shingles (
    node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    hash    INTEGER NOT NULL,
    PRIMARY KEY (node_id, hash)
) STRICT;

-- Copy rows back with identical ids (parents before children, so FK checks hold
-- and the FTS rebuild keys on the same ids). Every annotation column, edge
-- payload, and shingle is carried over verbatim — no row reverts to a default.
INSERT INTO nodes (id, symbol_id, kind, name, file_id, start_line, end_line,
                   derived, exported, cyclomatic_complexity, line_count, fingerprint,
                   is_dead, is_duplicate, layer_membership, test_evidence, is_test,
                   body, max_nesting_depth, clone_group)
    SELECT id, symbol_id, kind, name, file_id, start_line, end_line,
           derived, exported, cyclomatic_complexity, line_count, fingerprint,
           is_dead, is_duplicate, layer_membership, test_evidence, is_test,
           body, max_nesting_depth, clone_group
    FROM nodes_stash;
INSERT INTO edges (id, source, target, kind, derived, payload)
    SELECT id, source, target, kind, derived, payload FROM edges_stash;
INSERT INTO shingles (node_id, hash)
    SELECT node_id, hash FROM shingles_stash;

DROP TABLE nodes_stash;
DROP TABLE edges_stash;
DROP TABLE shingles_stash;

-- Recreate the indexes (migration 1 + 3 + 10). The old non-unique
-- idx_nodes_symbol_id is intentionally NOT recreated: the UNIQUE(symbol_id)
-- constraint above provides its own covering index, so a separate one would be
-- redundant.
CREATE INDEX idx_nodes_kind        ON nodes(kind);
CREATE INDEX idx_edges_source_kind ON edges(source, kind);
CREATE INDEX idx_edges_target_kind ON edges(target, kind);
CREATE INDEX idx_shingles_hash     ON shingles(hash);

-- Recreate the FTS sync triggers carrying `body` alongside `name` (migration 9/13
-- shape, FR-DB-03). The 'delete' command rows retract the OLD postings so the
-- external-content index never silently desyncs (NFR-RA-09, SRS §7.2 trap).
CREATE TRIGGER nodes_fts_ai AFTER INSERT ON nodes BEGIN
    INSERT INTO nodes_fts(rowid, name, body) VALUES (new.id, new.name, new.body);
END;
CREATE TRIGGER nodes_fts_ad AFTER DELETE ON nodes BEGIN
    INSERT INTO nodes_fts(nodes_fts, rowid, name, body) VALUES ('delete', old.id, old.name, old.body);
END;
CREATE TRIGGER nodes_fts_au AFTER UPDATE ON nodes BEGIN
    INSERT INTO nodes_fts(nodes_fts, rowid, name, body) VALUES ('delete', old.id, old.name, old.body);
    INSERT INTO nodes_fts(rowid, name, body) VALUES (new.id, new.name, new.body);
END;

-- Repopulate the external-content index from the final nodes. The dedup deleted
-- loser rows while the triggers were down, so a 'rebuild' is required to clear
-- their orphaned postings (NFR-RA-09) — unlike the rowid-preserving rebuilds of
-- migrations 3/8/13, which needed none.
INSERT INTO nodes_fts(nodes_fts) VALUES('rebuild');

-- Recreate the FR-AN-04 queryable view exactly as migration 13 left it
-- (projecting clone_group alongside the other verdicts).
CREATE VIEW annotations AS
    SELECT id AS node_id,
           cyclomatic_complexity,
           line_count,
           is_dead,
           is_duplicate,
           is_test,
           layer_membership,
           clone_group
    FROM nodes;
";

#[cfg(test)]
mod tests {
    use super::{
        MIGRATION_1, MIGRATION_10, MIGRATION_11, MIGRATION_12, MIGRATION_13, MIGRATION_14,
        MIGRATION_15, MIGRATION_16, MIGRATION_2, MIGRATION_3, MIGRATION_4, MIGRATION_8,
    };
    use crate::model::{EdgeKind, NodeKind, RefForm};

    /// Extract the `<column> IN (…)` discriminant list at the `nth` occurrence
    /// of the marker in the given migration SQL.
    fn check_discriminants(sql: &str, marker: &str, nth: usize) -> Vec<i32> {
        let (idx, _) = sql
            .match_indices(marker)
            .nth(nth)
            .expect("CHECK clause present");
        let tail = &sql[idx + marker.len()..];
        let inner = &tail[..tail.find(')').expect("closing paren")];
        inner
            .split(',')
            .map(|s| s.trim().parse::<i32>().expect("integer discriminant"))
            .collect()
    }

    /// The on-disk CHECK lists of the **latest** schema state must be exactly
    /// the model's frozen discriminants — no missing, extra, or reordered
    /// values. Guards against silent schema / model drift in BOTH directions
    /// (FR-DB-01). The authoritative `nodes.kind` CHECK lives in the migration-13
    /// rebuild (the CR-010 config-kind widening), where it is the first
    /// `kind IN (`; the authoritative `edges.kind` CHECK moved to the migration-14
    /// rebuild (the CR-011 artifact-edge widening), where it is the first
    /// `kind IN (` (the `unresolved_refs.kind` widening is the second).
    #[test]
    fn schema_check_matches_model_ontology() {
        let node_model: Vec<i32> = NodeKind::ALL.iter().map(|k| k.as_i32()).collect();
        let edge_model: Vec<i32> = EdgeKind::ALL.iter().map(|k| k.as_i32()).collect();
        assert_eq!(
            check_discriminants(MIGRATION_13, "kind IN (", 0),
            node_model,
            "nodes.kind CHECK (migration 13 rebuild) must equal NodeKind::ALL discriminants"
        );
        assert_eq!(
            check_discriminants(MIGRATION_14, "kind IN (", 0),
            edge_model,
            "edges.kind CHECK (migration 14 rebuild) must equal EdgeKind::ALL discriminants"
        );
        // The ledger's kind CHECK (the second `kind IN (` in migration 14) widens
        // in lockstep — an unindexed workspace-relative artifact ref must persist.
        assert_eq!(
            check_discriminants(MIGRATION_14, "kind IN (", 1),
            edge_model,
            "unresolved_refs.kind CHECK (migration 14) must equal EdgeKind::ALL discriminants"
        );
    }

    /// Migration 14 appends the two CR-011 artifact edge kinds (14, 15) to both
    /// the `edges` and `unresolved_refs` kind CHECKs and adds a nullable `payload`
    /// column to each — the forward-only edge-ontology widening ([FR-EX-05],
    /// [FR-DB-01], [ADR-26]). The load-bearing invariants: both CHECKs reach
    /// 1..=15, the rebuild drops and recreates both tables (SQLite cannot ALTER a
    /// CHECK), `payload` lands on both, the `UNIQUE` keys are unchanged, and
    /// `nodes` is left entirely untouched (CR-011 adds no node kind).
    #[test]
    fn migration_14_widens_edges_and_ledger_to_the_artifact_kinds() {
        // Both kind CHECKs widen to 1..=15.
        assert_eq!(
            check_discriminants(MIGRATION_14, "kind IN (", 0),
            (1..=15).collect::<Vec<i32>>(),
            "migration 14 edges.kind must widen to the artifact kinds (1..=15)"
        );
        assert_eq!(
            check_discriminants(MIGRATION_14, "kind IN (", 1),
            (1..=15).collect::<Vec<i32>>(),
            "migration 14 unresolved_refs.kind must widen to the artifact kinds (1..=15)"
        );
        // The relation payload column lands on both tables: a `payload` column
        // declaration is indented under its CREATE TABLE (`\n    payload`), one
        // per table — distinct from the word appearing in comment prose.
        assert_eq!(
            MIGRATION_14.matches("\n    payload").count(),
            2,
            "migration 14 must add a payload column to edges and unresolved_refs"
        );
        // A CHECK cannot be widened in place — both tables are rebuilt.
        assert!(
            MIGRATION_14.contains("DROP TABLE edges")
                && MIGRATION_14.contains("DROP TABLE unresolved_refs"),
            "migration 14 must rebuild both edges and unresolved_refs"
        );
        // The UNIQUE keys are unchanged — payload is an attribute, not a key.
        assert!(
            MIGRATION_14.contains("UNIQUE (source, target, kind)")
                && MIGRATION_14.contains("UNIQUE (source_symbol, target, form, kind)"),
            "migration 14 must keep both UNIQUE keys unchanged"
        );
        // nodes is untouched — CR-011 adds no node kind, so the FTS index, its
        // triggers, and the annotations view never enter this migration.
        assert!(
            !MIGRATION_14.contains("TABLE nodes") && !MIGRATION_14.contains("nodes_fts"),
            "migration 14 must not touch nodes (no new node kind)"
        );
    }

    /// Migration 15 adds the durable `project_metadata` key/value table (CR-004,
    /// ADR-20) and nothing else: it is a standalone additive table — no node/edge
    /// kind CHECK, no table rebuild, no FK — so it can never perturb the frozen
    /// discriminant contract the `schema_check_matches_model_ontology` test pins.
    #[test]
    fn migration_15_adds_the_project_metadata_kv_table_only() {
        assert!(
            MIGRATION_15.contains("CREATE TABLE project_metadata"),
            "migration 15 must create the project_metadata table"
        );
        assert!(
            MIGRATION_15.contains("key   TEXT PRIMARY KEY")
                && MIGRATION_15.contains("value TEXT NOT NULL"),
            "project_metadata is a (key PRIMARY KEY, value NOT NULL) kv table"
        );
        // Additive only — it never rebuilds an existing table or touches the
        // node/edge ontology, so the discriminant contract is untouched.
        assert!(
            !MIGRATION_15.contains("DROP TABLE")
                && !MIGRATION_15.contains("kind IN (")
                && !MIGRATION_15.contains("nodes_fts"),
            "migration 15 must be a standalone additive table (no rebuild, no ontology change)"
        );
    }

    /// Migration 16 establishes the one-node-per-symbol invariant (S-201,
    /// [CR-052], [ADR-46], [NFR-RA-13]): it rebuilds `nodes` with a
    /// `UNIQUE(symbol_id)` constraint and recreates the kind CHECKs
    /// **byte-identical** to their authoritative widenings — CR-052 burns no node
    /// or edge kind, so the `schema_check_matches_model_ontology` guard still reads
    /// migration 13 (nodes) / 14 (edges). The load-bearing migration invariants:
    /// the constraint lands, `nodes.kind` stays 1..=34 and `edges.kind` stays
    /// 1..=15 byte-for-byte, the dedup pass keeps the `MIN(id)` survivor and remaps
    /// edges/shingles with `INSERT OR IGNORE`, and the FTS triggers + annotations
    /// view are restored (the index repopulated with `'rebuild'` after the dedup
    /// deletes).
    #[test]
    fn migration_16_enforces_unique_symbol_id_via_dedup_rebuild() {
        // The new constraint is the whole point of the migration.
        assert!(
            MIGRATION_16.contains("UNIQUE (symbol_id)"),
            "migration 16 must add the UNIQUE(symbol_id) constraint (NFR-RA-13, ADR-46)"
        );
        // A constraint cannot be added in place — nodes (and mechanically edges,
        // which FK-references it) are rebuilt.
        assert!(
            MIGRATION_16.contains("DROP TABLE nodes") && MIGRATION_16.contains("DROP TABLE edges"),
            "migration 16 must rebuild nodes (and mechanically edges) — SQLite cannot add a constraint in place"
        );

        // The kind CHECKs are recreated byte-identical to their authoritative
        // form — CR-052 burns no node or edge kind. nodes.kind is the first
        // `kind IN (`, edges.kind the second.
        assert_eq!(
            check_discriminants(MIGRATION_16, "kind IN (", 0),
            (1..=34).collect::<Vec<i32>>(),
            "migration 16 nodes.kind must stay 1..=34 (no new node kind at CR-052)"
        );
        assert_eq!(
            check_discriminants(MIGRATION_16, "kind IN (", 1),
            (1..=15).collect::<Vec<i32>>(),
            "migration 16 edges.kind must stay 1..=15 (no new edge kind at CR-052)"
        );
        // Byte-identical to the authoritative widenings (migration 13 nodes / 14
        // edges) so a fresh database ending at migration 16 still matches
        // NodeKind/EdgeKind::ALL — the explicit "CHECK untouched" assertion.
        let kind_clause = |sql: &str, nth: usize| {
            let i = sql
                .match_indices("kind IN (")
                .nth(nth)
                .expect("CHECK clause")
                .0;
            let tail = &sql[i..];
            tail[..tail.find(')').expect("closing paren") + 1].to_string()
        };
        assert_eq!(
            kind_clause(MIGRATION_16, 0),
            kind_clause(MIGRATION_13, 0),
            "migration 16's nodes.kind CHECK must be byte-identical to migration 13's"
        );
        assert_eq!(
            kind_clause(MIGRATION_16, 1),
            kind_clause(MIGRATION_14, 0),
            "migration 16's edges.kind CHECK must be byte-identical to migration 14's"
        );

        // The dedup pass keeps the MIN(id) survivor and remaps dependents with
        // INSERT OR IGNORE before deleting losers (ADR-46).
        assert!(
            MIGRATION_16.contains("MIN(id) AS survivor_id"),
            "migration 16 must keep the MIN(id) survivor per symbol_id"
        );
        assert!(
            MIGRATION_16.contains("INSERT OR IGNORE INTO shingles")
                && MIGRATION_16.contains("INSERT OR IGNORE INTO edges"),
            "migration 16 must remap shingles and edges onto survivors with INSERT OR IGNORE"
        );
        // shingles must be stashed and rebuilt too — dropping nodes would
        // otherwise cascade-wipe it (and the dedup remap with it).
        assert!(
            MIGRATION_16.contains("CREATE TABLE shingles_stash")
                && MIGRATION_16.contains("DROP TABLE shingles")
                && MIGRATION_16.contains("CREATE TABLE shingles ("),
            "migration 16 must stash and rebuild shingles (it FK-references nodes)"
        );

        // The FTS triggers and the annotations view are restored around the
        // rebuild; the triggers carry `body` (the migration-9/13 shape) and the
        // index is repopulated with 'rebuild' (the dedup removed rows, NFR-RA-09).
        for trigger in ["nodes_fts_ai", "nodes_fts_ad", "nodes_fts_au"] {
            assert!(
                MIGRATION_16.contains(&format!("CREATE TRIGGER {trigger}")),
                "migration 16 must recreate the {trigger} FTS trigger (NFR-RA-09)"
            );
        }
        assert!(
            MIGRATION_16.contains("new.name, new.body"),
            "migration 16's FTS triggers must carry body (migration-9/13 shape, FR-DG-05)"
        );
        assert!(
            MIGRATION_16.contains("VALUES('rebuild')"),
            "migration 16 must rebuild the FTS index after the dedup deletes (NFR-RA-09)"
        );
        assert!(
            MIGRATION_16.contains("DROP VIEW annotations")
                && MIGRATION_16.contains("CREATE VIEW annotations")
                && MIGRATION_16.contains("clone_group"),
            "migration 16 must recreate the annotations view with clone_group (migration-13 shape)"
        );
    }

    /// Migration 13 widens **only** the node-kind CHECK to the CR-010 config
    /// kinds (`nodes.kind` to 1..=34, FR-EX-05/FR-DB-01) via a copy-rebuild — and
    /// leaves the edge ontology untouched: CR-010 is a `Contains`-only layer that
    /// burns no edge kind ([ADR-25]). The load-bearing migration invariant: the
    /// edges.kind CHECK in migration 13 is **byte-identical** to migration 10's
    /// (still 1..=13), and `unresolved_refs` is not rebuilt at all.
    #[test]
    fn migration_13_widens_nodes_only_and_leaves_edges_byte_identical() {
        // The node-kind CHECK widens to the twelve config kinds (1..=34).
        assert_eq!(
            check_discriminants(MIGRATION_13, "kind IN (", 0),
            (1..=34).collect::<Vec<i32>>(),
            "migration 13 nodes.kind must widen to the config kinds (1..=34)"
        );
        // The edge-kind CHECK is the SECOND `kind IN (` in migration 13 (after
        // nodes) and must stay UNCHANGED at 1..=13 — CR-010 added no edge kind.
        // (The authoritative edges CHECK that tracks `EdgeKind::ALL` later moved
        // to migration 14, which appends the two artifact kinds 14..=15.)
        assert_eq!(
            check_discriminants(MIGRATION_13, "kind IN (", 1),
            (1..=13).collect::<Vec<i32>>(),
            "migration 13 edges.kind must stay 1..=13 (no new edge kind at CR-010)"
        );
        // The edges CHECK clause is byte-identical to migration 10's authoritative
        // one — the explicit "edge CHECK untouched" assertion the sprint calls for.
        let edge_clause = |sql: &str| {
            let i = sql
                .match_indices("kind IN (")
                .nth(1)
                .expect("edges CHECK")
                .0;
            let tail = &sql[i..];
            tail[..tail.find(')').expect("closing paren") + 1].to_string()
        };
        assert_eq!(
            edge_clause(MIGRATION_13),
            edge_clause(MIGRATION_10),
            "migration 13's edges.kind CHECK must be byte-identical to migration 10's"
        );
        // A CHECK cannot be widened in place — the rebuild drops and recreates
        // both tables (edges is dropped only because it FK-references nodes).
        assert!(
            MIGRATION_13.contains("DROP TABLE nodes") && MIGRATION_13.contains("DROP TABLE edges"),
            "migration 13 must rebuild nodes (and mechanically edges) — SQLite cannot ALTER a CHECK"
        );
        // unresolved_refs is NOT rebuilt — CR-010 adds no edge kind, so the
        // ledger's kind CHECK (migration 10, 1..=13) is left exactly as-is.
        assert!(
            !MIGRATION_13.contains("unresolved_refs"),
            "migration 13 must not touch unresolved_refs — no new edge kind (Contains-only)"
        );
        // The FTS triggers and the annotations view are restored around the
        // rebuild; the triggers carry `body` (the migration-9 shape).
        for trigger in ["nodes_fts_ai", "nodes_fts_ad", "nodes_fts_au"] {
            assert!(
                MIGRATION_13.contains(&format!("CREATE TRIGGER {trigger}")),
                "migration 13 must recreate the {trigger} FTS trigger (NFR-RA-09)"
            );
        }
        assert!(
            MIGRATION_13.contains("new.name, new.body"),
            "migration 13's FTS triggers must carry body (migration-9 shape, FR-DG-05)"
        );
        assert!(
            MIGRATION_13.contains("DROP VIEW annotations")
                && MIGRATION_13.contains("CREATE VIEW annotations")
                && MIGRATION_13.contains("clone_group"),
            "migration 13 must recreate the annotations view with clone_group (migration-11 shape)"
        );
    }

    /// Migration 3's rebuilt CHECK lists are a shipped, frozen string — they
    /// must keep the 1..=17 / 1..=10 widening that was current when they shipped
    /// (the frozen-string rule). The CR-003 doc-kind widening happened in
    /// migration 8's rebuild, never by editing migration 3.
    #[test]
    fn migration_3_checks_remain_frozen() {
        assert_eq!(
            check_discriminants(MIGRATION_3, "kind IN (", 0),
            (1..=17).collect::<Vec<i32>>(),
            "MIGRATION_3 nodes.kind was edited — shipped migrations are immutable"
        );
        assert_eq!(
            check_discriminants(MIGRATION_3, "kind IN (", 1),
            (1..=10).collect::<Vec<i32>>(),
            "MIGRATION_3 edges.kind was edited — shipped migrations are immutable"
        );
    }

    /// Migration 8 widens the kind CHECKs to the CR-003 documentation kinds:
    /// `nodes.kind` to 1..=22 and `edges.kind` to 1..=12 (FR-EX-05, FR-DB-01),
    /// and must do so via a copy-rebuild — never an additive `ALTER` (SQLite
    /// cannot widen a CHECK in place). The FTS triggers and the `annotations`
    /// view are recreated so neither is left dangling.
    #[test]
    fn migration_8_widens_node_and_edge_checks_via_rebuild() {
        assert_eq!(
            check_discriminants(MIGRATION_8, "kind IN (", 0),
            (1..=22).collect::<Vec<i32>>(),
            "migration 8 nodes.kind must widen to the documentation kinds (1..=22)"
        );
        assert_eq!(
            check_discriminants(MIGRATION_8, "kind IN (", 1),
            (1..=12).collect::<Vec<i32>>(),
            "migration 8 edges.kind must widen to the documentation edges (1..=12)"
        );
        // A CHECK cannot be widened in place — the rebuild drops and recreates.
        assert!(
            MIGRATION_8.contains("DROP TABLE nodes") && MIGRATION_8.contains("DROP TABLE edges"),
            "migration 8 must rebuild both tables (SQLite cannot ALTER a CHECK)"
        );
        // The FTS triggers and the annotations view are restored around the rebuild.
        for trigger in ["nodes_fts_ai", "nodes_fts_ad", "nodes_fts_au"] {
            assert!(
                MIGRATION_8.contains(&format!("CREATE TRIGGER {trigger}")),
                "migration 8 must recreate the {trigger} FTS trigger (NFR-RA-09)"
            );
        }
        assert!(
            MIGRATION_8.contains("DROP VIEW annotations")
                && MIGRATION_8.contains("CREATE VIEW annotations"),
            "migration 8 must recreate the annotations view (FR-AN-04)"
        );
    }

    /// Migration 1's CHECK lists are shipped, frozen strings — they must keep
    /// the *original* lists forever (the frozen-string rule). The node-kind
    /// widening happened in migration 3's rebuild, never by editing v1.
    #[test]
    fn migration_1_checks_remain_frozen() {
        assert_eq!(
            check_discriminants(MIGRATION_1, "kind IN (", 0),
            (1..=15).collect::<Vec<i32>>(),
            "MIGRATION_1 nodes.kind was edited — shipped migrations are immutable"
        );
        assert_eq!(
            check_discriminants(MIGRATION_1, "kind IN (", 1),
            (1..=10).collect::<Vec<i32>>(),
            "MIGRATION_1 edges.kind was edited — shipped migrations are immutable"
        );
    }

    /// Migration 4 must persist **raw + normalized** columns for all five
    /// metrics plus the counts / `empty` flag / optional sha [FR-QM-07] names,
    /// and `aggregate_signal` must be nullable (the ADR-12 empty sentinel) —
    /// no NOT NULL on that column.
    #[test]
    fn migration_4_covers_the_fr_qm_07_snapshot_shape() {
        for metric in [
            "modularity",
            "acyclicity",
            "depth",
            "equality",
            "redundancy",
        ] {
            for suffix in ["raw", "normalized"] {
                let column = format!("{metric}_{suffix}");
                assert!(
                    MIGRATION_4.contains(&column),
                    "metric_snapshots must persist {column} (FR-QM-07)"
                );
            }
        }
        for column in [
            "created_at",
            "commit_sha",
            "node_count",
            "edge_count",
            "function_count",
            "empty",
            "aggregate_signal",
        ] {
            assert!(
                MIGRATION_4.contains(column),
                "metric_snapshots must carry {column} (FR-QM-07)"
            );
        }
        let signal_line = MIGRATION_4
            .lines()
            .find(|l| l.trim_start().starts_with("aggregate_signal"))
            .expect("aggregate_signal column present");
        assert!(
            !signal_line.contains("NOT NULL"),
            "aggregate_signal must be nullable — NULL is the ADR-12 empty-graph sentinel"
        );
    }

    /// Migration 5 carries the three governance tables in their SRS §5.1
    /// shapes (S-020): `baseline` (scope PK → snapshot_id), `violations`
    /// (rule_type/rule_key/node_id/message/severity), and the `rules_cache`
    /// singleton (rules_hash + parsed_json).
    #[test]
    fn migration_5_covers_the_governance_table_shapes() {
        use super::MIGRATION_5;

        for table in ["baseline", "violations", "rules_cache"] {
            assert!(
                MIGRATION_5.contains(&format!("CREATE TABLE {table}")),
                "migration 5 must create {table} (SRS §5.1)"
            );
        }
        for column in ["scope", "snapshot_id"] {
            assert!(
                MIGRATION_5.contains(column),
                "baseline must carry {column} (FR-GV-04)"
            );
        }
        for column in ["rule_type", "rule_key", "node_id", "message", "severity"] {
            assert!(
                MIGRATION_5.contains(column),
                "violations must carry {column} (FR-GV-02)"
            );
        }
        for column in ["rules_hash", "parsed_json"] {
            assert!(
                MIGRATION_5.contains(column),
                "rules_cache must carry {column} (FR-GV-01)"
            );
        }
        // The severity vocabulary is CHECK-bound to the FR-GV-03 pair.
        assert!(
            MIGRATION_5.contains("severity IN ('error','warning')"),
            "violations.severity must be CHECK-bound"
        );
        // rules_cache is a singleton by construction.
        assert!(
            MIGRATION_5.contains("CHECK (id = 1)"),
            "rules_cache must be a singleton row"
        );
    }

    /// Migration 6 adds the two test-annotation columns as **additive**
    /// `ALTER TABLE` statements (forward-only, no rebuild — FR-AN-05,
    /// NFR-MA-06) and recreates the `annotations` view to project `is_test`.
    /// Both columns are `NOT NULL DEFAULT 0` (positive-evidence classification,
    /// not the tri-state `is_dead`/`is_duplicate` carry).
    #[test]
    fn migration_6_adds_test_columns_and_widens_the_view() {
        use super::MIGRATION_6;

        // Additive only — never a destructive rebuild of `nodes`.
        assert!(
            MIGRATION_6.contains("ALTER TABLE nodes ADD COLUMN test_evidence"),
            "migration 6 must add test_evidence additively (FR-AN-04)"
        );
        assert!(
            MIGRATION_6.contains("ALTER TABLE nodes ADD COLUMN is_test"),
            "migration 6 must add is_test additively (FR-AN-05)"
        );
        assert!(
            !MIGRATION_6.contains("DROP TABLE"),
            "migration 6 must not rebuild a table — additive forward-only (NFR-MA-06)"
        );
        // Both columns are positive-evidence booleans, not the tri-state NULL.
        for column in ["test_evidence", "is_test"] {
            let line = MIGRATION_6
                .lines()
                .find(|l| l.contains(&format!("ADD COLUMN {column}")))
                .unwrap_or_else(|| panic!("{column} ADD COLUMN line present"));
            assert!(
                line.contains("NOT NULL DEFAULT 0"),
                "{column} must be NOT NULL DEFAULT 0 (FR-AN-05)"
            );
        }
        // The FR-AN-04 view now projects is_test alongside the other verdicts;
        // test_evidence stays an internal input, off the view.
        assert!(
            MIGRATION_6.contains("DROP VIEW annotations")
                && MIGRATION_6.contains("CREATE VIEW annotations"),
            "the annotations view is recreated to expose is_test (FR-AN-04)"
        );
        let view_start = MIGRATION_6
            .find("CREATE VIEW annotations")
            .expect("view recreation present");
        let view_sql = &MIGRATION_6[view_start..];
        assert!(
            view_sql.contains("is_test"),
            "the recreated annotations view must project is_test (FR-AN-05)"
        );
        assert!(
            !view_sql.contains("test_evidence"),
            "test_evidence is an internal input, never on the queryable view"
        );
    }

    /// Migration 7 adds the two production-scope `metric_snapshots` columns as
    /// **additive** `ALTER TABLE` statements (forward-only, no rebuild —
    /// FR-QM-08, FR-GV-10, NFR-MA-06). `metric_version` must default to `1` so a
    /// pre-upgrade baseline reads as the old test-inclusive semantics and the
    /// gate auto-re-baselines (UAT-GV-06).
    #[test]
    fn migration_7_adds_production_scope_columns_additively() {
        use super::MIGRATION_7;

        for column in ["test_function_count", "metric_version"] {
            assert!(
                MIGRATION_7.contains(&format!("ALTER TABLE metric_snapshots ADD COLUMN {column}")),
                "migration 7 must add {column} additively (FR-QM-07, FR-GV-10)"
            );
        }
        assert!(
            !MIGRATION_7.contains("DROP TABLE") && !MIGRATION_7.contains("CREATE TABLE"),
            "migration 7 must not rebuild a table — additive forward-only (NFR-MA-06)"
        );
        // test_function_count defaults to 0 (pre-upgrade snapshots excluded none).
        let tfc_line = MIGRATION_7
            .lines()
            .find(|l| l.contains("ADD COLUMN test_function_count"))
            .expect("test_function_count line present");
        assert!(
            tfc_line.contains("NOT NULL DEFAULT 0"),
            "test_function_count must be NOT NULL DEFAULT 0 (FR-QM-07)"
        );
        // metric_version DEFAULT 1 is the re-baseline trigger: pre-upgrade rows
        // read as the old (test-inclusive) semantics version.
        let version_line = MIGRATION_7
            .lines()
            .find(|l| l.contains("ADD COLUMN metric_version"))
            .expect("metric_version line present");
        assert!(
            version_line.contains("NOT NULL DEFAULT 1"),
            "metric_version must default to 1 so an old baseline is detected as \
             incomparable (FR-GV-10, UAT-GV-06)"
        );
    }

    /// The migration-2 `unresolved_refs.form` CHECK must equal the model's
    /// frozen [`RefForm`] discriminants — `form` is unchanged by CR-003 (still
    /// 1..=4), so this guard reads the original migration-2 string.
    #[test]
    fn migration_2_form_check_matches_ref_form_ontology() {
        let form_model: Vec<i32> = RefForm::ALL.iter().map(|f| f.as_i32()).collect();
        assert_eq!(
            check_discriminants(MIGRATION_2, "form IN (", 0),
            form_model,
            "unresolved_refs.form CHECK must equal RefForm::ALL discriminants"
        );
    }

    /// Migration 2's shipped `unresolved_refs.kind` CHECK is frozen at the
    /// edge kinds that existed when it shipped (1..=10) — the CR-003 widening to
    /// the doc edges happens in migration 8's rebuild, never by editing v2.
    #[test]
    fn migration_2_kind_check_remains_frozen() {
        assert_eq!(
            check_discriminants(MIGRATION_2, "kind IN (", 0),
            (1..=10).collect::<Vec<i32>>(),
            "MIGRATION_2 unresolved_refs.kind was edited — shipped migrations are immutable"
        );
    }

    /// The **latest** `unresolved_refs.kind` CHECK now lives in migration 14's
    /// rebuild (the second `kind IN (` there, after edges) and must equal the
    /// model's `EdgeKind::ALL` — the same exact drift guard the nodes/edges
    /// CHECKs carry, now that the ledger retries cross-artifact references too
    /// (CR-011/ADR-26, FR-CG-07). The migration-10 ledger CHECK is frozen at the
    /// kinds current when it shipped (1..=13) and is no longer authoritative.
    #[test]
    fn latest_unresolved_refs_kind_check_matches_edge_ontology() {
        let edge_model: Vec<i32> = EdgeKind::ALL.iter().map(|k| k.as_i32()).collect();
        assert_eq!(
            check_discriminants(MIGRATION_14, "kind IN (", 1),
            edge_model,
            "unresolved_refs.kind CHECK (migration 14 rebuild) must equal EdgeKind::ALL"
        );
        // Migration 10's ledger CHECK is frozen at its shipped value (1..=13).
        assert_eq!(
            check_discriminants(MIGRATION_10, "kind IN (", 1),
            (1..=13).collect::<Vec<i32>>(),
            "MIGRATION_10 unresolved_refs.kind is frozen at 1..=13"
        );
    }

    /// Migration 8's shipped `unresolved_refs.kind` CHECK is frozen at the edge
    /// kinds current when it shipped (1..=12) — the CR-005 widening to the
    /// `Accesses` kind happens in migration 10's rebuild, never by editing v8.
    #[test]
    fn migration_8_unresolved_refs_kind_check_remains_frozen() {
        assert_eq!(
            check_discriminants(MIGRATION_8, "kind IN (", 2),
            (1..=12).collect::<Vec<i32>>(),
            "MIGRATION_8 unresolved_refs.kind was edited — shipped migrations are immutable"
        );
    }

    /// Migration 10 widens the edge-bearing CHECKs to the CR-005 `Accesses` kind
    /// (`edges.kind` and `unresolved_refs.kind` to 1..=13, FR-EX-08, FR-DB-01)
    /// via a copy-rebuild — never an additive `ALTER` (SQLite cannot widen a
    /// CHECK in place). The `max_nesting_depth` column and the `shingles` table
    /// are additive, and `nodes` is never rebuilt (its FTS index and the
    /// `annotations` view stay untouched).
    #[test]
    fn migration_10_adds_structural_facts_and_widens_edge_checks() {
        // The Accesses widening on both edge-bearing tables (1..=13).
        assert_eq!(
            check_discriminants(MIGRATION_10, "kind IN (", 0),
            (1..=13).collect::<Vec<i32>>(),
            "migration 10 edges.kind must widen to the Accesses edge (1..=13)"
        );
        assert_eq!(
            check_discriminants(MIGRATION_10, "kind IN (", 1),
            (1..=13).collect::<Vec<i32>>(),
            "migration 10 unresolved_refs.kind must widen to the Accesses edge (1..=13)"
        );
        // The CHECK widening is a copy-rebuild of edges (+ unresolved_refs)…
        assert!(
            MIGRATION_10.contains("DROP TABLE edges")
                && MIGRATION_10.contains("DROP TABLE unresolved_refs"),
            "migration 10 must rebuild the edge-bearing tables (SQLite cannot ALTER a CHECK)"
        );
        // …and must NOT rebuild nodes (the only nodes change is additive), so
        // the FTS triggers and the annotations view are genuinely untouched —
        // unlike the migration-3/8 nodes rebuilds.
        assert!(
            !MIGRATION_10.contains("DROP TABLE nodes"),
            "migration 10 must not rebuild nodes — the column is additive"
        );
        assert!(
            !MIGRATION_10.contains("DROP TRIGGER"),
            "migration 10 must not touch the FTS triggers (nodes is not rebuilt)"
        );
        assert!(
            !MIGRATION_10.contains("DROP VIEW"),
            "migration 10 must not touch the annotations view (nodes is not rebuilt)"
        );
        assert!(
            MIGRATION_10.contains("ALTER TABLE nodes ADD COLUMN max_nesting_depth"),
            "migration 10 must add max_nesting_depth additively (FR-EX-07)"
        );
        assert!(
            MIGRATION_10.contains("CREATE TABLE shingles"),
            "migration 10 must create the shingles store (FR-EX-09)"
        );
        // The form CHECK is unchanged — CR-005 touches no ref form.
        assert_eq!(
            check_discriminants(MIGRATION_10, "form IN (", 0),
            RefForm::ALL
                .iter()
                .map(|f| f.as_i32())
                .collect::<Vec<i32>>(),
            "migration 10 must keep unresolved_refs.form at RefForm::ALL"
        );
    }

    /// Migration 11 adds the near-clone `clone_group` annotation additively
    /// (S-043, [FR-AN-06]): `nodes` is never rebuilt (the column is an
    /// in-place `ALTER`), the FTS triggers are untouched, and only the
    /// `annotations` view is recreated to project the new column.
    #[test]
    fn migration_11_adds_clone_group_additively_and_recreates_the_view() {
        assert!(
            MIGRATION_11.contains("ALTER TABLE nodes ADD COLUMN clone_group"),
            "migration 11 must add clone_group additively (FR-AN-06, FR-AN-04)"
        );
        // Additive: no nodes rebuild, no FTS trigger churn.
        assert!(
            !MIGRATION_11.contains("DROP TABLE nodes"),
            "migration 11 must not rebuild nodes — the column is additive (NFR-MA-06)"
        );
        assert!(
            !MIGRATION_11.contains("DROP TRIGGER"),
            "migration 11 must not touch the FTS triggers (nodes is not rebuilt)"
        );
        // The view must be recreated to project clone_group (a view cannot gain
        // a column in place); the projection still exposes the FR-AN-04 columns.
        assert!(
            MIGRATION_11.contains("DROP VIEW annotations")
                && MIGRATION_11.contains("CREATE VIEW annotations"),
            "migration 11 must recreate the annotations view (FR-AN-04)"
        );
        for column in ["is_duplicate", "is_test", "layer_membership", "clone_group"] {
            assert!(
                MIGRATION_11.contains(column),
                "the recreated annotations view must project {column}"
            );
        }
    }

    /// Migration 12 widens `metric_snapshots` with the CR-005 extended metric
    /// set additively (S-044, [FR-QM-09]..[FR-QM-14], [ADR-21]): the five new
    /// raw+normalized dimension pairs, the two applicability flags, and the
    /// effective-thresholds hash. The append-only ledger is never rebuilt — every
    /// new column is an in-place `ALTER` (the migration-7 shape, NFR-MA-06).
    #[test]
    fn migration_12_widens_metric_snapshots_with_the_extended_set_additively() {
        // The five new dimensions' raw + normalized pairs and the thresholds hash.
        for column in [
            "nesting_raw",
            "nesting_normalized",
            "conciseness_raw",
            "conciseness_normalized",
            "cohesion_raw",
            "cohesion_normalized",
            "focus_raw",
            "focus_normalized",
            "uniqueness_raw",
            "uniqueness_normalized",
            "thresholds_hash",
        ] {
            assert!(
                MIGRATION_12.contains(&format!("ADD COLUMN {column}")),
                "migration 12 must add the {column} column (FR-QM-07, FR-QM-14)"
            );
        }
        // The two drop-out dimensions carry a 0/1 applicability flag (FR-QM-11/12).
        for flag in ["cohesion_applicable", "focus_applicable"] {
            assert!(
                MIGRATION_12.contains(&format!("ADD COLUMN {flag}"))
                    && MIGRATION_12.contains(&format!("{flag} IN (0,1)")),
                "migration 12 must add the {flag} flag with a 0/1 CHECK (FR-QM-11, FR-QM-12)"
            );
        }
        // Additive only: the append-only ledger is never rebuilt (NFR-MA-06).
        assert!(
            !MIGRATION_12.contains("DROP TABLE metric_snapshots"),
            "migration 12 must not rebuild metric_snapshots — the columns are additive (NFR-MA-06)"
        );
    }
}
