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
//! the discriminant contract frozen in [`crate::model`] ([NodeKind] 1..=37,
//! [EdgeKind] 1..=17). The `schema_check_matches_model_ontology` test in
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
    (17, MIGRATION_17),
    (18, MIGRATION_18),
    (19, MIGRATION_19),
    (20, MIGRATION_20),
    (21, MIGRATION_21),
    (22, MIGRATION_22),
    (23, MIGRATION_23),
    (24, MIGRATION_24),
    (25, MIGRATION_25),
    (26, MIGRATION_26),
    (27, MIGRATION_27),
    (28, MIGRATION_28),
    (29, MIGRATION_29),
    (30, MIGRATION_30),
    (31, MIGRATION_31),
    (32, MIGRATION_32),
    (33, MIGRATION_33),
    (34, MIGRATION_34),
    (35, MIGRATION_35),
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
///   the metrics scope filter ([FR-QM-08]), the `[[require_tested]]` contract
///   ([FR-GV-13]), and the dead-code live roots ([FR-AN-01]) all consume — no
///   detector re-derives it.
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
/// `(key, value)` text pairs. Its first inhabitant is the `config_fingerprint`
/// row: a deterministic hash of the admission-relevant configuration
/// ([`crate::config::Config::admission_fingerprint`]) recorded at the last full
/// reconciliation, so the pipeline can detect that the admission policy has
/// changed and purge now-unadmitted files exactly once per change ([CR-004]
/// §3.1). Forward-only and standalone: no data migration, no rebuild, no FK to
/// any existing table.
///
/// # The table later stories reach for
/// Every small **scalar** per-project fact added since has landed here as another
/// row rather than as a column of its own — the monotonic graph revision
/// ([`super::GRAPH_REVISION_KEY`], CR-027), the index-time LOC roll-up
/// ([`crate::perf::INDEXED_LOC_KEY`], [`crate::perf::TEST_LOC_KEY`], CR-085) and
/// the last-full-index stamp ([`super::LAST_FULL_INDEX_AT_KEY`], [CR-130]).
/// Structured facts still earn their own tables — migration 19's `config_sources`
/// and `config_values` are not rows here.
///
/// The stamp is the one [CR-004] §3.1 named in passing, as the in-memory value
/// this table could not yet replace: `status` read an in-process `AtomicU64` that
/// only the indexing process could ever have set, so a read-only `status`
/// reported a fully indexed project as never indexed. Nothing about the table
/// changed to admit it — a new key was enough, which is the property the kv shape
/// was chosen for.
///
/// [CR-004]: ../../../../docs/requests/CR-004-config-change-reconciliation.md
/// [CR-130]: ../../../../docs/requests/CR-130-a-readout-names-a-remediation-that-cannot-apply.md
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

/// Migration 17 — first-class broker `Topic`/`Producer`/`Consumer` node kinds
/// and `Publishes`/`Subscribes` edge kinds (S-255, [CR-061], [ADR-55],
/// [FR-WS-11], [FR-DB-01]).
///
/// Promotes the M2 ledger-only broker binding ([ADR-54]) to first-class graph
/// entities: a per-repo `Topic` a `Producer` publishes to and a `Consumer`
/// subscribes from, visible before any cross-repo match. This burns three node
/// discriminants (`NodeKind` 1..=34 → 1..=37) and two edge discriminants
/// (`EdgeKind` 1..=15 → 1..=17) — the frozen on-disk contract widens in
/// lockstep with [`crate::model::kinds`] exactly as every prior kind addition
/// has ([FR-DB-01]).
///
/// SQLite cannot widen a `CHECK` in place, so — exactly as migrations 14 and 16
/// did — every table carrying a kind `CHECK` is rebuilt copy-style, following
/// the referenced-table-safe procedure: drop the FTS triggers and the
/// `annotations` view, stash every table's rows in plain holders (`shingles`
/// too, since dropping `nodes` performs an implicit `DELETE` that would
/// otherwise cascade through its `ON DELETE CASCADE` FK and wipe them), drop
/// children first (`edges`/`shingles` before `nodes`; `unresolved_refs`
/// independently — it carries no FK to `nodes`), recreate all four under their
/// final names with the widened `CHECK`s, copy every row back with identical
/// ids, recreate the indexes, and recreate the FTS triggers + `annotations`
/// view.
///
/// `unresolved_refs.kind` widens in lockstep with `edges.kind` — the same
/// dual-widening migration 14 did for the CR-011 artifact edges — so an
/// unresolved cross-member topic binding can persist in the ledger and retry
/// on sync ([NFR-RA-05]), exactly as an unindexed artifact reference does.
///
/// This migration only widens what the schema *accepts*; it inserts no `Topic`
/// row. Emission is [S-256]'s concern, so a graph that indexes no broker topic
/// is **byte-for-byte unaffected** — every row keeps its id, and (unlike
/// migration 16's dedup rebuild) no row is deleted, so the FTS index needs no
/// `'rebuild'` repopulation: the copy-back is a pure identity pass, the same
/// shape migration 13's widening took.
///
/// [CR-061]: ../../../../docs/requests/CR-061-multi-repo-workspace-federation.md
/// [ADR-55]: ../../../../docs/specs/architecture/decisions/ADR-55.md
/// [ADR-54]: ../../../../docs/specs/architecture/decisions/ADR-54.md
/// [FR-WS-11]: ../../../../docs/specs/requirements/FR-WS-11.md
/// [FR-DB-01]: ../../../../docs/specs/requirements/FR-DB-01.md
/// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
/// [S-256]: ../../../../docs/planning/journal.md#s-256-promote-broker-coupling-to-first-class-topics
const MIGRATION_17: &str = "\
-- Drop the FTS sync triggers around the nodes rebuild: the copy below must not
-- double-index, and the triggers are recreated verbatim afterwards (migration
-- 9/13/16 shape, carrying `body`). No node is deleted by this migration
-- (pure additive widening), so no 'rebuild' repopulation is needed afterwards
-- (mirrors migration 13, not migration 16's dedup rebuild).
DROP TRIGGER nodes_fts_ai;
DROP TRIGGER nodes_fts_ad;
DROP TRIGGER nodes_fts_au;

-- The annotations view (FR-AN-04) reads native `nodes` columns; drop it so it
-- never references the transiently-dropped table, and recreate it unchanged
-- (migration-16 projection) once the rebuild completes.
DROP VIEW annotations;

-- Stash every table's rows in plain holders (CTAS: no FKs/constraints) with the
-- FULL post-migration-16 column set, so the rebuild preserves every annotation
-- column, every edge payload, and every shingle — not just the shape.
-- `shingles` must be stashed too: dropping `nodes` below performs an implicit
-- DELETE that fires the shingles ON DELETE CASCADE, so an unstashed shingle
-- store would be wiped.
CREATE TABLE edges_stash AS SELECT id, source, target, kind, derived, payload FROM edges;
CREATE TABLE shingles_stash AS SELECT node_id, hash FROM shingles;
CREATE TABLE nodes_stash AS
    SELECT id, symbol_id, kind, name, file_id, start_line, end_line,
           derived, exported, cyclomatic_complexity, line_count, fingerprint,
           is_dead, is_duplicate, layer_membership, test_evidence, is_test,
           body, max_nesting_depth, clone_group
    FROM nodes;
CREATE TABLE unresolved_refs_stash AS
    SELECT id, file_id, source_symbol, target, alias, form, kind, line, resolved, payload
    FROM unresolved_refs;

-- Children first: dropping edges and shingles removes every FK reference to
-- nodes, so the nodes drop cascades nothing. Clean under live FK enforcement.
-- unresolved_refs carries no FK to nodes, so its drop is independent of order.
DROP TABLE edges;
DROP TABLE shingles;
DROP TABLE nodes;
DROP TABLE unresolved_refs;

-- The rebuilt nodes table: the migration-16 column set and UNIQUE(symbol_id)
-- constraint, byte-identical, with the kind CHECK widened to the three CR-061
-- broker kinds (35..=37 — Topic/Producer/Consumer, FR-WS-11).
CREATE TABLE nodes (
    id                    INTEGER PRIMARY KEY,
    symbol_id             INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
    kind                  INTEGER NOT NULL CHECK (kind IN (1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34,35,36,37)),
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

-- The rebuilt edges table: the migration-16 shape with the kind CHECK widened
-- to the two CR-061 broker edge kinds (16..=17 — Publishes/Subscribes).
CREATE TABLE edges (
    id      INTEGER PRIMARY KEY,
    source  INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    target  INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    kind    INTEGER NOT NULL CHECK (kind IN (1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17)),
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

-- The rebuilt ledger: the migration-14 shape with the kind CHECK widened in
-- lockstep with edges.kind — an unresolved cross-member topic binding must
-- persist and retry on sync (NFR-RA-05), exactly as an unresolved artifact
-- reference does.
CREATE TABLE unresolved_refs (
    id            INTEGER PRIMARY KEY,
    file_id       INTEGER REFERENCES files(id) ON DELETE CASCADE,
    source_symbol TEXT NOT NULL,
    target        TEXT NOT NULL,
    alias         TEXT,
    form          INTEGER NOT NULL CHECK (form IN (1,2,3,4)),
    kind          INTEGER NOT NULL CHECK (kind IN (1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17)),
    line          INTEGER,
    resolved      INTEGER NOT NULL DEFAULT 0 CHECK (resolved IN (0,1)),
    payload       TEXT,
    UNIQUE (source_symbol, target, form, kind)
) STRICT;

-- Copy rows back with identical ids (parents before children, so FK checks
-- hold and the FTS postings stay aligned by rowid). Every annotation column,
-- edge payload, and shingle is carried over verbatim — no row reverts to a
-- default.
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
INSERT INTO unresolved_refs
       (id, file_id, source_symbol, target, alias, form, kind, line, resolved, payload)
    SELECT id, file_id, source_symbol, target, alias, form, kind, line, resolved, payload
    FROM unresolved_refs_stash;

DROP TABLE nodes_stash;
DROP TABLE edges_stash;
DROP TABLE shingles_stash;
DROP TABLE unresolved_refs_stash;

-- Recreate the indexes (migration 1 + 3 + 10 + 14). The old non-unique
-- idx_nodes_symbol_id stays retired (migration 16): UNIQUE(symbol_id) already
-- provides its own covering index.
CREATE INDEX idx_nodes_kind        ON nodes(kind);
CREATE INDEX idx_edges_source_kind ON edges(source, kind);
CREATE INDEX idx_edges_target_kind ON edges(target, kind);
CREATE INDEX idx_shingles_hash     ON shingles(hash);
CREATE INDEX idx_unresolved_refs_file     ON unresolved_refs(file_id);
CREATE INDEX idx_unresolved_refs_resolved ON unresolved_refs(resolved);

-- Recreate the FTS sync triggers carrying `body` alongside `name` (migration
-- 9/13/16 shape, FR-DB-03). The 'delete' command rows retract the OLD postings
-- so the external-content index never silently desyncs (NFR-RA-09).
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

-- Recreate the FR-AN-04 queryable view exactly as migration 16 left it
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

/// Migration 18 — make the reference-ledger uniqueness key **relation-aware**
/// ([CR-080], [FR-DB-01], [FR-WS-11]).
///
/// The ledger's shipped `UNIQUE (source_symbol, target, form, kind)` key omits
/// the relation discriminator (`payload`), so a **relay** — one declaration that
/// both subscribes to and re-publishes on the *same* broker topic — produces two
/// rows that agree on all four keyed columns and differ only in relation
/// (`broker-publish` vs `broker-subscribe`). The upsert collapsed the second onto
/// the first, silently dropping the relay's subscribe before ledger promotion
/// ([resolution-engine]) ever saw it ([S-256]'s pinned defect). This migration
/// widens the key with the relation, restoring both legs.
///
/// SQLite cannot drop a table-level `UNIQUE` constraint in place, so — following
/// the referenced-table-safe copy-rebuild of migrations 14/16/17 — the ledger is
/// stashed, dropped, and recreated **without** the four-column table constraint,
/// then a `UNIQUE INDEX` over `(source_symbol, target, form, kind,
/// COALESCE(payload, ''))` re-establishes uniqueness with the relation folded in.
/// The `COALESCE` is load-bearing: SQLite treats each bare `NULL` as *distinct*
/// in a UNIQUE key, so keying on the raw `payload` would stop deduping ordinary
/// code refs (whose `payload` is `NULL`) and break [`super::SqliteGraphStore`]'s
/// documented insert idempotency. Normalising `NULL` to `''` keeps NULL-payload
/// dedup byte-for-byte identical to the pre-migration key while letting two rows
/// that differ *only* by a present relation coexist.
///
/// `unresolved_refs` carries **no** foreign key to `nodes` (only to `files`), so
/// this rebuild touches the ledger and nothing else: `nodes`, `edges`,
/// `shingles`, the FTS index, and the `annotations` view are never dropped, which
/// makes a no-relay graph's node/edge identity trivially byte-for-byte
/// unaffected. Every existing row was already deduped under the *narrower* key, so
/// each is unique under the wider one too — the copy-back preserves every row and
/// adds none. The `kind` CHECK is recreated identical to migration 17 (still
/// `EdgeKind::ALL`, 1..=17); this migration widens uniqueness, not the ontology.
///
/// [CR-080]: ../../../../docs/requests/CR-080-broker-relay-ledger-dedup.md
/// [FR-DB-01]: ../../../../docs/specs/requirements/FR-DB-01.md
/// [FR-WS-11]: ../../../../docs/specs/requirements/FR-WS-11.md
/// [resolution-engine]: ../../../../docs/specs/architecture/components/resolution-engine.md
/// [S-256]: ../../../../docs/planning/journal.md#s-256-promote-broker-coupling-to-first-class-topics
const MIGRATION_18: &str = "\
-- Stash the ledger's full column set (CTAS: no FKs/constraints) so the rebuild
-- preserves every row, id, and column verbatim. No other table is touched, so
-- the FTS index and the nodes/edges graph stay byte-for-byte identical.
CREATE TABLE unresolved_refs_stash AS
    SELECT id, file_id, source_symbol, target, alias, form, kind, line, resolved, payload
    FROM unresolved_refs;

-- Drop the ledger to shed its four-column table-level UNIQUE constraint (SQLite
-- cannot ALTER a constraint in place). unresolved_refs carries no FK to nodes,
-- so this drop cascades nothing.
DROP TABLE unresolved_refs;

-- Recreate the ledger with the migration-17 column set and kind CHECK
-- (EdgeKind::ALL, 1..=17) verbatim, but WITHOUT the four-column UNIQUE — the
-- relation-aware uniqueness moves to the expression index below.
CREATE TABLE unresolved_refs (
    id            INTEGER PRIMARY KEY,
    file_id       INTEGER REFERENCES files(id) ON DELETE CASCADE,
    source_symbol TEXT NOT NULL,
    target        TEXT NOT NULL,
    alias         TEXT,
    form          INTEGER NOT NULL CHECK (form IN (1,2,3,4)),
    kind          INTEGER NOT NULL CHECK (kind IN (1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17)),
    line          INTEGER,
    resolved      INTEGER NOT NULL DEFAULT 0 CHECK (resolved IN (0,1)),
    payload       TEXT
) STRICT;

-- Copy every stashed row back with its id verbatim. Existing rows were unique
-- under the narrower key, so none collides under the wider one — nothing is lost.
INSERT INTO unresolved_refs
       (id, file_id, source_symbol, target, alias, form, kind, line, resolved, payload)
    SELECT id, file_id, source_symbol, target, alias, form, kind, line, resolved, payload
    FROM unresolved_refs_stash;

DROP TABLE unresolved_refs_stash;

-- Recreate the migration-1/3 secondary indexes.
CREATE INDEX idx_unresolved_refs_file     ON unresolved_refs(file_id);
CREATE INDEX idx_unresolved_refs_resolved ON unresolved_refs(resolved);

-- The relation-aware uniqueness: the four shipped key columns plus the relation
-- discriminator, with NULL payloads normalised to '' so NULL-payload dedup is
-- byte-for-byte identical to the old key while a relay's publish and subscribe —
-- same source/target/form/kind, different relation — no longer collide.
CREATE UNIQUE INDEX idx_unresolved_refs_identity
    ON unresolved_refs(source_symbol, target, form, kind, COALESCE(payload, ''));
";

/// Migration 19 — the member-local **configuration corpus** tables (S-380,
/// [CR-121], [FR-WS-19], [FR-CG-02], [FR-DB-01]).
///
/// Configuration keys already become `ConfigSection` nodes, but a node carries
/// no value and the section walk stops at a fixed depth of 2 ([BR-30]), so on the
/// reference estate `mailbox-aggregate` is indexed and `api.uri-get-mailbox` is
/// not. These two tables make the **value** a fact, at full nesting depth, tagged
/// with the profile of the file that proves it.
///
/// The shape mirrors the promoted corpus model
/// ([`ConfigSourceFact`](crate::extract::config::corpus::ConfigSourceFact)) one
/// for one: a source is a file plus its profile, and it proves a set of
/// canonical key → value pairs. Splitting them is what makes the census
/// downstream stories reconcile against — sources, profiled sources, distinct
/// keys — a query rather than a scan.
///
/// **Purely additive**: it creates two new tables and their indexes and touches
/// nothing else. No table is dropped, rebuilt or copied, so `nodes`, `edges`,
/// `shingles` and the external-content `nodes_fts` index are byte-for-byte
/// unaffected across the boundary — the same standalone-table shape migration 15
/// used for `project_metadata`, and asserted on a populated store by
/// `migration_19_adds_the_config_corpus_tables_preserving_the_graph_byte_for_byte`
/// in [`super::migrate`].
///
/// [BR-30]: ../../../../docs/specs/software-spec.md
/// [CR-121]: ../../../../docs/requests/CR-121-caller-to-callee-and-producer-to-consumer-across-services.md
/// [FR-CG-02]: ../../../../docs/specs/requirements/FR-CG-02.md
/// [FR-WS-19]: ../../../../docs/specs/requirements/FR-WS-19.md
const MIGRATION_19: &str = "\
-- config_sources: one row per committed configuration source (an
-- `application*.{yml,yaml,properties}` file and its profile variants). `profile`
-- is NULL for the unprofiled `application.<ext>` — the census counts unprofiled
-- and profiled sources apart, so the distinction must be a value, not a sentinel.
-- UNIQUE(file_id) because a file is at most one source; the FK cascades so
-- removing a file removes its corpus contribution with it.
CREATE TABLE config_sources (
    id      INTEGER PRIMARY KEY,
    file_id INTEGER NOT NULL UNIQUE REFERENCES files(id) ON DELETE CASCADE,
    profile TEXT
) STRICT;

-- config_values: the canonical key -> value pairs one source proves. A source
-- that defines one key twice (multi-document YAML) proves BOTH values, so the
-- key alone is not unique within a source — the pair is. Disagreement is
-- represented, never averaged and never refused (FR-WS-19).
CREATE TABLE config_values (
    id        INTEGER PRIMARY KEY,
    source_id INTEGER NOT NULL REFERENCES config_sources(id) ON DELETE CASCADE,
    key       TEXT NOT NULL,
    value     TEXT NOT NULL,
    UNIQUE (source_id, key, value)
) STRICT;

-- The one hot path: `key -> every source that defines it`, which is how an
-- accessor resolves and how the agreement over a key is computed. The other two
-- joins in that query are already covered (config_sources.id is its primary key,
-- files.id is its own), and `profile` is deliberately NOT indexed: a member holds
-- single-digit config sources, so an index there would cost a write for a scan
-- that is already trivial.
CREATE INDEX idx_config_values_key ON config_values(key);
";

/// Migration 20 — the `check_rules` run marker (S-313, [CR-096], [FR-GV-21]).
///
/// One singleton table, created and nothing else. `violations` is replaced
/// wholesale per run ([FR-GV-02], BR-12 idempotence), so a run that finds
/// nothing leaves a table byte-identical to one a project that was never
/// checked has: the marker is the only place the *fact of a run* can live.
///
/// The singleton shape is the [FR-GV-01] `rules_cache` pattern
/// (`id INTEGER PRIMARY KEY CHECK (id = 1)`) — enforced by the schema so
/// BR-40 ("upserted, never appended") cannot be broken by a caller's
/// convention. A separate table rather than a column on `rules_cache`: that
/// row is a content-hash parse cache with its own lifetime.
///
/// Additive only — `CREATE TABLE` and nothing else. No table is dropped,
/// rebuilt or copied, so `nodes`, `edges`, `shingles` and the
/// external-content `nodes_fts` index are byte-for-byte unaffected across the
/// boundary and an existing store upgrades in place with no re-index
/// ([FR-DB-04], [NFR-MA-06]) — the same standalone-table shape migrations 15
/// and 19 used, asserted on a populated store by
/// `migration_20_adds_the_check_run_marker_preserving_the_graph_byte_for_byte`
/// in [`super::migrate`].
///
/// [CR-096]: ../../../../docs/requests/CR-096-recorded-check-marker.md
/// [FR-DB-04]: ../../../../docs/specs/requirements/FR-DB-04.md
/// [FR-GV-01]: ../../../../docs/specs/requirements/FR-GV-01.md
/// [FR-GV-02]: ../../../../docs/specs/requirements/FR-GV-02.md
/// [FR-GV-21]: ../../../../docs/specs/requirements/FR-GV-21.md
/// [NFR-MA-06]: ../../../../docs/specs/requirements/NFR-MA-06.md
const MIGRATION_20: &str = "\
-- check_run: the FR-GV-21 singleton marker recording the LAST check_rules run
-- (BR-40 — the last run, never a history; FR-GV-06 owns the quality time
-- series). Absence of the row means no run has happened, never a clean result.
--
-- commit_sha is what `HEAD` was WHEN THE RUN HAPPENED. It answers 'has the tree
-- moved since', and NOT 'these findings were introduced by that commit' — the
-- same field on metric_snapshots was once read the second way and produced a
-- confidently wrong attribution (CR-095 review, finding 12, retracted). NULL
-- when HEAD does not resolve (no git, no commits); never a placeholder.
--
-- violation_count is the number of `violations` rows the same transaction
-- wrote, so 0 with no rows is a recorded clean run.
CREATE TABLE check_run (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    ran_at          INTEGER NOT NULL,
    commit_sha      TEXT,
    violation_count INTEGER NOT NULL
) STRICT;
";

/// Migration 21 — the `check_run` marker records **what it evaluated**
/// (S-437, [CR-140] §3.1, [FR-GV-21], [FR-GV-02]).
///
/// Migration 20 gave the marker a numerator — `violation_count` — and no
/// denominator. A run that evaluated **no rules at all** therefore recorded
/// `0` and read back as a clean check: the [FR-GV-03] definition of "clean"
/// ("a contract was evaluated and held; it never means nothing was
/// evaluated") could not be honoured by a reader, because the fact it turns
/// on was never written down. [`check_rules`](crate::Engine::check_rules)
/// already computes both figures this adds; they were dropped at the
/// persistence boundary.
///
/// Three columns, all **nullable**, and the nullability is load-bearing
/// rather than lax: a marker written before this migration recorded none of
/// them, so it must read as *evaluated set unknown* — never as zero, which is
/// the very misreading this closes ([CR-140] CRA-05). A `NOT NULL` column
/// with a default would forge the fact for every store already on disk. This
/// is the migration-12 convention exactly (`cohesion_applicable` is NULL on a
/// pre-v3 snapshot, distinct from a real `0` — [NFR-CC-04]).
///
/// `checked_rules` and `rules_present` are two facts, not one: a **present**
/// contract authoring **zero** rules is what [`logos init`](crate::init)'s
/// default template produces, and a reader must be able to tell it from an
/// absent contract. `operation` names which run wrote the marker, since
/// [`scan`](crate::Engine::scan) shares the write site with
/// [`check_rules`](crate::Engine::check_rules) and the two are not
/// interchangeable to a reader.
///
/// Additive only — three `ALTER TABLE ... ADD COLUMN`s, the migration-12
/// shape. No table is dropped, rebuilt or copied, so `nodes`, `edges`,
/// `shingles` and the external-content `nodes_fts` index are byte-for-byte
/// unaffected and an existing store upgrades in place with no re-index
/// ([FR-DB-04], [NFR-MA-06]), asserted on a populated store by
/// `migration_21_widens_the_check_run_marker_preserving_the_graph_byte_for_byte`
/// in [`super::migrate`].
///
/// [CR-140]: ../../../../docs/requests/CR-140-the-recorded-check-marker-carries-what-it-evaluated.md
/// [FR-DB-04]: ../../../../docs/specs/requirements/FR-DB-04.md
/// [FR-GV-02]: ../../../../docs/specs/requirements/FR-GV-02.md
/// [FR-GV-03]: ../../../../docs/specs/requirements/FR-GV-03.md
/// [FR-GV-21]: ../../../../docs/specs/requirements/FR-GV-21.md
/// [NFR-CC-04]: ../../../../docs/specs/requirements/NFR-CC-04.md
/// [NFR-MA-06]: ../../../../docs/specs/requirements/NFR-MA-06.md
const MIGRATION_21: &str = "\
-- check_run gains the EVALUATED SET the run scored against (CR-140 §3.1,
-- FR-GV-21). violation_count is a numerator whose denominator the marker never
-- recorded, so a run that evaluated nothing recorded 0 and read as clean.
--
-- All three columns are NULLABLE on purpose. NULL means 'this marker predates
-- migration 21', which a reader must render as *unknown* rather than as zero.
-- A mandatory column would need a fallback value, and that value would forge an
-- evaluated set for every store already on disk (the migration-12 convention:
-- NULL is not a real 0, NFR-CC-04).
--
-- checked_rules is violation_count's denominator: how many rules the run
-- evaluated. rules_present is whether a contract was authored at all, which
-- checked_rules alone cannot say — a PRESENT contract authoring ZERO rules is
-- `logos init`'s default template, and it must be distinguishable from an
-- absent one.
--
-- operation is which run wrote the marker: 'check' or 'scan', the two
-- operations that call replace_violations. The CHECK pins the vocabulary in
-- the schema, so a mis-spelled operation fails the write rather than storing
-- a value no reader knows.
ALTER TABLE check_run ADD COLUMN checked_rules INTEGER;
ALTER TABLE check_run ADD COLUMN rules_present INTEGER CHECK (rules_present IN (0,1));
ALTER TABLE check_run ADD COLUMN operation     TEXT CHECK (operation IN ('check','scan'));
";

/// Migration 22 — the member-local **build-manifest facts** (S-462, [CR-148]
/// §3.2 A, [ADR-69] decision point 1).
///
/// Two tables, the migration-19 shape: a manifest, and the artifact facts it
/// yields. `build_manifests` holds one row per Maven `pom.xml` or Gradle
/// `build.gradle(.kts)` the discovery walk found — **read or not**, because the
/// census reports "files read, of manifests found" and a manifest that failed
/// to parse must stay in the denominator. `build_artifacts` holds what each read
/// manifest proves: the artifact it produces and every artifact it references,
/// each with its kind, declared scope and whether its coordinates resolved
/// ([`crate::extract::build_manifest`]).
///
/// Keyed by **path**, not by `files.id`. A `pom.xml` has no grammar and is never
/// a `files` row; admitting it there would add a file, and a LOC total, to every
/// Java member's index. The manifest's own `content_hash` is what an incremental
/// sync compares, exactly as `files.content_hash` is for source.
///
/// The CHECKs pin the vocabulary and its two pairings in the schema: a produced
/// fact carries no reference kind and a reference always carries one; a
/// resolved fact carries no reason and an unresolved one always does. A caller
/// cannot store a refusal without saying why.
///
/// **Purely additive**: two `CREATE TABLE`s and one index. No table is dropped,
/// rebuilt or copied, so `nodes`, `edges`, `shingles` and the external-content
/// `nodes_fts` index are byte-for-byte unaffected and an existing store upgrades
/// in place with no re-index ([FR-DB-04], [NFR-MA-06]) — asserted on a populated
/// store by
/// `migration_22_adds_the_build_manifest_tables_preserving_the_graph_byte_for_byte`
/// in [`super::migrate`]. A member with no build manifest keeps both tables
/// empty for ever.
///
/// [ADR-69]: ../../../../docs/specs/architecture/decisions/ADR-69.md
/// [CR-148]: ../../../../docs/requests/CR-148-build-manifests-yield-a-build-dependency-relation.md
/// [FR-DB-04]: ../../../../docs/specs/requirements/FR-DB-04.md
/// [NFR-MA-06]: ../../../../docs/specs/requirements/NFR-MA-06.md
const MIGRATION_22: &str = "\
-- build_manifests: one row per build manifest the walk found, read or not
-- (the census denominator is manifests FOUND). content_hash is NULL only for a
-- manifest that could not be read; detail says why a manifest yielded no facts
-- and is NULL exactly when it was read.
CREATE TABLE build_manifests (
    id           INTEGER PRIMARY KEY,
    path         TEXT NOT NULL UNIQUE,
    format       TEXT NOT NULL CHECK (format IN ('maven','gradle')),
    content_hash TEXT,
    status       TEXT NOT NULL CHECK (status IN ('read','malformed','unreadable')),
    detail       TEXT,
    CHECK ((status = 'read') = (detail IS NULL))
) STRICT;

-- build_artifacts: what one manifest produces and references. group_id /
-- artifact_id / version hold the RESOLVED value, or the declared text verbatim
-- when it did not resolve -- resolution and reason, never the text, say which.
-- A Gradle project(':x') reference names a project_path and no coordinate.
-- scope is as declared (NULL = undeclared, never defaulted to compile).
CREATE TABLE build_artifacts (
    id           INTEGER PRIMARY KEY,
    manifest_id  INTEGER NOT NULL REFERENCES build_manifests(id) ON DELETE CASCADE,
    role         TEXT NOT NULL CHECK (role IN ('produced','referenced')),
    kind         TEXT CHECK (kind IN ('parent','dependency','managed','bom-import')),
    group_id     TEXT,
    artifact_id  TEXT,
    version      TEXT,
    scope        TEXT,
    project_path TEXT,
    resolution   TEXT NOT NULL CHECK (resolution IN ('resolved','version-refused','refused')),
    reason       TEXT,
    CHECK ((role = 'produced') = (kind IS NULL)),
    CHECK ((resolution = 'resolved') = (reason IS NULL))
) STRICT;

-- The cascade's lookup: replacing a member's facts deletes its manifests, and
-- each delete finds its artifacts through this index. Coordinates are NOT
-- indexed: the workspace join reads every row of a member once, in memory.
CREATE INDEX idx_build_artifacts_manifest ON build_artifacts(manifest_id);
";

/// Migration 23 — Modularity's applicability flag (S-487, [CR-156], [ADR-21],
/// [FR-QM-07]).
///
/// Metric-semantics v6 makes Modularity **not applicable** when the graph it is
/// computed on has fewer than five edges
/// ([`MODULARITY_MIN_EDGES`](crate::metrics::MODULARITY_MIN_EDGES)): the dimension
/// leaves the aggregate but its computed raw/normalized pair is still persisted,
/// so the snapshot needs a flag to say which reading of that pair applies. Unlike
/// the migration-12 Cohesion/Focus flags, the value columns stay populated when
/// the flag is `0` — Modularity is always computable; it is the *evidence* that
/// is too thin.
///
/// `1` = applied, `0` = dropped out, `NULL` = a row persisted before this
/// migration. Those rows were scored under metric-semantics ≤ 5, when
/// Modularity always applied, so the read path takes `NULL` as applicable.
///
/// **Purely additive**: one nullable `ALTER TABLE … ADD COLUMN` on the
/// append-only ledger. No table is rebuilt; every existing row and id is
/// untouched ([FR-DB-04], [NFR-MA-06]) — asserted on a populated ledger by
/// `migration_23_adds_modularity_applicable_and_pre_migration_rows_read_applicable`
/// in [`super::migrate`].
///
/// [ADR-21]: ../../../../docs/specs/architecture/decisions/ADR-21.md
/// [CR-156]: ../../../../docs/requests/CR-156-modularity-drops-out-of-a-too-small-graph.md
/// [FR-DB-04]: ../../../../docs/specs/requirements/FR-DB-04.md
/// [FR-QM-07]: ../../../../docs/specs/requirements/FR-QM-07.md
/// [NFR-MA-06]: ../../../../docs/specs/requirements/NFR-MA-06.md
const MIGRATION_23: &str = "\
-- CR-156 (metric-semantics v6): Modularity is not applicable below five edges.
-- 1 = applied, 0 = dropped out of the mean and the zero short-circuit (its
-- raw/normalized pair is still stored), NULL = scored before this flag existed,
-- when Modularity always applied.
ALTER TABLE metric_snapshots ADD COLUMN modularity_applicable INTEGER CHECK (modularity_applicable IN (0,1));
";

/// Migration 24 — the member-local **declared-type facts** (S-472, [CR-152]
/// §3.2 B, [ADR-70] decision point 1, the [ADR-69] point-1 precedent).
///
/// Two tables, the migration-22 shape. `avro_schemas` holds one row per `.avsc`
/// schema the discovery walk found — **read or not**, because the census
/// reports "files read, of files found" and a malformed schema must stay in the
/// denominator. Like a build manifest it is keyed by **path**, not by
/// `files.id`: a schema has no grammar and is never a `files` row, and its own
/// `content_hash` is what an incremental sync compares.
///
/// `declared_types` holds one row per declared type, from exactly one of two
/// provenances: a top-level Java/Kotlin type (`file_id`, with its node's
/// `symbol` and its `tree`), or an Avro record/enum (`schema_id`). Each
/// provenance's FK cascades, so re-extracting or removing a source file, or
/// replacing or removing a schema, takes exactly that file's facts with it —
/// the per-file resync the incremental path relies on.
///
/// The CHECKs pin the vocabulary and its pairings in the schema: exactly one
/// provenance; a symbol and a tree exactly for a source fact; a resolved fact
/// carries a name and no reason, a refused one a reason and no name — a caller
/// cannot store a refusal without saying why, nor a guessed name beside one.
///
/// **Purely additive**: two `CREATE TABLE`s and two indexes. No table is
/// dropped, rebuilt or copied, so `nodes`, `edges`, `shingles` and the
/// external-content `nodes_fts` index are byte-for-byte unaffected and an
/// existing store upgrades in place with no re-index ([FR-DB-04], [NFR-MA-06])
/// — asserted on a populated store by
/// `migration_24_adds_the_declared_type_tables_preserving_the_graph_byte_for_byte`
/// in [`super::migrate`]. A member with no Java/Kotlin/Avro file keeps both
/// tables empty for ever.
///
/// [ADR-69]: ../../../../docs/specs/architecture/decisions/ADR-69.md
/// [ADR-70]: ../../../../docs/specs/architecture/decisions/ADR-70.md
/// [CR-152]: ../../../../docs/requests/CR-152-cross-member-type-references-overlay.md
/// [FR-DB-04]: ../../../../docs/specs/requirements/FR-DB-04.md
/// [NFR-MA-06]: ../../../../docs/specs/requirements/NFR-MA-06.md
const MIGRATION_24: &str = "\
-- avro_schemas: one row per .avsc schema the walk found, read or not (the
-- census denominator is schemas FOUND). content_hash is NULL exactly for a
-- schema that could not be read; detail says why a schema yielded no type and
-- is NULL exactly when it was read.
CREATE TABLE avro_schemas (
    id           INTEGER PRIMARY KEY,
    path         TEXT NOT NULL UNIQUE,
    content_hash TEXT,
    status       TEXT NOT NULL CHECK (status IN ('read','malformed','unreadable')),
    detail       TEXT,
    CHECK ((status = 'read') = (detail IS NULL)),
    CHECK ((status = 'unreadable') = (content_hash IS NULL))
) STRICT;

-- declared_types: one fully-qualified type a member declares. A source fact
-- names its file, its node's symbol and its tree; an Avro fact names its
-- schema. fqn is the dotted name, NULL exactly when the fact is refused (a
-- package statement that disagrees with the directory) -- never the path's
-- guess. name is the type's own name as declared.
CREATE TABLE declared_types (
    id         INTEGER PRIMARY KEY,
    file_id    INTEGER REFERENCES files(id) ON DELETE CASCADE,
    schema_id  INTEGER REFERENCES avro_schemas(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    fqn        TEXT,
    kind       TEXT NOT NULL CHECK (kind IN ('class','interface','enum','record')),
    symbol     TEXT,
    tree       TEXT CHECK (tree IN ('main','test')),
    resolution TEXT NOT NULL CHECK (resolution IN ('resolved','refused')),
    reason     TEXT,
    CHECK ((file_id IS NULL) <> (schema_id IS NULL)),
    CHECK ((file_id IS NULL) = (symbol IS NULL)),
    CHECK ((file_id IS NULL) = (tree IS NULL)),
    CHECK ((resolution = 'resolved') = (reason IS NULL)),
    CHECK ((resolution = 'resolved') = (fqn IS NOT NULL))
) STRICT;

-- The cascades' lookups: re-extracting a file, or replacing a schema, deletes
-- its facts through these. Names are NOT indexed: the workspace index reads
-- every row of a member once, in memory (ADR-52).
CREATE INDEX idx_declared_types_file   ON declared_types(file_id);
CREATE INDEX idx_declared_types_schema ON declared_types(schema_id);
";

/// Migration 25 — the per-callable **has-body** fact and its body's token count
/// (S-500, [CR-163], [FR-EX-11]).
///
/// Two nullable columns added **in place** on `nodes` (`ALTER TABLE … ADD
/// COLUMN`, the migration-10 shape — no rebuild, every existing row and id
/// untouched, the FTS index, its triggers and the `annotations` view
/// unaffected):
///
/// 1. **`nodes.has_body`** — `1` when a `Function`/`Method` declaration carries
///    an implementation body, `0` for an abstract method, an interface method
///    with no default, a C++ pure-virtual or prototype; from the language's
///    declared `body_node_kinds`. Read by duplicate eligibility, LCOM4 and the
///    Focus method count.
/// 2. **`nodes.body_tokens`** — that body's normalized token count, by the
///    normalization the near-clone shingles use and over the same subtree
///    wherever the declaration names a `body` field (`0` when there is no
///    body); the input to
///    the exact-duplicate `duplicate_min_tokens` floor (S-501). The `shingles`
///    table holds winnowed hashes, not a count, so nothing persisted before
///    could stand in for it.
///
/// Both are `NULL` on every non-callable node, and on a callable until its file
/// is re-extracted. **Re-extraction is triggered here** — a departure from
/// migration 10's ([FR-EX-07]) posture, which left its column `NULL` until a
/// file happened to change, and simpler than S-472's marker-gated pipeline
/// backfill after migration 24: every `files.content_hash` is cleared, so the
/// next sync takes its existing "never hashed" arm for each file it meets — a
/// full-walk sync every file — re-extracts it exactly as it would a modified
/// one, and records the fresh hash. `content_hash` is read by incremental-sync dirty
/// detection alone, so clearing it costs one re-extraction and changes nothing
/// else; no graph row is deleted.
///
/// Forward-only ([FR-DB-04], [NFR-MA-06]) — asserted on a populated store by
/// `migration_25_adds_the_has_body_columns_and_triggers_reextraction` in
/// [`super::migrate`].
///
/// [CR-163]: ../../../../docs/requests/CR-163-structural-metrics-stop-misfiring-on-declarative-code.md
/// [FR-EX-07]: ../../../../docs/specs/requirements/FR-EX-07.md
/// [FR-EX-11]: ../../../../docs/specs/requirements/FR-EX-11.md
/// [FR-DB-04]: ../../../../docs/specs/requirements/FR-DB-04.md
/// [NFR-MA-06]: ../../../../docs/specs/requirements/NFR-MA-06.md
const MIGRATION_25: &str = "\
-- 1. Per-callable has-body fact (FR-EX-11): nullable, added in place. NULL on
-- every non-callable node and on rows indexed before this migration.
ALTER TABLE nodes ADD COLUMN has_body INTEGER CHECK (has_body IN (0,1));

-- 2. The body's normalized token count (S-501's duplicate floor input): NULL
-- exactly where has_body is.
ALTER TABLE nodes ADD COLUMN body_tokens INTEGER CHECK (body_tokens >= 0);

-- 3. Trigger re-extraction: a file with no recorded hash is re-extracted on its
-- next sync like a modified one, filling both columns.
UPDATE files SET content_hash = NULL;
";

/// Migration 26 — every metric snapshot persists the **worst-offender lists** it
/// computed (S-498, [CR-162], [FR-QM-15]).
///
/// Two additions, both purely additive — no table is dropped, rebuilt or copied,
/// so every existing row (the graph, the FTS index, every snapshot) crosses the
/// boundary byte for byte and an upgraded store needs no re-index:
///
/// 1. **`metric_snapshots.offenders_recorded`** — `1` on a snapshot whose lists
///    were written with it, `NULL` on every snapshot written before this
///    migration. It is what tells a **recorded-empty** list (nothing crossed a
///    threshold) from a snapshot that **recorded no lists**: both have zero
///    offender rows, and only this column separates them ([NFR-CC-04]). The
///    [migration-23](MIGRATION_23) shape — a nullable flag added in place whose
///    `NULL` is the pre-migration reading. Only `1` is admitted, because the one
///    append path always records; there is no third state to spell.
/// 2. **`metric_snapshot_offenders`** — one row per offender entry, keyed by the
///    snapshot that computed it and its 1-based rank within its dimension's list
///    (computed order: severity, then node id). `file` is `NULL` for a node bound
///    to no file, the `violations.file` convention. Rows are append-only like
///    their snapshot; nothing updates or deletes them.
///
/// `dimension` carries **no** `CHECK` list on purpose: [CR-164] reuses this
/// table for further lists, and a `CHECK` would make each one a table rebuild.
/// The vocabulary is pinned in Rust instead
/// ([`WorstOffenders::DIMENSIONS`](crate::models::quality::WorstOffenders::DIMENSIONS)),
/// and the read path ignores a name outside it.
///
/// No back-fill: an old snapshot stays "not recorded". Recomputing its lists
/// would describe today's graph under yesterday's signal ([CR-162] §3.3).
///
/// Forward-only ([FR-DB-04], [NFR-MA-06]) — asserted on a populated store by
/// `migration_26_adds_snapshot_offenders_and_pre_migration_rows_read_not_recorded`
/// in [`super::migrate`].
///
/// [CR-162]: ../../../../docs/requests/CR-162-health-shows-the-worst-offenders-its-snapshot-computed.md
/// [CR-164]: ../../../../docs/requests/CR-164-insight-layer-ranks-what-to-fix-first.md
/// [FR-QM-15]: ../../../../docs/specs/requirements/FR-QM-15.md
/// [FR-DB-04]: ../../../../docs/specs/requirements/FR-DB-04.md
/// [NFR-CC-04]: ../../../../docs/specs/requirements/NFR-CC-04.md
/// [NFR-MA-06]: ../../../../docs/specs/requirements/NFR-MA-06.md
const MIGRATION_26: &str = "\
-- 1. 1 = this snapshot's offender lists were written with it (possibly all
-- empty); NULL = written before offenders were persisted (not recorded).
ALTER TABLE metric_snapshots ADD COLUMN offenders_recorded INTEGER CHECK (offenders_recorded = 1);

-- 2. One row per offender entry: the snapshot that computed it, its dimension
-- and 1-based rank in that dimension's list, and the entry itself.
CREATE TABLE metric_snapshot_offenders (
    snapshot_id INTEGER NOT NULL REFERENCES metric_snapshots(id),
    dimension   TEXT NOT NULL,
    rank        INTEGER NOT NULL CHECK (rank >= 1),
    name        TEXT NOT NULL,
    file        TEXT,
    line        INTEGER,
    detail      TEXT NOT NULL,
    PRIMARY KEY (snapshot_id, dimension, rank)
) STRICT;
";

/// Migration 27 — the record of files whose facts could not be persisted
/// (S-513, [CR-168], [FR-EH-05]).
///
/// One table, purely additive — nothing existing is touched, so every row
/// crosses the boundary byte for byte and an upgraded store needs no re-index.
/// A pre-migration store reads as "no file failed", which is what it was: before
/// this migration a persistence failure aborted the whole batch and recorded
/// nothing.
///
/// **`persist_failures`** — one row per file whose latest persistence attempt
/// failed, keyed by its project-relative path (the `files.path` key, but **not**
/// a foreign key: a file that failed on a full index, or on its first sync, has
/// no `files` row at all). `reason` is the error that rolled the file back.
/// `stale` separates the two outcomes [FR-EH-05] distinguishes: `1` when the
/// graph still holds the file's last successfully persisted facts (a `sync`, the
/// `serve` watcher or a reconcile failed to replace them), `0` when the file is
/// absent from the graph (a full index, or a file that never persisted).
///
/// Bounded: a row is replaced by the next failure of the same file and removed
/// by the file's next successful persist, by an unchanged re-read, or by its
/// removal from the graph; a full index rewrites the table to that run's
/// failures plus the rows of files it could not load; and a full-walk reconcile
/// clears the row of any file it no longer admits — which is how a file that
/// never persisted, and so has no graph rows to remove, leaves the record once
/// it leaves admission ([CR-168] §7).
///
/// Forward-only ([FR-DB-04], [NFR-MA-06]) — asserted on a populated store by
/// `migration_27_adds_the_persist_failure_record_and_touches_nothing_else` in
/// [`super::migrate`].
///
/// [CR-168]: ../../../../docs/requests/CR-168-an-index-never-silently-empties.md
/// [FR-EH-05]: ../../../../docs/specs/requirements/FR-EH-05.md
/// [FR-DB-04]: ../../../../docs/specs/requirements/FR-DB-04.md
/// [NFR-MA-06]: ../../../../docs/specs/requirements/NFR-MA-06.md
const MIGRATION_27: &str = "\
-- One row per file whose latest persistence attempt failed (FR-EH-05): its
-- path, why, and whether the graph still holds its last good facts (stale = 1)
-- or holds nothing for it (stale = 0).
CREATE TABLE persist_failures (
    path   TEXT PRIMARY KEY,
    reason TEXT NOT NULL,
    stale  INTEGER NOT NULL CHECK (stale IN (0,1))
) STRICT;
";

/// Migration 28 — a method's **self type**, recorded beside its node (S-493,
/// [CR-159], [FR-RS-11]).
///
/// One nullable column added **in place** on `nodes` (the migration-25 shape —
/// no rebuild, every existing row and id untouched, the FTS index, its triggers
/// and the `annotations` view unaffected):
///
/// **`nodes.self_type`** — the base type name of the type a method belongs to,
/// as its plugin's `symbols` query declares it with a `@symbol.self_type`
/// capture: a Rust impl method's enclosing `impl<..> T<..>` / `impl Trait for T`
/// → `T`. `NULL` on every node no query gives a self type — a free function, a
/// trait's default method, every node of a language whose query declares none.
/// **Plugin-agnostic by construction**: the column names no language, and a
/// second language fills it by adding the capture to its own query (Go's
/// receiver base type, S-509) with no further migration. The binder reads it to
/// bind a `self.m()` / `Self::m()` call through the caller's own type; method
/// symbols are unchanged ([ADR-07]).
///
/// `NULL` on every row until its file is re-extracted, and **re-extraction is
/// triggered here** exactly as migration 25 does it: every `files.content_hash`
/// is cleared, so the next scan, index or full-walk sync re-extracts each file
/// like a modified one and records the fresh hash. No graph row is deleted.
///
/// Forward-only ([FR-DB-04], [NFR-MA-06]) — asserted on a populated store by
/// `migration_28_adds_the_self_type_column_and_triggers_reextraction` in
/// [`super::migrate`].
///
/// [CR-159]: ../../../../docs/requests/CR-159-rust-self-calls-bind-through-the-enclosing-impl.md
/// [FR-RS-11]: ../../../../docs/specs/requirements/FR-RS-11.md
/// [ADR-07]: ../../../../docs/specs/architecture/decisions/ADR-07.md
/// [FR-DB-04]: ../../../../docs/specs/requirements/FR-DB-04.md
/// [NFR-MA-06]: ../../../../docs/specs/requirements/NFR-MA-06.md
const MIGRATION_28: &str = "\
-- 1. A method's self type (FR-RS-11): the base type name its plugin query
-- declares. NULL on every node with none, and on rows indexed before this
-- migration.
ALTER TABLE nodes ADD COLUMN self_type TEXT;

-- 2. Trigger re-extraction: a file with no recorded hash is re-extracted on its
-- next scan like a modified one, filling the column.
UPDATE files SET content_hash = NULL;
";

/// Migration 29 — a method-form call's **receiver shape**, recorded on its
/// ledger row (S-514, [CR-169], [FR-EX-13]).
///
/// **`unresolved_refs.receiver`** — `1` `self`, `2` `super`, `3` `other`
/// ([`ReceiverShape`](crate::model::ReceiverShape), whose discriminants this
/// `CHECK` freezes); `NULL` on every row with no shape: a non-method row, and a
/// call its plugin's query marks no receiver of. The binder dispatches a
/// Method-form row on it ([FR-RS-12]). One nullable column added **in place**
/// (the migration-25/28 shape): every existing row and id is untouched.
///
/// The shape is part of the ledger identity: a caller's `this.m()` and `x.m()`
/// are two rows that bind differently, so the identity index of migration 18 is
/// recreated with `COALESCE(receiver, 0)` appended — the `COALESCE` for the
/// reason migration 18 gives for `payload` (SQLite treats each bare `NULL` as
/// distinct in a UNIQUE key). Every existing row has a `NULL` shape, so each is
/// unique under the wider key exactly as it was under the narrower one. Only an
/// index is dropped and recreated: no table is rebuilt, so `nodes`, `edges`, the
/// FTS index and the `annotations` view are byte-for-byte unaffected.
///
/// `NULL` on every row until its file is re-extracted, and **re-extraction is
/// triggered here** as migrations 25 and 28 do it: every `files.content_hash`
/// is cleared, so the next scan, index or full-walk sync re-extracts each file
/// like a modified one. Until then every method-form row binds as one with no
/// shape — never through the caller's scope ([FR-RS-12]).
///
/// Forward-only ([FR-DB-04], [NFR-MA-06]) — asserted on a populated store by
/// `migration_29_adds_the_receiver_shape_to_the_ledger_identity` in
/// [`super::migrate`].
///
/// [CR-169]: ../../../../docs/requests/CR-169-a-call-on-another-object-never-binds-to-the-callers-own-method.md
/// [FR-EX-13]: ../../../../docs/specs/requirements/FR-EX-13.md
/// [FR-RS-12]: ../../../../docs/specs/requirements/FR-RS-12.md
/// [FR-DB-04]: ../../../../docs/specs/requirements/FR-DB-04.md
/// [NFR-MA-06]: ../../../../docs/specs/requirements/NFR-MA-06.md
const MIGRATION_29: &str = "\
-- 1. A method-form call's receiver shape (FR-EX-13): 1 self, 2 super, 3 other.
-- NULL on every row with none, and on rows extracted before this migration.
ALTER TABLE unresolved_refs ADD COLUMN receiver INTEGER CHECK (receiver IN (1,2,3));

-- 2. The shape joins the ledger identity (migration 18's index, widened): a
-- caller's `this.m()` and `x.m()` are two rows. NULL normalised to 0 so a row
-- with no shape dedups exactly as before.
DROP INDEX idx_unresolved_refs_identity;
CREATE UNIQUE INDEX idx_unresolved_refs_identity
    ON unresolved_refs(source_symbol, target, form, kind, COALESCE(payload, ''), COALESCE(receiver, 0));

-- 3. Trigger re-extraction: a file with no recorded hash is re-extracted on its
-- next scan like a modified one, filling the column.
UPDATE files SET content_hash = NULL;
";

/// Migration 30 — the namespace a file **declares**, recorded beside its row
/// (S-518, [CR-170], [FR-RS-13]).
///
/// One nullable column added **in place** on `files` (the migration-25/28
/// shape — no rebuild, every existing row and id untouched, the FTS index, its
/// triggers and the `annotations` view unaffected):
///
/// **`files.namespace`** — for a file of a language whose plugin declares the
/// declared-namespace module model (`[module_model] kind = "namespace"`: PHP,
/// C#, Kotlin, Scala), the namespace or package its top-level declarations sit
/// in, its segments joined by `.` (`Shop.Domain`; `''` for the global
/// namespace). `NULL` on every file of any other model, and on one whose
/// top-level declarations sit in two different namespaces. **Plugin-agnostic by
/// construction**: the column names no language; a language fills it by
/// declaring the model and capturing `@module.namespace`. The binder keys such a
/// file by it ([`crate::resolve::package_key`]); symbols are unchanged
/// ([ADR-07]).
///
/// `NULL` on every row until its file is re-extracted, and **re-extraction is
/// triggered here** as migrations 25, 28 and 29 do it: every
/// `files.content_hash` is cleared, so the next scan, index or full-walk sync
/// re-extracts each file like a modified one. Until then each such file keeps
/// the path-derived key it had.
///
/// Forward-only ([FR-DB-04], [NFR-MA-06]) — asserted on a populated store by
/// `migration_30_adds_the_file_namespace_and_triggers_reextraction` in
/// [`super::migrate`].
///
/// [CR-170]: ../../../../docs/requests/CR-170-modules-namespaces-and-types-beyond-rust-and-java.md
/// [FR-RS-13]: ../../../../docs/specs/requirements/FR-RS-13.md
/// [ADR-07]: ../../../../docs/specs/architecture/decisions/ADR-07.md
/// [FR-DB-04]: ../../../../docs/specs/requirements/FR-DB-04.md
/// [NFR-MA-06]: ../../../../docs/specs/requirements/NFR-MA-06.md
const MIGRATION_30: &str = "\
-- 1. The namespace a file declares (FR-RS-13): '.'-joined, '' for the global
-- namespace. NULL for a file keyed by its path, and on rows indexed before this
-- migration.
ALTER TABLE files ADD COLUMN namespace TEXT;

-- 2. Trigger re-extraction: a file with no recorded hash is re-extracted on its
-- next scan like a modified one, filling the column.
UPDATE files SET content_hash = NULL;
";

/// Migration 31 — the import **alias** joins the reference-ledger identity
/// (S-597, [CR-194], [FR-DB-07]).
///
/// An import binds the name it is aliased by ([FR-EX-14]), so `from pkg.m import
/// X as A` and `… as B` — and `import numpy` beside `import numpy as np` — are
/// two bindings of one target. The identity index of migration 29 omits the
/// alias, so the second row collided with the first and was dropped at insert:
/// the second local name never reached the ledger and never bound. The index is
/// recreated with `COALESCE(alias, '')` appended — the `COALESCE` for the reason
/// migration 18 gives for `payload` (SQLite treats each bare `NULL` as distinct
/// in a UNIQUE key), so a row with no alias dedups exactly as before.
///
/// Only an index is dropped and recreated: no table is rebuilt, no row, id or
/// column changes, and `nodes`, `edges`, the FTS index and the `annotations`
/// view are byte-for-byte unaffected. Every stored row is unique under the
/// narrower key, so each is unique under the wider one.
///
/// The rows the old identity dropped were never stored, so they cannot be
/// recovered here — only re-extracted, and **re-extraction is triggered here**
/// as migrations 25, 28, 29 and 30 do it: every `files.content_hash` is cleared,
/// so the next scan, index or full-walk sync re-extracts each file like a
/// modified one. Until then a store keeps the rows it had.
///
/// Its own migration, not an edit of 29 or 30: those are recorded once on every
/// store that applied them, so changing their text would reach no upgrader.
///
/// Forward-only ([FR-DB-04], [NFR-MA-06]) — asserted on a populated store by
/// `migration_31_adds_the_alias_to_the_ledger_identity_and_triggers_reextraction`
/// in [`super::migrate`].
///
/// [CR-194]: ../../../../docs/requests/CR-194-the-reference-ledger-identity-includes-the-alias.md
/// [FR-DB-07]: ../../../../docs/specs/requirements/FR-DB-07.md
/// [FR-EX-14]: ../../../../docs/specs/requirements/FR-EX-14.md
/// [FR-DB-04]: ../../../../docs/specs/requirements/FR-DB-04.md
/// [NFR-MA-06]: ../../../../docs/specs/requirements/NFR-MA-06.md
const MIGRATION_31: &str = "\
-- 1. The alias joins the ledger identity (migration 29's index, widened): two
-- local names of one target are two rows. NULL normalised to '' so a row with no
-- alias dedups exactly as before.
DROP INDEX idx_unresolved_refs_identity;
CREATE UNIQUE INDEX idx_unresolved_refs_identity
    ON unresolved_refs(source_symbol, target, form, kind, COALESCE(payload, ''), COALESCE(receiver, 0), COALESCE(alias, ''));

-- 2. Trigger re-extraction: a file with no recorded hash is re-extracted on its
-- next scan like a modified one, recovering the rows the old identity dropped.
UPDATE files SET content_hash = NULL;
";

/// Migration 32 — the wrappers a proven receiver was **peeled** of, recorded on
/// its ledger row, and joining the ledger identity (S-587, [CR-188],
/// [FR-RS-42]).
///
/// A Rust call `x.m()` whose receiver's declared type the file proves is
/// retyped to the Path-form `T::m` of shape `other` (migration 29's `receiver`
/// column, `3`). Reaching `T` may peel `&`, `&mut`, `Box`, `Arc` or `Rc` off the
/// declared type, and a method the wrapper itself provides (`Arc::clone`) is not
/// `T`'s — so which wrappers were peeled is part of the proof the binder reads.
/// No existing column can hold it: `payload` is a relation class that rides
/// onto the bound edge and keys per-relation coverage, and `alias` is the local
/// name an import binds.
///
/// **`unresolved_refs.peeled`** — the peeled wrappers, outermost first and
/// space-joined (`&`, `&mut`, `Box`, `Arc`, `Rc`; `& Arc` for `&Arc<T>`).
/// `NULL` on every other row, and on a proven receiver declared `T` itself. One
/// nullable column added **in place** (the migration-25/28/29 shape): every
/// existing row and id is untouched. Free text with a closed vocabulary the
/// extractor writes, rather than a `CHECK`: a nested peel is a sequence.
///
/// It joins the ledger identity: a caller's `a.m()` with `a: Arc<T>` and `b.m()`
/// with `b: T` both record `T::m` and bind differently, so the identity index of
/// migration 31 is recreated with `COALESCE(peeled, '')` appended — the
/// `COALESCE` for the reason migration 18 gives for `payload`. Every stored row
/// has a `NULL` value, so each is unique under the wider key exactly as it was
/// under the narrower one. Only an index is dropped and recreated: no table is
/// rebuilt, so `nodes`, `edges`, the FTS index and the `annotations` view are
/// byte-for-byte unaffected.
///
/// **Re-extraction is triggered here** as migrations 25, 28–31 do it: every
/// `files.content_hash` is cleared, so the next scan, index or full-walk sync
/// re-extracts each file like a modified one and records the retyped rows.
/// Until then a store keeps the `other` Method rows it had.
///
/// Its own migration, not an edit of 29 or 31: those are recorded once on every
/// store that applied them, so changing their text would reach no upgrader.
///
/// Forward-only ([FR-DB-04], [NFR-MA-06]) — asserted on a populated store by
/// `migration_32_adds_the_peeled_wrappers_to_the_ledger_identity_and_triggers_reextraction`
/// in [`super::migrate`].
///
/// [CR-188]: ../../../../docs/requests/CR-188-rust-receiver-typing.md
/// [FR-RS-42]: ../../../../docs/specs/requirements/FR-RS-42.md
/// [FR-DB-04]: ../../../../docs/specs/requirements/FR-DB-04.md
/// [NFR-MA-06]: ../../../../docs/specs/requirements/NFR-MA-06.md
const MIGRATION_32: &str = "\
-- 1. The wrappers a proven receiver was peeled of (FR-RS-42): outermost first,
-- space-joined. NULL on every other row, and on rows extracted before this
-- migration.
ALTER TABLE unresolved_refs ADD COLUMN peeled TEXT;

-- 2. The peeled wrappers join the ledger identity (migration 31's index,
-- widened): a call through `Arc<T>` and one through `T` are two rows. NULL
-- normalised to '' so a row that peeled nothing dedups exactly as before.
DROP INDEX idx_unresolved_refs_identity;
CREATE UNIQUE INDEX idx_unresolved_refs_identity
    ON unresolved_refs(source_symbol, target, form, kind, COALESCE(payload, ''), COALESCE(receiver, 0), COALESCE(alias, ''), COALESCE(peeled, ''));

-- 3. Trigger re-extraction: a file with no recorded hash is re-extracted on its
-- next scan like a modified one, recording the retyped rows.
UPDATE files SET content_hash = NULL;
";

/// Migration 33 — a callable's **parameter range**, a call's **argument
/// count**, and whether a Rust `impl` function **takes `self`** (S-591,
/// [CR-190], [CR-200], [FR-EX-32]).
///
/// Three nullable columns on `nodes` and one on `unresolved_refs`, added **in
/// place** (the migration-25/28/29 shape — no table rebuilt, every existing row
/// and id untouched, the FTS index, its triggers and the `annotations` view
/// unaffected):
///
/// - **`nodes.param_min`** / **`nodes.param_max`** — the fewest and the most
///   arguments a call may pass a `Function`/`Method` node
///   ([`ParamRange`](crate::model::ParamRange)): a receiver parameter is not
///   counted, a defaulted one raises only the maximum. Both `NULL` for an
///   unknown range — every non-callable node, and a callable whose plugin
///   cannot count its parameters; `param_max` alone `NULL` for an unbounded one
///   (a variadic parameter). The `CHECK`s refuse a maximum without a minimum or
///   below it.
/// - **`nodes.takes_self`** — `1` when a Rust `impl` function's first parameter
///   is a receiver, `0` for an associated function (`fn new() -> Self`); `NULL`
///   on every other node.
/// - **`unresolved_refs.arg_count`** — how many arguments a `Calls` row's call
///   passes; `NULL` when it cannot be counted (a spread argument, a token tree
///   whose tokens may hide a comma) and on every non-call row.
///
/// **Plugin-agnostic by construction**: no column names a language; a plugin
/// fills them by declaring `@arity.*` captures in its own queries
/// (`extract::arity`), and one that declares none records unknown everywhere.
/// They are the input to the binder's candidate filters (S-604, S-592); an
/// unknown fact never filters.
///
/// The argument count joins the ledger identity: a caller's `f(a)` and
/// `f(a, b)` are two rows a binder admitting by range binds differently, so the
/// identity index of migration 32 is recreated with `COALESCE(arg_count, -1)`
/// appended — the `COALESCE` for the reason migration 18 gives for `payload`
/// (SQLite treats each bare `NULL` as distinct in a UNIQUE key), and `-1`
/// because `0` is a real count. Every stored row has a `NULL` count, so each is
/// unique under the wider key exactly as it was under the narrower one.
///
/// `NULL` on every row until its file is re-extracted, and **re-extraction is
/// triggered here** as migrations 25, 28–32 do it: every `files.content_hash`
/// is cleared, so the next scan, index or full-walk sync re-extracts each file
/// like a modified one and records the facts. No graph row is deleted.
///
/// One migration for all three facts, shared by S-591 and S-604 so a store
/// migrates once. Forward-only ([FR-DB-04], [NFR-MA-06]) — asserted on a
/// populated store by
/// `migration_33_adds_the_arity_facts_and_the_argument_count_to_the_ledger_identity`
/// in [`super::migrate`].
///
/// [CR-190]: ../../../../docs/requests/CR-190-a-self-call-binds-only-a-callable-whose-arity-admits-it.md
/// [CR-200]: ../../../../docs/requests/CR-200-a-rust-method-call-binds-only-a-callable-that-takes-self.md
/// [FR-EX-32]: ../../../../docs/specs/requirements/FR-EX-32.md
/// [FR-DB-04]: ../../../../docs/specs/requirements/FR-DB-04.md
/// [NFR-MA-06]: ../../../../docs/specs/requirements/NFR-MA-06.md
const MIGRATION_33: &str = "\
-- 1. A callable's parameter range (FR-EX-32): the fewest and most arguments a
-- call may pass. Both NULL for an unknown range, param_max alone for an
-- unbounded one, and on rows indexed before this migration.
ALTER TABLE nodes ADD COLUMN param_min INTEGER CHECK (param_min >= 0);
ALTER TABLE nodes ADD COLUMN param_max INTEGER CHECK (param_max IS NULL OR (param_min IS NOT NULL AND param_max >= param_min));

-- 2. Whether a Rust impl function takes `self` (CR-200). NULL on every other
-- node.
ALTER TABLE nodes ADD COLUMN takes_self INTEGER CHECK (takes_self IN (0,1));

-- 3. A call's argument count (FR-EX-32). NULL when it cannot be counted, on
-- every non-call row, and on rows extracted before this migration.
ALTER TABLE unresolved_refs ADD COLUMN arg_count INTEGER CHECK (arg_count >= 0);

-- 4. The argument count joins the ledger identity (migration 32's index,
-- widened): a caller's `f(a)` and `f(a, b)` are two rows. NULL normalised to -1
-- (0 is a real count) so a row with no count dedups exactly as before.
DROP INDEX idx_unresolved_refs_identity;
CREATE UNIQUE INDEX idx_unresolved_refs_identity
    ON unresolved_refs(source_symbol, target, form, kind, COALESCE(payload, ''), COALESCE(receiver, 0), COALESCE(alias, ''), COALESCE(peeled, ''), COALESCE(arg_count, -1));

-- 5. Trigger re-extraction: a file with no recorded hash is re-extracted on its
-- next scan like a modified one, filling the columns.
UPDATE files SET content_hash = NULL;
";

/// Migration 34 — the facts one Rust **associated-item lookup** reads (S-606,
/// [CR-202] F1–F3, [FR-EX-34]): every impl block's header, a callable's
/// receiver mode and required-signature marker, an enum's variant names, and
/// whether an import is a re-export.
///
/// Three nullable columns on `nodes`, one on `unresolved_refs` — added **in
/// place**, the migration-28/33 shape — and one new table:
///
/// - **`nodes.receiver_mode`** — how a callable writes its receiver
///   ([`ReceiverMode`](crate::model::ReceiverMode)): `0` none (an associated
///   function), `1` by value, `2` `&self`, `3` `&mut self`, `4` typed
///   `self: X`. Recorded wherever `takes_self` is; `NULL` elsewhere.
/// - **`nodes.variants`** — an enum's variant names in declaration order,
///   space-joined; `''` for an enum with none, `NULL` for every other node.
/// - **`nodes.signature`** — `1` for a **required signature** (a trait's
///   `fn m(&self);`), a bodyless `Method` node added by this story; `NULL` on
///   every other node. The binder keeps these nodes out of every candidate set
///   until the lookup that reads them ([FR-RS-47]) says how a call reaches one.
/// - **`unresolved_refs.exported`** — `1` for an `Imports` row of a `pub use`
///   (any `pub(…)`), `0` for a private one, `NULL` for every other row. Not in
///   the ledger identity: one declaration is either exported or not, so the
///   identity index of migration 33 is unchanged.
/// - **`impl_blocks`** — one row per impl block a file declares, an empty one
///   included: its self type as written (generics stripped; for a `&T`/`&mut T`
///   header the referent, with `self_ref` 1), its trait path for a trait impl,
///   and an `impl Deref`'s `type Target`. Keyed by the file, whose re-extraction
///   or removal takes the file's rows with it (the migration-24
///   `declared_types` cascade); the lines tie each block to the methods it
///   holds.
///
/// **Plugin-agnostic by construction**: no column or row names a language; a
/// plugin fills them by declaring the `@item.*` captures (`extract::assoc`) and
/// `@ref.use.exported`, and one that declares none records nothing.
///
/// `NULL`, and no row, until a file is re-extracted, and **re-extraction is
/// triggered here** as migrations 25 and 28–33 do it: every
/// `files.content_hash` is cleared, so the next scan, index or full-walk sync
/// re-extracts each file like a modified one. No graph row is deleted.
///
/// Forward-only ([FR-DB-04], [NFR-MA-06]) — asserted on a populated store by
/// `migration_34_adds_the_associated_item_facts_and_the_impl_block_table` in
/// [`super::migrate`].
///
/// [CR-202]: ../../../../docs/requests/CR-202-one-rust-associated-item-lookup.md
/// [FR-EX-34]: ../../../../docs/specs/requirements/FR-EX-34.md
/// [FR-RS-47]: ../../../../docs/specs/requirements/FR-RS-47.md
/// [FR-DB-04]: ../../../../docs/specs/requirements/FR-DB-04.md
/// [NFR-MA-06]: ../../../../docs/specs/requirements/NFR-MA-06.md
const MIGRATION_34: &str = "\
-- 1. How a callable writes its receiver (FR-EX-34): 0 none, 1 by value, 2 &,
-- 3 &mut, 4 typed. Recorded wherever takes_self is, NULL wherever takes_self
-- is NULL, and on rows indexed before this migration.
ALTER TABLE nodes ADD COLUMN receiver_mode INTEGER CHECK (receiver_mode IN (0,1,2,3,4));

-- 2. An enum's variant names, space-joined in declaration order, '' for none.
ALTER TABLE nodes ADD COLUMN variants TEXT;

-- 3. A required signature (CR-202 F3): a bodyless trait member. NULL elsewhere.
ALTER TABLE nodes ADD COLUMN signature INTEGER CHECK (signature = 1);

-- 4. Whether an import row's declaration is a re-export (`pub use`). Outside
-- the ledger identity: one declaration is exported or not.
ALTER TABLE unresolved_refs ADD COLUMN exported INTEGER CHECK (exported IN (0,1));

-- 5. Every impl block's header (CR-202 F1), an empty block included. A file's
-- rows go with the file. trait_path is NULL for an inherent impl, and
-- deref_target is NULL but for an impl of a trait.
CREATE TABLE impl_blocks (
    id           INTEGER PRIMARY KEY,
    file_id      INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    start_line   INTEGER NOT NULL CHECK (start_line >= 1),
    end_line     INTEGER NOT NULL CHECK (end_line >= start_line),
    self_type    TEXT NOT NULL CHECK (self_type <> ''),
    self_ref     INTEGER NOT NULL CHECK (self_ref IN (0,1)),
    trait_path   TEXT,
    deref_target TEXT,
    CHECK (deref_target IS NULL OR trait_path IS NOT NULL)
) STRICT;

-- The cascade's lookup: re-extracting a file deletes its rows through it.
CREATE INDEX idx_impl_blocks_file ON impl_blocks(file_id);

-- 6. Trigger re-extraction: a file with no recorded hash is re-extracted on its
-- next scan like a modified one, filling the columns and the table.
UPDATE files SET content_hash = NULL;
";

/// Migration 35 — the interface members an implementing type does **not**
/// inherit (S-609, [CR-202] Part 2, [FR-RS-48]).
///
/// One nullable column on `nodes`, added **in place** (the migration-28/33/34
/// shape):
///
/// - **`nodes.uninherited`** — `1` for an interface member a type
///   implementing the interface does not inherit: a Java `static` or
///   `private` interface method, a Kotlin `private` interface `fun`. `NULL` on
///   every other node. A supertype walk that goes on from a class's `Extends`
///   chain to its interfaces never binds one, nor an abstract member (a
///   callable recorded with `has_body` 0, migration 25); the interface's own
///   members still include it, so `I.m()` written on the interface binds it as
///   before.
///
/// **Plugin-agnostic by construction**: no column names a language; a plugin
/// fills it by declaring the `@item.uninherited` capture (`extract::assoc`),
/// and one that declares none records nothing.
///
/// `NULL` until a file is re-extracted, and **re-extraction is triggered
/// here** as migrations 25 and 28–34 do it: every `files.content_hash` is
/// cleared, so the next scan, index or full-walk sync re-extracts each file
/// like a modified one. No graph row is deleted.
///
/// Forward-only ([FR-DB-04], [NFR-MA-06]) — asserted on a populated store by
/// `migration_35_adds_the_uninherited_marker` in [`super::migrate`].
///
/// [CR-202]: ../../../../docs/requests/CR-202-one-rust-associated-item-lookup.md
/// [FR-RS-48]: ../../../../docs/specs/requirements/FR-RS-48.md
/// [FR-DB-04]: ../../../../docs/specs/requirements/FR-DB-04.md
/// [NFR-MA-06]: ../../../../docs/specs/requirements/NFR-MA-06.md
const MIGRATION_35: &str = "\
-- 1. An interface member its implementing types do not inherit (FR-RS-48): a
-- static or private interface method. NULL elsewhere, and on rows indexed
-- before this migration.
ALTER TABLE nodes ADD COLUMN uninherited INTEGER CHECK (uninherited = 1);

-- 2. Trigger re-extraction: a file with no recorded hash is re-extracted on its
-- next scan like a modified one, filling the column.
UPDATE files SET content_hash = NULL;
";

#[cfg(test)]
mod tests {
    use super::{
        MIGRATION_1, MIGRATION_10, MIGRATION_11, MIGRATION_12, MIGRATION_13, MIGRATION_14,
        MIGRATION_15, MIGRATION_16, MIGRATION_17, MIGRATION_18, MIGRATION_19, MIGRATION_2,
        MIGRATION_20, MIGRATION_21, MIGRATION_22, MIGRATION_25, MIGRATION_29, MIGRATION_3,
        MIGRATION_31, MIGRATION_32, MIGRATION_33, MIGRATION_34, MIGRATION_4, MIGRATION_8,
    };
    use crate::model::{EdgeKind, NodeKind, ReceiverMode, ReceiverShape, RefForm};

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
    /// (FR-DB-01). The authoritative `nodes.kind`/`edges.kind` CHECKs live in the
    /// migration-17 rebuild (the CR-061 broker-kind widening) — its first
    /// `kind IN (` is `nodes.kind`, its second `edges.kind`. The authoritative
    /// `unresolved_refs.kind` CHECK now lives in migration 18's ledger rebuild
    /// (CR-080's relation-aware key), the sole `kind IN (` there.
    #[test]
    fn schema_check_matches_model_ontology() {
        let node_model: Vec<i32> = NodeKind::ALL.iter().map(|k| k.as_i32()).collect();
        let edge_model: Vec<i32> = EdgeKind::ALL.iter().map(|k| k.as_i32()).collect();
        assert_eq!(
            check_discriminants(MIGRATION_17, "kind IN (", 0),
            node_model,
            "nodes.kind CHECK (migration 17 rebuild) must equal NodeKind::ALL discriminants"
        );
        assert_eq!(
            check_discriminants(MIGRATION_17, "kind IN (", 1),
            edge_model,
            "edges.kind CHECK (migration 17 rebuild) must equal EdgeKind::ALL discriminants"
        );
        // The ledger's kind CHECK (the sole `kind IN (` in migration 18's rebuild)
        // widens in lockstep — an unindexed workspace-relative topic binding must
        // persist. Migration 18 carries the latest ledger table definition.
        assert_eq!(
            check_discriminants(MIGRATION_18, "kind IN (", 0),
            edge_model,
            "unresolved_refs.kind CHECK (migration 18) must equal EdgeKind::ALL discriminants"
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

    /// Migration 17 widens `nodes.kind`, `edges.kind`, and `unresolved_refs.kind`
    /// together to the CR-061 broker kinds (S-255, [ADR-55], [FR-WS-11]): three
    /// node discriminants (35..=37) and two edge discriminants (16..=17). The
    /// load-bearing invariants: all three CHECKs land at their widened bounds,
    /// the rebuild drops and recreates `nodes`/`edges`/`shingles`/
    /// `unresolved_refs` (children-first), the UNIQUE keys and the
    /// `UNIQUE(symbol_id)` constraint (carried from migration 16) are unchanged,
    /// and no `'rebuild'` FTS repopulation is needed — this migration deletes no
    /// row (pure additive widening, unlike migration 16's dedup rebuild).
    #[test]
    fn migration_17_widens_nodes_edges_and_ledger_to_the_broker_kinds() {
        // All three kind CHECKs widen to their new bounds.
        assert_eq!(
            check_discriminants(MIGRATION_17, "kind IN (", 0),
            (1..=37).collect::<Vec<i32>>(),
            "migration 17 nodes.kind must widen to the broker kinds (1..=37)"
        );
        assert_eq!(
            check_discriminants(MIGRATION_17, "kind IN (", 1),
            (1..=17).collect::<Vec<i32>>(),
            "migration 17 edges.kind must widen to the broker kinds (1..=17)"
        );
        assert_eq!(
            check_discriminants(MIGRATION_17, "kind IN (", 2),
            (1..=17).collect::<Vec<i32>>(),
            "migration 17 unresolved_refs.kind must widen to the broker kinds (1..=17)"
        );

        // A CHECK cannot be widened in place — every kind-bearing table (plus
        // shingles, which FK-references nodes) is rebuilt.
        assert!(
            MIGRATION_17.contains("DROP TABLE nodes")
                && MIGRATION_17.contains("DROP TABLE edges")
                && MIGRATION_17.contains("DROP TABLE shingles")
                && MIGRATION_17.contains("DROP TABLE unresolved_refs"),
            "migration 17 must rebuild nodes, edges, shingles, and unresolved_refs"
        );
        assert!(
            MIGRATION_17.contains("CREATE TABLE shingles_stash")
                && MIGRATION_17.contains("CREATE TABLE unresolved_refs_stash"),
            "migration 17 must stash shingles and unresolved_refs before the drop"
        );

        // The UNIQUE keys — including the migration-16 UNIQUE(symbol_id) — are
        // unchanged; this migration widens CHECKs only, adds no key.
        assert!(
            MIGRATION_17.contains("UNIQUE (symbol_id)")
                && MIGRATION_17.contains("UNIQUE (source, target, kind)")
                && MIGRATION_17.contains("UNIQUE (source_symbol, target, form, kind)"),
            "migration 17 must keep every UNIQUE key unchanged (including UNIQUE(symbol_id))"
        );

        // No row is deleted (pure additive widening), so the FTS index needs no
        // 'rebuild' repopulation — unlike migration 16's dedup rebuild.
        assert!(
            !MIGRATION_17.contains("VALUES('rebuild')"),
            "migration 17 deletes no row, so no FTS 'rebuild' repopulation is needed"
        );
        for trigger in ["nodes_fts_ai", "nodes_fts_ad", "nodes_fts_au"] {
            assert!(
                MIGRATION_17.contains(&format!("CREATE TRIGGER {trigger}")),
                "migration 17 must recreate the {trigger} FTS trigger (NFR-RA-09)"
            );
        }
        assert!(
            MIGRATION_17.contains("DROP VIEW annotations")
                && MIGRATION_17.contains("CREATE VIEW annotations")
                && MIGRATION_17.contains("clone_group"),
            "migration 17 must recreate the annotations view with clone_group (migration-16 shape)"
        );
    }

    /// Migration 18 makes the reference-ledger uniqueness key **relation-aware**
    /// (CR-080, FR-DB-01, FR-WS-11): it rebuilds `unresolved_refs` to shed the
    /// shipped four-column table constraint and re-establishes uniqueness through
    /// a `UNIQUE INDEX` folding in the relation discriminator, NULL-normalised.
    /// The load-bearing invariants: only the ledger is rebuilt (no `nodes`/`edges`/
    /// `shingles` drop, no FTS/view churn — so a no-relay graph is byte-for-byte
    /// unaffected); the widened key adds the discriminator via `COALESCE(payload,
    /// '')`; the four-column table `UNIQUE` is gone; and the `kind` CHECK is
    /// byte-identical to migration 17's ledger CHECK (uniqueness widened, ontology
    /// unchanged).
    #[test]
    fn migration_18_widens_the_ledger_unique_to_be_relation_aware() {
        // Only the ledger is rebuilt — the graph tables and their sync machinery
        // are never touched, which is what keeps a no-relay graph byte-identical.
        assert!(
            MIGRATION_18.contains("DROP TABLE unresolved_refs")
                && MIGRATION_18.contains("CREATE TABLE unresolved_refs_stash"),
            "migration 18 must stash and rebuild the unresolved_refs ledger"
        );
        assert!(
            !MIGRATION_18.contains("TABLE nodes")
                && !MIGRATION_18.contains("TABLE edges")
                && !MIGRATION_18.contains("TABLE shingles")
                && !MIGRATION_18.contains("nodes_fts")
                && !MIGRATION_18.contains("VIEW annotations"),
            "migration 18 must touch only the ledger (no nodes/edges/shingles/FTS/view churn)"
        );

        // The relation-aware uniqueness: the four shipped key columns plus the
        // NULL-normalised relation discriminator, as a UNIQUE INDEX (a table-level
        // UNIQUE cannot carry an expression).
        assert!(
            MIGRATION_18.contains(
                "CREATE UNIQUE INDEX idx_unresolved_refs_identity\n    \
                 ON unresolved_refs(source_symbol, target, form, kind, COALESCE(payload, ''))"
            ),
            "migration 18 must key uniqueness on the relation discriminator (COALESCE(payload,''))"
        );
        // The shipped four-column table-level UNIQUE is retired by the rebuild.
        assert!(
            !MIGRATION_18.contains("UNIQUE (source_symbol, target, form, kind)"),
            "migration 18's rebuilt ledger must drop the relation-blind table UNIQUE"
        );

        // Uniqueness widened, ontology unchanged: the kind CHECK stays EdgeKind::ALL.
        assert_eq!(
            check_discriminants(MIGRATION_18, "kind IN (", 0),
            (1..=17).collect::<Vec<i32>>(),
            "migration 18 unresolved_refs.kind is byte-identical to migration 17 (1..=17)"
        );
        // The secondary indexes are recreated alongside the identity index.
        assert!(
            MIGRATION_18.contains("CREATE INDEX idx_unresolved_refs_file")
                && MIGRATION_18.contains("CREATE INDEX idx_unresolved_refs_resolved"),
            "migration 18 must recreate the ledger's secondary indexes"
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

    /// The **latest** `unresolved_refs.kind` CHECK now lives in migration 18's
    /// ledger rebuild (its sole `kind IN (`) and must equal the model's
    /// `EdgeKind::ALL` — the same exact drift guard the nodes/edges CHECKs carry,
    /// now that CR-080's relation-aware key rebuilds the ledger after migration 17
    /// first taught it the broker-topic bindings (CR-061/ADR-55, FR-WS-11). The
    /// migration-10, migration-14, and migration-17 ledger CHECKs are frozen at
    /// the kinds current when each shipped and are no longer authoritative.
    #[test]
    fn latest_unresolved_refs_kind_check_matches_edge_ontology() {
        let edge_model: Vec<i32> = EdgeKind::ALL.iter().map(|k| k.as_i32()).collect();
        assert_eq!(
            check_discriminants(MIGRATION_18, "kind IN (", 0),
            edge_model,
            "unresolved_refs.kind CHECK (migration 18 rebuild) must equal EdgeKind::ALL"
        );
        // Migration 17's ledger CHECK is frozen at its shipped value (1..=17).
        assert_eq!(
            check_discriminants(MIGRATION_17, "kind IN (", 2),
            (1..=17).collect::<Vec<i32>>(),
            "MIGRATION_17 unresolved_refs.kind is frozen at 1..=17"
        );
        // Migration 10's ledger CHECK is frozen at its shipped value (1..=13).
        assert_eq!(
            check_discriminants(MIGRATION_10, "kind IN (", 1),
            (1..=13).collect::<Vec<i32>>(),
            "MIGRATION_10 unresolved_refs.kind is frozen at 1..=13"
        );
        // Migration 14's ledger CHECK is frozen at its shipped value (1..=15).
        assert_eq!(
            check_discriminants(MIGRATION_14, "kind IN (", 1),
            (1..=15).collect::<Vec<i32>>(),
            "MIGRATION_14 unresolved_refs.kind is frozen at 1..=15"
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

    /// Migration 19 is additive by construction: it creates the two new corpus
    /// tables and nothing else. The runtime half — that a *populated* store's
    /// nodes, edges, shingles and FTS content survive the boundary verbatim — is
    /// `migration_19_adds_the_config_corpus_tables_preserving_the_graph_byte_for_byte`
    /// in `super::migrate`. This half guards the SQL text, so a later edit that
    /// reaches for a rebuild fails here before it can reach a database.
    #[test]
    fn migration_19_creates_only_the_config_corpus_tables() {
        for table in ["config_sources", "config_values"] {
            assert!(
                MIGRATION_19.contains(&format!("CREATE TABLE {table} (")),
                "migration 19 must create {table} (FR-WS-19, FR-DB-01)"
            );
        }
        for forbidden in ["DROP TABLE", "ALTER TABLE", "DROP INDEX", "DROP TRIGGER"] {
            assert!(
                !MIGRATION_19.contains(forbidden),
                "migration 19 must be purely additive — found `{forbidden}` (NFR-MA-06)"
            );
        }
        // Purely additive also means it creates NOTHING but those two tables: a
        // third `CREATE TABLE` here would be a table the losslessness test never
        // saw and the corpus model does not name.
        assert_eq!(
            MIGRATION_19.matches("CREATE TABLE").count(),
            2,
            "migration 19 creates exactly the two corpus tables"
        );
        // The graph tables are not so much as mentioned, which is the whole
        // argument for byte-for-byte preservation across the boundary.
        for untouched in ["nodes", "edges", "shingles", "nodes_fts", "unresolved_refs"] {
            assert!(
                !MIGRATION_19.contains(untouched),
                "migration 19 must not mention `{untouched}` (AC3: unchanged across the boundary)"
            );
        }
        // STRICT, like every table since migration 1 — a typed column is what
        // stops a value arriving as a blob and reading back as a different value.
        assert_eq!(
            MIGRATION_19.matches(") STRICT;").count(),
            2,
            "both corpus tables are STRICT (FR-DB-01)"
        );
        // An unprofiled source must be representable: `profile` carries no NOT
        // NULL, because NULL is the unprofiled `application.<ext>` and the census
        // counts it apart from the profiled files.
        assert!(
            MIGRATION_19.contains("profile TEXT\n"),
            "config_sources.profile must be nullable — NULL is the unprofiled source"
        );
    }

    /// Migration 20 adds the [FR-GV-21] check-run marker as **one** singleton
    /// table and nothing else (S-313, [CR-096]).
    ///
    /// The load-bearing invariants: the singleton is a schema `CHECK (id = 1)`
    /// rather than a caller convention (BR-40), `commit_sha` is nullable
    /// because a tree with no resolvable `HEAD` records NULL rather than a
    /// placeholder, `ran_at`/`violation_count` are NOT NULL because a marker
    /// that cannot say when or how many is not a marker, and the migration is
    /// purely additive so an existing store upgrades with no rebuild.
    ///
    /// [CR-096]: ../../../../docs/requests/CR-096-recorded-check-marker.md
    /// [FR-GV-21]: ../../../../docs/specs/requirements/FR-GV-21.md
    #[test]
    fn migration_20_adds_the_check_run_singleton_only() {
        assert!(
            MIGRATION_20.contains("CREATE TABLE check_run"),
            "migration 20 must create check_run (FR-GV-21)"
        );
        assert_eq!(
            MIGRATION_20.matches("CREATE TABLE").count(),
            1,
            "migration 20 creates exactly the one marker table"
        );
        // The singleton is structural — the same shape rules_cache has carried
        // since migration 5 (FR-GV-01), not a convention the writer must honour.
        assert!(
            MIGRATION_20.contains("id              INTEGER PRIMARY KEY CHECK (id = 1)"),
            "check_run must be a schema-enforced singleton (BR-40, the FR-GV-01 pattern)"
        );
        // ran_at and violation_count are mandatory; commit_sha is not, because
        // a tree with no resolvable HEAD stores NULL rather than a placeholder.
        assert!(
            MIGRATION_20.contains("ran_at          INTEGER NOT NULL")
                && MIGRATION_20.contains("violation_count INTEGER NOT NULL"),
            "ran_at and violation_count must be NOT NULL"
        );
        assert!(
            MIGRATION_20.contains("commit_sha      TEXT,\n"),
            "check_run.commit_sha must be nullable — NULL is 'HEAD did not resolve'"
        );
        // Purely additive: no rebuild, so an existing store upgrades in place
        // with no re-index (FR-DB-04, NFR-MA-06).
        for forbidden in ["DROP TABLE", "ALTER TABLE", "DROP INDEX", "DROP TRIGGER"] {
            assert!(
                !MIGRATION_20.contains(forbidden),
                "migration 20 must be purely additive — found `{forbidden}` (NFR-MA-06)"
            );
        }
        // The graph tables are not so much as mentioned — the argument for
        // byte-for-byte preservation across the boundary.
        for untouched in ["nodes", "edges", "shingles", "nodes_fts", "unresolved_refs"] {
            assert!(
                !MIGRATION_20.contains(untouched),
                "migration 20 must not mention `{untouched}` (additive upgrade in place)"
            );
        }
        assert_eq!(
            MIGRATION_20.matches(") STRICT;").count(),
            1,
            "check_run is STRICT like every table since migration 1 (FR-DB-01)"
        );
    }

    /// Migration 21 widens the `check_run` marker with the evaluated set
    /// (S-437, [CR-140] §3.1, [FR-GV-21]): the rule count that is
    /// `violation_count`'s denominator, whether a contract was present at all,
    /// and which operation wrote the row.
    ///
    /// The three claims worth doubting, in order: every added column is an
    /// in-place `ADD COLUMN` (the migration-12 shape) rather than a rebuild;
    /// **none** of them is `NOT NULL`, because a marker written before this
    /// migration recorded none of them and must read back as *unknown* rather
    /// than as a real zero ([NFR-CC-04]); and the two enumerated columns pin
    /// their vocabulary in the schema, so a mis-spelled operation fails the
    /// write instead of storing a value no reader knows.
    ///
    /// The runtime half — that a *populated* store crosses the boundary
    /// byte-for-byte and its existing marker keeps reading — is
    /// `migration_21_widens_the_check_run_marker_preserving_the_graph_byte_for_byte`
    /// in `super::migrate`. This half guards the SQL text, so a later edit
    /// reaching for a rebuild or a defaulted `NOT NULL` fails here before it
    /// can reach a database.
    ///
    /// [CR-140]: ../../../../docs/requests/CR-140-the-recorded-check-marker-carries-what-it-evaluated.md
    /// [FR-GV-21]: ../../../../docs/specs/requirements/FR-GV-21.md
    /// [NFR-CC-04]: ../../../../docs/specs/requirements/NFR-CC-04.md
    #[test]
    fn migration_21_widens_the_check_run_marker_with_the_evaluated_set_additively() {
        for column in ["checked_rules", "rules_present", "operation"] {
            assert!(
                MIGRATION_21.contains(&format!("ADD COLUMN {column}")),
                "migration 21 must add the {column} column (CR-140 §3.1, FR-GV-21)"
            );
        }
        assert_eq!(
            MIGRATION_21.matches("ALTER TABLE check_run ADD COLUMN").count(),
            3,
            "migration 21 adds exactly the three evaluated-set columns and nothing else"
        );
        // NULL is the pre-migration marker's honest reading (CRA-05): a
        // mandatory column would need a fallback value, and that value would be
        // a fabricated evaluated set on every store already on disk. Checked on
        // the column DEFINITIONS, not on the whole blob — the SQL comments are
        // free to discuss nullability, and an earlier draft of this assertion
        // failed on its own explanatory comment.
        for stmt in MIGRATION_21
            .lines()
            .filter(|line| line.starts_with("ALTER TABLE"))
        {
            assert!(
                !stmt.contains("NOT NULL") && !stmt.contains("DEFAULT"),
                "added column must be nullable with no default — NULL is 'written before \
                 migration 21', never a real zero (CR-140 CRA-05, NFR-CC-04): {stmt}"
            );
        }
        // The enumerated columns pin their vocabulary in the schema, the way
        // migration 12's applicability flags do.
        assert!(
            MIGRATION_21.contains("rules_present IN (0,1)"),
            "rules_present must carry a 0/1 CHECK (the migration-12 flag shape)"
        );
        assert!(
            MIGRATION_21.contains("operation IN ('check','scan')"),
            "operation must pin its vocabulary in the schema — the two operations that \
             call replace_violations"
        );
        // Additive only: no rebuild, so an existing store upgrades in place
        // with no re-index (FR-DB-04, NFR-MA-06).
        for forbidden in ["DROP TABLE", "DROP INDEX", "DROP TRIGGER", "CREATE TABLE"] {
            assert!(
                !MIGRATION_21.contains(forbidden),
                "migration 21 must be purely additive — found `{forbidden}` (NFR-MA-06)"
            );
        }
        // The graph tables are not so much as mentioned — the argument for
        // byte-for-byte preservation across the boundary.
        for untouched in ["nodes", "edges", "shingles", "nodes_fts", "unresolved_refs"] {
            assert!(
                !MIGRATION_21.contains(untouched),
                "migration 21 must not mention `{untouched}` (additive upgrade in place)"
            );
        }
    }

    /// Migration 22's vocabulary is exactly the build-manifest reader's (S-462,
    /// [CR-148] §3.2 A) — the token lists of its CHECKs equal the `as_str` of
    /// every variant, in both directions — and the migration is purely additive.
    ///
    /// The reader and the schema spell the same enumerations twice; this is the
    /// guard that they cannot drift, the migration-17 discriminant contract's
    /// shape for string vocabularies. The runtime half — a populated store
    /// crossing the boundary byte-for-byte — is
    /// `migration_22_adds_the_build_manifest_tables_preserving_the_graph_byte_for_byte`
    /// in `super::migrate`.
    ///
    /// [CR-148]: ../../../../docs/requests/CR-148-build-manifests-yield-a-build-dependency-relation.md
    #[test]
    fn migration_22_pins_the_build_manifest_vocabulary_and_is_purely_additive() {
        use crate::extract::build_manifest::{
            ArtifactRole, ManifestFormat, ManifestStatus, ReferenceKind, Resolution,
        };
        fn tokens(sql: &str, column: &str) -> Vec<String> {
            let marker = format!("{column} IN (");
            let (idx, _) = sql
                .match_indices(&marker)
                .next()
                .unwrap_or_else(|| panic!("a CHECK on {column}"));
            let tail = &sql[idx + marker.len()..];
            tail[..tail.find(')').expect("closing paren")]
                .split(',')
                .map(|t| t.trim().trim_matches('\'').to_string())
                .collect()
        }
        let expect = |column: &str, model: &[&str]| {
            assert_eq!(
                tokens(MIGRATION_22, column),
                model.iter().map(|t| t.to_string()).collect::<Vec<_>>(),
                "migration 22's {column} CHECK must equal the reader's vocabulary"
            );
        };
        expect("format", &[ManifestFormat::Maven.as_str(), ManifestFormat::Gradle.as_str()]);
        expect(
            "status",
            &[ManifestStatus::Read, ManifestStatus::Malformed, ManifestStatus::Unreadable].map(ManifestStatus::as_str),
        );
        expect("role", &[ArtifactRole::Produced.as_str(), ArtifactRole::Referenced.as_str()]);
        expect(
            "kind",
            &[ReferenceKind::Parent, ReferenceKind::Dependency, ReferenceKind::Managed, ReferenceKind::BomImport]
                .map(ReferenceKind::as_str),
        );
        expect(
            "resolution",
            &[Resolution::Resolved, Resolution::VersionRefused, Resolution::Refused].map(Resolution::as_str),
        );

        assert_eq!(MIGRATION_22.matches("CREATE TABLE").count(), 2, "exactly two new tables");
        for forbidden in ["DROP ", "ALTER TABLE", "INSERT INTO", "UPDATE "] {
            assert!(
                !MIGRATION_22.contains(forbidden),
                "migration 22 must be purely additive — found `{forbidden}` (NFR-MA-06)"
            );
        }
        for untouched in ["nodes", "edges", "shingles", "unresolved_refs", "files"] {
            assert!(
                !MIGRATION_22.contains(untouched),
                "migration 22 must not mention `{untouched}` (additive upgrade in place)"
            );
        }
    }

    /// S-500 / CR-163 / FR-EX-11: migration 25 adds the two per-callable columns
    /// to `nodes` in place and clears `files.content_hash` to trigger
    /// re-extraction — and does nothing else: no table is created, dropped or
    /// rebuilt, and no other table is written (NFR-MA-06). The populated-store
    /// upgrade is asserted by
    /// `migration_25_adds_the_has_body_columns_and_triggers_reextraction` in
    /// `super::migrate`.
    #[test]
    fn migration_25_adds_two_nodes_columns_and_clears_the_file_hashes_only() {
        let statements: Vec<String> = MIGRATION_25
            .split(';')
            .map(|stmt| {
                stmt.lines()
                    .filter(|l| !l.trim_start().starts_with("--"))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .map(|stmt| stmt.trim().to_string())
            .filter(|stmt| !stmt.is_empty())
            .collect();
        assert_eq!(
            statements,
            [
                "ALTER TABLE nodes ADD COLUMN has_body INTEGER CHECK (has_body IN (0,1))",
                "ALTER TABLE nodes ADD COLUMN body_tokens INTEGER CHECK (body_tokens >= 0)",
                "UPDATE files SET content_hash = NULL",
            ],
            "exactly the two in-place columns and the re-extraction trigger"
        );
    }

    /// Migration 29 (S-514) adds the receiver shape in place, widens the ledger
    /// identity index by it, and triggers re-extraction — nothing else; its
    /// `CHECK` is the [`ReceiverShape`] discriminant contract.
    #[test]
    fn migration_29_adds_the_receiver_shape_and_widens_the_ledger_identity_only() {
        let statements: Vec<String> = MIGRATION_29
            .split(';')
            .map(|stmt| {
                stmt.lines()
                    .filter(|l| !l.trim_start().starts_with("--"))
                    .map(str::trim)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .map(|stmt| stmt.trim().to_string())
            .filter(|stmt| !stmt.is_empty())
            .collect();
        assert_eq!(
            statements,
            [
                "ALTER TABLE unresolved_refs ADD COLUMN receiver INTEGER CHECK (receiver IN (1,2,3))",
                "DROP INDEX idx_unresolved_refs_identity",
                "CREATE UNIQUE INDEX idx_unresolved_refs_identity ON unresolved_refs(source_symbol, \
                 target, form, kind, COALESCE(payload, ''), COALESCE(receiver, 0))",
                "UPDATE files SET content_hash = NULL",
            ],
            "exactly the in-place column, the widened identity and the re-extraction trigger"
        );
        let shapes: Vec<i32> = ReceiverShape::ALL.iter().map(|s| s.as_i32()).collect();
        assert_eq!(
            check_discriminants(MIGRATION_29, "receiver IN (", 0),
            shapes,
            "unresolved_refs.receiver CHECK must equal ReceiverShape::ALL discriminants"
        );
    }

    /// Migration 31 (S-597) rebuilds the ledger identity index with the alias
    /// and triggers re-extraction — nothing else: no column, table or row is
    /// touched, and it is its own migration, not an edit of 29 or 30.
    #[test]
    fn migration_31_widens_the_ledger_identity_by_the_alias_only() {
        let statements: Vec<String> = MIGRATION_31
            .split(';')
            .map(|stmt| {
                stmt.lines()
                    .filter(|l| !l.trim_start().starts_with("--"))
                    .map(str::trim)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .map(|stmt| stmt.trim().to_string())
            .filter(|stmt| !stmt.is_empty())
            .collect();
        assert_eq!(
            statements,
            [
                "DROP INDEX idx_unresolved_refs_identity",
                "CREATE UNIQUE INDEX idx_unresolved_refs_identity ON unresolved_refs(source_symbol, \
                 target, form, kind, COALESCE(payload, ''), COALESCE(receiver, 0), COALESCE(alias, ''))",
                "UPDATE files SET content_hash = NULL",
            ],
            "exactly the widened identity and the re-extraction trigger"
        );
        assert!(
            !MIGRATION_29.contains("COALESCE(alias"),
            "the alias joins the identity in migration 31, never by editing migration 29"
        );
    }

    /// Migration 32 (S-587) adds the `peeled` column in place, rebuilds the
    /// ledger identity index with it and triggers re-extraction — nothing
    /// else, and it is its own migration, not an edit of 31.
    #[test]
    fn migration_32_adds_the_peeled_wrappers_and_widens_the_ledger_identity_only() {
        let statements: Vec<String> = MIGRATION_32
            .split(';')
            .map(|stmt| {
                stmt.lines()
                    .filter(|l| !l.trim_start().starts_with("--"))
                    .map(str::trim)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .map(|stmt| stmt.trim().to_string())
            .filter(|stmt| !stmt.is_empty())
            .collect();
        assert_eq!(
            statements,
            [
                "ALTER TABLE unresolved_refs ADD COLUMN peeled TEXT",
                "DROP INDEX idx_unresolved_refs_identity",
                "CREATE UNIQUE INDEX idx_unresolved_refs_identity ON unresolved_refs(source_symbol, \
                 target, form, kind, COALESCE(payload, ''), COALESCE(receiver, 0), COALESCE(alias, ''), \
                 COALESCE(peeled, ''))",
                "UPDATE files SET content_hash = NULL",
            ],
            "exactly the column, the widened identity and the re-extraction trigger"
        );
        assert!(
            !MIGRATION_31.contains("peeled"),
            "the peeled wrappers join the identity in migration 32, never by editing migration 31"
        );
    }

    /// Migration 33 (S-591) adds the three arity facts in place, rebuilds the
    /// ledger identity index with the argument count and triggers re-extraction
    /// — nothing else, and it is its own migration, not an edit of 32.
    #[test]
    fn migration_33_adds_the_arity_facts_and_widens_the_ledger_identity_only() {
        let statements: Vec<String> = MIGRATION_33
            .split(';')
            .map(|stmt| {
                stmt.lines()
                    .filter(|l| !l.trim_start().starts_with("--"))
                    .map(str::trim)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .map(|stmt| stmt.trim().to_string())
            .filter(|stmt| !stmt.is_empty())
            .collect();
        assert_eq!(
            statements,
            [
                "ALTER TABLE nodes ADD COLUMN param_min INTEGER CHECK (param_min >= 0)",
                "ALTER TABLE nodes ADD COLUMN param_max INTEGER CHECK (param_max IS NULL OR \
                 (param_min IS NOT NULL AND param_max >= param_min))",
                "ALTER TABLE nodes ADD COLUMN takes_self INTEGER CHECK (takes_self IN (0,1))",
                "ALTER TABLE unresolved_refs ADD COLUMN arg_count INTEGER CHECK (arg_count >= 0)",
                "DROP INDEX idx_unresolved_refs_identity",
                "CREATE UNIQUE INDEX idx_unresolved_refs_identity ON unresolved_refs(source_symbol, \
                 target, form, kind, COALESCE(payload, ''), COALESCE(receiver, 0), COALESCE(alias, ''), \
                 COALESCE(peeled, ''), COALESCE(arg_count, -1))",
                "UPDATE files SET content_hash = NULL",
            ],
            "exactly the columns, the widened identity and the re-extraction trigger"
        );
        assert!(
            !MIGRATION_32.contains("arg_count"),
            "the argument count joins the identity in migration 33, never by editing migration 32"
        );
    }

    /// Migration 34 (S-606) adds the associated-item facts in place, creates
    /// the impl-block table and triggers re-extraction — nothing else: the
    /// ledger identity of migration 33 is not touched, and it is its own
    /// migration, not an edit of 33. The receiver-mode CHECK is the model's.
    #[test]
    fn migration_34_adds_the_associated_item_facts_and_the_impl_block_table_only() {
        let statements: Vec<String> = MIGRATION_34
            .split(';')
            .map(|stmt| {
                stmt.lines()
                    .filter(|l| !l.trim_start().starts_with("--"))
                    .map(str::trim)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .map(|stmt| stmt.trim().to_string())
            .filter(|stmt| !stmt.is_empty())
            .collect();
        assert_eq!(
            statements,
            [
                "ALTER TABLE nodes ADD COLUMN receiver_mode INTEGER CHECK (receiver_mode IN (0,1,2,3,4))",
                "ALTER TABLE nodes ADD COLUMN variants TEXT",
                "ALTER TABLE nodes ADD COLUMN signature INTEGER CHECK (signature = 1)",
                "ALTER TABLE unresolved_refs ADD COLUMN exported INTEGER CHECK (exported IN (0,1))",
                "CREATE TABLE impl_blocks ( id           INTEGER PRIMARY KEY, \
                 file_id      INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE, \
                 start_line   INTEGER NOT NULL CHECK (start_line >= 1), \
                 end_line     INTEGER NOT NULL CHECK (end_line >= start_line), \
                 self_type    TEXT NOT NULL CHECK (self_type <> ''), \
                 self_ref     INTEGER NOT NULL CHECK (self_ref IN (0,1)), \
                 trait_path   TEXT, deref_target TEXT, \
                 CHECK (deref_target IS NULL OR trait_path IS NOT NULL) ) STRICT",
                "CREATE INDEX idx_impl_blocks_file ON impl_blocks(file_id)",
                "UPDATE files SET content_hash = NULL",
            ],
            "exactly the in-place columns, the impl-block table and the re-extraction trigger"
        );
        let modes: Vec<i32> = ReceiverMode::ALL.iter().map(|m| m.as_i32()).collect();
        assert_eq!(
            check_discriminants(MIGRATION_34, "receiver_mode IN (", 0),
            modes,
            "nodes.receiver_mode CHECK must equal ReceiverMode::ALL discriminants"
        );
        assert!(
            !MIGRATION_34.contains("idx_unresolved_refs_identity"),
            "the export mark stays outside the ledger identity"
        );
        assert!(
            !MIGRATION_33.contains("impl_blocks") && !MIGRATION_33.contains("receiver_mode"),
            "the associated-item facts arrive in migration 34, never by editing migration 33"
        );
    }
}
