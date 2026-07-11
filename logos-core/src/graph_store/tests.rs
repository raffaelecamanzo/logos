//! Behaviour tests for the graph store, organised by acceptance criterion
//! (S-005 / FR-DB-01..04, NFR-RA-07/09/10, NFR-DM-03).
//!
//! These are in-crate unit tests, so they may reach the private `conn` field to
//! drive *raw* SQL — the only way to attempt an out-of-range discriminant the
//! typed [`NewNode`] API forbids by construction, which is exactly what the
//! `CHECK`-constraint tests need to exercise.

use super::*;
use crate::model::{EdgeKind, LogosSymbol, NodeKind};

/// A fresh in-memory store, migrated and ready.
fn mem() -> SqliteGraphStore {
    SqliteGraphStore::open_in_memory().expect("in-memory store opens")
}

/// Insert a node named `name` under a fresh distinct local symbol, returning
/// its id. `n` keys the symbol so callers get distinct symbols cheaply.
fn seed(store: &SqliteGraphStore, n: u32, name: &str, kind: NodeKind) -> NodeId {
    let sym = LogosSymbol::parse(&format!("local {n}")).expect("local symbol parses");
    let symbol_id = store.upsert_symbol(&sym).expect("symbol upserts");
    store
        .insert_node(&NewNode::plain(symbol_id, kind, name))
        .expect("node inserts")
}

fn node_count(store: &SqliteGraphStore) -> i64 {
    store
        .conn
        .query_row("SELECT count(*) FROM nodes", [], |r| r.get(0))
        .unwrap()
}

fn edge_count(store: &SqliteGraphStore) -> i64 {
    store
        .conn
        .query_row("SELECT count(*) FROM edges", [], |r| r.get(0))
        .unwrap()
}

// ── FR-DB-01: kind CHECK constraints (15 node / 10 edge) ─────────────────────

#[test]
fn every_valid_node_kind_discriminant_is_accepted() {
    let store = mem();
    for (i, kind) in NodeKind::ALL.iter().enumerate() {
        seed(&store, i as u32, kind.as_str(), *kind);
    }
    assert_eq!(node_count(&store), NodeKind::ALL.len() as i64);
}

#[test]
fn out_of_range_node_kind_discriminants_are_rejected_by_check() {
    let store = mem();
    let sym = store
        .upsert_symbol(&LogosSymbol::parse("local 0").unwrap())
        .unwrap();
    // 0 (the deliberately-never-valid zero) and 35 (one past the taxonomy:
    // migration 8 widened to the CR-003 documentation kinds 18..=22 and
    // migration 13 to the CR-010 config kinds 23..=34, so 35 is the first
    // out-of-range discriminant).
    for bad in [0_i64, 35, -1, 99] {
        let res = store.conn.execute(
            "INSERT INTO nodes (symbol_id, kind, name) VALUES (?1, ?2, 'x')",
            rusqlite::params![sym, bad],
        );
        assert!(
            res.is_err(),
            "node kind {bad} must violate the CHECK constraint"
        );
    }
    assert_eq!(node_count(&store), 0);
}

#[test]
fn every_valid_edge_kind_discriminant_is_accepted() {
    let store = mem();
    // 11 nodes so each of the 10 edge kinds gets a distinct (source,target).
    let ids: Vec<NodeId> = (0..=EdgeKind::ALL.len() as u32)
        .map(|i| seed(&store, i, &format!("n{i}"), NodeKind::Function))
        .collect();
    for (i, kind) in EdgeKind::ALL.iter().enumerate() {
        store
            .insert_edge(ids[0], ids[i + 1], *kind)
            .expect("valid edge inserts");
    }
    assert_eq!(edge_count(&store), EdgeKind::ALL.len() as i64);
}

#[test]
fn out_of_range_edge_kind_discriminants_are_rejected_by_check() {
    let store = mem();
    let a = seed(&store, 0, "a", NodeKind::Function);
    let b = seed(&store, 1, "b", NodeKind::Function);
    // 16 is one past the taxonomy: the doc edges 11/12 (doc_reference,
    // traces_to) were added by migration 8, the member-access `accesses` edge 13
    // by migration 10 (CR-005), and the artifact edges 14/15 (artifact_ref,
    // artifact_binding) by migration 14 (CR-011), so 15 is now valid and 16 is
    // the first out-of-range discriminant.
    for bad in [0_i64, 16, -1] {
        let res = store.conn.execute(
            "INSERT INTO edges (source, target, kind) VALUES (?1, ?2, ?3)",
            rusqlite::params![a.get(), b.get(), bad],
        );
        assert!(
            res.is_err(),
            "edge kind {bad} must violate the CHECK constraint"
        );
    }
    assert_eq!(edge_count(&store), 0);
}

// ── FR-DB-01: (source, target, kind) uniqueness rule ─────────────────────────

#[test]
fn duplicate_source_target_kind_edge_is_rejected() {
    let store = mem();
    let a = seed(&store, 0, "a", NodeKind::Function);
    let b = seed(&store, 1, "b", NodeKind::Function);
    store
        .insert_edge(a, b, EdgeKind::Calls)
        .expect("first edge inserts");
    let dup = store.insert_edge(a, b, EdgeKind::Calls);
    assert!(
        dup.is_err(),
        "duplicate (source,target,kind) must be rejected"
    );
    assert_eq!(edge_count(&store), 1);
}

#[test]
fn same_endpoints_different_kind_is_allowed() {
    let store = mem();
    let a = seed(&store, 0, "a", NodeKind::Function);
    let b = seed(&store, 1, "b", NodeKind::Function);
    // The uniqueness key includes `kind`, so two different relationships
    // between the same pair coexist.
    store.insert_edge(a, b, EdgeKind::Calls).unwrap();
    store.insert_edge(a, b, EdgeKind::References).unwrap();
    assert_eq!(edge_count(&store), 2);
}

// ── FR-DB-02 / NFR-RA-10: per-connection pragma contract ─────────────────────

#[test]
fn in_memory_connection_honours_the_pragma_contract() {
    let store = mem();
    store
        .verify_connection_contract()
        .expect("contract holds in memory");
}

#[test]
fn on_disk_connection_sets_wal_foreign_keys_and_synchronous() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteGraphStore::open(dir.path().join("logos.db")).unwrap();

    let journal: String = store
        .conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    let fk: i64 = store
        .conn
        .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
        .unwrap();
    let sync: i64 = store
        .conn
        .query_row("PRAGMA synchronous", [], |r| r.get(0))
        .unwrap();

    assert_eq!(
        journal, "wal",
        "on-disk journal_mode must be WAL (FR-DB-02)"
    );
    assert_eq!(fk, 1, "foreign_keys must be ON (the ADR-05 footgun)");
    assert_eq!(sync, 1, "synchronous must be NORMAL");
    store.verify_connection_contract().unwrap();
}

// ── FR-DB-02 / CR-057: writer bulk-load pragmas ──────────────────────────────

#[test]
fn on_disk_writer_connection_sets_bulk_load_pragmas() {
    let dir = tempfile::tempdir().unwrap();
    // `open` is the writer connection's configuration path (`from_connection` →
    // `configure_connection`), the same one `Runtime`'s single-writer actor
    // opens through — so inspecting it proves the live writer's contract.
    let store = SqliteGraphStore::open(dir.path().join("logos.db")).unwrap();

    let cache_size: i64 = store
        .conn
        .query_row("PRAGMA cache_size", [], |r| r.get(0))
        .unwrap();
    let mmap_size: i64 = store
        .conn
        .query_row("PRAGMA mmap_size", [], |r| r.get(0))
        .unwrap();
    let temp_store: i64 = store
        .conn
        .query_row("PRAGMA temp_store", [], |r| r.get(0))
        .unwrap();

    assert_eq!(
        cache_size, WRITER_CACHE_SIZE_KIB,
        "writer cache_size must be the 64 MiB bulk-load value (FR-DB-02, CR-057)"
    );
    assert_eq!(
        mmap_size, WRITER_MMAP_SIZE_BYTES,
        "writer mmap_size must be the 256 MiB memory-map window (FR-DB-02, CR-057)"
    );
    assert_eq!(
        temp_store, TEMP_STORE_MEMORY,
        "writer temp_store must be MEMORY (FR-DB-02, CR-057)"
    );
    // The full contract check (base + bulk-load) agrees.
    store.verify_connection_contract().unwrap();
}

#[test]
fn verify_connection_contract_rejects_a_connection_missing_the_bulk_pragmas() {
    // The new bulk-load guard branches exist to fail loudly: a connection carrying
    // only the base pragmas (no CR-057 cache_size/temp_store/mmap_size) must be
    // reported out of contract, naming the missing pragma.
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;\n\
         PRAGMA synchronous = NORMAL;",
    )
    .unwrap();
    let store = SqliteGraphStore { conn };

    let err = store
        .verify_connection_contract()
        .expect_err("a connection without the bulk-load pragmas is out of contract");
    assert!(
        err.to_string().contains("cache_size"),
        "the error must name the missing bulk-load pragma (FR-DB-02, CR-057), got: {err}"
    );
}

#[test]
fn readonly_connection_omits_the_writer_only_bulk_load_pragmas() {
    // The bulk-load pragmas are writer-only ([FR-DB-02], CR-057): a read-only
    // connection keeps SQLite's defaults. Replicating the heap-backed cache_size
    // across the N-connection reader pool is exactly the cost the NFR-PE-06 guard
    // avoids, and a read-side mmap window was measured (S-225 benchmark) to give
    // no pass-2–5 read-time win, so neither is applied to readers.
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("logos.db");
    // The writer must open (and migrate) before a reader can attach.
    let _writer = SqliteGraphStore::open(&db).unwrap();
    let reader = SqliteGraphStore::open_readonly(&db).unwrap();

    let reader_cache: i64 = reader
        .conn
        .query_row("PRAGMA cache_size", [], |r| r.get(0))
        .unwrap();
    assert_ne!(
        reader_cache, WRITER_CACHE_SIZE_KIB,
        "readers must not each carry the writer's 64 MiB cache (NFR-PE-06 guard)"
    );

    let reader_mmap: i64 = reader
        .conn
        .query_row("PRAGMA mmap_size", [], |r| r.get(0))
        .unwrap();
    assert_ne!(
        reader_mmap, WRITER_MMAP_SIZE_BYTES,
        "the bulk-load mmap window is writer-only (FR-DB-02); readers keep the default"
    );
}

#[test]
fn foreign_keys_are_actually_enforced() {
    let store = mem();
    // symbol_id 9999 does not exist → FK violation, proving foreign_keys=ON is
    // not merely declared but enforced on this connection.
    let res = store.insert_node(&NewNode::plain(9999, NodeKind::Function, "orphan"));
    assert!(
        res.is_err(),
        "dangling FK must be rejected with foreign_keys ON"
    );
}

// ── FR-DB-03 / NFR-RA-09: FTS5 external-content sync + integrity ─────────────

#[test]
fn search_returns_inserted_nodes_ranked() {
    let store = mem();
    seed(&store, 0, "foo_handler", NodeKind::Function);
    seed(&store, 1, "bar", NodeKind::Function);

    let hits = store.search("foo", None, 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].name, "foo_handler");

    // No match → empty, not error.
    assert!(store.search("nonesuch", None, 10).unwrap().is_empty());
}

#[test]
fn search_matches_a_hyphenated_name_via_its_phrase_form() {
    // Regression: names like `web-surface` (a crate node) FTS-tokenise to
    // [web, surface]. A bare-word `surface` query matches them, but the raw
    // `web-surface` query is mis-parsed by FTS5's grammar (the `-`), yielding
    // nothing. The navigate layer now phrase-quotes raw user text — the form
    // asserted here — so the whole token resolves to its node again.
    let store = mem();
    seed(&store, 0, "web-surface", NodeKind::Module);
    seed(&store, 1, "cli-surface", NodeKind::Module);

    // The bare token still finds both (unchanged behaviour).
    assert_eq!(store.search("surface", None, 10).unwrap().len(), 2);

    // The phrase form (what `navigate::phrase_query` builds) pins the one node,
    // matching [web, surface] adjacently rather than erroring on the `-`.
    let hits = store.search("\"web-surface\"", None, 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].name, "web-surface");
}

#[test]
fn search_can_filter_by_kind() {
    let store = mem();
    seed(&store, 0, "widget", NodeKind::Function);
    seed(&store, 1, "widget", NodeKind::Struct);

    let funcs = store
        .search("widget", Some(NodeKind::Function), 10)
        .unwrap();
    assert_eq!(funcs.len(), 1);
    assert_eq!(funcs[0].kind, NodeKind::Function);

    let all = store.search("widget", None, 10).unwrap();
    assert_eq!(all.len(), 2);
}

#[test]
fn fts_stays_consistent_across_insert_update_delete() {
    let store = mem();
    let a = seed(&store, 0, "alpha", NodeKind::Function);
    seed(&store, 1, "beta", NodeKind::Function);

    // Integrity holds after inserts.
    store
        .fts_integrity_check()
        .expect("consistent after inserts");
    assert_eq!(store.search("alpha", None, 10).unwrap().len(), 1);

    // UPDATE fires nodes_fts_au (the 'delete' row + re-index).
    store
        .conn
        .execute("UPDATE nodes SET name = 'gamma' WHERE name = 'beta'", [])
        .unwrap();
    store
        .fts_integrity_check()
        .expect("consistent after update");
    assert!(store.search("beta", None, 10).unwrap().is_empty());
    assert_eq!(store.search("gamma", None, 10).unwrap().len(), 1);

    // DELETE fires nodes_fts_ad (the 'delete' row).
    assert_eq!(store.delete_node(a).unwrap(), 1);
    store
        .fts_integrity_check()
        .expect("consistent after delete");
    assert!(
        store.search("alpha", None, 10).unwrap().is_empty(),
        "the 'delete' trigger row must retract the deleted name (NFR-RA-09)"
    );
}

// ── FR-DB-04 / NFR-MA-06: forward-only migrations apply on open ──────────────

#[test]
fn fresh_database_applies_all_migrations_and_records_them() {
    let store = mem();
    assert_eq!(
        store.schema_version().unwrap(),
        16,
        "v16 = migration 16 (S-201 CR-052 UNIQUE(symbol_id) dedup rebuild)"
    );

    let recorded: i64 = store
        .conn
        .query_row("SELECT count(*) FROM schema_versions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        recorded, 16,
        "schema_versions records every applied migration"
    );
}

#[test]
fn reopening_an_up_to_date_database_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("logos.db");
    {
        let store = SqliteGraphStore::open(&path).unwrap();
        assert_eq!(store.schema_version().unwrap(), 16);
    }
    // Reopen: migrations must NOT re-apply (no duplicate schema_versions rows).
    let store = SqliteGraphStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), 16);
    let rows: i64 = store
        .conn
        .query_row("SELECT count(*) FROM schema_versions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 16, "migrations must not re-apply on reopen");
}

// ── NFR-RA-07: an interrupted write batch rolls back atomically ──────────────

#[test]
fn failed_write_batch_rolls_back_wholesale() {
    let mut store = mem();
    let a = seed(&store, 0, "a", NodeKind::Function);
    let b = seed(&store, 1, "b", NodeKind::Function);
    let before = (node_count(&store), edge_count(&store));

    // A batch that inserts a valid edge, then a duplicate that violates the
    // uniqueness rule — simulating an interrupted/failed sync batch.
    let result = store.write_batch(|w| {
        w.insert_edge(a, b, EdgeKind::Calls)?;
        let new_sym = w.upsert_symbol(&LogosSymbol::parse("local 99").unwrap())?;
        w.insert_node(&NewNode::plain(new_sym, NodeKind::Struct, "should_vanish"))?;
        w.insert_edge(a, b, EdgeKind::Calls)?; // duplicate → Err, aborts the batch
        Ok(())
    });

    assert!(result.is_err(), "the batch must fail on the duplicate edge");
    assert_eq!(
        (node_count(&store), edge_count(&store)),
        before,
        "no partial state may survive a failed batch (NFR-RA-07)"
    );
    assert!(
        store.search("should_vanish", None, 10).unwrap().is_empty(),
        "the rolled-back node must not linger in the FTS index either"
    );
}

#[test]
fn successful_write_batch_commits_atomically() {
    let mut store = mem();
    let ids = store
        .write_batch(|w| {
            let s0 = w.upsert_symbol(&LogosSymbol::parse("local 0").unwrap())?;
            let s1 = w.upsert_symbol(&LogosSymbol::parse("local 1").unwrap())?;
            let a = w.insert_node(&NewNode::plain(s0, NodeKind::Function, "caller"))?;
            let b = w.insert_node(&NewNode::plain(s1, NodeKind::Function, "callee"))?;
            w.insert_edge(a, b, EdgeKind::Calls)?;
            Ok((a, b))
        })
        .expect("batch commits");
    assert_eq!(node_count(&store), 2);
    assert_eq!(store.callees(ids.0).unwrap().len(), 1);
}

// ── NFR-DM-03: the database is a single copyable file ────────────────────────

#[test]
fn database_file_is_copyable_and_reopens_intact() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("logos.db");
    {
        let store = SqliteGraphStore::open(&original).unwrap();
        seed(&store, 0, "portable", NodeKind::Function);
        store.checkpoint().expect("WAL folds into the single file");
    } // drop closes the connection, finalising the file

    // Copy ONLY the .db file (NFR-DM-03: no WAL shard split needed).
    let copy = dir.path().join("seed-copy.db");
    std::fs::copy(&original, &copy).unwrap();

    let reopened = SqliteGraphStore::open(&copy).unwrap();
    assert_eq!(reopened.schema_version().unwrap(), 16);
    let hits = reopened.search("portable", None, 10).unwrap();
    assert_eq!(hits.len(), 1, "all data must survive a plain file copy");
    assert_eq!(hits[0].name, "portable");
}

// ── Point queries: node / callers / callees ──────────────────────────────────

#[test]
fn node_point_query_round_trips_model_types() {
    let store = mem();
    let file = store.insert_file("src/lib.rs", Some("rust"), None).unwrap();
    let sym = LogosSymbol::parse("local 7").unwrap();
    let symbol_id = store.upsert_symbol(&sym).unwrap();
    let id = store
        .insert_node(&NewNode {
            file_id: Some(file),
            start_line: Some(10),
            end_line: Some(20),
            ..NewNode::plain(symbol_id, NodeKind::Method, "compute")
        })
        .unwrap();

    let got = store.node(id).unwrap().expect("node present");
    assert_eq!(got.id, id);
    assert_eq!(got.kind, NodeKind::Method);
    assert_eq!(got.name, "compute");
    assert_eq!(got.symbol, sym);
    assert_eq!(got.file_path.as_deref(), Some("src/lib.rs"));

    // Absent id → Ok(None), never an error.
    assert!(store.node(NodeId(999)).unwrap().is_none());
}

#[test]
fn callers_and_callees_follow_calls_edges_only() {
    let store = mem();
    let a = seed(&store, 0, "a", NodeKind::Function);
    let b = seed(&store, 1, "b", NodeKind::Function);
    let c = seed(&store, 2, "c", NodeKind::Function);

    // a -> b (calls), c -> b (calls), a -> b (contains, must be ignored).
    store.insert_edge(a, b, EdgeKind::Calls).unwrap();
    store.insert_edge(c, b, EdgeKind::Calls).unwrap();
    store.insert_edge(a, b, EdgeKind::Contains).unwrap();

    let callers: Vec<NodeId> = store
        .callers(b)
        .unwrap()
        .into_iter()
        .map(|n| n.id)
        .collect();
    assert_eq!(
        callers,
        vec![a, c],
        "callers of b are a and c via calls edges"
    );

    let callees: Vec<NodeId> = store
        .callees(a)
        .unwrap()
        .into_iter()
        .map(|n| n.id)
        .collect();
    assert_eq!(callees, vec![b], "a only calls b (contains edge excluded)");

    assert!(store.callees(b).unwrap().is_empty(), "b calls nobody");
}

// ── Integrity: stored symbols upsert idempotently ────────────────────────────

#[test]
fn upsert_symbol_is_idempotent() {
    let store = mem();
    let sym = LogosSymbol::parse("local 0").unwrap();
    let first = store.upsert_symbol(&sym).unwrap();
    let second = store.upsert_symbol(&sym).unwrap();
    assert_eq!(first, second, "re-upserting a symbol returns the same id");
    let count: i64 = store
        .conn
        .query_row("SELECT count(*) FROM symbols", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

// ── Integrity: node insertion upserts idempotently on symbol_id (S-202, CR-052) ─

#[test]
fn insert_node_returns_correct_id_on_insert_and_conflict_update_paths() {
    let store = mem();
    let sym = LogosSymbol::parse("local 0").unwrap();
    let symbol_id = store.upsert_symbol(&sym).unwrap();

    // Insert path: a fresh row, its id is whatever SQLite assigns.
    let inserted = store
        .insert_node(&NewNode::plain(symbol_id, NodeKind::Function, "first_name"))
        .unwrap();

    // Conflict-update path: same symbol_id, different fields — must fold into
    // the same row and read back the *same* id, not a fresh one from
    // `last_insert_rowid()` (unreliable on DO UPDATE).
    let updated = store
        .insert_node(&NewNode::plain(
            symbol_id,
            NodeKind::Method,
            "second_name",
        ))
        .unwrap();

    assert_eq!(
        inserted, updated,
        "the conflict-update path must read back the original row's id"
    );

    let got = store.node(updated).unwrap().expect("node present");
    assert_eq!(got.kind, NodeKind::Method, "the update applied");
    assert_eq!(got.name, "second_name", "the update applied");
}

#[test]
fn insert_node_folds_same_symbol_to_a_single_row() {
    let store = mem();
    let sym = LogosSymbol::parse("local 0").unwrap();
    let symbol_id = store.upsert_symbol(&sym).unwrap();

    store
        .insert_node(&NewNode::plain(symbol_id, NodeKind::Function, "a"))
        .unwrap();
    store
        .insert_node(&NewNode::plain(symbol_id, NodeKind::Function, "b"))
        .unwrap();

    assert_eq!(
        node_count(&store),
        1,
        "two inserts for one symbol_id must fold to a single row, not \
         duplicate or error against UNIQUE(symbol_id)"
    );
}

#[test]
fn resyncing_an_unchanged_symbol_holds_node_count_steady() {
    let store = mem();
    let sym = LogosSymbol::parse("local 0").unwrap();
    let symbol_id = store.upsert_symbol(&sym).unwrap();
    let node = NewNode::plain(symbol_id, NodeKind::Function, "steady");

    let first = store.insert_node(&node).unwrap();
    for _ in 0..3 {
        let again = store.insert_node(&node).unwrap();
        assert_eq!(again, first, "re-syncing the same node keeps its id stable");
        assert_eq!(
            node_count(&store),
            1,
            "re-syncing an unchanged node must never accumulate rows"
        );
    }
}

#[test]
fn insert_node_upsert_preserves_prior_annotations() {
    let mut store = mem();
    let id = seed(&store, 0, "annotated", NodeKind::Function);
    let symbol_id = store
        .conn
        .query_row("SELECT symbol_id FROM nodes WHERE id = ?1", [id.get()], |r| {
            r.get(0)
        })
        .unwrap();

    store
        .write_batch(|w| {
            w.set_node_annotations(id, Some(true), Some(false), true, Some("domain"), None)
        })
        .unwrap();

    // The conflict-update path must touch only NewNode's extraction-owned
    // columns; a prior annotation pass's verdict must survive untouched.
    let updated = store
        .insert_node(&NewNode::plain(symbol_id, NodeKind::Method, "renamed"))
        .unwrap();
    assert_eq!(updated, id, "the upsert folds into the annotated row");

    let (dead, dup, test, layer): (Option<i64>, Option<i64>, i64, Option<String>) = store
        .conn
        .query_row(
            "SELECT is_dead, is_duplicate, is_test, layer_membership \
             FROM annotations WHERE node_id = ?1",
            [id.get()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        (dead, dup, test, layer.as_deref()),
        (Some(1), Some(0), 1, Some("domain")),
        "annotation columns must survive an insert_node upsert unchanged"
    );

    let got = store.node(updated).unwrap().expect("node present");
    assert_eq!(got.kind, NodeKind::Method, "the extraction fields did update");
    assert_eq!(got.name, "renamed");
}

#[test]
fn batch_writer_insert_node_upserts_within_a_transaction() {
    let mut store = mem();
    let sym = LogosSymbol::parse("local 0").unwrap();

    let (first, second) = store
        .write_batch(|w| {
            let symbol_id = w.upsert_symbol(&sym)?;
            let first = w.insert_node(&NewNode::plain(symbol_id, NodeKind::Function, "a"))?;
            let second = w.insert_node(&NewNode::plain(symbol_id, NodeKind::Function, "b"))?;
            Ok((first, second))
        })
        .unwrap();

    assert_eq!(
        first, second,
        "two BatchWriter::insert_node calls for one symbol_id within the same \
         transaction must read back the same id"
    );
    assert_eq!(
        node_count(&store),
        1,
        "the upsert must fold to a single row inside an open write_batch, not \
         just at the top-level SqliteGraphStore::insert_node entry point"
    );
}

#[test]
fn insert_node_upsert_keeps_fts_consistent() {
    let store = mem();
    let sym = LogosSymbol::parse("local 0").unwrap();
    let symbol_id = store.upsert_symbol(&sym).unwrap();

    store
        .insert_node(&NewNode::plain(symbol_id, NodeKind::Function, "old_name"))
        .unwrap();
    assert_eq!(store.search("old_name", None, 10).unwrap().len(), 1);

    // The conflict-update path — not a raw `UPDATE` — must still fire the
    // `nodes_fts_au` trigger so the FTS postings retract the old name and pick
    // up the new one (NFR-RA-09).
    store
        .insert_node(&NewNode::plain(symbol_id, NodeKind::Function, "new_name"))
        .unwrap();

    store.fts_integrity_check().unwrap();
    assert!(
        store.search("old_name", None, 10).unwrap().is_empty(),
        "the old FTS posting must be retracted on upsert"
    );
    assert_eq!(
        store.search("new_name", None, 10).unwrap().len(),
        1,
        "the new FTS posting must be present on upsert"
    );
}

// ── Should-fix follow-ups (review S2/S4 + public-API negative paths) ──────────

#[test]
fn deleting_a_node_cascades_to_its_edges() {
    let store = mem();
    let a = seed(&store, 0, "a", NodeKind::Function);
    let b = seed(&store, 1, "b", NodeKind::Function);
    store.insert_edge(a, b, EdgeKind::Calls).unwrap();
    store.insert_edge(b, a, EdgeKind::References).unwrap();
    assert_eq!(edge_count(&store), 2);

    // ON DELETE CASCADE on both endpoints removes every incident edge, so no
    // stale edge can dangle to a deleted node and break callers/callees.
    store.delete_node(a).unwrap();
    assert_eq!(
        edge_count(&store),
        0,
        "edges incident to a deleted node must cascade away"
    );
}

#[test]
fn duplicate_file_path_is_rejected() {
    let store = mem();
    store.insert_file("src/lib.rs", None, None).unwrap();
    let dup = store.insert_file("src/lib.rs", Some("rust"), None);
    assert!(
        dup.is_err(),
        "files.path is UNIQUE — a duplicate must be rejected"
    );
}

#[test]
fn deleting_a_missing_node_removes_zero_rows() {
    let store = mem();
    assert_eq!(
        store.delete_node(NodeId(9999)).unwrap(),
        0,
        "deleting a non-existent node is a no-op returning 0"
    );
}

#[test]
fn empty_search_query_returns_no_rows() {
    let store = mem();
    seed(&store, 0, "alpha", NodeKind::Function);
    // An empty or whitespace query is a defined no-op, not an opaque FTS5 error.
    assert!(store.search("", None, 10).unwrap().is_empty());
    assert!(store.search("   ", None, 10).unwrap().is_empty());
}

// ── ADR-02: the read-only WAL connection (open_readonly) ─────────────────────

#[test]
fn open_readonly_sees_writer_committed_data_and_rejects_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("logos.db");

    // The writer creates + migrates the file and commits a node.
    let writer = SqliteGraphStore::open(&path).expect("writer opens");
    seed(&writer, 0, "ro_visible", NodeKind::Function);

    // A read-only connection over the same file sees the committed row (WAL).
    let reader = SqliteGraphStore::open_readonly(&path).expect("read-only opens");
    assert_eq!(
        reader.search("ro_visible", None, 10).unwrap().len(),
        1,
        "the read-only connection sees the writer's committed data"
    );

    // query_only = ON makes the read-only contract structural: a write fails.
    let blocked = reader.insert_file("src/should_fail.rs", Some("rust"), None);
    assert!(
        blocked.is_err(),
        "a write through a read-only (query_only) connection must be rejected"
    );
}

#[test]
fn open_readonly_rejects_an_absent_database() {
    let dir = tempfile::tempdir().unwrap();
    // READ_ONLY never creates the file, so opening a path with no database errors
    // rather than silently materializing an empty, unmigrated store.
    let missing = dir.path().join("nope.db");
    assert!(
        SqliteGraphStore::open_readonly(&missing).is_err(),
        "opening a non-existent database read-only must fail"
    );
}

// ── Whole-graph enumeration (S-009 hydration read seam) ──────────────────────

#[test]
fn all_nodes_returns_every_node_in_id_order() {
    let store = mem();
    let a = seed(&store, 0, "alpha", NodeKind::Module);
    let b = seed(&store, 1, "beta", NodeKind::Function);
    let c = seed(&store, 2, "gamma", NodeKind::Struct);

    let nodes = store.all_nodes().expect("all_nodes succeeds");
    let ids: Vec<NodeId> = nodes.iter().map(|n| n.id).collect();
    assert_eq!(ids, vec![a, b, c], "nodes stream in ascending id order");
    // The kind/name round-trip through the model conversion.
    assert_eq!(nodes[0].kind, NodeKind::Module);
    assert_eq!(nodes[1].name, "beta");
}

#[test]
fn all_edges_returns_every_edge_in_canonical_order() {
    let store = mem();
    let a = seed(&store, 0, "a", NodeKind::Function);
    let b = seed(&store, 1, "b", NodeKind::Function);
    let c = seed(&store, 2, "c", NodeKind::Module);
    // Insert out of canonical order to prove the query sorts, not the caller.
    store.insert_edge(c, a, EdgeKind::Contains).unwrap();
    store.insert_edge(a, b, EdgeKind::Calls).unwrap();
    store.insert_edge(a, b, EdgeKind::References).unwrap();

    let edges = store.all_edges().expect("all_edges succeeds");
    // Ordered by (source, target, kind): (a,b,Calls=2), (a,b,References=4), (c,a,Contains=1).
    assert_eq!(
        edges,
        vec![
            EdgeRow {
                source: a,
                target: b,
                kind: EdgeKind::Calls
            },
            EdgeRow {
                source: a,
                target: b,
                kind: EdgeKind::References
            },
            EdgeRow {
                source: c,
                target: a,
                kind: EdgeKind::Contains
            },
        ]
    );
}

#[test]
fn all_nodes_and_all_edges_are_empty_on_a_fresh_store() {
    let store = mem();
    assert!(store.all_nodes().unwrap().is_empty());
    assert!(store.all_edges().unwrap().is_empty());
}

/// `proto_service_bodies` (S-253 provider enrichment) returns the `(symbol, body)`
/// of exactly the `ProtoService` nodes carrying rpc-method body text — the narrow
/// read the federation bridge expands into per-method gRPC provider keys. A
/// body-less service and a body-carrying non-`ProtoService` node are both excluded.
#[test]
fn proto_service_bodies_returns_only_proto_services_with_a_body() {
    let store = mem();
    // A ProtoService with an rpc-method body — the one row expected back.
    let svc = LogosSymbol::parse("local svc").unwrap();
    let svc_id = store.upsert_symbol(&svc).unwrap();
    store
        .insert_node(&NewNode {
            body: Some("GetUser\nListUsers"),
            ..NewNode::plain(svc_id, NodeKind::ProtoService, "example.v1.UserService")
        })
        .unwrap();
    // A ProtoService with no body (a method-less service) — excluded by IS NOT NULL.
    seed(&store, 1, "example.v1.Empty", NodeKind::ProtoService);
    // A body-carrying node of another kind (a DocSection) — excluded by the kind filter.
    let doc = LogosSymbol::parse("local doc").unwrap();
    let doc_id = store.upsert_symbol(&doc).unwrap();
    store
        .insert_node(&NewNode {
            body: Some("some prose"),
            ..NewNode::plain(doc_id, NodeKind::DocSection, "Heading")
        })
        .unwrap();

    let bodies = store
        .proto_service_bodies()
        .expect("proto_service_bodies succeeds");
    assert_eq!(bodies.len(), 1, "only the ProtoService carrying a body is returned");
    assert_eq!(bodies[0].0.as_str(), "local svc");
    assert_eq!(bodies[0].1, "GetUser\nListUsers");
}

// ── S-024-HF: the incremental framework-gate footprint probe ─────────────────

#[test]
fn framework_footprint_is_false_for_a_plain_graph() {
    let mut store = mem();
    // A plain library: an ordinary node and an ordinary (non-detector) ref.
    seed(&store, 0, "helper", NodeKind::Function);
    store
        .write_batch(|w| w.insert_unresolved_ref(&path_ref(None, "local src", "crate::b::helper")))
        .unwrap();
    assert!(
        !store
            .has_framework_footprint(&["axum".to_string(), "actix_web".to_string()])
            .unwrap(),
        "no promoted node and no detector ref ⇒ no footprint (the sync skip case)"
    );
    // No detectors declared at all is also footprint-free with no promoted node.
    assert!(!store.has_framework_footprint(&[]).unwrap());
}

#[test]
fn framework_footprint_is_true_when_a_promoted_node_exists() {
    // A route node forces the full reconcile (a demotion may be due) even with
    // no detector list supplied.
    let store = mem();
    seed(&store, 0, "GET /users", NodeKind::Route);
    assert!(store.has_framework_footprint(&[]).unwrap());

    let store2 = mem();
    seed(&store2, 0, "AppState", NodeKind::Component);
    assert!(store2.has_framework_footprint(&[]).unwrap());
}

#[test]
fn framework_footprint_matches_an_unresolved_detector_ref_by_whole_segment() {
    let detectors = ["axum".to_string()];

    // An exact detector target and one extending it by a whole `::` segment are
    // both the candidacy fingerprint (mirroring `matches_detector`).
    for target in ["axum", "axum::routing::get"] {
        let mut store = mem();
        store
            .write_batch(|w| w.insert_unresolved_ref(&path_ref(None, "local src", target)))
            .unwrap();
        assert!(
            store.has_framework_footprint(&detectors).unwrap(),
            "an unresolved `{target}` ref is a framework footprint"
        );
    }

    // A look-alike sharing only a substring (not a whole `::` segment) is not a
    // detector — `axumish` is never under `axum`.
    let mut store = mem();
    store
        .write_batch(|w| w.insert_unresolved_ref(&path_ref(None, "local src", "axumish::route")))
        .unwrap();
    assert!(
        !store.has_framework_footprint(&detectors).unwrap(),
        "`axumish::route` shares only a substring prefix — not a detector"
    );
}

#[test]
fn framework_footprint_ignores_a_resolved_detector_ref() {
    // A detector names an external package that never binds, so the candidacy
    // fingerprint is the *unresolved* tail (the `resolved = 0` scope): a row that
    // somehow bound is outside it.
    let mut store = mem();
    store
        .write_batch(|w| {
            w.insert_unresolved_ref(&path_ref(None, "local src", "axum::routing::get"))?;
            w.mark_ref_resolved(1, true)
        })
        .unwrap();
    assert!(
        !store
            .has_framework_footprint(&["axum".to_string()])
            .unwrap(),
        "a resolved ref is outside the unresolved candidacy fingerprint"
    );
}

// ── S-011 / FR-RS-03 / ADR-10: the unresolved_refs reference ledger ──────────

/// A NewUnresolvedRef with the common defaults for ledger tests.
fn path_ref<'a>(
    file_id: Option<i64>,
    source_symbol: &'a str,
    target: &'a str,
) -> NewUnresolvedRef<'a> {
    NewUnresolvedRef {
        file_id,
        source_symbol,
        target,
        alias: None,
        form: RefForm::Path,
        kind: EdgeKind::Calls,
        line: Some(3),
        payload: None,
    }
}

#[test]
fn upgrading_a_v1_database_applies_migration_two_forward_only() {
    // Build a genuine v1 database: apply ONLY migration 1, exactly as the
    // runner would have at the time v1 shipped (NFR-MA-06 forward-only).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("logos.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        let (v1, sql) = schema::MIGRATIONS[0];
        conn.execute_batch(sql).unwrap();
        conn.execute(
            "INSERT INTO schema_versions (version, applied_at) VALUES (?1, unixepoch())",
            [v1],
        )
        .unwrap();
        conn.pragma_update(None, "user_version", v1).unwrap();
    }

    // Opening through the store must upgrade v1 → latest without touching v1
    // data (the runner applies v2..v16 forward-only).
    let store = SqliteGraphStore::open(&path).unwrap();
    assert_eq!(
        store.schema_version().unwrap(),
        16,
        "v1 store upgrades to the latest version"
    );
    assert!(
        store.unresolved_refs().unwrap().is_empty(),
        "the ledger exists and is empty after the upgrade"
    );
}

#[test]
fn ledger_roundtrips_a_row_through_model_types() {
    let mut store = mem();
    let file_id = store.insert_file("src/a.rs", Some("rust"), None).unwrap();
    store
        .write_batch(|w| {
            w.insert_unresolved_ref(&NewUnresolvedRef {
                file_id: Some(file_id),
                source_symbol: "logos . . . src/a.rs/caller().",
                target: "crate::b::helper",
                alias: Some("helper"),
                form: RefForm::Path,
                kind: EdgeKind::Imports,
                line: Some(7),
                payload: None,
            })
        })
        .unwrap();

    let rows = store.unresolved_refs().unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.file_id, Some(file_id));
    assert_eq!(row.source_symbol, "logos . . . src/a.rs/caller().");
    assert_eq!(row.target, "crate::b::helper");
    assert_eq!(row.alias.as_deref(), Some("helper"));
    assert_eq!(row.form, RefForm::Path);
    assert_eq!(row.kind, EdgeKind::Imports);
    assert_eq!(row.line, Some(7));
    assert!(!row.resolved, "a fresh ref starts unresolved");
}

#[test]
fn ledger_insert_is_idempotent_over_the_uniqueness_rule() {
    let mut store = mem();
    store
        .write_batch(|w| {
            // The same (source, target, form, kind) twice — e.g. one function
            // calling the same path on two lines.
            w.insert_unresolved_ref(&path_ref(None, "local src", "helper"))?;
            w.insert_unresolved_ref(&NewUnresolvedRef {
                line: Some(9),
                ..path_ref(None, "local src", "helper")
            })?;
            // A different form of the same target is a distinct row.
            w.insert_unresolved_ref(&NewUnresolvedRef {
                form: RefForm::Method,
                ..path_ref(None, "local src", "helper")
            })
        })
        .unwrap();

    let rows = store.unresolved_refs().unwrap();
    assert_eq!(rows.len(), 2, "duplicate collapses; distinct form does not");
    assert_eq!(rows[0].line, Some(3), "the first row's line is kept");
}

#[test]
fn ledger_rows_replace_per_file_and_cascade_on_file_delete() {
    let mut store = mem();
    let a = store.insert_file("src/a.rs", Some("rust"), None).unwrap();
    let b = store.insert_file("src/b.rs", Some("rust"), None).unwrap();
    store
        .write_batch(|w| {
            w.insert_unresolved_ref(&path_ref(Some(a), "local a", "x"))?;
            w.insert_unresolved_ref(&path_ref(Some(b), "local b", "y"))
        })
        .unwrap();

    // Re-extract replace: file a's rows go, file b's stay.
    let removed = store
        .write_batch(|w| w.delete_unresolved_refs_for_file(a))
        .unwrap();
    assert_eq!(removed, 1);
    let rows = store.unresolved_refs().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].file_id, Some(b));

    // True file removal: the FK cascade clears the ledger too.
    store
        .write_batch(|w| {
            w.delete_nodes_for_file(b)?;
            w.delete_file(b)
        })
        .unwrap();
    assert!(store.unresolved_refs().unwrap().is_empty());
}

/// The live store → `resolve::coverage` path surfaces per-relation-class
/// bound/unresolved counts (CR-011, FR-CG-11, FR-RS-04) — the read `Engine::stats`
/// wraps to populate `StatsInfo.artifact_bindings`. Seeds artifact-ref ledger rows
/// carrying a relation payload (some resolved, some not) and asserts the grouped
/// `by_relation` map, exercising the column read-back and grouping end-to-end
/// against a real SQLite store rather than the pure helper.
#[test]
fn coverage_surfaces_per_relation_class_counts_from_the_ledger() {
    let mut store = mem();
    let f = store.insert_file("svc.proto", Some("proto"), None).unwrap();
    store
        .write_batch(|w| {
            // Two proto-import refs and one proto-type ref, plus a plain code ref.
            w.insert_unresolved_ref(&NewUnresolvedRef {
                file_id: Some(f),
                source_symbol: "cfg svc",
                target: "common.proto",
                alias: None,
                form: RefForm::Path,
                kind: EdgeKind::ArtifactRef,
                line: Some(1),
                payload: Some("proto-import"),
            })?;
            w.insert_unresolved_ref(&NewUnresolvedRef {
                file_id: Some(f),
                source_symbol: "cfg svc",
                target: "types.proto",
                alias: None,
                form: RefForm::Path,
                kind: EdgeKind::ArtifactRef,
                line: Some(2),
                payload: Some("proto-import"),
            })?;
            w.insert_unresolved_ref(&NewUnresolvedRef {
                file_id: Some(f),
                source_symbol: "cfg svc",
                target: "Common",
                alias: None,
                form: RefForm::Method,
                kind: EdgeKind::ArtifactRef,
                line: Some(3),
                payload: Some("proto-type"),
            })?;
            // A payloadless code ref must not appear in the artifact breakdown.
            w.insert_unresolved_ref(&path_ref(Some(f), "cfg svc", "helper"))
        })
        .unwrap();

    // Mark the first proto-import row resolved (id order is insertion order).
    let first_id = store.unresolved_refs().unwrap()[0].id;
    store
        .write_batch(|w| w.mark_ref_resolved(first_id, true))
        .unwrap();

    let cov = crate::resolve::coverage(&store).unwrap();
    assert_eq!(cov.by_relation.len(), 2, "two artifact relation classes");
    let imports = &cov.by_relation["proto-import"];
    assert_eq!(
        (imports.bound, imports.unresolved),
        (1, 1),
        "one proto-import bound, one still unresolved"
    );
    let types = &cov.by_relation["proto-type"];
    assert_eq!((types.bound, types.unresolved), (0, 1));
    assert!(
        !cov.by_relation.contains_key("calls"),
        "a payloadless code ref contributes nothing to the artifact breakdown"
    );
}

#[test]
fn ledger_resolved_state_is_recordable_and_readable() {
    let mut store = mem();
    store
        .write_batch(|w| w.insert_unresolved_ref(&path_ref(None, "local s", "t")))
        .unwrap();
    let id = store.unresolved_refs().unwrap()[0].id;

    store
        .write_batch(|w| w.mark_ref_resolved(id, true))
        .unwrap();
    assert!(store.unresolved_refs().unwrap()[0].resolved);

    // And back: a target deleted later flips the row to retry state.
    store
        .write_batch(|w| w.mark_ref_resolved(id, false))
        .unwrap();
    assert!(!store.unresolved_refs().unwrap()[0].resolved);
}

#[test]
fn insert_edge_if_absent_dedups_and_reports() {
    let mut store = mem();
    let a = seed(&store, 1, "a", NodeKind::Function);
    let b = seed(&store, 2, "b", NodeKind::Function);

    let (first, second) = store
        .write_batch(|w| {
            let first = w.insert_edge_if_absent(a, b, EdgeKind::Calls)?;
            let second = w.insert_edge_if_absent(a, b, EdgeKind::Calls)?;
            Ok((first, second))
        })
        .unwrap();
    assert!(first, "the first insert creates the edge");
    assert!(!second, "the duplicate is a quiet no-op (never an error)");
    assert_eq!(store.all_edges().unwrap().len(), 1);
}

#[test]
fn ledger_rejects_out_of_range_form_at_the_check_constraint() {
    // The typed API cannot express an invalid form; raw SQL proves the CHECK
    // is the on-disk backstop (defence in depth, FR-DB-01 pattern).
    let store = mem();
    let err = store.conn.execute(
        "INSERT INTO unresolved_refs (source_symbol, target, form, kind) \
         VALUES ('s', 't', 99, 2)",
        [],
    );
    assert!(err.is_err(), "form=99 must violate the CHECK constraint");
}

// ── S-013: the navigation read seam (FR-NV-04/05/07/09) ─────────────────────

/// Insert a node bound to a file row with line spans, for the navigation
/// point-query tests.
fn seed_located(
    store: &SqliteGraphStore,
    n: u32,
    name: &str,
    kind: NodeKind,
    file_id: i64,
    lines: (i64, i64),
) -> NodeId {
    let sym = LogosSymbol::parse(&format!("local {n}")).expect("local symbol parses");
    let symbol_id = store.upsert_symbol(&sym).expect("symbol upserts");
    store
        .insert_node(&NewNode {
            file_id: Some(file_id),
            start_line: Some(lines.0),
            end_line: Some(lines.1),
            ..NewNode::plain(symbol_id, kind, name)
        })
        .expect("node inserts")
}

#[test]
fn node_row_round_trips_line_spans_and_file() {
    let store = mem();
    let file_id = store.insert_file("src/lib.rs", Some("rust"), None).unwrap();
    let id = seed_located(&store, 1, "alpha", NodeKind::Function, file_id, (3, 9));

    let row = store.node(id).unwrap().expect("node exists");
    assert_eq!(row.start_line, Some(3));
    assert_eq!(row.end_line, Some(9));
    assert_eq!(row.file_path.as_deref(), Some("src/lib.rs"));
}

#[test]
fn node_by_symbol_finds_the_exact_canonical_string() {
    let store = mem();
    let id = seed(&store, 7, "alpha", NodeKind::Function);
    let row = store.node(id).unwrap().unwrap();

    let hit = store.node_by_symbol(row.symbol.as_str()).unwrap();
    assert_eq!(hit.expect("exact symbol resolves").id, id);
    assert!(store.node_by_symbol("local nope").unwrap().is_none());
}

#[test]
fn nodes_by_name_matches_exactly_and_orders_by_id() {
    let store = mem();
    let a = seed(&store, 1, "run", NodeKind::Function);
    let b = seed(&store, 2, "run", NodeKind::Method);
    seed(&store, 3, "runner", NodeKind::Function);

    let hits = store.nodes_by_name("run").unwrap();
    assert_eq!(
        hits.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![a, b],
        "exact name matches only, ascending id"
    );
    assert!(store.nodes_by_name("missing").unwrap().is_empty());
}

#[test]
fn neighbours_cover_every_edge_kind_with_direction() {
    let store = mem();
    let module = seed(&store, 1, "m", NodeKind::Module);
    let f = seed(&store, 2, "f", NodeKind::Function);
    let g = seed(&store, 3, "g", NodeKind::Function);
    store.insert_edge(module, f, EdgeKind::Contains).unwrap();
    store.insert_edge(f, g, EdgeKind::Calls).unwrap();
    store.insert_edge(g, f, EdgeKind::References).unwrap();

    let inbound = store.neighbours_in(f).unwrap();
    assert_eq!(
        inbound.iter().map(|(k, n)| (*k, n.id)).collect::<Vec<_>>(),
        vec![(EdgeKind::Contains, module), (EdgeKind::References, g)],
        "inbound spans all kinds, ordered by (kind, id)"
    );

    let outbound = store.neighbours_out(f).unwrap();
    assert_eq!(
        outbound.iter().map(|(k, n)| (*k, n.id)).collect::<Vec<_>>(),
        vec![(EdgeKind::Calls, g)]
    );
}

#[test]
fn counts_reflect_rows_in_every_table() {
    let store = mem();
    assert_eq!(store.counts().unwrap(), StoreCounts::default());

    let file_id = store.insert_file("src/lib.rs", Some("rust"), None).unwrap();
    let a = seed_located(&store, 1, "a", NodeKind::Function, file_id, (1, 2));
    let b = seed_located(&store, 2, "b", NodeKind::Function, file_id, (4, 5));
    store.insert_edge(a, b, EdgeKind::Calls).unwrap();

    let counts = store.counts().unwrap();
    assert_eq!(
        (counts.files, counts.nodes, counts.edges),
        (1, 2, 1),
        "graph tables counted"
    );
    assert_eq!((counts.refs_total, counts.refs_resolved), (0, 0));

    // The ledger split feeds the status coverage ratio (FR-NV-07): a row
    // marked resolved must move the resolved count, not just the total.
    let mut store = store;
    store
        .write_batch(|w| {
            w.insert_unresolved_ref(&NewUnresolvedRef {
                file_id: Some(file_id),
                source_symbol: "local 1",
                target: "b",
                alias: None,
                form: RefForm::Path,
                kind: EdgeKind::Calls,
                line: Some(1),
                payload: None,
            })
        })
        .unwrap();
    let counts = store.counts().unwrap();
    assert_eq!((counts.refs_total, counts.refs_resolved), (1, 0));

    let ledger_id = store.unresolved_refs().unwrap()[0].id;
    store
        .write_batch(|w| w.mark_ref_resolved(ledger_id, true))
        .unwrap();
    let counts = store.counts().unwrap();
    assert_eq!(
        (counts.refs_total, counts.refs_resolved),
        (1, 1),
        "the WHERE resolved = 1 subselect tracks the mark"
    );
}

// ── S-014 / FR-AN-01..04: annotation columns, snapshot read, derived clears ──

#[test]
fn annotation_payload_round_trips_through_insert_and_snapshot() {
    let store = mem();
    let sym = LogosSymbol::parse("local ann0").unwrap();
    let symbol_id = store.upsert_symbol(&sym).unwrap();
    let file = store.insert_file("src/a.rs", Some("rust"), None).unwrap();
    let id = store
        .insert_node(&NewNode {
            file_id: Some(file),
            exported: true,
            cyclomatic_complexity: Some(4),
            line_count: Some(12),
            fingerprint: Some("fp-abc"),
            ..NewNode::plain(symbol_id, NodeKind::Function, "alpha")
        })
        .unwrap();

    let rows = store.annotation_nodes().unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.id, id);
    assert_eq!(row.kind, NodeKind::Function);
    assert_eq!(row.name, "alpha");
    assert!(row.exported, "exported flag survives the round trip");
    assert!(!row.derived);
    assert_eq!(row.fingerprint.as_deref(), Some("fp-abc"));
    assert_eq!(row.file_id, Some(file));
    assert_eq!(row.file_path.as_deref(), Some("src/a.rs"));
    // Fresh insert: the honest un-annotated NULL state on the result columns.
    assert_eq!(row.is_dead, None);
    assert_eq!(row.is_duplicate, None);
    assert_eq!(row.layer_membership, None);

    // The metrics columns land on the FR-AN-04 queryable view too.
    let (cc, lines): (Option<i64>, Option<i64>) = store
        .conn
        .query_row(
            "SELECT cyclomatic_complexity, line_count FROM annotations WHERE node_id = ?1",
            [id.get()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((cc, lines), (Some(4), Some(12)));
}

#[test]
fn node_annotations_are_writable_and_queryable_on_the_view() {
    let mut store = mem();
    let id = seed(&store, 1, "maybe_dead", NodeKind::Function);

    // Freshly inserted: the tri-state verdicts are the honest un-annotated
    // NULL; is_test is a definite 0 (positive-evidence classification, FR-AN-05).
    let (dead0, dup0, test0, layer0): (Option<i64>, Option<i64>, i64, Option<String>) = store
        .conn
        .query_row(
            "SELECT is_dead, is_duplicate, is_test, layer_membership \
             FROM annotations WHERE node_id = ?1",
            [id.get()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!((dead0, dup0, test0, layer0), (None, None, 0, None));

    store
        .write_batch(|w| {
            w.set_node_annotations(id, Some(true), Some(false), true, Some("domain"), None)
        })
        .unwrap();

    let (dead, dup, test, layer): (Option<i64>, Option<i64>, i64, Option<String>) = store
        .conn
        .query_row(
            "SELECT is_dead, is_duplicate, is_test, layer_membership \
             FROM annotations WHERE node_id = ?1",
            [id.get()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(dead, Some(1));
    assert_eq!(dup, Some(0));
    assert_eq!(
        test, 1,
        "is_test persists and projects on the FR-AN-04 view"
    );
    assert_eq!(layer.as_deref(), Some("domain"));
}

#[test]
fn annotations_is_a_view_over_nodes_never_a_sidecar_table() {
    // FR-AN-04: the queryable annotations shape exists, but storage is native
    // node columns — sqlite_master must list `annotations` as a view only.
    let store = mem();
    let views: i64 = store
        .conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='view' AND name='annotations'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let tables: i64 = store
        .conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='annotations'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(views, 1, "the FR-AN-04 view exists");
    assert_eq!(tables, 0, "no sidecar annotations table exists");
}

// ── FR-AN-06 / FR-EX-09: the near-clone shingle index + clone_group column ───

#[test]
fn shingle_index_returns_id_hash_ordered_deduplicated_rows() {
    let mut store = mem();
    let a = seed(&store, 0, "a", NodeKind::Function);
    let b = seed(&store, 1, "b", NodeKind::Function);
    // Insert unsorted, with a duplicate `(a, 20)` the PRIMARY KEY must collapse
    // and a high-bit hash (u64::MAX) whose signed storage is -1 — the round-trip
    // through `i64` must be lossless (S-043 clustering reads back with `as u64`).
    let high = u64::MAX;
    store
        .write_batch(|w| {
            w.insert_shingles(b, &[30, 10])?;
            w.insert_shingles(a, &[20, 5, high, 20])?;
            Ok(())
        })
        .unwrap();

    // ORDER BY (node_id, hash) sorts the hash by its stored *signed* value, so the
    // high-bit hash (stored as -1) sorts ahead of the small positives within `a`;
    // the duplicate `20` is gone. The high-bit value round-trips to u64::MAX.
    assert_eq!(
        store.shingle_index().unwrap(),
        vec![(a, high), (a, 5), (a, 20), (b, 10), (b, 30)],
        "rows arrive in (node_id, hash) order, deduplicated, with a high-bit hash \
         round-tripping losslessly through signed storage"
    );
}

#[test]
fn clone_group_roundtrips_and_clears_through_set_node_annotations() {
    let mut store = mem();
    let id = seed(&store, 0, "fn_a", NodeKind::Function);
    let group = seed(&store, 1, "fn_b", NodeKind::Function);

    let read_group = |store: &SqliteGraphStore| -> Option<i64> {
        store
            .conn
            .query_row(
                "SELECT clone_group FROM annotations WHERE node_id = ?1",
                [id.get()],
                |r| r.get(0),
            )
            .unwrap()
    };

    // FR-AN-06/FR-AN-04: a clone-group id persists and projects on the view.
    store
        .write_batch(|w| w.set_node_annotations(id, None, None, false, None, Some(group)))
        .unwrap();
    assert_eq!(
        read_group(&store),
        Some(group.get()),
        "clone_group persists natively and projects on the FR-AN-04 view"
    );

    // NFR-RA-06 idempotency: a later pass with no membership clears the column.
    store
        .write_batch(|w| w.set_node_annotations(id, None, None, false, None, None))
        .unwrap();
    assert_eq!(
        read_group(&store),
        None,
        "writing None clears stale clone-group membership on a re-pass"
    );
}

#[test]
fn clear_derived_removes_policy_nodes_and_derived_edges_only() {
    let mut store = mem();
    let a = seed(&store, 1, "a", NodeKind::Function);
    let b = seed(&store, 2, "b", NodeKind::Function);

    let (layer_node, removed) = store
        .write_batch(|w| {
            w.insert_edge(a, b, EdgeKind::Calls)?; // extracted, must survive
            let sym = w.upsert_symbol(&LogosSymbol::parse("local pol0").unwrap())?;
            let layer_node = w.insert_node(&NewNode {
                derived: true,
                ..NewNode::plain(sym, NodeKind::Layer, "domain")
            })?;
            w.insert_derived_edge(a, b, EdgeKind::ForbiddenDependency)?;
            let removed = w.clear_derived()?;
            Ok((layer_node, removed))
        })
        .unwrap();

    assert_eq!(
        removed,
        (1, 1),
        "exactly the policy node and the derived edge"
    );
    assert!(
        store.node(layer_node).unwrap().is_none(),
        "the policy node is gone"
    );
    let edges = store.all_edges().unwrap();
    assert_eq!(edges.len(), 1, "the extracted Calls edge survives");
    assert_eq!(edges[0].kind, EdgeKind::Calls);
}

#[test]
fn insert_derived_edge_is_idempotent_and_marked_derived() {
    let mut store = mem();
    let a = seed(&store, 1, "a", NodeKind::Function);
    let b = seed(&store, 2, "b", NodeKind::Function);

    let (first, second) = store
        .write_batch(|w| {
            let first = w.insert_derived_edge(a, b, EdgeKind::ForbiddenDependency)?;
            let second = w.insert_derived_edge(a, b, EdgeKind::ForbiddenDependency)?;
            Ok((first, second))
        })
        .unwrap();
    assert!(first, "first materialisation inserts");
    assert!(!second, "re-materialisation is a quiet no-op");

    let derived: i64 = store
        .conn
        .query_row(
            "SELECT derived FROM edges WHERE source = ?1 AND target = ?2",
            [a.get(), b.get()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        derived, 1,
        "the forbidden_dependency edge carries derived=1"
    );
}

#[test]
fn suggest_prefers_fts_prefix_then_falls_back_to_substring() {
    let store = mem();
    seed(&store, 1, "parse_config", NodeKind::Function);
    seed(&store, 2, "parse_rules", NodeKind::Function);
    seed(&store, 3, "unrelated", NodeKind::Function);

    // Prefix of a real identifier → FTS prefix hits both parse_* names.
    let hits = store.suggest("parse", 5).unwrap();
    assert!(hits.contains(&"parse_config".to_string()), "{hits:?}");
    assert!(hits.contains(&"parse_rules".to_string()), "{hits:?}");
    assert!(!hits.contains(&"unrelated".to_string()));

    // Mid-identifier fragment FTS cannot prefix-match → LIKE substring path.
    let hits = store.suggest("_config", 5).unwrap();
    assert_eq!(hits, vec!["parse_config".to_string()]);

    // Hostile input degrades to empty, never errors (FR-NV-09).
    assert!(store.suggest("\"*)(%%", 5).is_ok());
    assert!(store.suggest("", 5).unwrap().is_empty());
    assert!(store.suggest("zzz_missing", 5).unwrap().is_empty());
}

#[test]
fn file_layer_is_writable_and_clearable() {
    let mut store = mem();
    let file = store
        .insert_file("src/ui/view.rs", Some("rust"), None)
        .unwrap();

    store
        .write_batch(|w| w.set_file_layer(file, Some("presentation")))
        .unwrap();
    let layer: Option<String> = store
        .conn
        .query_row("SELECT layer FROM files WHERE id = ?1", [file], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(layer.as_deref(), Some("presentation"));

    store.write_batch(|w| w.set_file_layer(file, None)).unwrap();
    let cleared: Option<String> = store
        .conn
        .query_row("SELECT layer FROM files WHERE id = ?1", [file], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(cleared, None, "a no-longer-matching file clears its layer");
}

#[test]
fn policy_kinds_insert_through_the_typed_api() {
    let store = mem();
    let layer = seed(&store, 1, "domain", NodeKind::Layer);
    let boundary = seed(&store, 2, "ui->domain", NodeKind::Boundary);
    assert_eq!(store.node(layer).unwrap().unwrap().kind, NodeKind::Layer);
    assert_eq!(
        store.node(boundary).unwrap().unwrap().kind,
        NodeKind::Boundary
    );
}

// ── seed_copy (S-021, ADR-15, FR-WT-03) ─────────────────────────────────────

/// `seed_copy` clones a checkpointed store byte-for-byte into a fresh
/// destination (creating parents) and copies ONLY the DB — sibling state like
/// `telemetry.db` never travels (NFR-DM-04).
#[test]
fn seed_copy_clones_the_database_and_nothing_else() {
    let tmp = tempfile::TempDir::new().unwrap();
    let src_dir = tmp.path().join("main/.logos");
    std::fs::create_dir_all(&src_dir).unwrap();
    let src_db = src_dir.join("logos.db");
    {
        let store = SqliteGraphStore::open(&src_db).expect("source store opens");
        seed(&store, 1, "seeded_fn", NodeKind::Function);
    } // dropped → connection closed, WAL checkpointed into the main file
    std::fs::write(src_dir.join("telemetry.db"), b"derived state").unwrap();

    let dst_db = tmp.path().join("wt/.logos/logos.db");
    seed_copy(&src_db, &dst_db).expect("seed copy succeeds");

    let copy = SqliteGraphStore::open(&dst_db).expect("the seeded copy opens");
    assert_eq!(
        node_count(&copy),
        1,
        "the seeded store carries the source's graph"
    );
    assert!(
        !tmp.path().join("wt/.logos/telemetry.db").exists(),
        "derived sibling state does not travel (NFR-DM-04)"
    );
}

/// Seeding from a LIVE primary (its writer still open, recent commits only in
/// the WAL) carries those commits: the `-wal` sidecar is copied alongside and
/// replayed when the copy first opens.
#[test]
fn seed_copy_carries_uncheckpointed_wal_commits() {
    let tmp = tempfile::TempDir::new().unwrap();
    let src_db = tmp.path().join("main/.logos/logos.db");
    std::fs::create_dir_all(src_db.parent().unwrap()).unwrap();
    let store = SqliteGraphStore::open(&src_db).expect("source store opens");
    seed(&store, 1, "wal_resident", NodeKind::Function);
    // Keep `store` open: the insert above lives in `logos.db-wal`.
    assert!(
        src_db.with_extension("db-wal").exists() || {
            // rusqlite names the sidecar `<file>-wal`, not `<stem>.db-wal`.
            let mut os = src_db.as_os_str().to_os_string();
            os.push("-wal");
            std::path::PathBuf::from(os).exists()
        },
        "fixture precondition: a live WAL sidecar exists"
    );

    let dst_db = tmp.path().join("wt/.logos/logos.db");
    seed_copy(&src_db, &dst_db).expect("seed copy succeeds");
    drop(store);

    let copy = SqliteGraphStore::open(&dst_db).expect("the seeded copy opens");
    assert_eq!(
        node_count(&copy),
        1,
        "WAL-resident commits replay into the seeded copy"
    );
}

/// A failed seed leaves NO partial artifacts behind: neither a half-written
/// `logos.db` nor an orphan `-wal` a later fresh store could mispair with.
#[test]
fn seed_copy_failure_leaves_no_partial_artifacts() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dst_db = tmp.path().join("wt/.logos/logos.db");

    // Source DB missing entirely → the db copy itself fails.
    let err = seed_copy(&tmp.path().join("main/.logos/logos.db"), &dst_db);
    assert!(err.is_err(), "a missing source is an error, never a no-op");
    assert!(!dst_db.exists(), "no partial logos.db is left behind");

    // Source db + live WAL present, but the WAL copy fails (the destination
    // sidecar path is occupied by a directory) → the already-copied db must
    // be rolled back.
    let src_db = tmp.path().join("main/.logos/logos.db");
    std::fs::create_dir_all(src_db.parent().unwrap()).unwrap();
    let store = SqliteGraphStore::open(&src_db).expect("source store opens");
    seed(&store, 1, "wal_resident", NodeKind::Function);
    let dst_wal = {
        let mut os = dst_db.as_os_str().to_os_string();
        os.push("-wal");
        std::path::PathBuf::from(os)
    };
    std::fs::create_dir_all(&dst_wal).unwrap(); // blocks the sidecar copy
    let err = seed_copy(&src_db, &dst_db);
    assert!(
        err.is_err(),
        "an unwritable sidecar destination is an error"
    );
    assert!(
        !dst_db.exists(),
        "the half-seeded logos.db is rolled back with its failed wal"
    );
}

// ── CR-004 / FR-SY-07: the project_metadata kv table (migration 15) ──────────

#[test]
fn project_metadata_round_trips_and_upserts() {
    // The durable config-fingerprint store: an absent key reads None, a write
    // round-trips, and a second write to the same key overwrites (ON CONFLICT
    // DO UPDATE) rather than erroring on the primary key.
    let mut store = mem();
    assert_eq!(
        store.project_metadata(CONFIG_FINGERPRINT_KEY).unwrap(),
        None,
        "a never-written key reads None"
    );

    store
        .write_batch(|w| w.set_project_metadata(CONFIG_FINGERPRINT_KEY, "fp-one"))
        .unwrap();
    assert_eq!(
        store.project_metadata(CONFIG_FINGERPRINT_KEY).unwrap(),
        Some("fp-one".to_string()),
        "the written fingerprint round-trips"
    );

    store
        .write_batch(|w| w.set_project_metadata(CONFIG_FINGERPRINT_KEY, "fp-two"))
        .unwrap();
    assert_eq!(
        store.project_metadata(CONFIG_FINGERPRINT_KEY).unwrap(),
        Some("fp-two".to_string()),
        "a re-record overwrites in place (upsert), never duplicating the key"
    );

    // Distinct keys are independent — it is a general kv table.
    store
        .write_batch(|w| w.set_project_metadata("other", "x"))
        .unwrap();
    assert_eq!(
        store.project_metadata(CONFIG_FINGERPRINT_KEY).unwrap(),
        Some("fp-two".to_string()),
        "an unrelated key does not disturb the fingerprint row"
    );
}

// ── CR-027 / FR-SY-09 / ADR-32: the persisted monotonic graph revision ───────

#[test]
fn graph_revision_starts_at_zero_and_advances_monotonically() {
    // A never-advanced store reads 0 — "no graph yet" — without erroring on the
    // absent row (FR-SY-09: `0` before the first index).
    let mut store = mem();
    assert_eq!(
        store.graph_revision().unwrap(),
        0,
        "a never-advanced revision reads 0"
    );

    // Each advance returns the new value and strictly increases; the read-side
    // accessor reads back the same value (the durable successor to SyncStamp).
    let first = store.write_batch(|w| w.advance_graph_revision()).unwrap();
    assert_eq!(first, 1, "the first advance returns 1");
    assert_eq!(
        store.graph_revision().unwrap(),
        1,
        "the read accessor reflects the first advance"
    );

    let second = store.write_batch(|w| w.advance_graph_revision()).unwrap();
    assert_eq!(second, 2, "the second advance returns 2 — strictly monotonic");
    assert_eq!(store.graph_revision().unwrap(), 2);

    // The revision lives under its own key, independent of the config fingerprint
    // row (both inhabit the same kv table).
    store
        .write_batch(|w| w.set_project_metadata(CONFIG_FINGERPRINT_KEY, "fp"))
        .unwrap();
    assert_eq!(
        store.graph_revision().unwrap(),
        2,
        "writing an unrelated metadata key never disturbs the revision"
    );
    let third = store.write_batch(|w| w.advance_graph_revision()).unwrap();
    assert_eq!(third, 3, "advancing past an unrelated write still increments");
}

#[test]
fn graph_revision_is_read_identically_by_a_second_process() {
    // FR-SY-09 AC: a second process opening the same logos.db reads the identical
    // current revision. A read-only WAL connection is that second reader.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("logos.db");

    let mut writer = SqliteGraphStore::open(&path).expect("writer opens");
    writer
        .write_batch(|w| w.advance_graph_revision())
        .unwrap();
    let committed = writer
        .write_batch(|w| w.advance_graph_revision())
        .unwrap();
    assert_eq!(committed, 2);

    let reader = SqliteGraphStore::open_readonly(&path).expect("read-only opens");
    assert_eq!(
        reader.graph_revision().unwrap(),
        2,
        "a second process reads the identical current revision (FR-SY-09)"
    );
}

#[test]
fn advance_graph_revision_writes_exactly_one_row() {
    // NFR-PE-03: persisting the revision is a *single-row* write — the sprint's
    // verification step asks us to confirm exactly that. The first advance inserts
    // one row; every later advance upserts in place (never a second row).
    let mut store = mem();

    let rows = |s: &SqliteGraphStore| -> i64 {
        s.conn
            .query_row(
                "SELECT count(*) FROM project_metadata WHERE key = ?1",
                [GRAPH_REVISION_KEY],
                |r| r.get(0),
            )
            .unwrap()
    };

    assert_eq!(rows(&store), 0, "no revision row before the first advance");
    store.write_batch(|w| w.advance_graph_revision()).unwrap();
    assert_eq!(rows(&store), 1, "the first advance writes exactly one row");
    store.write_batch(|w| w.advance_graph_revision()).unwrap();
    assert_eq!(
        rows(&store),
        1,
        "a later advance upserts in place — still a single row (NFR-PE-03)"
    );
}

#[test]
fn graph_revision_treats_an_unparseable_value_as_zero() {
    // FR-SY-09 / ADR-32 design choice: the revision is a freshness hint, never a
    // correctness gate, so a non-numeric stored value (a hand-edited or
    // future-format row) reads as 0 rather than erroring — and the next advance
    // restarts cleanly from 1.
    let mut store = mem();
    store
        .write_batch(|w| w.set_project_metadata(GRAPH_REVISION_KEY, "not-a-number"))
        .unwrap();
    assert_eq!(
        store.graph_revision().unwrap(),
        0,
        "an unparseable stored revision reads as 0"
    );
    let next = store.write_batch(|w| w.advance_graph_revision()).unwrap();
    assert_eq!(next, 1, "advancing past an unparseable value restarts from 1");
}

// ── FR-UI-10 / CR-021: per-project language composition ──────────────────────
//
// `language_composition` reports the languages **present** in the graph (≥1
// indexed node), each with its node count and contributing-file count, in
// deterministic order (node count desc, then language asc — NFR-RA-06). Every
// count is a graph fact, never fabricated (NFR-RA-05).

/// Insert a file row with `language`, returning its id.
fn seed_file(store: &SqliteGraphStore, path: &str, language: &str) -> i64 {
    store
        .insert_file(path, Some(language), None)
        .expect("file inserts")
}

/// Insert a node bound to `file_id` under a fresh distinct symbol, returning its
/// id. `n` keys the symbol so callers get distinct symbols cheaply.
fn seed_in_file(store: &SqliteGraphStore, n: u32, name: &str, file_id: i64) -> NodeId {
    let sym = LogosSymbol::parse(&format!("local {n}")).expect("local symbol parses");
    let symbol_id = store.upsert_symbol(&sym).expect("symbol upserts");
    let mut node = NewNode::plain(symbol_id, NodeKind::Function, name);
    node.file_id = Some(file_id);
    store.insert_node(&node).expect("node inserts")
}

#[test]
fn language_composition_on_empty_store_is_empty() {
    let store = mem();
    assert!(
        store.language_composition().unwrap().is_empty(),
        "an un-indexed graph has no language composition"
    );
}

#[test]
fn language_composition_groups_present_languages_with_node_and_file_counts() {
    let store = mem();
    // rust: 3 + 2 nodes across two files → nodes=5, files=2.
    let r1 = seed_file(&store, "src/a.rs", "rust");
    let r2 = seed_file(&store, "src/b.rs", "rust");
    for i in 0..3 {
        seed_in_file(&store, i, &format!("a{i}"), r1);
    }
    for i in 3..5 {
        seed_in_file(&store, i, &format!("b{i}"), r2);
    }
    // python: 1 node in one file → nodes=1, files=1.
    let p1 = seed_file(&store, "svc/app.py", "python");
    seed_in_file(&store, 10, "main", p1);

    let comp = store.language_composition().unwrap();
    assert_eq!(comp.len(), 2, "exactly the two present languages");
    // Ordered by node count descending: rust (5) before python (1).
    assert_eq!(comp[0].language, "rust");
    assert_eq!(comp[0].nodes, 5, "every node in the two rust files");
    assert_eq!(comp[0].files, 2, "distinct rust files that carry nodes");
    assert_eq!(comp[1].language, "python");
    assert_eq!(comp[1].nodes, 1);
    assert_eq!(comp[1].files, 1);
}

#[test]
fn language_composition_orders_by_node_count_then_name() {
    let store = mem();
    // Two languages tied at 2 nodes (1 file each) plus one larger; the tie is
    // broken by language name ascending, the larger sorts first (NFR-RA-06).
    let big = seed_file(&store, "x.go", "go");
    for i in 0..4 {
        seed_in_file(&store, i, &format!("g{i}"), big);
    }
    let a = seed_file(&store, "a.toml", "alpha");
    seed_in_file(&store, 10, "a0", a);
    seed_in_file(&store, 11, "a1", a);
    let b = seed_file(&store, "b.txt", "beta");
    seed_in_file(&store, 20, "b0", b);
    seed_in_file(&store, 21, "b1", b);

    let comp = store.language_composition().unwrap();
    let order: Vec<&str> = comp.iter().map(|e| e.language.as_str()).collect();
    assert_eq!(
        order,
        ["go", "alpha", "beta"],
        "node count descending, then language name ascending on the tie"
    );
}

#[test]
fn language_composition_omits_a_language_with_files_but_no_nodes() {
    let store = mem();
    let rust = seed_file(&store, "src/a.rs", "rust");
    seed_in_file(&store, 0, "a", rust);
    // A file tagged with a language but carrying no nodes (empty / failed
    // extraction) must not fabricate a phantom language (FR-UI-10, NFR-RA-05).
    seed_file(&store, "vendor/empty.py", "python");

    let comp = store.language_composition().unwrap();
    assert_eq!(comp.len(), 1, "only the language with indexed nodes");
    assert_eq!(comp[0].language, "rust");
}

#[test]
fn language_composition_excludes_untagged_files_and_orphaned_nodes() {
    let store = mem();
    let rust = seed_file(&store, "src/a.rs", "rust");
    seed_in_file(&store, 0, "a", rust);
    // A file with no language (language IS NULL) contributes no entry …
    let untagged = store.insert_file("LICENSE", None, None).expect("file inserts");
    seed_in_file(&store, 1, "license_node", untagged);
    // … and a node whose file_id is NULL (its file was deleted, FK SET NULL)
    // is attributable to no language, so it is excluded too.
    seed(&store, 2, "orphan", NodeKind::Function);

    let comp = store.language_composition().unwrap();
    assert_eq!(comp.len(), 1, "only the tagged, node-bearing language");
    assert_eq!(comp[0].language, "rust");
    assert_eq!(comp[0].nodes, 1, "the untagged and orphaned nodes are not counted");
    assert_eq!(comp[0].files, 1);
}

// ── FR-GV-18 / NFR-RA-13 / ADR-46: the fast structural-integrity check ───────

/// A minimal raw connection with just the four tables `structural_report`
/// reads — no `UNIQUE(symbol_id)`, no FTS triggers, no foreign keys. It exists
/// to build dirty fixtures the real schema forbids at write time (a duplicate
/// `symbol_id`), so the census SQL can be exercised on every fault branch.
fn dirty_conn() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE files   (id INTEGER PRIMARY KEY, path TEXT NOT NULL);
         CREATE TABLE nodes   (id INTEGER PRIMARY KEY, symbol_id INTEGER NOT NULL, \
                               kind INTEGER NOT NULL, name TEXT NOT NULL, file_id INTEGER);
         CREATE TABLE edges   (id INTEGER PRIMARY KEY, source INTEGER NOT NULL, \
                               target INTEGER NOT NULL, kind INTEGER NOT NULL);
         CREATE TABLE shingles(node_id INTEGER NOT NULL, hash INTEGER NOT NULL);",
    )
    .unwrap();
    conn
}

#[test]
fn structural_check_reports_ok_on_a_healthy_graph() {
    let mut store = mem();
    let file = seed_file(&store, "src/lib.rs", "rust");
    let a = seed_in_file(&store, 0, "caller", file);
    let b = seed_in_file(&store, 1, "callee", file);
    store
        .write_batch(|w| {
            w.insert_edge(a, b, EdgeKind::Calls)?;
            w.insert_shingles(a, &[0xdead_beef])?;
            w.insert_shingles(b, &[0x0bad_f00d])
        })
        .unwrap();

    let report = store.structural_check().unwrap();
    assert!(report.is_ok(), "a well-formed graph is structurally sound");
    assert_eq!(report.node_count, 2);
    assert_eq!(report.distinct_symbol_ids, 2);
    assert_eq!(report.duplicate_symbol_nodes, 0);
    assert_eq!(report.dangling_file_refs, 0);
    assert_eq!(report.dangling_edge_endpoints, 0);
    assert_eq!(report.orphan_shingles, 0);
    assert!(report.faults().is_empty());
}

#[test]
fn structural_check_reports_ok_on_an_empty_store() {
    let store = mem();
    let report = store.structural_check().unwrap();
    assert!(report.is_ok(), "an empty graph is trivially sound");
    assert_eq!(report.node_count, 0);
    assert_eq!(report.distinct_symbol_ids, 0);
}

#[test]
fn structural_check_detects_a_duplicate_symbol_id() {
    // The real schema's UNIQUE(symbol_id) makes this unreachable at write time
    // (NFR-RA-13), so a hand-built fixture forces the Channel-A drift the
    // constraint is meant to prevent — the census must still detect it.
    let conn = dirty_conn();
    conn.execute("INSERT INTO files(id, path) VALUES (1, 'src/lib.rs')", [])
        .unwrap();
    // Two nodes folding to the same symbol_id — the leak.
    conn.execute(
        "INSERT INTO nodes(id, symbol_id, kind, name, file_id) VALUES (1, 7, 1, 'a', 1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO nodes(id, symbol_id, kind, name, file_id) VALUES (2, 7, 1, 'b', 1)",
        [],
    )
    .unwrap();

    let report = structural_report(&conn).unwrap();
    assert!(!report.is_ok(), "a duplicate symbol_id is drift");
    assert_eq!(report.node_count, 2);
    assert_eq!(report.distinct_symbol_ids, 1);
    assert_eq!(report.duplicate_symbol_nodes, 1);
    let faults = report.faults();
    assert_eq!(faults.len(), 1);
    assert!(
        faults[0].contains("duplicate-symbol"),
        "the fault names the invariant: {}",
        faults[0]
    );
}

#[test]
fn structural_check_detects_a_dangling_file_reference() {
    let conn = dirty_conn();
    // A node whose file_id points at a files row that does not exist.
    conn.execute(
        "INSERT INTO nodes(id, symbol_id, kind, name, file_id) VALUES (1, 1, 1, 'a', 999)",
        [],
    )
    .unwrap();

    let report = structural_report(&conn).unwrap();
    assert!(!report.is_ok());
    assert_eq!(report.dangling_file_refs, 1);
    assert_eq!(report.duplicate_symbol_nodes, 0, "one node, one symbol_id");
    assert!(report.faults().iter().any(|f| f.contains("dangling file_id")));
}

#[test]
fn structural_check_detects_dangling_edge_endpoints_and_orphan_shingles() {
    let conn = dirty_conn();
    conn.execute(
        "INSERT INTO nodes(id, symbol_id, kind, name, file_id) VALUES (1, 1, 1, 'a', NULL)",
        [],
    )
    .unwrap();
    // An edge whose target node is missing, and a shingle whose node is missing.
    conn.execute("INSERT INTO edges(source, target, kind) VALUES (1, 999, 1)", [])
        .unwrap();
    conn.execute("INSERT INTO shingles(node_id, hash) VALUES (999, 42)", [])
        .unwrap();

    let report = structural_report(&conn).unwrap();
    assert!(!report.is_ok());
    assert_eq!(report.dangling_edge_endpoints, 1);
    assert_eq!(report.orphan_shingles, 1);
    let faults = report.faults();
    assert!(faults.iter().any(|f| f.contains("dangling endpoint")));
    assert!(faults.iter().any(|f| f.contains("orphan shingle")));
}

#[test]
fn structural_check_on_the_real_schema_detects_orphans_inserted_past_the_fk() {
    // Prove the census SQL binds to the *real* v16 schema, not just the minimal
    // fixture: seed a clean graph, then force orphan rows with foreign keys
    // disabled (the state a bug bypassing the cascade would leave).
    let store = mem();
    let file = seed_file(&store, "src/lib.rs", "rust");
    let a = seed_in_file(&store, 0, "keep", file);
    assert!(store.structural_check().unwrap().is_ok());

    store
        .conn
        .pragma_update(None, "foreign_keys", false)
        .unwrap();
    // A dangling edge endpoint and an orphan shingle the FK cascade would forbid.
    store
        .conn
        .execute("INSERT INTO edges(source, target, kind) VALUES (?1, 12345, 1)", [a.0])
        .unwrap();
    store
        .conn
        .execute("INSERT INTO shingles(node_id, hash) VALUES (12345, 7)", [])
        .unwrap();

    let report = store.structural_check().unwrap();
    assert!(!report.is_ok(), "forced orphans on the real schema are detected");
    assert_eq!(report.dangling_edge_endpoints, 1);
    assert_eq!(report.orphan_shingles, 1);
}

#[test]
fn structural_check_stays_within_the_point_query_budget() {
    // NFR-PE-01: the check is a handful of indexed queries — it must not scan
    // row content. A few hundred nodes/edges/shingles completes far inside a
    // deliberately generous bound (the queries are microseconds on this size;
    // the bound only guards against an accidental O(n²) or table scan).
    let mut store = mem();
    let file = seed_file(&store, "src/lib.rs", "rust");
    let ids: Vec<NodeId> = (0..300)
        .map(|n| seed_in_file(&store, n, &format!("n{n}"), file))
        .collect();
    store
        .write_batch(|w| {
            for pair in ids.windows(2) {
                w.insert_edge(pair[0], pair[1], EdgeKind::Calls)?;
                w.insert_shingles(pair[0], &[pair[0].0 as u64])?;
            }
            Ok(())
        })
        .unwrap();

    let start = std::time::Instant::now();
    let report = store.structural_check().unwrap();
    let elapsed = start.elapsed();
    assert!(report.is_ok(), "the large healthy fixture is sound");
    assert!(
        elapsed < std::time::Duration::from_millis(250),
        "structural_check must stay within the point-query budget (NFR-PE-01), took {elapsed:?}"
    );
}
