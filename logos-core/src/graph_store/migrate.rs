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

        // Idempotent: re-running the runner applies nothing and changes no row.
        apply_migrations_from(&mut conn, MIGRATIONS).unwrap();
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
}
