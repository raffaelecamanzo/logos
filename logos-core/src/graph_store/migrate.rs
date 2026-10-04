//! The forward-only migration runner ([FR-DB-04], [NFR-MA-06], [NFR-RA-07]).
//!
//! On open, every embedded migration whose version exceeds the database's
//! current `PRAGMA user_version` is applied **in its own transaction**. Each
//! migration is therefore atomic: a crash or error mid-migration rolls back
//! cleanly, leaving the database at its previous version with no partial schema
//! ([NFR-RA-07]).
//!
//! `user_version` — not the `schema_versions` table — is the authoritative
//! gate. It is a value in the database header, so reading it needs no table to
//! exist yet (avoiding the chicken-and-egg of querying `schema_versions` before
//! migration 1 creates it), and writing it is transactional, so a rolled-back
//! migration also reverts the version pointer.
//!
//! [FR-DB-04]: ../../../../docs/specs/requirements/FR-DB-04.md
//! [NFR-MA-06]: ../../../../docs/specs/requirements/NFR-MA-06.md
//! [NFR-RA-07]: ../../../../docs/specs/requirements/NFR-RA-07.md

use anyhow::{Context, Result};
use rusqlite::Connection;

use super::schema::MIGRATIONS;

/// Apply every embedded migration newer than the database's current version.
///
/// Idempotent: re-running on an up-to-date database applies nothing. Each
/// migration runs in a single transaction that also records the version in
/// `schema_versions` and advances `user_version` — all or nothing
/// ([NFR-RA-07]).
///
/// # Errors
/// Returns an error (and leaves the database at its prior version) if a
/// migration's SQL fails to apply or the transaction cannot commit.
pub(crate) fn apply_migrations(conn: &mut Connection) -> Result<()> {
    debug_assert!(
        migrations_are_strictly_increasing(),
        "embedded MIGRATIONS must be dense, 1-based, and strictly increasing"
    );
    apply_migrations_from(conn, MIGRATIONS)
}

/// Apply a specific migration ledger. Factored out of [`apply_migrations`] so
/// the atomic-rollback path can be tested with a deliberately failing ledger
/// without mutating the embedded [`MIGRATIONS`] const.
fn apply_migrations_from(conn: &mut Connection, migrations: &[(i64, &str)]) -> Result<()> {
    let current = current_version(conn)?;

    for &(version, sql) in migrations {
        if version <= current {
            continue;
        }

        // One transaction per migration. On any `?` below, `tx` is dropped
        // without committing and rusqlite rolls it back — the atomic-batch
        // crash-safety contract (NFR-RA-07).
        let tx = conn
            .transaction()
            .with_context(|| format!("opening transaction for migration {version}"))?;

        tx.execute_batch(sql)
            .with_context(|| format!("applying migration {version}"))?;

        tx.execute(
            "INSERT INTO schema_versions (version, applied_at) VALUES (?1, unixepoch())",
            [version],
        )
        .with_context(|| format!("recording migration {version} in schema_versions"))?;

        // user_version is part of the database header and updates inside the
        // transaction, so a rollback reverts it together with the schema.
        tx.pragma_update(None, "user_version", version)
            .with_context(|| format!("advancing user_version to {version}"))?;

        tx.commit()
            .with_context(|| format!("committing migration {version}"))?;
    }

    Ok(())
}

/// Read the database's current schema version from `PRAGMA user_version`.
///
/// A freshly created database reports `0`, so migration 1 always applies.
pub(crate) fn current_version(conn: &Connection) -> Result<i64> {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
        .context("reading PRAGMA user_version")
}

/// The newest schema version the embedded ledger knows about.
///
/// This is the version a fully migrated database reports. A read-only consumer
/// (the WAL reader pool, [`super::SqliteGraphStore::open_readonly`]) checks the
/// store it attaches to is at this version, since a read-only connection cannot
/// migrate the database itself.
pub(crate) fn latest_version() -> i64 {
    MIGRATIONS.last().map(|&(version, _)| version).unwrap_or(0)
}

/// `true` if the embedded ledger is dense, 1-based, and strictly increasing.
///
/// Guards the forward-only invariant structurally: a fat-fingered duplicate or
/// out-of-order version would corrupt the apply logic, so we assert the shape
/// in debug builds and in the test below.
fn migrations_are_strictly_increasing() -> bool {
    MIGRATIONS
        .iter()
        .enumerate()
        .all(|(idx, &(version, _))| version == idx as i64 + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// Open an in-memory connection with the production pragma contract
    /// (notably `foreign_keys = ON` — the migration-3 rebuild must work under
    /// exactly the enforcement the real runner applies).
    fn contract_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;\n\
             PRAGMA foreign_keys = ON;\n\
             PRAGMA synchronous = NORMAL;",
        )
        .unwrap();
        conn
    }

    #[test]
    fn ledger_is_dense_one_based_and_increasing() {
        assert!(migrations_are_strictly_increasing());
        assert_eq!(MIGRATIONS[0].0, 1, "v1 must be migration 1 (FR-DB-04)");
    }

    /// A migration whose SQL fails mid-way must roll back atomically: the
    /// version pointer stays put and no partial `schema_versions` row survives
    /// (NFR-RA-07). Uses an injected ledger so the real MIGRATIONS const is
    /// untouched.
    #[test]
    fn failed_migration_rolls_back_atomically() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        // v1 is valid; v2 references a non-existent table → fails mid-batch.
        let ledger: &[(i64, &str)] = &[
            (1, "CREATE TABLE schema_versions (version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL) STRICT;"),
            (2, "CREATE TABLE ok (id INTEGER PRIMARY KEY); INSERT INTO does_not_exist VALUES (1);"),
        ];

        let err = apply_migrations_from(&mut conn, ledger);
        assert!(err.is_err(), "the broken migration must error");

        // v1 committed; v2 rolled back wholesale.
        assert_eq!(
            current_version(&conn).unwrap(),
            1,
            "user_version stops at the last good migration"
        );
        let recorded: i64 = conn
            .query_row("SELECT count(*) FROM schema_versions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            recorded, 1,
            "no schema_versions row for the failed migration"
        );
        // The half of v2 that 'succeeded' before the failing statement must not persist.
        let ok_table: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='ok'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            ok_table, 0,
            "no partial schema from the failed migration (NFR-RA-07)"
        );
    }

    /// A populated v2 database upgraded to v3 keeps every node, every edge
    /// (the rebuild must not let the rename/drop cascade through the edges
    /// FKs), and a consistent FTS index — and gains the annotation columns and
    /// the `annotations` view (S-014, FR-AN-04).
    #[test]
    fn migration_3_rebuild_preserves_graph_data_and_fts() {
        let mut conn = contract_conn();

        // Stop at v2, then populate a small cross-file caller/callee graph.
        apply_migrations_from(&mut conn, &MIGRATIONS[..2]).unwrap();
        conn.execute_batch(
            "INSERT INTO files (id, path) VALUES (1, 'a.rs'), (2, 'b.rs');
             INSERT INTO symbols (id, symbol) VALUES (1, 'local a'), (2, 'local b');
             INSERT INTO nodes (id, symbol_id, kind, name, file_id) VALUES
                 (10, 1, 7, 'caller', 1),
                 (20, 2, 7, 'callee', 2);
             INSERT INTO edges (source, target, kind) VALUES (10, 20, 2);",
        )
        .unwrap();

        // Upgrade to v3 under the production FK contract (this test pins the
        // v2 → v3 rebuild specifically; later migrations are additive).
        apply_migrations_from(&mut conn, &MIGRATIONS[..3]).unwrap();
        assert_eq!(current_version(&conn).unwrap(), 3);

        // Every node and — critically — every edge survives the rebuild.
        let nodes: i64 = conn
            .query_row("SELECT count(*) FROM nodes", [], |r| r.get(0))
            .unwrap();
        let edges: i64 = conn
            .query_row("SELECT count(*) FROM edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(nodes, 2, "nodes copied through the rebuild");
        assert_eq!(edges, 1, "the cross-node edge must survive (no FK cascade)");

        // Row ids are preserved, so the FTS external-content index stays
        // aligned: the integrity check passes and a name still matches.
        conn.execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES('integrity-check');")
            .expect("FTS index consistent after the rebuild (NFR-RA-09)");
        let hit: i64 = conn
            .query_row(
                "SELECT count(*) FROM nodes_fts WHERE nodes_fts MATCH 'caller'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hit, 1, "FTS still finds the pre-migration name");

        // Old rows carry the un-annotated defaults on the new columns.
        let (derived, exported, is_dead): (i64, i64, Option<i64>) = conn
            .query_row(
                "SELECT derived, exported, is_dead FROM nodes WHERE id = 10",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((derived, exported, is_dead), (0, 0, None));

        // The FR-AN-04 queryable shape exists — as a view over native columns,
        // not a sidecar table.
        let view_rows: i64 = conn
            .query_row("SELECT count(*) FROM annotations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(view_rows, 2, "annotations view projects every node");
        let sidecar_tables: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='annotations'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            sidecar_tables, 0,
            "annotations is a view, never a table (FR-AN-04)"
        );

        // The widened CHECK accepts the appended policy kinds (16/17)…
        conn.execute(
            "INSERT INTO nodes (symbol_id, kind, name, derived) VALUES (1, 16, 'domain', 1)",
            [],
        )
        .expect("policy kind 16 accepted after the widening");
        // …and still rejects out-of-ontology values.
        assert!(
            conn.execute(
                "INSERT INTO nodes (symbol_id, kind, name) VALUES (1, 18, 'nope')",
                [],
            )
            .is_err(),
            "kind 18 is outside the frozen ontology"
        );

        // The rename did not leak: legacy_alter_table is restored and the FK
        // relationship is live (deleting a node cascades its edge).
        conn.execute("DELETE FROM nodes WHERE id = 20", []).unwrap();
        let edges_after: i64 = conn
            .query_row("SELECT count(*) FROM edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            edges_after, 0,
            "edges FK still cascades against the rebuilt nodes"
        );
    }

    /// A database created **before** migration 6 (at v5, populated) upgrades to
    /// v6 in place — additively, with no table rebuild — and its existing rows
    /// gain the `test_evidence`/`is_test` columns at the honest `0` default
    /// (forward-only, FR-AN-05, FR-AN-04, NFR-MA-06). The `annotations` view
    /// projects `is_test`.
    #[test]
    fn migration_6_adds_test_columns_in_place_without_rebuild() {
        let mut conn = contract_conn();

        // Stop at v5 and populate a node, exactly as a pre-S-028 database holds.
        apply_migrations_from(&mut conn, &MIGRATIONS[..5]).unwrap();
        conn.execute_batch(
            "INSERT INTO files (id, path) VALUES (1, 'a.rs');
             INSERT INTO symbols (id, symbol) VALUES (1, 'local a');
             INSERT INTO nodes (id, symbol_id, kind, name, file_id) VALUES (10, 1, 7, 'legacy', 1);",
        )
        .unwrap();

        // The pre-migration database has no idea about the new columns.
        assert!(
            conn.query_row("SELECT is_test FROM nodes WHERE id = 10", [], |r| r
                .get::<_, i64>(0))
                .is_err(),
            "is_test does not exist before migration 6"
        );

        // Upgrade across the v5 → v6 bump — the row survives untouched. Pin to
        // the first six migrations so this test stays scoped to migration 6 as
        // later additive migrations land.
        apply_migrations_from(&mut conn, &MIGRATIONS[..6]).unwrap();
        assert_eq!(current_version(&conn).unwrap(), 6);

        let nodes: i64 = conn
            .query_row("SELECT count(*) FROM nodes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(nodes, 1, "the pre-migration row survives in place");

        // The legacy row gains the new columns at the honest non-test default.
        let (evidence, is_test): (i64, i64) = conn
            .query_row(
                "SELECT test_evidence, is_test FROM nodes WHERE id = 10",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (evidence, is_test),
            (0, 0),
            "old rows default to non-test until their next annotation run"
        );

        // The FR-AN-04 view now projects is_test, and it remains a view.
        let view_is_test: i64 = conn
            .query_row(
                "SELECT is_test FROM annotations WHERE node_id = 10",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            view_is_test, 0,
            "is_test is queryable on the annotations view"
        );
        let sidecar_tables: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='annotations'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sidecar_tables, 0, "annotations stays a view, never a table");

        // The CHECK rejects an out-of-range value — the column is a real boolean.
        assert!(
            conn.execute("UPDATE nodes SET is_test = 2 WHERE id = 10", [])
                .is_err(),
            "is_test is CHECK-bound to (0,1)"
        );
    }

    /// A database created **before** migration 7 (at v6, with a metric snapshot)
    /// upgrades to v7 in place — additively — and its existing snapshot rows
    /// gain `test_function_count = 0` and, crucially, `metric_version = 1` (the
    /// old test-inclusive semantics), so the gate detects the baseline as
    /// incomparable and auto-re-baselines (S-029, FR-QM-08, FR-GV-10,
    /// NFR-MA-06, UAT-GV-06).
    #[test]
    fn migration_7_adds_production_scope_columns_in_place_without_rebuild() {
        let mut conn = contract_conn();

        // Stop at v6 and insert a metric snapshot exactly as a pre-S-029 run did
        // (no test_function_count / metric_version columns yet).
        apply_migrations_from(&mut conn, &MIGRATIONS[..6]).unwrap();
        conn.execute_batch(
            "INSERT INTO metric_snapshots
                 (created_at, node_count, edge_count, function_count, empty,
                  modularity_raw, modularity_normalized,
                  acyclicity_raw, acyclicity_normalized,
                  depth_raw, depth_normalized,
                  equality_raw, equality_normalized,
                  redundancy_raw, redundancy_normalized,
                  aggregate_signal)
             VALUES (100, 5, 4, 5, 0, 0.2, 0.6, 0.0, 1.0, 2.0, 0.8, 0.0, 1.0, 0.0, 1.0, 8000);",
        )
        .unwrap();

        // The pre-migration ledger has no idea about the new columns.
        assert!(
            conn.query_row("SELECT metric_version FROM metric_snapshots", [], |r| r
                .get::<_, i64>(0))
                .is_err(),
            "metric_version does not exist before migration 7"
        );

        // Upgrade across the v6 → v7 bump — the row survives untouched. Pin to
        // the first seven migrations so this test stays scoped to migration 7 as
        // later migrations (the CR-003 doc widening, migration 8) land.
        apply_migrations_from(&mut conn, &MIGRATIONS[..7]).unwrap();
        assert_eq!(current_version(&conn).unwrap(), 7);

        let snapshots: i64 = conn
            .query_row("SELECT count(*) FROM metric_snapshots", [], |r| r.get(0))
            .unwrap();
        assert_eq!(snapshots, 1, "the pre-migration snapshot survives in place");

        // The legacy snapshot gains the new columns at their forward-only
        // defaults: nothing excluded, and the OLD (v1) semantics version.
        let (tfc, version): (i64, i64) = conn
            .query_row(
                "SELECT test_function_count, metric_version FROM metric_snapshots",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (tfc, version),
            (0, 1),
            "a pre-upgrade snapshot excluded no tests and is the v1 (test-inclusive) \
             semantics — the re-baseline trigger (FR-GV-10)"
        );
    }

    /// A populated database created **before** migration 8 (at v7, with a
    /// cross-file caller/callee graph carrying real annotation data) upgrades to
    /// v8 in place: every node, every edge, every annotation column, and the FTS
    /// index survive the CHECK-widening rebuild with NO data loss
    /// (S-033, CR-003, ADR-19, FR-DB-01, NFR-MA-06, NFR-RA-07). Afterwards the
    /// widened CHECK accepts the documentation node kinds (18..=22) and edge
    /// kinds (11/12) and still rejects out-of-ontology values.
    #[test]
    fn migration_8_widens_checks_preserving_graph_data_and_fts() {
        let mut conn = contract_conn();

        // Stop at v7 and populate a small annotated graph exactly as a pre-CR-003
        // database holds (a `pub` exported function calling another, both with
        // per-function metrics and a test verdict).
        apply_migrations_from(&mut conn, &MIGRATIONS[..7]).unwrap();
        conn.execute_batch(
            "INSERT INTO files (id, path) VALUES (1, 'a.rs'), (2, 'b.rs');
             INSERT INTO symbols (id, symbol) VALUES (1, 'local a'), (2, 'local b');
             INSERT INTO nodes (id, symbol_id, kind, name, file_id, exported,
                                cyclomatic_complexity, line_count, is_test) VALUES
                 (10, 1, 7, 'caller', 1, 1, 3, 12, 0),
                 (20, 2, 7, 'callee', 2, 1, 1, 4, 1);
             INSERT INTO edges (source, target, kind) VALUES (10, 20, 2);
             INSERT INTO unresolved_refs (file_id, source_symbol, target, form, kind, resolved)
                 VALUES (1, 'local a', 'helper', 1, 2, 0);",
        )
        .unwrap();

        // The pre-migration CHECK rejects a documentation kind.
        assert!(
            conn.execute(
                "INSERT INTO nodes (symbol_id, kind, name) VALUES (1, 18, 'doc')",
                [],
            )
            .is_err(),
            "kind 18 (DocFile) is outside the pre-migration ontology"
        );

        // Upgrade across the v7 → v8 bump under the production FK contract. Pin
        // to the first eight migrations so this test stays scoped to migration 8
        // as later additive migrations (the S-037 FTS-body extension, migration
        // 9) land.
        apply_migrations_from(&mut conn, &MIGRATIONS[..8]).unwrap();
        assert_eq!(current_version(&conn).unwrap(), 8);

        // Every node and edge survives the rebuild (no FK cascade through edges).
        let nodes: i64 = conn
            .query_row("SELECT count(*) FROM nodes", [], |r| r.get(0))
            .unwrap();
        let edges: i64 = conn
            .query_row("SELECT count(*) FROM edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(nodes, 2, "nodes copied through the widening rebuild");
        assert_eq!(edges, 1, "the cross-node edge survives (no FK cascade)");

        // The real annotation data is carried over verbatim — NOT reset to
        // defaults (this is a widening of a populated table).
        let (exported, cc, lc, is_test): (i64, i64, i64, i64) = conn
            .query_row(
                "SELECT exported, cyclomatic_complexity, line_count, is_test \
                 FROM nodes WHERE id = 20",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            (exported, cc, lc, is_test),
            (1, 1, 4, 1),
            "annotation columns survive the rebuild unchanged (no data loss)"
        );

        // Row ids are preserved, so the FTS external-content index stays aligned.
        conn.execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES('integrity-check');")
            .expect("FTS index consistent after the rebuild (NFR-RA-09)");
        let hit: i64 = conn
            .query_row(
                "SELECT count(*) FROM nodes_fts WHERE nodes_fts MATCH 'caller'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hit, 1, "FTS still finds the pre-migration name");

        // The annotations view still projects is_test over the rebuilt table.
        let view_is_test: i64 = conn
            .query_row(
                "SELECT is_test FROM annotations WHERE node_id = 20",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(view_is_test, 1, "annotations view survives the rebuild");

        // The pre-migration unresolved_refs row survives its rebuild, and the
        // widened ledger CHECK now accepts a doc edge kind (11 = doc_reference),
        // so S-035 can bind doc→code references through the same ledger (ADR-19).
        let refs: i64 = conn
            .query_row("SELECT count(*) FROM unresolved_refs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(refs, 1, "the pre-migration ledger row survives the rebuild");
        conn.execute(
            "INSERT INTO unresolved_refs (file_id, source_symbol, target, form, kind, resolved) \
             VALUES (1, 'local a', 'docs/x.md', 1, 11, 0)",
            [],
        )
        .expect("the ledger accepts the doc_reference edge kind after the widening");

        // The widened CHECK now accepts the documentation node kinds…
        for kind in 18..=22 {
            conn.execute(
                "INSERT INTO nodes (symbol_id, kind, name) VALUES (1, ?1, 'doc')",
                [kind],
            )
            .unwrap_or_else(|e| panic!("doc node kind {kind} must be accepted: {e}"));
        }
        // …and the documentation edge kinds (between the two doc nodes just added)…
        let doc_ids: Vec<i64> = {
            let mut stmt = conn
                .prepare("SELECT id FROM nodes WHERE kind = 18 OR kind = 19 LIMIT 2")
                .unwrap();
            let ids = stmt
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            ids
        };
        for kind in [11, 12] {
            conn.execute(
                "INSERT INTO edges (source, target, kind) VALUES (?1, ?2, ?3)",
                rusqlite::params![doc_ids[0], doc_ids[1], kind],
            )
            .unwrap_or_else(|e| panic!("doc edge kind {kind} must be accepted: {e}"));
        }
        // …while still rejecting out-of-ontology values in both tables.
        assert!(
            conn.execute(
                "INSERT INTO nodes (symbol_id, kind, name) VALUES (1, 23, 'nope')",
                [],
            )
            .is_err(),
            "kind 23 is outside the widened ontology"
        );
        assert!(
            conn.execute(
                "INSERT INTO edges (source, target, kind) VALUES (10, 20, 13)",
                [],
            )
            .is_err(),
            "edge kind 13 is outside the widened ontology"
        );

        // The edges FK still cascades against the rebuilt nodes table.
        conn.execute("DELETE FROM nodes WHERE id = 20", []).unwrap();
        let edges_after: i64 = conn
            .query_row(
                "SELECT count(*) FROM edges WHERE source = 10 AND target = 20",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(edges_after, 0, "edges FK still cascades after the rebuild");
    }

    /// A populated database created **before** migration 9 (at v8, with a code
    /// node already FTS-indexed by name) upgrades to v9 in place: the `body`
    /// column is added additively (no rebuild, every row and id untouched), the
    /// FTS index is rebuilt over (name, body), and — the payoff — a `DocSection`
    /// row inserted afterwards is findable by a phrase living only in its body
    /// (S-037, CR-003, ADR-19, FR-DG-05, FR-DB-03, NFR-MA-06, NFR-RA-09).
    #[test]
    fn migration_9_extends_fts_to_doc_body_in_place() {
        let mut conn = contract_conn();

        // Stop at v8 and index a code node by name, exactly as a pre-S-037 store
        // holds (no `body` column yet; the heading-only doc layer).
        apply_migrations_from(&mut conn, &MIGRATIONS[..8]).unwrap();
        conn.execute_batch(
            "INSERT INTO files (id, path) VALUES (1, 'a.rs');
             INSERT INTO symbols (id, symbol) VALUES (1, 'local a');
             INSERT INTO nodes (id, symbol_id, kind, name, file_id) VALUES (10, 1, 7, 'caller', 1);",
        )
        .unwrap();

        // The pre-migration store has no `body` column.
        assert!(
            conn.query_row("SELECT body FROM nodes WHERE id = 10", [], |r| r
                .get::<_, Option<String>>(0))
                .is_err(),
            "body does not exist before migration 9"
        );

        // Upgrade across the v8 → v9 bump. The existing row survives untouched.
        apply_migrations_from(&mut conn, &MIGRATIONS[..9]).unwrap();
        assert_eq!(current_version(&conn).unwrap(), 9);
        let (name, body): (String, Option<String>) = conn
            .query_row("SELECT name, body FROM nodes WHERE id = 10", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(name, "caller", "the pre-migration row survives in place");
        assert_eq!(body, None, "a code node carries no body (FR-DG-05)");

        // The FTS index was rebuilt and still finds the pre-migration name.
        conn.execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES('integrity-check');")
            .expect("FTS index consistent after the rebuild (NFR-RA-09)");
        let by_name: i64 = conn
            .query_row(
                "SELECT count(*) FROM nodes_fts WHERE nodes_fts MATCH 'caller'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(by_name, 1, "FTS still finds the pre-migration name");

        // The payoff: a DocSection whose body holds a phrase absent from its
        // name is now searchable — the FR-DG-05 acceptance criterion, exercised
        // straight against the schema (the doc kind 19 the migration-8 CHECK
        // accepts; the body trigger indexes its prose).
        conn.execute(
            "INSERT INTO nodes (id, symbol_id, kind, name, body) \
             VALUES (20, 1, 19, 'Overview', ?1)",
            ["the quasiquibble step lives only in this body"],
        )
        .unwrap();
        let by_body: Vec<i64> = {
            let mut stmt = conn
                .prepare("SELECT rowid FROM nodes_fts WHERE nodes_fts MATCH 'quasiquibble'")
                .unwrap();
            stmt.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(
            by_body,
            [20],
            "a phrase only in a DocSection body is FTS-findable (FR-DG-05)"
        );

        // The UPDATE trigger (nodes_fts_au) must retract the OLD body posting
        // before re-indexing the new one, or an edited doc silently desyncs
        // (NFR-RA-09, SRS §7.2). Rewrite id 20's body and assert the old phrase
        // is gone while the new one is found.
        conn.execute(
            "UPDATE nodes SET body = ?1 WHERE id = 20",
            ["now the body says floomptard instead"],
        )
        .unwrap();
        conn.execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES('integrity-check');")
            .expect("FTS index consistent after a body update (NFR-RA-09)");
        let old_gone: i64 = conn
            .query_row(
                "SELECT count(*) FROM nodes_fts WHERE nodes_fts MATCH 'quasiquibble'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(old_gone, 0, "the old body posting is retracted on UPDATE");
        let new_found: i64 = conn
            .query_row(
                "SELECT count(*) FROM nodes_fts WHERE nodes_fts MATCH 'floomptard'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(new_found, 1, "the new body posting is indexed on UPDATE");

        // The delete trigger retracts body postings too — no silent desync.
        conn.execute("DELETE FROM nodes WHERE id = 20", []).unwrap();
        conn.execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES('integrity-check');")
            .expect("FTS index consistent after a body-bearing delete (NFR-RA-09)");
        let after_delete: i64 = conn
            .query_row(
                "SELECT count(*) FROM nodes_fts WHERE nodes_fts MATCH 'floomptard'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(after_delete, 0, "the body posting is retracted on delete");
    }

    /// A populated database created **before** migration 10 (at v9, with a
    /// cross-file caller/callee graph and a reference-ledger row) upgrades to v10
    /// forward-only with no data loss (S-042, CR-005, ADR-21, FR-EX-07,
    /// FR-EX-08, FR-EX-09, NFR-MA-06, NFR-RA-07): every node, every edge, every
    /// annotation column, and the ledger row survive; the additive
    /// `max_nesting_depth` column and the `shingles` store appear; the widened
    /// `edges`/`unresolved_refs` CHECKs now accept the `Accesses` kind (13); and
    /// a re-applied set of the same facts round-trips equal (the re-index
    /// equality check). `nodes` is never rebuilt — its FTS index survives intact.
    #[test]
    fn migration_10_adds_structural_facts_preserving_graph_data_and_fts() {
        let mut conn = contract_conn();

        // Stop at v9 and populate a small annotated graph exactly as a pre-CR-005
        // database holds (a function calling another, with per-function metrics).
        apply_migrations_from(&mut conn, &MIGRATIONS[..9]).unwrap();
        conn.execute_batch(
            "INSERT INTO files (id, path) VALUES (1, 'a.rs'), (2, 'b.rs');
             INSERT INTO symbols (id, symbol) VALUES (1, 'local a'), (2, 'local b'), (3, 'local f');
             INSERT INTO nodes (id, symbol_id, kind, name, file_id, exported,
                                cyclomatic_complexity, line_count) VALUES
                 (10, 1, 7, 'caller', 1, 1, 3, 12),
                 (20, 2, 9, 'field',  2, 0, NULL, NULL),
                 (30, 3, 8, 'method', 2, 1, 2, 6);
             INSERT INTO edges (source, target, kind) VALUES (10, 30, 2);
             INSERT INTO unresolved_refs (file_id, source_symbol, target, form, kind, resolved)
                 VALUES (2, 'local f', 'helper', 1, 2, 0);",
        )
        .unwrap();

        // The pre-migration store has neither the column nor the shingles table.
        assert!(
            conn.query_row(
                "SELECT max_nesting_depth FROM nodes WHERE id = 10",
                [],
                |r| r.get::<_, Option<i64>>(0)
            )
            .is_err(),
            "max_nesting_depth does not exist before migration 10"
        );
        // The pre-migration CHECK rejects the Accesses edge kind (13).
        assert!(
            conn.execute(
                "INSERT INTO edges (source, target, kind) VALUES (30, 20, 13)",
                [],
            )
            .is_err(),
            "edge kind 13 (Accesses) is outside the pre-migration ontology"
        );

        // Upgrade across the v9 → v10 bump under the production FK contract.
        apply_migrations_from(&mut conn, &MIGRATIONS[..10]).unwrap();
        assert_eq!(current_version(&conn).unwrap(), 10);

        // Every node, edge, and ledger row survives (edges rebuilt, no FK cascade).
        let nodes: i64 = conn
            .query_row("SELECT count(*) FROM nodes", [], |r| r.get(0))
            .unwrap();
        let edges: i64 = conn
            .query_row("SELECT count(*) FROM edges", [], |r| r.get(0))
            .unwrap();
        let refs: i64 = conn
            .query_row("SELECT count(*) FROM unresolved_refs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(nodes, 3, "nodes are untouched (additive column only)");
        assert_eq!(
            edges, 1,
            "the edge survives the edges rebuild (no FK cascade)"
        );
        assert_eq!(
            refs, 1,
            "the ledger row survives the unresolved_refs rebuild"
        );

        // Annotation columns carried over verbatim; the new column defaults NULL.
        let (cc, depth): (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT cyclomatic_complexity, max_nesting_depth FROM nodes WHERE id = 10",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(cc, Some(3), "existing annotation columns are unchanged");
        assert_eq!(
            depth, None,
            "max_nesting_depth defaults NULL until re-extract"
        );

        // nodes was NOT rebuilt, so the FTS external-content index is intact.
        conn.execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES('integrity-check');")
            .expect("FTS index consistent (nodes untouched, NFR-RA-09)");
        let hit: i64 = conn
            .query_row(
                "SELECT count(*) FROM nodes_fts WHERE nodes_fts MATCH 'caller'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hit, 1, "FTS still finds the pre-migration name");

        // The shingles store exists with the inverted-index shape and cascades
        // on node delete (a re-extract replaces a function's shingles wholesale).
        conn.execute(
            "INSERT INTO shingles (node_id, hash) VALUES (30, 111), (30, 222)",
            [],
        )
        .expect("the shingles store accepts (node_id, hash) rows");
        // ON CONFLICT is the writer's concern; the PK forbids a duplicate here.
        assert!(
            conn.execute("INSERT INTO shingles (node_id, hash) VALUES (30, 111)", [])
                .is_err(),
            "(node_id, hash) is the primary key — no duplicate shingle"
        );

        // The widened CHECK now accepts the Accesses edge (Method 30 → Field 20)…
        conn.execute(
            "INSERT INTO edges (source, target, kind) VALUES (30, 20, 13)",
            [],
        )
        .expect("edge kind 13 (Accesses) is accepted after the widening");
        // …and the ledger accepts an unresolved Accesses ref for retry (NFR-RA-05).
        conn.execute(
            "INSERT INTO unresolved_refs (file_id, source_symbol, target, form, kind, resolved) \
             VALUES (2, 'local f', 'gone', 3, 13, 0)",
            [],
        )
        .expect("the ledger accepts the Accesses kind after the widening");
        // …while still rejecting out-of-ontology values in both tables.
        assert!(
            conn.execute(
                "INSERT INTO edges (source, target, kind) VALUES (10, 20, 14)",
                [],
            )
            .is_err(),
            "edge kind 14 is outside the widened ontology"
        );

        // Deleting the method cascades its shingles and its Accesses edge — the
        // re-index replace-wholesale path (FR-EX-09).
        conn.execute("DELETE FROM nodes WHERE id = 30", []).unwrap();
        let orphan_shingles: i64 = conn
            .query_row(
                "SELECT count(*) FROM shingles WHERE node_id = 30",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphan_shingles, 0, "shingles cascade on node delete (FK)");
    }

    /// A populated database created **before** migration 14 (at v13, with the
    /// config layer present) upgrades to v14 forward-only with no data loss
    /// (S-068, CR-011, ADR-26, FR-CG-07, FR-DB-01, NFR-MA-06): every node, edge,
    /// and ledger row survives the `edges`/`unresolved_refs` rebuild, the additive
    /// `payload` column appears defaulting NULL, both kind CHECKs now accept the
    /// two artifact kinds (14, 15) carrying a relation payload, the ledger accepts
    /// an unindexed workspace-relative artifact reference for retry, the UNIQUE
    /// keys still dedup, and out-of-ontology kinds stay rejected. `nodes` is never
    /// rebuilt — its FTS index stays intact.
    #[test]
    fn migration_14_adds_artifact_edge_kinds_and_payload_preserving_data() {
        let mut conn = contract_conn();

        // Stop at v13 and populate a small graph exactly as a post-CR-010 /
        // pre-CR-011 database holds: a code edge, a config-file node, and a
        // resolved ledger row.
        apply_migrations_from(&mut conn, &MIGRATIONS[..13]).unwrap();
        conn.execute_batch(
            "INSERT INTO files (id, path) VALUES (1, 'a.rs'), (2, 'svc.proto');
             INSERT INTO symbols (id, symbol) VALUES (1, 'local a'), (2, 'local b'), (3, 'cfg svc');
             INSERT INTO nodes (id, symbol_id, kind, name, file_id) VALUES
                 (10, 1, 7, 'caller', 1),
                 (20, 2, 8, 'method', 1),
                 (30, 3, 23, 'svc.proto', 2);
             INSERT INTO edges (source, target, kind) VALUES (10, 20, 2);
             INSERT INTO unresolved_refs (file_id, source_symbol, target, form, kind, resolved)
                 VALUES (1, 'local a', 'method', 1, 2, 1);",
        )
        .unwrap();

        // The pre-migration store has no payload column and rejects kind 14.
        assert!(
            conn.query_row("SELECT payload FROM edges WHERE source = 10", [], |r| {
                r.get::<_, Option<String>>(0)
            })
            .is_err(),
            "edges.payload does not exist before migration 14"
        );
        assert!(
            conn.execute(
                "INSERT INTO edges (source, target, kind) VALUES (30, 20, 15)",
                [],
            )
            .is_err(),
            "edge kind 15 is outside the pre-migration ontology"
        );

        // Upgrade across the v13 → v14 bump under the production FK contract.
        apply_migrations_from(&mut conn, &MIGRATIONS[..14]).unwrap();
        assert_eq!(current_version(&conn).unwrap(), 14);

        // Every node, edge, and ledger row survives the rebuild (no FK cascade).
        let (nodes, edges, refs): (i64, i64, i64) = conn
            .query_row(
                "SELECT (SELECT count(*) FROM nodes), (SELECT count(*) FROM edges), \
                        (SELECT count(*) FROM unresolved_refs)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((nodes, edges, refs), (3, 1, 1), "every row survives v14");

        // The existing edge and ledger row carry a NULL payload (additive column).
        let edge_payload: Option<String> = conn
            .query_row("SELECT payload FROM edges WHERE source = 10", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(edge_payload, None, "an existing edge has a NULL payload");
        let (ref_kind, ref_resolved): (i64, i64) = conn
            .query_row(
                "SELECT kind, resolved FROM unresolved_refs WHERE source_symbol = 'local a'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (ref_kind, ref_resolved),
            (2, 1),
            "the resolved code ref carries over verbatim"
        );

        // nodes was NOT rebuilt, so the FTS external-content index is intact.
        conn.execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES('integrity-check');")
            .expect("FTS index consistent (nodes untouched, NFR-RA-09)");

        // The widened CHECK accepts an ArtifactBinding (15) carrying a relation
        // payload (a schema-name → code-method binding)…
        conn.execute(
            "INSERT INTO edges (source, target, kind, payload) VALUES (30, 20, 15, 'type-name')",
            [],
        )
        .expect("edge kind 15 (ArtifactBinding) with payload is accepted after the widening");
        // …and an ArtifactRef (14) carrying its own relation payload.
        conn.execute(
            "INSERT INTO edges (source, target, kind, payload) VALUES (30, 10, 14, 'proto-import')",
            [],
        )
        .expect("edge kind 14 (ArtifactRef) with payload is accepted");
        let bound_payload: String = conn
            .query_row(
                "SELECT payload FROM edges WHERE source = 30 AND kind = 15",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bound_payload, "type-name", "the relation payload persists");

        // The ledger accepts an unindexed workspace-relative artifact reference
        // for retry, carrying its relation class (NFR-RA-05).
        conn.execute(
            "INSERT INTO unresolved_refs (file_id, source_symbol, target, form, kind, resolved, payload) \
             VALUES (2, 'cfg svc', 'common/types.proto', 1, 14, 0, 'proto-import')",
            [],
        )
        .expect("the ledger accepts an artifact ref with payload after the widening");

        // The UNIQUE (source, target, kind) key still dedups — a duplicate edge is
        // rejected, so payload is an attribute, not a key (no NULL-distinct break).
        assert!(
            conn.execute(
                "INSERT INTO edges (source, target, kind, payload) VALUES (10, 20, 2, 'x')",
                [],
            )
            .is_err(),
            "the edges UNIQUE key still rejects a duplicate (source, target, kind)"
        );

        // Out-of-ontology kinds stay rejected in both tables.
        assert!(
            conn.execute(
                "INSERT INTO edges (source, target, kind) VALUES (10, 30, 16)",
                [],
            )
            .is_err(),
            "edge kind 16 is outside the widened ontology"
        );
        assert!(
            conn.execute(
                "INSERT INTO unresolved_refs (file_id, source_symbol, target, form, kind, resolved) \
                 VALUES (2, 'cfg svc', 'x', 1, 16, 0)",
                [],
            )
            .is_err(),
            "ledger kind 16 is outside the widened ontology"
        );
    }

    /// A populated database created **before** migration 11 (at v10, with nodes,
    /// annotation columns, and shingles) upgrades to v11 forward-only with no
    /// data loss (S-043, CR-005, ADR-21, FR-AN-06, NFR-MA-06): every node and
    /// its annotation columns survive, the additive `clone_group` column appears
    /// defaulting NULL, the recreated `annotations` view projects it, and a
    /// clone-group id can be written and read back. `nodes` is never rebuilt —
    /// its FTS index stays intact.
    #[test]
    fn migration_11_adds_clone_group_preserving_graph_data_and_fts() {
        let mut conn = contract_conn();

        // Stop at v10 and populate a small annotated graph with shingles, exactly
        // as a post-S-042 / pre-S-043 database holds.
        apply_migrations_from(&mut conn, &MIGRATIONS[..10]).unwrap();
        conn.execute_batch(
            "INSERT INTO files (id, path) VALUES (1, 'a.rs');
             INSERT INTO symbols (id, symbol) VALUES (1, 'local a'), (2, 'local b');
             INSERT INTO nodes (id, symbol_id, kind, name, file_id, is_dead, is_duplicate, is_test) VALUES
                 (10, 1, 7, 'compute', 1, 0, 0, 0),
                 (20, 2, 7, 'tally',   1, 0, 0, 0);
             INSERT INTO shingles (node_id, hash) VALUES (10, 111), (10, 222), (20, 111), (20, 222);",
        )
        .unwrap();

        // The pre-migration store has no clone_group column.
        assert!(
            conn.query_row("SELECT clone_group FROM nodes WHERE id = 10", [], |r| {
                r.get::<_, Option<i64>>(0)
            })
            .is_err(),
            "clone_group does not exist before migration 11"
        );

        // Upgrade across the v10 → v11 bump under the production FK contract.
        apply_migrations_from(&mut conn, &MIGRATIONS[..11]).unwrap();
        assert_eq!(current_version(&conn).unwrap(), 11);

        // Every node and its shingles survive (nodes is not rebuilt).
        let nodes: i64 = conn
            .query_row("SELECT count(*) FROM nodes", [], |r| r.get(0))
            .unwrap();
        let shingles: i64 = conn
            .query_row("SELECT count(*) FROM shingles", [], |r| r.get(0))
            .unwrap();
        assert_eq!(nodes, 2, "nodes are untouched (additive column only)");
        assert_eq!(shingles, 4, "the shingle index survives");

        // The new column defaults NULL until the next annotation pass.
        let group: Option<i64> = conn
            .query_row("SELECT clone_group FROM nodes WHERE id = 10", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(group, None, "clone_group defaults NULL until re-annotate");

        // The pre-existing annotation columns survive the additive ALTER
        // unchanged — the data-preservation guarantee this migration advertises.
        let (dead, dup, test): (i64, i64, i64) = conn
            .query_row(
                "SELECT is_dead, is_duplicate, is_test FROM nodes WHERE id = 10",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (dead, dup, test),
            (0, 0, 0),
            "is_dead/is_duplicate/is_test carry over verbatim (no column reset)"
        );

        // The recreated annotations view projects clone_group (FR-AN-04), and a
        // group id round-trips through the native column.
        conn.execute("UPDATE nodes SET clone_group = 10 WHERE id IN (10, 20)", [])
            .unwrap();
        let (a, b): (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT (SELECT clone_group FROM annotations WHERE node_id = 10), \
                        (SELECT clone_group FROM annotations WHERE node_id = 20)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (a, b),
            (Some(10), Some(10)),
            "clone_group projects on the view"
        );

        // nodes was NOT rebuilt, so the FTS external-content index is intact.
        conn.execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES('integrity-check');")
            .expect("FTS index consistent (nodes untouched, NFR-RA-09)");
        let hit: i64 = conn
            .query_row(
                "SELECT count(*) FROM nodes_fts WHERE nodes_fts MATCH 'compute'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hit, 1, "FTS still finds the pre-migration name");
    }

    /// A populated database created **before** migration 12 (at v11, with a
    /// metric snapshot scored under the original five dimensions) upgrades to v12
    /// forward-only with no data loss (S-044, CR-005, ADR-21, FR-QM-07,
    /// NFR-MA-06): the existing snapshot row survives verbatim, the five new
    /// dimension pairs + two applicability flags + the thresholds hash appear
    /// defaulting NULL (a pre-v3 snapshot scored none of them — NFR-CC-04), and a
    /// full extended row round-trips through the widened ledger. The append-only
    /// table is never rebuilt — every existing row and id is untouched.
    #[test]
    fn migration_12_widens_metric_snapshots_preserving_existing_rows() {
        let mut conn = contract_conn();

        // Stop at v11 and record one snapshot as a post-S-043 / pre-S-044
        // database holds: the original five dimensions, semantics version 2.
        apply_migrations_from(&mut conn, &MIGRATIONS[..11]).unwrap();
        conn.execute(
            "INSERT INTO metric_snapshots (
                 id, created_at, node_count, edge_count, function_count,
                 test_function_count, metric_version, empty,
                 modularity_raw, modularity_normalized,
                 acyclicity_raw, acyclicity_normalized,
                 depth_raw, depth_normalized,
                 equality_raw, equality_normalized,
                 redundancy_raw, redundancy_normalized,
                 aggregate_signal)
             VALUES (1, 1000, 5, 4, 3, 1, 2, 0,
                     0.5, 0.667, 0.0, 1.0, 2.0, 0.8, 0.1, 0.9, 0.0, 1.0, 8033)",
            [],
        )
        .unwrap();

        // The pre-migration ledger has no extended columns.
        assert!(
            conn.query_row(
                "SELECT nesting_raw FROM metric_snapshots WHERE id = 1",
                [],
                |r| { r.get::<_, Option<f64>>(0) }
            )
            .is_err(),
            "the extended dimension columns do not exist before migration 12"
        );

        // Upgrade across the v11 → v12 bump.
        apply_migrations_from(&mut conn, &MIGRATIONS[..12]).unwrap();
        assert_eq!(current_version(&conn).unwrap(), 12);

        // The pre-v3 snapshot survives: its original-five values carry over and
        // every new column defaults NULL (a real "not scored", distinct from 0.0).
        let (sig, modn, nesting, cohesion_applicable, thresh): (
            Option<i64>,
            f64,
            Option<f64>,
            Option<i64>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT aggregate_signal, modularity_normalized, nesting_normalized, \
                        cohesion_applicable, thresholds_hash \
                 FROM metric_snapshots WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(sig, Some(8033), "the original signal carries over verbatim");
        assert_eq!(modn, 0.667, "the original-five values are untouched");
        assert_eq!(
            nesting, None,
            "a pre-v3 snapshot scored no Nesting (NULL, not 0.0)"
        );
        assert_eq!(
            cohesion_applicable, None,
            "pre-v3 applicability flag is NULL"
        );
        assert_eq!(thresh, None, "pre-v3 thresholds hash is NULL");

        // Every original column survives the additive ALTER verbatim — not just
        // the two sampled above (UAT-QM "every existing row verbatim").
        let (acy, depth, eq, red, mver, fns, nodes_c): (f64, f64, f64, f64, i64, i64, i64) = conn
            .query_row(
                "SELECT acyclicity_normalized, depth_normalized, equality_normalized, \
                        redundancy_normalized, metric_version, function_count, node_count \
                 FROM metric_snapshots WHERE id = 1",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            (acy, depth, eq, red, mver, fns, nodes_c),
            (1.0, 0.8, 0.9, 1.0, 2, 3, 5),
            "all original columns (incl. metric_version=2) carry over verbatim (NFR-MA-06)"
        );

        // A full v3 extended row round-trips through the widened ledger, including
        // a dropped-out Cohesion (NULL value + applicable = 0) and the hash.
        conn.execute(
            "INSERT INTO metric_snapshots (
                 id, created_at, node_count, edge_count, function_count,
                 test_function_count, metric_version, empty,
                 modularity_raw, modularity_normalized,
                 acyclicity_raw, acyclicity_normalized,
                 depth_raw, depth_normalized,
                 equality_raw, equality_normalized,
                 redundancy_raw, redundancy_normalized,
                 nesting_raw, nesting_normalized,
                 conciseness_raw, conciseness_normalized,
                 cohesion_raw, cohesion_normalized, cohesion_applicable,
                 focus_raw, focus_normalized, focus_applicable,
                 uniqueness_raw, uniqueness_normalized,
                 thresholds_hash, aggregate_signal)
             VALUES (2, 2000, 6, 5, 4, 0, 3, 0,
                     0.5, 0.667, 0.0, 1.0, 2.0, 0.8, 0.1, 0.9, 0.0, 1.0,
                     0.25, 0.75, 0.0, 1.0,
                     NULL, NULL, 0,
                     0.5, 0.5, 1,
                     0.0, 1.0,
                     'abc123', 7500)",
            [],
        )
        .expect("a v3 extended row inserts under the widened ledger");
        let (coh, coh_app, foc_app): (Option<f64>, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT cohesion_normalized, cohesion_applicable, focus_applicable \
                 FROM metric_snapshots WHERE id = 2",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (coh, coh_app, foc_app),
            (None, Some(0), Some(1)),
            "a dropped-out Cohesion stores NULL value + applicable=0; Focus applied"
        );

        // The applicability flag CHECK rejects an out-of-range value.
        assert!(
            conn.execute(
                "UPDATE metric_snapshots SET cohesion_applicable = 2 WHERE id = 2",
                [],
            )
            .is_err(),
            "cohesion_applicable is constrained to 0/1"
        );
    }

    /// Count the rows `PRAGMA foreign_key_check` reports — zero means the store
    /// has no dangling foreign-key references (the S-201 acceptance assertion).
    fn foreign_key_violations(conn: &Connection) -> usize {
        let mut stmt = conn.prepare("PRAGMA foreign_key_check").unwrap();
        stmt.query_map([], |_| Ok(())).unwrap().count()
    }

    /// A **dirty** v15 database carrying duplicate `symbol_id` rows (the Channel-A
    /// drift, [CR-052]) upgrades to v16 by deduplicating to one node per
    /// `symbol_id`: the `MIN(id)` survivor is kept, the losers' edges and shingles
    /// are remapped onto it (`INSERT OR IGNORE` dedups collisions), the loser rows
    /// are deleted, and the FTS index is resynced. Afterwards `node_count ==
    /// distinct(symbol_id)`, `PRAGMA foreign_key_check` is empty, and the
    /// `UNIQUE(symbol_id)` constraint is live (S-201, ADR-46, NFR-RA-13, FR-SY-10).
    #[test]
    fn migration_16_dedups_duplicate_symbols_and_remaps_dependents() {
        let mut conn = contract_conn();

        // Stop at v15 and seed a DIRTY graph: symbol 1 has two nodes (10 survivor,
        // 11 loser); symbol 2 has one (20). Dependent edges/shingles hang off both
        // the survivor and the loser, including collisions that must dedup.
        apply_migrations_from(&mut conn, &MIGRATIONS[..15]).unwrap();
        conn.execute_batch(
            "INSERT INTO files (id, path) VALUES (1, 'a.rs'), (2, 'b.rs');
             INSERT INTO symbols (id, symbol) VALUES (1, 'local a'), (2, 'local b');
             INSERT INTO nodes (id, symbol_id, kind, name, file_id, exported) VALUES
                 (10, 1, 7, 'alpha', 1, 1),
                 (11, 1, 7, 'beta',  1, 0),
                 (20, 2, 7, 'tally', 2, 0);
             -- An edge present on BOTH the survivor and the loser (a remap
             -- collision the (source,target,kind) UNIQUE must fold to one), plus a
             -- loser-target edge that remaps to a fresh survivor edge.
             INSERT INTO edges (source, target, kind) VALUES
                 (10, 20, 2),
                 (11, 20, 2),
                 (20, 11, 3);
             -- Shingles on the loser: one colliding with the survivor's, one fresh.
             INSERT INTO shingles (node_id, hash) VALUES
                 (10, 111),
                 (11, 111),
                 (11, 222);",
        )
        .unwrap();

        // Upgrade across the v15 → v16 bump under the production FK contract.
        apply_migrations_from(&mut conn, &MIGRATIONS[..16]).unwrap();
        assert_eq!(current_version(&conn).unwrap(), 16);

        // One node per symbol_id — the MIN(id) survivor kept, the loser gone.
        let (nodes, distinct_syms): (i64, i64) = conn
            .query_row(
                "SELECT (SELECT count(*) FROM nodes), \
                        (SELECT count(DISTINCT symbol_id) FROM nodes)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (nodes, distinct_syms),
            (2, 2),
            "deduplicated to one node per symbol_id (NFR-RA-13)"
        );
        let (survivor, loser): (i64, i64) = conn
            .query_row(
                "SELECT (SELECT count(*) FROM nodes WHERE id = 10), \
                        (SELECT count(*) FROM nodes WHERE id = 11)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((survivor, loser), (1, 0), "the MIN(id) survivor (10) is kept, the loser (11) deleted");

        // The survivor's annotation column (exported) is preserved.
        let exported: i64 = conn
            .query_row("SELECT exported FROM nodes WHERE id = 10", [], |r| r.get(0))
            .unwrap();
        assert_eq!(exported, 1, "the survivor's annotation columns carry over");

        // Edges remapped onto the survivor and deduped by (source,target,kind):
        // the collision folds to one (10,20,2), the loser-target edge becomes
        // (20,10,3), and no edge references the deleted loser.
        let edges: Vec<(i64, i64, i64)> = {
            let mut stmt = conn
                .prepare("SELECT source, target, kind FROM edges ORDER BY source, target, kind")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(
            edges,
            vec![(10, 20, 2), (20, 10, 3)],
            "edges remapped onto the survivor and deduped (INSERT OR IGNORE)"
        );

        // Shingles remapped onto the survivor and deduped by (node_id,hash).
        let shingles: Vec<(i64, i64)> = {
            let mut stmt = conn
                .prepare("SELECT node_id, hash FROM shingles ORDER BY node_id, hash")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(
            shingles,
            vec![(10, 111), (10, 222)],
            "shingles remapped onto the survivor and deduped (INSERT OR IGNORE)"
        );

        // PRAGMA foreign_key_check is empty — no dangling references (acceptance).
        assert_eq!(
            foreign_key_violations(&conn),
            0,
            "foreign_key_check is empty after the dedup rebuild"
        );

        // The FTS index is resynced: the survivor's name is found, the loser's
        // stale posting is cleared by the 'rebuild' (NFR-RA-09).
        conn.execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES('integrity-check');")
            .expect("FTS index consistent after the dedup rebuild (NFR-RA-09)");
        let (by_survivor, by_loser): (i64, i64) = conn
            .query_row(
                "SELECT (SELECT count(*) FROM nodes_fts WHERE nodes_fts MATCH 'alpha'), \
                        (SELECT count(*) FROM nodes_fts WHERE nodes_fts MATCH 'beta')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(by_survivor, 1, "the survivor's name is still FTS-findable");
        assert_eq!(by_loser, 0, "the deleted loser's stale FTS posting is cleared");

        // UNIQUE(symbol_id) is now enforced — a second node for symbol 1 fails.
        assert!(
            conn.execute(
                "INSERT INTO nodes (symbol_id, kind, name) VALUES (1, 7, 'dup again')",
                [],
            )
            .is_err(),
            "UNIQUE(symbol_id) rejects a second node for an existing symbol (ADR-46)"
        );
    }

    /// A **clean** v15 database (already one node per `symbol_id`) upgrades to v16
    /// as a population-preserving rebuild: every node, edge, and shingle survives
    /// with its rowid unchanged, annotation columns and edge payloads carry over
    /// verbatim, `PRAGMA foreign_key_check` is empty, the `UNIQUE(symbol_id)`
    /// constraint is enforced, and re-running the migration runner is a no-op
    /// (idempotent) — the S-201 clean-path acceptance criteria.
    #[test]
    fn migration_16_is_population_preserving_and_idempotent_on_a_clean_db() {
        let mut conn = contract_conn();

        apply_migrations_from(&mut conn, &MIGRATIONS[..15]).unwrap();
        conn.execute_batch(
            "INSERT INTO files (id, path) VALUES (1, 'a.rs'), (2, 'b.rs');
             INSERT INTO symbols (id, symbol) VALUES (1, 'local a'), (2, 'local b'), (3, 'local c');
             INSERT INTO nodes (id, symbol_id, kind, name, file_id, exported,
                                cyclomatic_complexity, is_test, body) VALUES
                 (10, 1, 7,  'alpha',    1, 1, 3,    0, NULL),
                 (20, 2, 7,  'bravo',    2, 0, 1,    1, NULL),
                 (30, 3, 19, 'Overview', 2, 0, NULL, 0, 'the body prose');
             INSERT INTO edges (source, target, kind, payload) VALUES
                 (10, 20, 2,  NULL),
                 (30, 10, 11, 'doc-ref');
             INSERT INTO shingles (node_id, hash) VALUES (10, 111), (10, 222), (20, 333);",
        )
        .unwrap();

        apply_migrations_from(&mut conn, &MIGRATIONS[..16]).unwrap();
        assert_eq!(current_version(&conn).unwrap(), 16);

        // Every row survives — no node, edge, or shingle lost.
        let (nodes, edges, shingles): (i64, i64, i64) = conn
            .query_row(
                "SELECT (SELECT count(*) FROM nodes), (SELECT count(*) FROM edges), \
                        (SELECT count(*) FROM shingles)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (nodes, edges, shingles),
            (3, 2, 3),
            "the clean rebuild preserves every row (no data loss)"
        );

        // Surviving rowids are unchanged (the copy-back keeps ids).
        let ids: Vec<i64> = {
            let mut stmt = conn.prepare("SELECT id FROM nodes ORDER BY id").unwrap();
            stmt.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(ids, vec![10, 20, 30], "surviving rowids are unchanged");

        // Annotation columns, body, and edge payload carry over verbatim.
        let (exported, cc, is_test, body): (i64, Option<i64>, i64, Option<String>) = conn
            .query_row(
                "SELECT exported, cyclomatic_complexity, is_test, body FROM nodes WHERE id = 30",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            (exported, cc, is_test, body),
            (0, None, 0, Some("the body prose".to_string())),
            "annotation columns + body carry over verbatim"
        );
        let payload: Option<String> = conn
            .query_row("SELECT payload FROM edges WHERE source = 30", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(payload, Some("doc-ref".to_string()), "edge payload carries over");

        // foreign_key_check empty; FTS still finds a name and a doc-body phrase.
        assert_eq!(
            foreign_key_violations(&conn),
            0,
            "no FK violations on the clean rebuild"
        );
        conn.execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES('integrity-check');")
            .expect("FTS index consistent after the clean rebuild (NFR-RA-09)");
        let (by_name, by_body): (i64, i64) = conn
            .query_row(
                "SELECT (SELECT count(*) FROM nodes_fts WHERE nodes_fts MATCH 'alpha'), \
                        (SELECT count(*) FROM nodes_fts WHERE nodes_fts MATCH 'prose')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(by_name, 1, "FTS still finds a name after the clean rebuild");
        assert_eq!(by_body, 1, "a doc-body phrase is still FTS-findable (body column survived)");

        // UNIQUE(symbol_id) is enforced.
        assert!(
            conn.execute(
                "INSERT INTO nodes (symbol_id, kind, name) VALUES (1, 7, 'dup')",
                [],
            )
            .is_err(),
            "UNIQUE(symbol_id) rejects a duplicate symbol"
        );

        // Idempotent: re-running the runner (pinned to the first sixteen
        // migrations, so this test stays scoped to migration 16 as later
        // migrations land) applies nothing and changes no row.
        apply_migrations_from(&mut conn, &MIGRATIONS[..16]).unwrap();
        assert_eq!(
            current_version(&conn).unwrap(),
            16,
            "re-running the runner stays at v16 (no migration re-applied)"
        );
        let nodes_after: i64 = conn
            .query_row("SELECT count(*) FROM nodes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(nodes_after, 3, "a second runner pass changes nothing (idempotent)");
    }

    /// A populated database created **before** migration 17 (at v16, with a
    /// cross-file graph carrying real annotation data, a shingle, and a
    /// resolved ledger row — and, critically, **no** broker topic) upgrades to
    /// v17 forward-only with no data loss (S-255, CR-061, ADR-55, FR-WS-11,
    /// FR-DB-01, NFR-MA-06): `PRAGMA user_version` advances by exactly one, and
    /// the graph is byte-for-byte unaffected — every node, edge, shingle, and
    /// ledger row survives with its id and every column verbatim, and the FTS
    /// index needs no `'rebuild'` (no row is deleted, unlike migration 16's
    /// dedup rebuild). Afterwards the widened CHECKs accept the three broker
    /// node kinds and the two broker edge kinds (on both `edges` and the
    /// `unresolved_refs` ledger) and still reject out-of-ontology values, and
    /// the migration-16 `UNIQUE(symbol_id)` constraint is still enforced.
    #[test]
    fn migration_17_widens_to_broker_kinds_preserving_a_no_topic_graph_byte_for_byte() {
        let mut conn = contract_conn();

        // Stop at v16 and populate a small annotated graph exactly as a
        // pre-CR-061 database holds: a cross-file caller/callee edge, a
        // doc-body node, a shingle, and a resolved ledger row. No broker kind
        // appears anywhere — the byte-for-byte-unaffected acceptance case.
        apply_migrations_from(&mut conn, &MIGRATIONS[..16]).unwrap();
        conn.execute_batch(
            "INSERT INTO files (id, path) VALUES (1, 'a.rs'), (2, 'b.rs');
             INSERT INTO symbols (id, symbol) VALUES (1, 'local a'), (2, 'local b');
             INSERT INTO nodes (id, symbol_id, kind, name, file_id, exported,
                                cyclomatic_complexity, is_test, body) VALUES
                 (10, 1, 7,  'caller',   1, 1, 3,    0, NULL),
                 (20, 2, 19, 'Overview', 2, 0, NULL, 0, 'the body prose');
             INSERT INTO edges (source, target, kind, payload) VALUES (20, 10, 11, 'doc-ref');
             INSERT INTO shingles (node_id, hash) VALUES (10, 111), (10, 222);
             INSERT INTO unresolved_refs (file_id, source_symbol, target, form, kind, resolved)
                 VALUES (1, 'local a', 'helper', 1, 2, 1);",
        )
        .unwrap();

        // Upgrade across the v16 → v17 bump under the production FK contract.
        apply_migrations_from(&mut conn, &MIGRATIONS[..17]).unwrap();
        assert_eq!(
            current_version(&conn).unwrap(),
            17,
            "PRAGMA user_version advances by exactly one (16 → 17)"
        );

        // Byte-for-byte: every row, every id, every column survives verbatim.
        let (nodes, edges, shingles, refs): (i64, i64, i64, i64) = conn
            .query_row(
                "SELECT (SELECT count(*) FROM nodes), (SELECT count(*) FROM edges), \
                        (SELECT count(*) FROM shingles), (SELECT count(*) FROM unresolved_refs)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            (nodes, edges, shingles, refs),
            (2, 1, 2, 1),
            "a no-topic graph is byte-for-byte unaffected: every row survives v17"
        );
        let (exported, cc, is_test, body): (i64, Option<i64>, i64, Option<String>) = conn
            .query_row(
                "SELECT exported, cyclomatic_complexity, is_test, body FROM nodes WHERE id = 10",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            (exported, cc, is_test, body),
            (1, Some(3), 0, None),
            "annotation columns carry over verbatim (no data loss)"
        );
        let edge_payload: Option<String> = conn
            .query_row("SELECT payload FROM edges WHERE source = 20", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(edge_payload, Some("doc-ref".to_string()), "edge payload carries over");
        let ref_resolved: i64 = conn
            .query_row(
                "SELECT resolved FROM unresolved_refs WHERE source_symbol = 'local a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ref_resolved, 1, "the pre-migration ledger row survives verbatim");

        // No row was deleted, so the FTS index was never desynced — no
        // 'rebuild' was needed, and it still finds the pre-migration name and
        // the doc-body phrase.
        conn.execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES('integrity-check');")
            .expect("FTS index consistent after the widening rebuild (NFR-RA-09)");
        let (by_name, by_body): (i64, i64) = conn
            .query_row(
                "SELECT (SELECT count(*) FROM nodes_fts WHERE nodes_fts MATCH 'caller'), \
                        (SELECT count(*) FROM nodes_fts WHERE nodes_fts MATCH 'prose')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(by_name, 1, "FTS still finds the pre-migration name");
        assert_eq!(by_body, 1, "FTS still finds the pre-migration doc-body phrase");

        // The widened CHECK accepts the three broker node kinds — each under
        // its own fresh symbol, since migration 16's UNIQUE(symbol_id) forbids
        // a second node on symbol 1.
        for (i, kind) in (35..=37).enumerate() {
            let symbol_id = i as i64 + 3;
            conn.execute(
                "INSERT INTO symbols (id, symbol) VALUES (?1, ?2)",
                rusqlite::params![symbol_id, format!("local {symbol_id}")],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO nodes (symbol_id, kind, name) VALUES (?1, ?2, 'broker')",
                rusqlite::params![symbol_id, kind],
            )
            .unwrap_or_else(|e| panic!("broker node kind {kind} must be accepted: {e}"));
        }
        // …and the two broker edge kinds, on both edges and the ledger…
        for kind in [16, 17] {
            conn.execute(
                "INSERT INTO edges (source, target, kind) VALUES (10, 20, ?1)",
                [kind],
            )
            .unwrap_or_else(|e| panic!("broker edge kind {kind} must be accepted: {e}"));
            conn.execute(
                "INSERT INTO unresolved_refs (file_id, source_symbol, target, form, kind, resolved) \
                 VALUES (1, 'local a', 'topic', 1, ?1, 0)",
                [kind],
            )
            .unwrap_or_else(|e| panic!("the ledger must accept broker edge kind {kind}: {e}"));
        }
        // …while still rejecting out-of-ontology values in every table.
        assert!(
            conn.execute(
                "INSERT INTO nodes (symbol_id, kind, name) VALUES (1, 38, 'nope')",
                [],
            )
            .is_err(),
            "node kind 38 is outside the widened ontology"
        );
        assert!(
            conn.execute(
                "INSERT INTO edges (source, target, kind) VALUES (10, 20, 18)",
                [],
            )
            .is_err(),
            "edge kind 18 is outside the widened ontology"
        );
        assert!(
            conn.execute(
                "INSERT INTO unresolved_refs (file_id, source_symbol, target, form, kind, resolved) \
                 VALUES (1, 'local a', 'x', 1, 18, 0)",
                [],
            )
            .is_err(),
            "ledger kind 18 is outside the widened ontology"
        );

        // The migration-16 UNIQUE(symbol_id) constraint survives this rebuild.
        assert!(
            conn.execute(
                "INSERT INTO nodes (symbol_id, kind, name) VALUES (1, 7, 'dup')",
                [],
            )
            .is_err(),
            "UNIQUE(symbol_id) still rejects a duplicate symbol after v17"
        );

        assert_eq!(
            foreign_key_violations(&conn),
            0,
            "no FK violations after the migration-17 rebuild"
        );
    }

    /// A populated database created **before** migration 18 (at v17, carrying a
    /// graph plus a ledger with a resolved code ref and two **non-colliding**
    /// broker rows — a publish and a subscribe on *different* topics, i.e. no
    /// relay) upgrades to v18 forward-only with no data loss (S-290, CR-080,
    /// FR-DB-01, FR-WS-11, NFR-MA-06): `PRAGMA user_version` advances by exactly
    /// one, and the graph is byte-for-byte unaffected — every node, edge, shingle,
    /// and ledger row survives with its id and every column verbatim, and (since
    /// only the ledger is rebuilt) the FTS index is never disturbed. Afterwards
    /// the widened key admits a **relay** (a publish and a subscribe agreeing on
    /// all four shipped key columns, differing only by relation) as two rows,
    /// while still deduping a true duplicate and a duplicate NULL-payload code ref
    /// — proving the `COALESCE(payload, '')` normalisation preserves the old
    /// dedup for NULL payloads.
    #[test]
    fn migration_18_widens_ledger_unique_preserving_a_no_relay_graph_byte_for_byte() {
        let mut conn = contract_conn();

        // Stop at v17 and populate a small graph plus a ledger with a resolved
        // code ref and two broker rows on DIFFERENT topics (a publisher and a
        // listener — never a relay), the byte-for-byte-unaffected acceptance case.
        apply_migrations_from(&mut conn, &MIGRATIONS[..17]).unwrap();
        conn.execute_batch(
            "INSERT INTO files (id, path) VALUES (1, 'a.rs'), (2, 'Svc.java');
             INSERT INTO symbols (id, symbol) VALUES (1, 'local a'), (2, 'local b');
             INSERT INTO nodes (id, symbol_id, kind, name, file_id, exported,
                                cyclomatic_complexity, is_test, body) VALUES
                 (10, 1, 7,  'caller',   1, 1, 3,    0, NULL),
                 (20, 2, 19, 'Overview', 2, 0, NULL, 0, 'the body prose');
             INSERT INTO edges (source, target, kind, payload) VALUES (20, 10, 11, 'doc-ref');
             INSERT INTO shingles (node_id, hash) VALUES (10, 111), (10, 222);
             -- A resolved plain code ref (payload NULL, and a non-NULL alias + line
             -- so the verbatim snapshot exercises those copied columns) and two
             -- non-colliding broker rows: publish 'orders', subscribe 'events'.
             INSERT INTO unresolved_refs (file_id, source_symbol, target, alias, form, kind, line, resolved, payload) VALUES
                 (1, 'local a', 'helper', 'h', 1, 2,  42, 1, NULL),
                 (2, 'method Svc#emit',   'orders', NULL, 3, 14, 7, 0, 'broker-publish'),
                 (2, 'method Svc#listen', 'events', NULL, 3, 14, 9, 0, 'broker-subscribe');",
        )
        .unwrap();

        // Snapshot the full ledger before the migration for a verbatim diff.
        let ledger_before = read_ledger(&conn);

        // Upgrade across the v17 → v18 bump under the production FK contract.
        apply_migrations_from(&mut conn, &MIGRATIONS[..18]).unwrap();
        assert_eq!(
            current_version(&conn).unwrap(),
            18,
            "PRAGMA user_version advances by exactly one (17 → 18)"
        );

        // Byte-for-byte: every row, id, and column of every table survives.
        let (nodes, edges, shingles, refs): (i64, i64, i64, i64) = conn
            .query_row(
                "SELECT (SELECT count(*) FROM nodes), (SELECT count(*) FROM edges), \
                        (SELECT count(*) FROM shingles), (SELECT count(*) FROM unresolved_refs)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            (nodes, edges, shingles, refs),
            (2, 1, 2, 3),
            "a no-relay graph is byte-for-byte unaffected: every row survives v18"
        );
        // The ledger is identical column-for-column (id, keys, resolved, payload).
        assert_eq!(
            read_ledger(&conn),
            ledger_before,
            "every ledger row survives migration 18 verbatim (no loss, no addition)"
        );
        // The graph nodes/edges are byte-identical — proven here by their
        // annotation columns and payloads carrying through untouched.
        let (exported, cc, body): (i64, Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT exported, cyclomatic_complexity, body FROM nodes WHERE id = 10",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (exported, cc, body),
            (1, Some(3), None),
            "node annotation columns carry over verbatim (nodes never rebuilt)"
        );
        let edge_payload: Option<String> = conn
            .query_row("SELECT payload FROM edges WHERE source = 20", [], |r| r.get(0))
            .unwrap();
        assert_eq!(edge_payload, Some("doc-ref".to_string()), "edge payload untouched");

        // Only the ledger was rebuilt, so the FTS external-content index was never
        // disturbed — it still finds the pre-migration name and doc-body phrase.
        conn.execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES('integrity-check');")
            .expect("FTS index consistent (nodes never touched by migration 18, NFR-RA-09)");
        let (by_name, by_body): (i64, i64) = conn
            .query_row(
                "SELECT (SELECT count(*) FROM nodes_fts WHERE nodes_fts MATCH 'caller'), \
                        (SELECT count(*) FROM nodes_fts WHERE nodes_fts MATCH 'prose')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((by_name, by_body), (1, 1), "FTS still finds the pre-migration name and body");

        // The widened key admits a RELAY: a publish and a subscribe that agree on
        // all four shipped key columns and differ ONLY by relation now coexist —
        // exactly the row pair the relation-blind key used to collapse.
        conn.execute(
            "INSERT INTO unresolved_refs (file_id, source_symbol, target, form, kind, resolved, payload) \
             VALUES (2, 'method Svc#relay', 'trades', 3, 14, 0, 'broker-publish')",
            [],
        )
        .expect("the relay's publish inserts");
        conn.execute(
            "INSERT INTO unresolved_refs (file_id, source_symbol, target, form, kind, resolved, payload) \
             VALUES (2, 'method Svc#relay', 'trades', 3, 14, 0, 'broker-subscribe')",
            [],
        )
        .expect("the relay's subscribe now survives alongside its publish (CR-080)");
        let relay_rows: i64 = conn
            .query_row(
                "SELECT count(*) FROM unresolved_refs WHERE source_symbol = 'method Svc#relay'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(relay_rows, 2, "a relay keeps BOTH its publish and its subscribe leg");

        // A true duplicate (same key, same relation) is still rejected by the
        // widened UNIQUE — the relation is an added key column, not a bypass.
        assert!(
            conn.execute(
                "INSERT INTO unresolved_refs (file_id, source_symbol, target, form, kind, resolved, payload) \
                 VALUES (2, 'method Svc#relay', 'trades', 3, 14, 0, 'broker-publish')",
                [],
            )
            .is_err(),
            "an exact-relation duplicate is still deduped by the widened key"
        );

        // NULL-payload dedup is preserved byte-for-byte: 'local a' → 'helper' is
        // already in the ledger with a NULL payload, so a second NULL-payload row
        // on the same four key columns is rejected (COALESCE folds NULL to '').
        assert!(
            conn.execute(
                "INSERT INTO unresolved_refs (file_id, source_symbol, target, form, kind, resolved, payload) \
                 VALUES (1, 'local a', 'helper', 1, 2, 0, NULL)",
                [],
            )
            .is_err(),
            "a duplicate NULL-payload code ref is still deduped (COALESCE(payload,'') normalisation)"
        );

        assert_eq!(
            foreign_key_violations(&conn),
            0,
            "no FK violations after the migration-18 ledger rebuild"
        );
    }

    /// One ledger row projected for a verbatim cross-migration diff — **every**
    /// column the migration copies: `(id, file_id, source_symbol, target, alias,
    /// form, kind, line, resolved, payload)`.
    type LedgerRow = (
        i64,
        Option<i64>,
        String,
        String,
        Option<String>,
        i64,
        i64,
        Option<i64>,
        i64,
        Option<String>,
    );

    /// The full ledger as [`LedgerRow`] tuples ordered by id — a verbatim,
    /// all-columns snapshot for byte-for-byte diffing across a migration.
    fn read_ledger(conn: &Connection) -> Vec<LedgerRow> {
        let mut stmt = conn
            .prepare(
                "SELECT id, file_id, source_symbol, target, alias, form, kind, line, \
                        resolved, payload \
                 FROM unresolved_refs ORDER BY id",
            )
            .unwrap();
        stmt.query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
                r.get(9)?,
            ))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
    }

    /// Migration 19 admits the member-local configuration-corpus tables on a
    /// **populated** store and leaves every pre-existing fact verbatim (S-380,
    /// [FR-WS-19] AC7, [FR-DB-01], AC3).
    ///
    /// The claim being tested is *byte-for-byte across the boundary*, so a row
    /// count is not enough — the node, edge, shingle and full-text content is
    /// snapshotted before the upgrade and diffed against the same projection
    /// after it, and the FTS index is put through its own integrity check. Then
    /// the new tables are exercised, because "additive" also means the additions
    /// actually work: the profile is nullable, the pair-uniqueness dedups a
    /// re-inserted pair while keeping a second value for the same key, and both
    /// FKs cascade.
    ///
    /// [FR-DB-01]: ../../../../docs/specs/requirements/FR-DB-01.md
    /// [FR-WS-19]: ../../../../docs/specs/requirements/FR-WS-19.md
    #[test]
    fn migration_19_adds_the_config_corpus_tables_preserving_the_graph_byte_for_byte() {
        let mut conn = contract_conn();

        // Stop at v18 and populate every table the acceptance criterion names:
        // nodes (with their annotation columns), edges (with a payload), shingles,
        // and FTS-indexed name + doc-body content.
        apply_migrations_from(&mut conn, &MIGRATIONS[..18]).unwrap();
        conn.execute_batch(
            "INSERT INTO files (id, path) VALUES (1, 'a.rs'), (2, 'application-dev.yml');
             INSERT INTO symbols (id, symbol) VALUES (1, 'local a'), (2, 'local b');
             INSERT INTO nodes (id, symbol_id, kind, name, file_id, exported,
                                cyclomatic_complexity, is_test, body) VALUES
                 (10, 1, 7,  'caller',   1, 1, 3,    0, NULL),
                 (20, 2, 19, 'Overview', 2, 0, NULL, 0, 'the body prose');
             INSERT INTO edges (source, target, kind, payload) VALUES (20, 10, 11, 'doc-ref');
             INSERT INTO shingles (node_id, hash) VALUES (10, 111), (10, 222);
             INSERT INTO unresolved_refs (file_id, source_symbol, target, alias, form, kind, line, resolved, payload) VALUES
                 (1, 'local a', 'helper', 'h', 1, 2, 42, 1, NULL);",
        )
        .unwrap();

        let graph_before = read_graph(&conn);
        let ledger_before = read_ledger(&conn);

        apply_migrations_from(&mut conn, &MIGRATIONS[..19]).unwrap();
        assert_eq!(
            current_version(&conn).unwrap(),
            19,
            "PRAGMA user_version advances by exactly one (18 → 19)"
        );

        // The graph content — every node column, every edge column, every
        // shingle — is identical, not merely the same size.
        assert_eq!(
            read_graph(&conn),
            graph_before,
            "nodes, edges and shingles are byte-for-byte unchanged across migration 19"
        );
        assert_eq!(
            read_ledger(&conn),
            ledger_before,
            "the reference ledger is byte-for-byte unchanged across migration 19"
        );

        // The external-content FTS index was never disturbed: it passes its own
        // integrity check and still finds the pre-migration name and body prose.
        conn.execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES('integrity-check');")
            .expect("FTS index consistent (nodes never touched by migration 19, NFR-RA-09)");
        let (by_name, by_body): (i64, i64) = conn
            .query_row(
                "SELECT (SELECT count(*) FROM nodes_fts WHERE nodes_fts MATCH 'caller'), \
                        (SELECT count(*) FROM nodes_fts WHERE nodes_fts MATCH 'prose')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (by_name, by_body),
            (1, 1),
            "FTS still finds the pre-migration name and body across migration 19"
        );

        // The new tables start empty — an upgrade indexes nothing by itself, so a
        // member is unaffected until its next extract pass runs.
        let (sources, values): (i64, i64) = conn
            .query_row(
                "SELECT (SELECT count(*) FROM config_sources), (SELECT count(*) FROM config_values)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((sources, values), (0, 0), "the upgrade itself ingests nothing");

        // The unprofiled source is representable as NULL, and two sources can
        // define the same key differently — disagreement is retained, not refused.
        conn.execute_batch(
            "INSERT INTO config_sources (id, file_id, profile) VALUES (1, 2, 'dev');
             INSERT INTO config_sources (id, file_id, profile) VALUES (2, 1, NULL);
             INSERT INTO config_values (source_id, key, value) VALUES
                 (1, 'mailserver.api.urigetmailbox', '/dev/mailbox'),
                 (2, 'mailserver.api.urigetmailbox', '/mailbox'),
                 (1, 'a', 'one'),
                 (1, 'a', 'two');",
        )
        .expect("a profiled and an unprofiled source coexist, and a key may disagree");
        let unprofiled: i64 = conn
            .query_row("SELECT count(*) FROM config_sources WHERE profile IS NULL", [], |r| r.get(0))
            .unwrap();
        assert_eq!(unprofiled, 1, "NULL profile is the unprofiled source, countable as such");
        let same_key: i64 = conn
            .query_row(
                "SELECT count(*) FROM config_values WHERE key = 'a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(same_key, 2, "one source proving a key twice keeps BOTH values");

        // The pair is the key: an identical (source, key, value) is rejected, so a
        // re-extract cannot double-count, while the second value above survived.
        assert!(
            conn.execute(
                "INSERT INTO config_values (source_id, key, value) VALUES (1, 'a', 'one')",
                [],
            )
            .is_err(),
            "an exact duplicate pair is deduped by UNIQUE(source_id, key, value)"
        );
        // A file is at most one source.
        assert!(
            conn.execute(
                "INSERT INTO config_sources (file_id, profile) VALUES (2, 'prod')",
                [],
            )
            .is_err(),
            "a file maps to at most one configuration source"
        );

        // Both FKs cascade: deleting the file removes its source, which removes
        // its values — so a removed file leaves no orphaned corpus rows.
        conn.execute("DELETE FROM files WHERE id = 2", []).unwrap();
        let (sources, values): (i64, i64) = conn
            .query_row(
                "SELECT (SELECT count(*) FROM config_sources), (SELECT count(*) FROM config_values)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (sources, values),
            (1, 1),
            "deleting a file cascades away its source and every value under it"
        );
        assert_eq!(
            foreign_key_violations(&conn),
            0,
            "no FK violations after migration 19 and its cascade"
        );
    }

    /// Migration 20 adds the [FR-GV-21] check-run marker to a **populated**
    /// store, in place, leaving every pre-existing fact verbatim — including
    /// the `violations` rows a pre-migration `check_rules` already wrote, which
    /// still carry the `created_at` that dates them (S-313, [CR-096]).
    ///
    /// Four claims, in the order a reader should doubt them: the version
    /// advances by exactly **one**; the graph and the existing governance rows
    /// are byte-for-byte unchanged (so the upgrade needs no re-index); the new
    /// table arrives **empty**, so an upgraded store honestly reads "no check
    /// has run" rather than "clean"; and the singleton is enforced by the
    /// schema — a second row is rejected by SQLite, not by a caller remembering
    /// to upsert.
    ///
    /// [CR-096]: ../../../../docs/requests/CR-096-recorded-check-marker.md
    /// [FR-GV-21]: ../../../../docs/specs/requirements/FR-GV-21.md
    #[test]
    fn migration_20_adds_the_check_run_marker_preserving_the_graph_byte_for_byte() {
        let mut conn = contract_conn();

        // Stop at v19 and populate the graph plus the governance rows a store
        // that has already been checked would carry.
        apply_migrations_from(&mut conn, &MIGRATIONS[..19]).unwrap();
        conn.execute_batch(
            "INSERT INTO files (id, path) VALUES (1, 'a.rs');
             INSERT INTO symbols (id, symbol) VALUES (1, 'local a');
             INSERT INTO nodes (id, symbol_id, kind, name, file_id, exported,
                                cyclomatic_complexity, is_test, body) VALUES
                 (10, 1, 7, 'caller', 1, 1, 3, 0, 'the body prose');
             INSERT INTO shingles (node_id, hash) VALUES (10, 111);
             INSERT INTO violations
                 (id, snapshot_id, rule_type, rule_key, node_id, file, message, severity, created_at)
             VALUES (1, NULL, 'constraint', 'max_cc', 10, 'a.rs', 'too complex', 'error', 1700000000);",
        )
        .unwrap();

        let graph_before = read_graph(&conn);
        let violations_before = read_table(&conn, "violations", "id");

        apply_migrations_from(&mut conn, &MIGRATIONS[..20]).unwrap();
        assert_eq!(
            current_version(&conn).unwrap(),
            20,
            "PRAGMA user_version advances by exactly one (19 → 20)"
        );

        assert_eq!(
            read_graph(&conn),
            graph_before,
            "nodes, edges and shingles are byte-for-byte unchanged across migration 20"
        );
        // The pre-migration findings survive WITH their created_at — the half of
        // CR-096 that dates an existing store's violations without a marker.
        assert_eq!(
            read_table(&conn, "violations", "id"),
            violations_before,
            "the violations already recorded are unchanged, created_at included"
        );
        conn.execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES('integrity-check');")
            .expect("FTS index consistent (nodes never touched by migration 20, NFR-RA-09)");

        // No marker yet: an upgraded store has not been checked *since*, and
        // absence of the marker means absence of a run — never a clean result.
        let markers: i64 = conn
            .query_row("SELECT count(*) FROM check_run", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            markers, 0,
            "the upgrade itself records no run (FR-GV-21: no marker = never checked)"
        );

        // The singleton is structural. A second row is refused by the CHECK,
        // and an upsert on id 1 replaces rather than accumulates (BR-40).
        conn.execute(
            "INSERT INTO check_run (id, ran_at, commit_sha, violation_count) VALUES (1, 100, NULL, 0)",
            [],
        )
        .unwrap();
        let second_row = conn.execute(
            "INSERT INTO check_run (id, ran_at, commit_sha, violation_count) VALUES (2, 200, NULL, 7)",
            [],
        );
        assert!(
            second_row.is_err(),
            "CHECK (id = 1) must reject a second marker row — the singleton is the schema's job"
        );
        conn.execute(
            "INSERT INTO check_run (id, ran_at, commit_sha, violation_count) VALUES (1, 200, 'abc', 7) \
             ON CONFLICT(id) DO UPDATE SET ran_at = 200, commit_sha = 'abc', violation_count = 7",
            [],
        )
        .unwrap();
        let (rows, ran_at, sha, count): (i64, i64, Option<String>, i64) = conn
            .query_row(
                "SELECT (SELECT count(*) FROM check_run), ran_at, commit_sha, violation_count \
                 FROM check_run",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            (rows, ran_at, sha.as_deref(), count),
            (1, 200, Some("abc"), 7),
            "N runs leave exactly one marker row, carrying the LAST run"
        );

        assert_eq!(
            foreign_key_violations(&conn),
            0,
            "no FK violations after migration 20"
        );
    }

    /// Migration 21 widens the `check_run` marker with the evaluated set
    /// (S-437, [CR-140] §3.1, [FR-GV-21], [FR-DB-04]): the rule count that is
    /// `violation_count`'s denominator, whether a contract was present at all,
    /// and which operation wrote the row.
    ///
    /// The migration-20 claims, re-run because [CR-140] CRA-06 requires them
    /// re-run: the version advances by exactly **one**; a populated graph and
    /// its governance rows cross the boundary byte-for-byte, so the upgrade
    /// needs no re-index.
    ///
    /// The claim this migration adds, and the one worth doubting hardest: the
    /// marker a store **already has** keeps its three original fields and reads
    /// NULL — *evaluated set unknown* — for the three new ones. Rendering that
    /// absence as a zero is the exact misreading [CR-140] exists to remove, so
    /// reintroducing it here would be the fix restoring its own defect.
    ///
    /// [CR-140]: ../../../../docs/requests/CR-140-the-recorded-check-marker-carries-what-it-evaluated.md
    /// [FR-DB-04]: ../../../../docs/specs/requirements/FR-DB-04.md
    /// [FR-GV-21]: ../../../../docs/specs/requirements/FR-GV-21.md
    #[test]
    fn migration_21_widens_the_check_run_marker_preserving_the_graph_byte_for_byte() {
        let mut conn = contract_conn();

        // Stop at v20 and populate the graph, the governance rows, and the
        // marker a store that has already been checked carries — the common
        // case this migration upgrades, not a fresh database.
        apply_migrations_from(&mut conn, &MIGRATIONS[..20]).unwrap();
        conn.execute_batch(
            "INSERT INTO files (id, path) VALUES (1, 'a.rs');
             INSERT INTO symbols (id, symbol) VALUES (1, 'local a');
             INSERT INTO nodes (id, symbol_id, kind, name, file_id, exported,
                                cyclomatic_complexity, is_test, body) VALUES
                 (10, 1, 7, 'caller', 1, 1, 3, 0, 'the body prose');
             INSERT INTO shingles (node_id, hash) VALUES (10, 111);
             INSERT INTO violations
                 (id, snapshot_id, rule_type, rule_key, node_id, file, message, severity, created_at)
             VALUES (1, NULL, 'constraint', 'max_cc', 10, 'a.rs', 'too complex', 'error', 1700000000);
             INSERT INTO check_run (id, ran_at, commit_sha, violation_count)
             VALUES (1, 1700000000, 'abc1234', 1);",
        )
        .unwrap();

        let graph_before = read_graph(&conn);
        let violations_before = read_table(&conn, "violations", "id");

        // The evaluated set does not exist yet — the omission CR-140 closes.
        assert!(
            conn.query_row("SELECT checked_rules FROM check_run WHERE id = 1", [], |r| {
                r.get::<_, Option<i64>>(0)
            })
            .is_err(),
            "the evaluated-set columns do not exist before migration 21"
        );

        apply_migrations_from(&mut conn, &MIGRATIONS[..21]).unwrap();
        assert_eq!(
            current_version(&conn).unwrap(),
            21,
            "PRAGMA user_version advances by exactly one (20 → 21)"
        );

        assert_eq!(
            read_graph(&conn),
            graph_before,
            "nodes, edges and shingles are byte-for-byte unchanged across migration 21"
        );
        assert_eq!(
            read_table(&conn, "violations", "id"),
            violations_before,
            "the violations already recorded are unchanged, created_at included"
        );
        conn.execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES('integrity-check');")
            .expect("FTS index consistent (nodes never touched by migration 21, NFR-RA-09)");

        // The load-bearing claim. The existing marker keeps every field it had
        // and reads NULL — *unknown* — for every field it never recorded.
        // A 0 here would be the fabricated denominator CR-140 exists to remove.
        // Read in two halves — what migration 20 wrote, then what 21 added —
        // rather than one wide tuple. The split is what the assertions are
        // about anyway: the first must be verbatim, the second must be absent.
        let (rows, ran_at, sha, count): (i64, i64, Option<String>, i64) = conn
            .query_row(
                "SELECT (SELECT count(*) FROM check_run), ran_at, commit_sha, violation_count \
                 FROM check_run",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            (rows, ran_at, sha.as_deref(), count),
            (1, 1700000000, Some("abc1234"), 1),
            "the pre-migration marker survives the upgrade verbatim"
        );

        let evaluated_set: (Option<i64>, Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT checked_rules, rules_present, operation FROM check_run",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            evaluated_set,
            (None, None, None),
            "a marker written before migration 21 records NULL for the evaluated set — \
             *unknown*, never zero and never 'no contract' (CR-140 CRA-05)"
        );

        // The singleton is still structural: migration 21 adds columns, it does
        // not rebuild the table, so `CHECK (id = 1)` is still enforcing.
        let second_row = conn.execute(
            "INSERT INTO check_run (id, ran_at, commit_sha, violation_count) VALUES (2, 200, NULL, 7)",
            [],
        );
        assert!(
            second_row.is_err(),
            "CHECK (id = 1) must still reject a second marker row after the widening"
        );

        // The widened upsert round-trips: one row, carrying the LAST run's
        // count AND the evaluated set that run scored against.
        conn.execute(
            "INSERT INTO check_run (id, ran_at, commit_sha, violation_count, checked_rules, \
                                    rules_present, operation) \
             VALUES (1, 200, 'def', 7, 9, 1, 'check') \
             ON CONFLICT(id) DO UPDATE SET ran_at = 200, commit_sha = 'def', violation_count = 7, \
                                           checked_rules = 9, rules_present = 1, operation = 'check'",
            [],
        )
        .unwrap();
        let widened: (i64, i64, Option<i64>, Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT (SELECT count(*) FROM check_run), violation_count, checked_rules, \
                        rules_present, operation FROM check_run",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(
            widened,
            (1, 7, Some(9), Some(1), Some("check".to_string())),
            "the upsert still leaves one row, now carrying its denominator"
        );

        // The vocabulary is the schema's job, not a caller's convention: an
        // operation no reader knows is refused at the write.
        assert!(
            conn.execute("UPDATE check_run SET operation = 'gate' WHERE id = 1", [])
                .is_err(),
            "operation must be constrained to the two operations that write the marker"
        );
        assert!(
            conn.execute("UPDATE check_run SET rules_present = 2 WHERE id = 1", [])
                .is_err(),
            "rules_present is a 0/1 flag, enforced by the schema"
        );

        assert_eq!(
            foreign_key_violations(&conn),
            0,
            "no FK violations after migration 21"
        );
    }

    /// Migration 22 admits the member-local build-manifest tables on a
    /// **populated** v21 store and leaves every pre-existing fact verbatim
    /// (S-462, [CR-148] §3.2 A, [FR-DB-04]).
    ///
    /// The migration-19 claims, in the order a reader should doubt them: the
    /// version advances by exactly **one**; the graph, the reference ledger, the
    /// configuration corpus and the check-run marker cross the boundary
    /// byte-for-byte, so an upgraded store needs no re-index; the new tables
    /// arrive **empty**, so a member is unaffected until its next index or sync
    /// reads a manifest. Then the additions are exercised, because "additive"
    /// also means they work: the pairing CHECKs refuse a refusal with no reason
    /// and a reference with no kind, a manifest path is unique, and deleting a
    /// manifest cascades its artifacts away.
    ///
    /// [CR-148]: ../../../../docs/requests/CR-148-build-manifests-yield-a-build-dependency-relation.md
    /// [FR-DB-04]: ../../../../docs/specs/requirements/FR-DB-04.md
    #[test]
    fn migration_22_adds_the_build_manifest_tables_preserving_the_graph_byte_for_byte() {
        let mut conn = contract_conn();

        // Stop at v21 — the pre-migration fixture — and populate every table a
        // member's store already carries at that version.
        apply_migrations_from(&mut conn, &MIGRATIONS[..21]).unwrap();
        conn.execute_batch(
            "INSERT INTO files (id, path) VALUES (1, 'a.rs'), (2, 'application.yml');
             INSERT INTO symbols (id, symbol) VALUES (1, 'local a'), (2, 'local b');
             INSERT INTO nodes (id, symbol_id, kind, name, file_id, exported,
                                cyclomatic_complexity, is_test, body) VALUES
                 (10, 1, 7,  'caller',   1, 1, 3,    0, NULL),
                 (20, 2, 19, 'Overview', 2, 0, NULL, 0, 'the body prose');
             INSERT INTO edges (source, target, kind, payload) VALUES (20, 10, 11, 'doc-ref');
             INSERT INTO shingles (node_id, hash) VALUES (10, 111), (10, 222);
             INSERT INTO unresolved_refs (file_id, source_symbol, target, alias, form, kind, line, resolved, payload) VALUES
                 (1, 'local a', 'helper', 'h', 1, 2, 42, 1, NULL);
             INSERT INTO config_sources (id, file_id, profile) VALUES (1, 2, NULL);
             INSERT INTO config_values (source_id, key, value) VALUES (1, 'server.port', '8080');
             INSERT INTO check_run (id, ran_at, commit_sha, violation_count, checked_rules, rules_present, operation)
                 VALUES (1, 1700000000, 'abc1234', 0, 9, 1, 'check');",
        )
        .unwrap();

        let graph_before = read_graph(&conn);
        let ledger_before = read_ledger(&conn);
        let corpus_before = (
            read_table(&conn, "config_sources", "id"),
            read_table(&conn, "config_values", "id"),
        );
        let marker_before = read_table(&conn, "check_run", "id");

        // The tables do not exist before the migration.
        assert!(
            conn.query_row("SELECT count(*) FROM build_manifests", [], |r| r.get::<_, i64>(0))
                .is_err(),
            "build_manifests does not exist at v21"
        );

        apply_migrations_from(&mut conn, &MIGRATIONS[..22]).unwrap();
        assert_eq!(
            current_version(&conn).unwrap(),
            22,
            "PRAGMA user_version advances by exactly one (21 → 22)"
        );
        // Forward-only: re-running the ledger through v22 on a v22 store applies
        // nothing. Through v22, not the full ledger: a later migration may itself
        // add a `nodes` column (migration 25 does), which is not this one's effect.
        apply_migrations_from(&mut conn, &MIGRATIONS[..22]).unwrap();
        let recorded: i64 = conn
            .query_row("SELECT count(*) FROM schema_versions WHERE version = 22", [], |r| r.get(0))
            .unwrap();
        assert_eq!(recorded, 1, "migration 22 is recorded once and never re-applied");

        assert_eq!(
            read_graph(&conn),
            graph_before,
            "nodes, edges and shingles are byte-for-byte unchanged across migration 22"
        );
        assert_eq!(
            read_ledger(&conn),
            ledger_before,
            "the reference ledger is byte-for-byte unchanged across migration 22"
        );
        assert_eq!(
            (
                read_table(&conn, "config_sources", "id"),
                read_table(&conn, "config_values", "id"),
            ),
            corpus_before,
            "the configuration corpus is byte-for-byte unchanged across migration 22"
        );
        assert_eq!(
            read_table(&conn, "check_run", "id"),
            marker_before,
            "the check-run marker is byte-for-byte unchanged across migration 22"
        );
        conn.execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES('integrity-check');")
            .expect("FTS index consistent (nodes never touched by migration 22, NFR-RA-09)");

        // The upgrade itself reads no manifest.
        let (manifests, artifacts): (i64, i64) = conn
            .query_row(
                "SELECT (SELECT count(*) FROM build_manifests), (SELECT count(*) FROM build_artifacts)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((manifests, artifacts), (0, 0), "the upgrade itself ingests nothing");

        // The additions work: a read manifest, an unreadable one, and facts of
        // every shape the reader emits.
        conn.execute_batch(
            "INSERT INTO build_manifests (id, path, format, content_hash, status, detail) VALUES
                 (1, 'pom.xml', 'maven', 'h1', 'read', NULL),
                 (2, 'broken/pom.xml', 'maven', NULL, 'unreadable', 'not UTF-8');
             INSERT INTO build_artifacts (manifest_id, role, kind, group_id, artifact_id, version,
                                          scope, project_path, resolution, reason) VALUES
                 (1, 'produced',   NULL,         'g', 'a', '1',           NULL,   NULL, 'resolved',        NULL),
                 (1, 'referenced', 'bom-import', 'g', 'b', '${v}',        'import', NULL, 'version-refused', 'version: undefined'),
                 (1, 'referenced', 'dependency', NULL, NULL, NULL,        'implementation', ':core', 'resolved', NULL);",
        )
        .expect("every shape the reader emits is admitted");

        for (label, sql) in [
            (
                "a refusal without a reason",
                "INSERT INTO build_artifacts (manifest_id, role, kind, resolution, reason) \
                 VALUES (1, 'referenced', 'dependency', 'refused', NULL)",
            ),
            (
                "a resolved fact with a reason",
                "INSERT INTO build_artifacts (manifest_id, role, kind, resolution, reason) \
                 VALUES (1, 'referenced', 'dependency', 'resolved', 'why')",
            ),
            (
                "a reference with no kind",
                "INSERT INTO build_artifacts (manifest_id, role, kind, resolution) \
                 VALUES (1, 'referenced', NULL, 'resolved')",
            ),
            (
                "a produced fact with a kind",
                "INSERT INTO build_artifacts (manifest_id, role, kind, resolution) \
                 VALUES (1, 'produced', 'parent', 'resolved')",
            ),
            (
                "an unknown kind",
                "INSERT INTO build_artifacts (manifest_id, role, kind, resolution) \
                 VALUES (1, 'referenced', 'plugin', 'resolved')",
            ),
            (
                "a read manifest with a detail",
                "INSERT INTO build_manifests (path, format, status, detail) \
                 VALUES ('x/pom.xml', 'maven', 'read', 'why')",
            ),
            (
                "an unknown format",
                "INSERT INTO build_manifests (path, format, status, detail) \
                 VALUES ('package.json', 'npm', 'malformed', 'x')",
            ),
            (
                "a duplicate path",
                "INSERT INTO build_manifests (path, format, status) VALUES ('pom.xml', 'maven', 'read')",
            ),
        ] {
            assert!(conn.execute(sql, []).is_err(), "migration 22 must refuse {label}");
        }

        // Deleting a manifest cascades its facts away — no orphaned artifact.
        conn.execute("DELETE FROM build_manifests WHERE id = 1", []).unwrap();
        let artifacts: i64 = conn
            .query_row("SELECT count(*) FROM build_artifacts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(artifacts, 0, "deleting a manifest cascades every artifact under it");
        assert_eq!(foreign_key_violations(&conn), 0, "no FK violations after migration 22");
    }

    /// Migration 23 ([CR-156], S-487) adds `metric_snapshots.modularity_applicable`
    /// and nothing else: every pre-migration snapshot row survives byte for byte
    /// with the new column `NULL` — which the read path takes as **applicable**,
    /// since those rows were scored under metric-semantics ≤ 5, when Modularity
    /// always applied. The flag admits 0/1 only.
    ///
    /// [CR-156]: ../../../../docs/requests/CR-156-modularity-drops-out-of-a-too-small-graph.md
    #[test]
    fn migration_23_adds_modularity_applicable_and_pre_migration_rows_read_applicable() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        apply_migrations_from(&mut conn, &MIGRATIONS[..22]).unwrap();
        // A v5 snapshot, every CR-005 column populated, Cohesion dropped out.
        conn.execute(
            "INSERT INTO metric_snapshots (
                 id, created_at, node_count, edge_count, function_count,
                 test_function_count, metric_version, empty,
                 modularity_raw, modularity_normalized,
                 acyclicity_raw, acyclicity_normalized,
                 depth_raw, depth_normalized,
                 equality_raw, equality_normalized,
                 redundancy_raw, redundancy_normalized,
                 nesting_raw, nesting_normalized,
                 conciseness_raw, conciseness_normalized,
                 cohesion_raw, cohesion_normalized, cohesion_applicable,
                 focus_raw, focus_normalized, focus_applicable,
                 uniqueness_raw, uniqueness_normalized,
                 thresholds_hash, aggregate_signal)
             VALUES (1, 1000, 14, 1, 9, 2, 5, 0,
                     -0.5, 0.0, 0.0, 1.0, 2.0, 0.8, 0.1, 0.9, 0.0, 1.0,
                     0.0, 1.0, 0.0, 1.0,
                     NULL, NULL, 0,
                     0.0, 1.0, 1,
                     0.0, 1.0,
                     'h', 0)",
            [],
        )
        .unwrap();
        let before = read_table(&conn, "metric_snapshots", "id");
        assert!(
            conn.query_row("SELECT modularity_applicable FROM metric_snapshots", [], |r| {
                r.get::<_, Option<i64>>(0)
            })
            .is_err(),
            "the flag does not exist at v22"
        );

        apply_migrations_from(&mut conn, &MIGRATIONS[..23]).unwrap();
        assert_eq!(current_version(&conn).unwrap(), 23, "22 → 23, exactly one step");
        // Read at v23: migration 26 (S-498) adds a later `metric_snapshots`
        // column of its own, which is not this migration's to account for.
        let after = read_table(&conn, "metric_snapshots", "id");
        apply_migrations_from(&mut conn, MIGRATIONS).unwrap();
        let recorded: i64 = conn
            .query_row("SELECT count(*) FROM schema_versions WHERE version = 23", [], |r| r.get(0))
            .unwrap();
        assert_eq!(recorded, 1, "migration 23 is recorded once and never re-applied");

        // Every pre-migration column verbatim; the one new column is NULL.
        assert_eq!(after.len(), before.len());
        for (old, new) in before.iter().zip(&after) {
            assert_eq!(new.len(), old.len() + 1, "exactly one column added");
            assert_eq!(&new[..old.len()], &old[..], "every existing column verbatim");
            assert_eq!(new[old.len()], "NULL", "a pre-migration row carries no flag");
        }

        // A v6 not-applicable row round-trips; the CHECK admits 0/1 only.
        conn.execute(
            "UPDATE metric_snapshots SET modularity_applicable = 0 WHERE id = 1",
            [],
        )
        .expect("0 is admitted");
        conn.execute(
            "UPDATE metric_snapshots SET modularity_applicable = 1 WHERE id = 1",
            [],
        )
        .expect("1 is admitted");
        assert!(
            conn.execute(
                "UPDATE metric_snapshots SET modularity_applicable = 2 WHERE id = 1",
                [],
            )
            .is_err(),
            "modularity_applicable is constrained to 0/1"
        );
    }

    /// Migration 24 ([CR-152], S-472) is purely additive: every table a v23
    /// member store carries — the graph, the reference ledger, the configuration
    /// corpus, the check-run marker and the build-manifest facts — crosses the
    /// boundary byte-for-byte, so an upgraded store needs no re-index; the two
    /// new tables arrive **empty**. Then the additions are exercised, because
    /// "additive" also means they work: the pairing CHECKs refuse a fact with
    /// two provenances or none, a source fact without its symbol, a refusal
    /// without a reason and a refusal that still names a type; a schema path is
    /// unique; and deleting a file or a schema cascades exactly its facts away.
    ///
    /// [CR-152]: ../../../../docs/requests/CR-152-cross-member-type-references-overlay.md
    #[test]
    fn migration_24_adds_the_declared_type_tables_preserving_the_graph_byte_for_byte() {
        let mut conn = contract_conn();

        // Stop at v23 — the pre-migration fixture — and populate every table a
        // member's store already carries at that version.
        apply_migrations_from(&mut conn, &MIGRATIONS[..23]).unwrap();
        conn.execute_batch(
            "INSERT INTO files (id, path) VALUES (1, 'src/main/java/com/x/Svc.java'), (2, 'application.yml');
             INSERT INTO symbols (id, symbol) VALUES (1, 'local a'), (2, 'local b');
             INSERT INTO nodes (id, symbol_id, kind, name, file_id, exported,
                                cyclomatic_complexity, is_test, body) VALUES
                 (10, 1, 7,  'caller',   1, 1, 3,    0, NULL),
                 (20, 2, 19, 'Overview', 2, 0, NULL, 0, 'the body prose');
             INSERT INTO edges (source, target, kind, payload) VALUES (20, 10, 11, 'doc-ref');
             INSERT INTO shingles (node_id, hash) VALUES (10, 111), (10, 222);
             INSERT INTO unresolved_refs (file_id, source_symbol, target, alias, form, kind, line, resolved, payload) VALUES
                 (1, 'local a', 'helper', 'h', 1, 2, 42, 1, NULL);
             INSERT INTO config_sources (id, file_id, profile) VALUES (1, 2, NULL);
             INSERT INTO config_values (source_id, key, value) VALUES (1, 'server.port', '8080');
             INSERT INTO check_run (id, ran_at, commit_sha, violation_count, checked_rules, rules_present, operation)
                 VALUES (1, 1700000000, 'abc1234', 0, 9, 1, 'check');
             INSERT INTO build_manifests (id, path, format, content_hash, status, detail)
                 VALUES (1, 'pom.xml', 'maven', 'h1', 'read', NULL);
             INSERT INTO build_artifacts (manifest_id, role, kind, group_id, artifact_id, version, resolution)
                 VALUES (1, 'produced', NULL, 'g', 'a', '1', 'resolved');",
        )
        .unwrap();

        let graph_before = read_graph(&conn);
        let ledger_before = read_ledger(&conn);
        let others = [
            ("config_sources", "id"),
            ("config_values", "id"),
            ("check_run", "id"),
            ("build_manifests", "id"),
            ("build_artifacts", "id"),
            ("project_metadata", "key"),
        ];
        let others_before: Vec<_> = others.iter().map(|(t, o)| read_table(&conn, t, o)).collect();

        for table in ["declared_types", "avro_schemas"] {
            assert!(
                conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get::<_, i64>(0))
                    .is_err(),
                "{table} does not exist at v23"
            );
        }

        apply_migrations_from(&mut conn, &MIGRATIONS[..24]).unwrap();
        assert_eq!(
            current_version(&conn).unwrap(),
            24,
            "PRAGMA user_version advances by exactly one (23 → 24)"
        );
        // Forward-only: re-running the ledger through v24 on a v24 store applies
        // nothing. Through v24, not the full ledger: a later migration may itself
        // add a `nodes` column (migration 25 does), which is not this one's effect.
        apply_migrations_from(&mut conn, &MIGRATIONS[..24]).unwrap();
        let recorded: i64 = conn
            .query_row("SELECT count(*) FROM schema_versions WHERE version = 24", [], |r| r.get(0))
            .unwrap();
        assert_eq!(recorded, 1, "migration 24 is recorded once and never re-applied");

        assert_eq!(
            read_graph(&conn),
            graph_before,
            "nodes, edges and shingles are byte-for-byte unchanged across migration 24"
        );
        assert_eq!(
            read_ledger(&conn),
            ledger_before,
            "the reference ledger is byte-for-byte unchanged across migration 24"
        );
        let others_after: Vec<_> = others.iter().map(|(t, o)| read_table(&conn, t, o)).collect();
        assert_eq!(
            others_after, others_before,
            "the corpus, the check-run marker, the build facts and the metadata are \
             byte-for-byte unchanged across migration 24"
        );
        conn.execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES('integrity-check');")
            .expect("FTS index consistent (nodes never touched by migration 24, NFR-RA-09)");

        // The upgrade itself reads no type.
        let (types, schemas): (i64, i64) = conn
            .query_row(
                "SELECT (SELECT count(*) FROM declared_types), (SELECT count(*) FROM avro_schemas)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((types, schemas), (0, 0), "the upgrade itself ingests nothing");

        // The additions work: every shape the pipeline writes is admitted.
        conn.execute_batch(
            "INSERT INTO avro_schemas (id, path, content_hash, status, detail) VALUES
                 (1, 'src/main/avro/a.avsc', 'h1', 'read', NULL),
                 (2, 'src/main/avro/bad.avsc', 'h2', 'malformed', 'not valid JSON'),
                 (3, 'src/main/avro/gone.avsc', NULL, 'unreadable', 'not UTF-8');
             INSERT INTO declared_types (file_id, schema_id, name, fqn, kind, symbol, tree, resolution, reason) VALUES
                 (1, NULL, 'Svc',   'com.x.Svc', 'class',  'local a', 'main', 'resolved', NULL),
                 (1, NULL, 'Other', NULL,        'interface', 'local b', 'test', 'refused', 'package-mismatch'),
                 (NULL, 1, 'Ev',    'com.x.Ev',  'record', NULL, NULL, 'resolved', NULL),
                 (NULL, 1, 'Kind',  'com.x.Kind', 'enum',  NULL, NULL, 'resolved', NULL);",
        )
        .expect("every shape the pipeline writes is admitted");

        for (label, sql) in [
            (
                "a fact with no provenance",
                "INSERT INTO declared_types (name, fqn, kind, resolution) VALUES ('X', 'a.X', 'class', 'resolved')",
            ),
            (
                "a fact with two provenances",
                "INSERT INTO declared_types (file_id, schema_id, name, fqn, kind, symbol, tree, resolution) \
                 VALUES (1, 1, 'X', 'a.X', 'class', 's', 'main', 'resolved')",
            ),
            (
                "a source fact without its symbol",
                "INSERT INTO declared_types (file_id, name, fqn, kind, tree, resolution) \
                 VALUES (1, 'X', 'a.X', 'class', 'main', 'resolved')",
            ),
            (
                "a source fact without its tree",
                "INSERT INTO declared_types (file_id, name, fqn, kind, symbol, resolution) \
                 VALUES (1, 'X', 'a.X', 'class', 's', 'resolved')",
            ),
            (
                "an Avro fact with a symbol",
                "INSERT INTO declared_types (schema_id, name, fqn, kind, symbol, resolution) \
                 VALUES (1, 'X', 'a.X', 'record', 's', 'resolved')",
            ),
            (
                "a refusal without a reason",
                "INSERT INTO declared_types (file_id, name, fqn, kind, symbol, tree, resolution) \
                 VALUES (1, 'X', NULL, 'class', 's', 'main', 'refused')",
            ),
            (
                "a refusal that still names a type",
                "INSERT INTO declared_types (file_id, name, fqn, kind, symbol, tree, resolution, reason) \
                 VALUES (1, 'X', 'a.X', 'class', 's', 'main', 'refused', 'why')",
            ),
            (
                "a resolved fact without a name",
                "INSERT INTO declared_types (file_id, name, fqn, kind, symbol, tree, resolution) \
                 VALUES (1, 'X', NULL, 'class', 's', 'main', 'resolved')",
            ),
            (
                "an unknown kind",
                "INSERT INTO declared_types (schema_id, name, fqn, kind, resolution) \
                 VALUES (1, 'X', 'a.X', 'fixed', 'resolved')",
            ),
            (
                "an unknown tree",
                "INSERT INTO declared_types (file_id, name, fqn, kind, symbol, tree, resolution) \
                 VALUES (1, 'X', 'a.X', 'class', 's', 'it', 'resolved')",
            ),
            (
                "a read schema with a detail",
                "INSERT INTO avro_schemas (path, content_hash, status, detail) VALUES ('x.avsc', 'h', 'read', 'why')",
            ),
            (
                "an unreadable schema with a hash",
                "INSERT INTO avro_schemas (path, content_hash, status, detail) \
                 VALUES ('y.avsc', 'h', 'unreadable', 'why')",
            ),
            (
                "a duplicate schema path",
                "INSERT INTO avro_schemas (path, content_hash, status) VALUES ('src/main/avro/a.avsc', 'h', 'read')",
            ),
        ] {
            assert!(conn.execute(sql, []).is_err(), "migration 24 must refuse {label}");
        }

        // Deleting a file cascades its facts away, and a schema's — no orphan,
        // and nothing of the other provenance moves.
        let count = |conn: &Connection| -> i64 {
            conn.query_row("SELECT count(*) FROM declared_types", [], |r| r.get(0))
                .unwrap()
        };
        conn.execute("DELETE FROM files WHERE id = 1", []).unwrap();
        assert_eq!(count(&conn), 2, "deleting a file cascades exactly its two facts");
        conn.execute("DELETE FROM avro_schemas WHERE id = 1", []).unwrap();
        assert_eq!(count(&conn), 0, "deleting a schema cascades exactly its two facts");
        assert_eq!(foreign_key_violations(&conn), 0, "no FK violations after migration 24");
    }

    /// S-500 / CR-163 / FR-EX-11: a populated v24 store upgrades to v25 forward
    /// only. `nodes` gains `has_body` and `body_tokens` in place — `NULL` on every
    /// existing row until re-extraction — while every pre-existing column of
    /// `nodes`, and all of `edges`, `shingles` and the ledger, is byte-for-byte
    /// unchanged. Every `files.content_hash` is cleared, the re-extraction
    /// trigger: the next sync treats each file as modified (asserted end to end by
    /// `tests/indexing.rs`'s `migration_25_triggers_a_re_extraction_that_fills_the_has_body_column`).
    #[test]
    fn migration_25_adds_the_has_body_columns_and_triggers_reextraction() {
        let mut conn = contract_conn();
        apply_migrations_from(&mut conn, &MIGRATIONS[..24]).unwrap();
        conn.execute_batch(
            "INSERT INTO files (id, path, language, content_hash) VALUES
                 (1, 'src/main/java/com/x/Svc.java', 'java', 'h-java'),
                 (2, 'docs/guide.md', 'markdown', 'h-md');
             INSERT INTO symbols (id, symbol) VALUES (1, 'local a'), (2, 'local b');
             INSERT INTO nodes (id, symbol_id, kind, name, file_id, exported,
                                cyclomatic_complexity, line_count, fingerprint,
                                max_nesting_depth, is_test, body) VALUES
                 (10, 1, 7,  'caller',   1, 1, 3,    4,    'fp', 1,    0, NULL),
                 (20, 2, 19, 'Overview', 2, 0, NULL, NULL, NULL, NULL, 0, 'the body prose');
             INSERT INTO edges (source, target, kind, payload) VALUES (20, 10, 11, 'doc-ref');
             INSERT INTO shingles (node_id, hash) VALUES (10, 111), (10, 222);
             INSERT INTO unresolved_refs (file_id, source_symbol, target, alias, form, kind, line, resolved, payload) VALUES
                 (1, 'local a', 'helper', 'h', 1, 2, 42, 1, NULL);",
        )
        .unwrap();
        let (nodes_before, edges_before, shingles_before) = read_graph(&conn);
        let ledger_before = read_ledger(&conn);
        assert!(
            conn.query_row("SELECT has_body FROM nodes WHERE id = 10", [], |r| r.get::<_, Option<i64>>(0))
                .is_err(),
            "has_body does not exist at v24"
        );

        apply_migrations_from(&mut conn, &MIGRATIONS[..25]).unwrap();
        assert_eq!(current_version(&conn).unwrap(), 25, "24 → 25, exactly one step");
        // Read at v25, so a later migration's column is never counted as this one's.
        let (nodes_after, edges_after, shingles_after) = read_graph(&conn);
        let tail: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('nodes') ORDER BY cid DESC LIMIT 2")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        // Forward-only: re-running the full ledger on a v25 store never re-applies migration 25.
        apply_migrations_from(&mut conn, MIGRATIONS).unwrap();
        let recorded: i64 = conn
            .query_row("SELECT count(*) FROM schema_versions WHERE version = 25", [], |r| r.get(0))
            .unwrap();
        assert_eq!(recorded, 1, "migration 25 is recorded once and never re-applied");

        // The two columns are appended NULL; every other column is untouched.
        let (old_columns, new_columns): (Vec<Vec<String>>, Vec<Vec<String>>) = nodes_after
            .iter()
            .map(|row| {
                let (old, new) = row.split_at(row.len() - 2);
                (old.to_vec(), new.to_vec())
            })
            .unzip();
        assert_eq!(old_columns, nodes_before, "every pre-v25 nodes column is byte-for-byte unchanged");
        assert_eq!(
            new_columns,
            vec![vec!["NULL".to_string(), "NULL".to_string()]; 2],
            "has_body and body_tokens are NULL on every existing row until re-extraction"
        );
        assert_eq!((edges_after, shingles_after), (edges_before, shingles_before));
        assert_eq!(read_ledger(&conn), ledger_before, "the reference ledger is unchanged");
        assert_eq!(tail, ["body_tokens", "has_body"], "the appended columns are the two S-500 adds");

        // The re-extraction trigger: every file's hash is cleared, nothing else.
        let files: Vec<(i64, String, Option<String>, Option<String>)> = conn
            .prepare("SELECT id, path, language, content_hash FROM files ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            files,
            vec![
                (1, "src/main/java/com/x/Svc.java".to_string(), Some("java".to_string()), None),
                (2, "docs/guide.md".to_string(), Some("markdown".to_string()), None),
            ],
            "every content_hash is cleared, so the next sync re-extracts the file; ids, \
             paths and languages stay"
        );
        conn.execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES('integrity-check');")
            .expect("FTS index consistent (an in-place ADD COLUMN, NFR-RA-09)");

        // The extraction values are admitted; out-of-range ones are refused.
        conn.execute("UPDATE nodes SET has_body = 0, body_tokens = 0 WHERE id = 10", [])
            .expect("a bodyless callable is admitted");
        conn.execute("UPDATE nodes SET has_body = 1, body_tokens = 57 WHERE id = 10", [])
            .expect("a bodied callable is admitted");
        assert!(
            conn.execute("UPDATE nodes SET has_body = 2 WHERE id = 10", []).is_err(),
            "has_body is a 0/1 flag"
        );
        assert!(
            conn.execute("UPDATE nodes SET body_tokens = -1 WHERE id = 10", []).is_err(),
            "a token count is never negative"
        );
        assert_eq!(foreign_key_violations(&conn), 0, "no FK violations after migration 25");
    }

    /// Migration 26 ([CR-162], S-498) is purely additive: every pre-migration
    /// snapshot row survives byte for byte with `offenders_recorded` `NULL` —
    /// the "offenders not recorded" reading, which no back-fill overwrites — and
    /// the offender table arrives **empty**. Then the additions are exercised:
    /// the flag admits only `1`, an offender row must name an existing snapshot
    /// and a positive rank, and a `(snapshot, dimension, rank)` key is unique.
    ///
    /// [CR-162]: ../../../../docs/requests/CR-162-health-shows-the-worst-offenders-its-snapshot-computed.md
    #[test]
    fn migration_26_adds_snapshot_offenders_and_pre_migration_rows_read_not_recorded() {
        let mut conn = contract_conn();
        apply_migrations_from(&mut conn, &MIGRATIONS[..25]).unwrap();
        // Two v6 snapshots, the second the latest — the row the Health bundle reads.
        for id in [1, 2] {
            conn.execute(
                "INSERT INTO metric_snapshots (
                     id, created_at, node_count, edge_count, function_count,
                     test_function_count, metric_version, empty,
                     modularity_raw, modularity_normalized,
                     acyclicity_raw, acyclicity_normalized,
                     depth_raw, depth_normalized,
                     equality_raw, equality_normalized,
                     redundancy_raw, redundancy_normalized,
                     nesting_raw, nesting_normalized,
                     conciseness_raw, conciseness_normalized,
                     cohesion_raw, cohesion_normalized, cohesion_applicable,
                     focus_raw, focus_normalized, focus_applicable,
                     uniqueness_raw, uniqueness_normalized,
                     thresholds_hash, aggregate_signal, modularity_applicable)
                 VALUES (?1, 1000, 14, 6, 9, 2, 6, 0,
                         0.2, 0.46, 0.0, 1.0, 2.0, 0.8, 0.1, 0.9, 0.0, 1.0,
                         0.1, 0.9, 0.0, 1.0,
                         NULL, NULL, 0,
                         0.0, 1.0, 1,
                         0.0, 1.0,
                         'h', 8000, 1)",
                [id],
            )
            .unwrap();
        }
        let before = read_table(&conn, "metric_snapshots", "id");
        assert!(
            conn.query_row("SELECT count(*) FROM metric_snapshot_offenders", [], |r| r.get::<_, i64>(0))
                .is_err(),
            "the offender table does not exist at v25"
        );

        apply_migrations_from(&mut conn, &MIGRATIONS[..26]).unwrap();
        assert_eq!(current_version(&conn).unwrap(), 26, "25 → 26, exactly one step");
        // Read at v26, so a later migration's column is never counted as this one's.
        let after = read_table(&conn, "metric_snapshots", "id");
        apply_migrations_from(&mut conn, MIGRATIONS).unwrap();
        let recorded: i64 = conn
            .query_row("SELECT count(*) FROM schema_versions WHERE version = 26", [], |r| r.get(0))
            .unwrap();
        assert_eq!(recorded, 1, "migration 26 is recorded once and never re-applied");

        // Every pre-migration column verbatim; the one new column is NULL.
        assert_eq!(after.len(), before.len());
        for (old, new) in before.iter().zip(&after) {
            assert_eq!(new.len(), old.len() + 1, "exactly one column added");
            assert_eq!(&new[..old.len()], &old[..], "every existing column verbatim");
            assert_eq!(new[old.len()], "NULL", "a pre-migration snapshot recorded no offenders");
        }
        let offenders: i64 = conn
            .query_row("SELECT count(*) FROM metric_snapshot_offenders", [], |r| r.get(0))
            .unwrap();
        assert_eq!(offenders, 0, "no back-fill: old snapshots stay not recorded");

        // The flag admits 1 only; NULL stays the pre-migration reading.
        conn.execute("UPDATE metric_snapshots SET offenders_recorded = 1 WHERE id = 2", [])
            .expect("1 is admitted");
        for refused in [0, 2] {
            assert!(
                conn.execute(
                    "UPDATE metric_snapshots SET offenders_recorded = ?1 WHERE id = 2",
                    [refused],
                )
                .is_err(),
                "offenders_recorded = {refused} is refused: the one append path always records"
            );
        }

        // An offender row: keyed by an existing snapshot, rank ≥ 1, unique key.
        let insert = |snapshot: i64, dimension: &str, rank: i64| {
            conn.execute(
                "INSERT INTO metric_snapshot_offenders
                     (snapshot_id, dimension, rank, name, file, line, detail)
                 VALUES (?1, ?2, ?3, 'f', NULL, NULL, 'nesting depth 5')",
                rusqlite::params![snapshot, dimension, rank],
            )
        };
        insert(2, "nesting", 1).expect("a well-formed row with an unbound file is admitted");
        assert!(insert(2, "nesting", 1).is_err(), "a (snapshot, dimension, rank) is unique");
        assert!(insert(2, "nesting", 0).is_err(), "ranks are 1-based");
        assert!(insert(99, "nesting", 1).is_err(), "an offender names an existing snapshot");
        assert_eq!(foreign_key_violations(&conn), 0, "no FK violations after migration 26");
    }

    /// Migration 27 ([CR-168], S-513) is purely additive: a populated v26 graph —
    /// files, nodes, edges, shingles and the ledger — crosses it byte for byte,
    /// and the persist-failure record arrives **empty**, which is the true
    /// reading of a pre-migration store (a failed persist then recorded
    /// nothing). Then the table is exercised: a path is the key, `stale` admits
    /// only `0`/`1`, and a path needs no `files` row (a file that failed on a
    /// full index has none).
    ///
    /// [CR-168]: ../../../../docs/requests/CR-168-an-index-never-silently-empties.md
    #[test]
    fn migration_27_adds_the_persist_failure_record_and_touches_nothing_else() {
        let mut conn = contract_conn();
        apply_migrations_from(&mut conn, &MIGRATIONS[..26]).unwrap();
        conn.execute_batch(
            "INSERT INTO files (id, path, language, content_hash) VALUES
                 (1, 'src/main/java/com/x/Svc.java', 'java', 'h-java'),
                 (2, 'docs/guide.md', 'markdown', 'h-md');
             INSERT INTO symbols (id, symbol) VALUES (1, 'local a'), (2, 'local b');
             INSERT INTO nodes (id, symbol_id, kind, name, file_id, exported,
                                cyclomatic_complexity, line_count, fingerprint,
                                max_nesting_depth, is_test, body, has_body, body_tokens) VALUES
                 (10, 1, 7,  'caller',   1, 1, 3,    4,    'fp', 1,    0, NULL, 1, 12),
                 (20, 2, 19, 'Overview', 2, 0, NULL, NULL, NULL, NULL, 0, 'the body prose', NULL, NULL);
             INSERT INTO edges (source, target, kind, payload) VALUES (20, 10, 11, 'doc-ref');
             INSERT INTO shingles (node_id, hash) VALUES (10, 111), (10, 222);
             INSERT INTO unresolved_refs (file_id, source_symbol, target, alias, form, kind, line, resolved, payload) VALUES
                 (1, 'local a', 'helper', 'h', 1, 2, 42, 1, NULL);",
        )
        .unwrap();
        let graph_before = read_graph(&conn);
        let files_before = read_table(&conn, "files", "id");
        let ledger_before = read_ledger(&conn);
        assert!(
            conn.query_row("SELECT count(*) FROM persist_failures", [], |r| r.get::<_, i64>(0))
                .is_err(),
            "the persist-failure record does not exist at v26"
        );

        apply_migrations_from(&mut conn, &MIGRATIONS[..27]).unwrap();
        assert_eq!(current_version(&conn).unwrap(), 27, "26 → 27, exactly one step");
        // Read at v27, so a later migration's column or hash reset is never counted as this one's.
        assert_eq!(read_graph(&conn), graph_before, "nodes, edges and shingles are byte-for-byte unchanged");
        assert_eq!(read_table(&conn, "files", "id"), files_before, "files are byte-for-byte unchanged");
        apply_migrations_from(&mut conn, MIGRATIONS).unwrap();
        let recorded: i64 = conn
            .query_row("SELECT count(*) FROM schema_versions WHERE version = 27", [], |r| r.get(0))
            .unwrap();
        assert_eq!(recorded, 1, "migration 27 is recorded once and never re-applied");

        assert_eq!(read_ledger(&conn), ledger_before, "the ledger is byte-for-byte unchanged");
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM persist_failures", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0, "no back-fill: a pre-migration store recorded no failure");

        let insert = |path: &str, stale: i64| {
            conn.execute(
                "INSERT INTO persist_failures (path, reason, stale) VALUES (?1, 'boom', ?2)",
                rusqlite::params![path, stale],
            )
        };
        insert("src/never_indexed.go", 0).expect("a path with no files row is admitted");
        insert("src/main/java/com/x/Svc.java", 1).expect("a stale row is admitted");
        assert!(insert("src/never_indexed.go", 1).is_err(), "a path is recorded once");
        assert!(insert("src/other.go", 2).is_err(), "stale admits 0 or 1 only");
        assert_eq!(foreign_key_violations(&conn), 0, "no FK violations after migration 27");
    }

    /// S-493 / CR-159 / FR-RS-11: a populated v27 store upgrades to v28 forward
    /// only. `nodes` gains `self_type` in place — `NULL` on every existing row
    /// until re-extraction — while every pre-existing column of `nodes`, and all
    /// of `edges`, `shingles` and the ledger, is byte-for-byte unchanged. Every
    /// `files.content_hash` is cleared, the migration-25 re-extraction trigger.
    #[test]
    fn migration_28_adds_the_self_type_column_and_triggers_reextraction() {
        let mut conn = contract_conn();
        apply_migrations_from(&mut conn, &MIGRATIONS[..27]).unwrap();
        conn.execute_batch(
            "INSERT INTO files (id, path, language, content_hash) VALUES
                 (1, 'src/roster.rs', 'rust', 'h-rs'),
                 (2, 'docs/guide.md', 'markdown', 'h-md');
             INSERT INTO symbols (id, symbol) VALUES (1, 'local a'), (2, 'local b');
             INSERT INTO nodes (id, symbol_id, kind, name, file_id, exported,
                                cyclomatic_complexity, line_count, fingerprint,
                                max_nesting_depth, is_test, body, has_body, body_tokens) VALUES
                 (10, 1, 8,  'helper',   1, 0, 3,    4,    'fp', 1,    0, NULL, 1, 12),
                 (20, 2, 19, 'Overview', 2, 0, NULL, NULL, NULL, NULL, 0, 'the body prose', NULL, NULL);
             INSERT INTO edges (source, target, kind, payload) VALUES (20, 10, 11, 'doc-ref');
             INSERT INTO shingles (node_id, hash) VALUES (10, 111), (10, 222);
             INSERT INTO unresolved_refs (file_id, source_symbol, target, alias, form, kind, line, resolved, payload) VALUES
                 (1, 'local a', 'Self::helper', NULL, 1, 2, 42, 0, NULL);",
        )
        .unwrap();
        let (nodes_before, edges_before, shingles_before) = read_graph(&conn);
        let ledger_before = read_ledger(&conn);
        assert!(
            conn.query_row("SELECT self_type FROM nodes WHERE id = 10", [], |r| r.get::<_, Option<String>>(0))
                .is_err(),
            "self_type does not exist at v27"
        );

        apply_migrations_from(&mut conn, &MIGRATIONS[..28]).unwrap();
        assert_eq!(current_version(&conn).unwrap(), 28, "27 → 28, exactly one step");
        // Read at v28, so a later migration's column is never counted as this one's.
        let (nodes_after, edges_after, shingles_after) = read_graph(&conn);
        let tail: String = conn
            .query_row("SELECT name FROM pragma_table_info('nodes') ORDER BY cid DESC LIMIT 1", [], |r| r.get(0))
            .unwrap();
        // Forward-only: re-running the full ledger on a v28 store never re-applies migration 28.
        apply_migrations_from(&mut conn, MIGRATIONS).unwrap();
        let recorded: i64 = conn
            .query_row("SELECT count(*) FROM schema_versions WHERE version = 28", [], |r| r.get(0))
            .unwrap();
        assert_eq!(recorded, 1, "migration 28 is recorded once and never re-applied");

        // The one column is appended NULL; every other column is untouched.
        for (old, new) in nodes_before.iter().zip(&nodes_after) {
            assert_eq!(new.len(), old.len() + 1, "exactly one nodes column added");
            assert_eq!(&new[..old.len()], &old[..], "every pre-v28 nodes column is byte-for-byte unchanged");
            assert_eq!(new[old.len()], "NULL", "self_type is NULL on every existing row until re-extraction");
        }
        assert_eq!(nodes_after.len(), nodes_before.len());
        assert_eq!(tail, "self_type", "the appended column is the S-493 add");
        assert_eq!((edges_after, shingles_after), (edges_before, shingles_before));
        assert_eq!(read_ledger(&conn), ledger_before, "the reference ledger is unchanged");

        // The re-extraction trigger: every file's hash is cleared, nothing else.
        let files: Vec<(i64, String, Option<String>, Option<String>)> = conn
            .prepare("SELECT id, path, language, content_hash FROM files ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            files,
            vec![
                (1, "src/roster.rs".to_string(), Some("rust".to_string()), None),
                (2, "docs/guide.md".to_string(), Some("markdown".to_string()), None),
            ],
            "every content_hash is cleared, so the next scan re-extracts the file; ids, \
             paths and languages stay"
        );
        conn.execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES('integrity-check');")
            .expect("FTS index consistent (an in-place ADD COLUMN, NFR-RA-09)");
        conn.execute("UPDATE nodes SET self_type = 'WorkspaceRoster' WHERE id = 10", [])
            .expect("a self type is admitted");
        assert_eq!(foreign_key_violations(&conn), 0, "no FK violations after migration 28");
    }

    /// S-514 / CR-169 / FR-EX-13: a populated v28 store upgrades to v29 forward
    /// only. The ledger gains `receiver` in place — `NULL` on every existing row
    /// until re-extraction — while every pre-existing ledger column, and all of
    /// `nodes`, `edges` and `shingles`, is byte-for-byte unchanged. The identity
    /// index is widened by the shape: a row with none dedups exactly as before,
    /// and two rows differing only in shape coexist. Every `files.content_hash`
    /// is cleared, the migration-25 re-extraction trigger.
    #[test]
    fn migration_29_adds_the_receiver_shape_to_the_ledger_identity() {
        let mut conn = contract_conn();
        apply_migrations_from(&mut conn, &MIGRATIONS[..28]).unwrap();
        conn.execute_batch(
            "INSERT INTO files (id, path, language, content_hash) VALUES
                 (1, 'src/a.py', 'python', 'h-py');
             INSERT INTO symbols (id, symbol) VALUES (1, 'local n'), (2, 'local m');
             INSERT INTO nodes (id, symbol_id, kind, name, file_id) VALUES
                 (10, 1, 8, 'n', 1), (11, 2, 8, 'm', 1);
             INSERT INTO edges (source, target, kind) VALUES (10, 11, 2);
             INSERT INTO unresolved_refs (file_id, source_symbol, target, alias, form, kind, line, resolved, payload) VALUES
                 (1, 'local n', 'm', NULL, 3, 2, 4, 1, NULL),
                 (1, 'local n', 'topic', NULL, 3, 14, 5, 0, 'broker-publish');",
        )
        .unwrap();
        let graph_before = read_graph(&conn);
        let ledger_before = read_ledger(&conn);

        apply_migrations_from(&mut conn, &MIGRATIONS[..29]).unwrap();
        assert_eq!(current_version(&conn).unwrap(), 29, "28 → 29, exactly one step");
        assert_eq!(read_graph(&conn), graph_before, "nodes, edges and shingles are untouched");
        assert_eq!(read_ledger(&conn), ledger_before, "every pre-v29 ledger column is unchanged");
        let shapes: Vec<Option<i64>> = conn
            .prepare("SELECT receiver FROM unresolved_refs ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(shapes, vec![None, None], "no row has a shape until re-extraction");

        // The widened identity: a shapeless duplicate is still one row; the same
        // call with a shape is another, once per shape.
        let insert = |receiver: Option<i64>| {
            conn.execute(
                "INSERT INTO unresolved_refs (file_id, source_symbol, target, form, kind, line, receiver) \
                 VALUES (1, 'local n', 'm', 3, 2, 9, ?1) \
                 ON CONFLICT(source_symbol, target, form, kind, COALESCE(payload, ''), \
                             COALESCE(receiver, 0)) DO NOTHING",
                [receiver],
            )
        };
        assert_eq!(insert(None).unwrap(), 0, "a shapeless duplicate dedups as before");
        assert_eq!(insert(Some(1)).unwrap(), 1, "`self` is a second row");
        assert_eq!(insert(Some(3)).unwrap(), 1, "`other` is a third");
        assert_eq!(insert(Some(1)).unwrap(), 0, "a repeated shape dedups");
        assert!(insert(Some(4)).is_err(), "the CHECK admits the three shapes only");

        // Forward-only: re-running the full ledger never re-applies migration 29.
        apply_migrations_from(&mut conn, MIGRATIONS).unwrap();
        let recorded: i64 = conn
            .query_row("SELECT count(*) FROM schema_versions WHERE version = 29", [], |r| r.get(0))
            .unwrap();
        assert_eq!(recorded, 1, "migration 29 is recorded once and never re-applied");
        let hash: Option<String> = conn
            .query_row("SELECT content_hash FROM files WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(hash, None, "the content hash is cleared, so the next scan re-extracts");
        assert_eq!(foreign_key_violations(&conn), 0, "no FK violations after migration 29");
    }

    /// Every column of `nodes`, `edges` and `shingles`, as SQLite reports them —
    /// so "unchanged" is content, not row counts.
    ///
    /// `SELECT *` with a generic row reader, deliberately, rather than a column
    /// list. An enumerated projection is the thing that goes stale: the first
    /// draft of this helper listed 8 of the 20 columns `nodes` carries at v18 and
    /// its doc comment claimed "every column" — it silently ignored `is_test`,
    /// which the fixture below explicitly populates. Reading whatever columns the
    /// table has keeps the guard honest as migration 20 and beyond add more.
    type GraphSnapshot = (Vec<Vec<String>>, Vec<Vec<String>>, Vec<Vec<String>>);

    /// Read a whole table as text, every column, ordered by the given clause.
    fn read_table(conn: &Connection, table: &str, order: &str) -> Vec<Vec<String>> {
        let mut stmt = conn
            .prepare(&format!("SELECT * FROM {table} ORDER BY {order}"))
            .unwrap();
        let columns = stmt.column_count();
        stmt.query_map([], |r| {
            (0..columns)
                .map(|i| {
                    // Render every storage class, so a NULL is distinguishable
                    // from the string "NULL" and a blob from its text spelling.
                    Ok(match r.get_ref(i)? {
                        rusqlite::types::ValueRef::Null => "NULL".to_string(),
                        rusqlite::types::ValueRef::Integer(v) => format!("i:{v}"),
                        rusqlite::types::ValueRef::Real(v) => format!("r:{v}"),
                        rusqlite::types::ValueRef::Text(v) => {
                            format!("t:{}", String::from_utf8_lossy(v))
                        }
                        rusqlite::types::ValueRef::Blob(v) => format!("b:{v:?}"),
                    })
                })
                .collect::<rusqlite::Result<Vec<String>>>()
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    }

    /// The full node/edge/shingle content, each ordered deterministically.
    fn read_graph(conn: &Connection) -> GraphSnapshot {
        (
            read_table(conn, "nodes", "id"),
            read_table(conn, "edges", "id"),
            read_table(conn, "shingles", "node_id, hash"),
        )
    }
}
