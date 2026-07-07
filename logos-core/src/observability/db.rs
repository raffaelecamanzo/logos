//! `telemetry.db` — the separate observability store ([FR-OB-03], [ADR-13]).
//!
//! Deliberately a **different SQLite file** from `logos.db`: a full `index`
//! rebuilds the graph store wholesale, and telemetry must accumulate across
//! re-indexes ([NFR-OO-05]). A separate file also means a separate WAL writer
//! lock, so a telemetry flush can never contend with the single-writer actor
//! on the graph store ([AR-01], the [ADR-13] risk note).
//!
//! Schema (documented per [FR-OB-03]):
//! - `events` — one row per telemetry-tagged `tracing` event (raw, bounded
//!   retention, [NFR-OO-04]).
//! - `daily_rollup` — permanent per-`(day, surface, tool)` aggregates that raw
//!   events fold into when they age out, keeping the DB small while `stats`
//!   still serves long windows ([NFR-OO-04], DL-13).
//!
//! Migrations follow the same forward-only `user_version` discipline as
//! [`crate::graph_store::migrate`] — two files, two migration tracks (the
//! accepted [ADR-13] cost).
//!
//! [FR-OB-03]: ../../../docs/specs/requirements/FR-OB-03.md
//! [NFR-OO-04]: ../../../docs/specs/requirements/NFR-OO-04.md
//! [NFR-OO-05]: ../../../docs/specs/requirements/NFR-OO-05.md
//! [ADR-13]: ../../../docs/specs/architecture/decisions/ADR-13.md
//! [AR-01]: ../../../docs/specs/architecture.md#13-risk-register

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;

use super::EventRecord;

/// Raw-event retention window in days (NFR-OO-04: "default ~90 days").
/// Events older than this fold into `daily_rollup` and are deleted.
pub(crate) const RETENTION_DAYS: u32 = 90;

/// Forward-only migration ledger for `telemetry.db` (mirrors
/// `graph_store::schema::MIGRATIONS` — dense, 1-based, strictly increasing).
///
/// v2 adds the nullable `origin` column ([FR-OB-08]): a plain
/// `ADD COLUMN`, so it applies cleanly to an existing v1 store and leaves
/// pre-migration rows `NULL`, which the read-model treats as `'main'`
/// (`COALESCE(origin, 'main')`).
///
/// [FR-OB-08]: ../../../docs/specs/requirements/FR-OB-08.md
const MIGRATIONS: &[(i64, &str)] = &[
    (
        1,
        "CREATE TABLE schema_versions (
         version    INTEGER PRIMARY KEY,
         applied_at INTEGER NOT NULL
     ) STRICT;
     CREATE TABLE events (
         id          INTEGER PRIMARY KEY,
         at          INTEGER NOT NULL,            -- unix seconds, emission time
         surface     TEXT    NOT NULL,            -- 'cli' | 'mcp'
         tool        TEXT    NOT NULL,            -- engine method or pipeline pass
         duration_ms INTEGER NOT NULL,
         ok          INTEGER NOT NULL CHECK (ok IN (0, 1))
     ) STRICT;
     CREATE INDEX events_at ON events (at);
     CREATE TABLE daily_rollup (
         day               TEXT    NOT NULL,      -- 'YYYY-MM-DD' (UTC)
         surface           TEXT    NOT NULL,
         tool              TEXT    NOT NULL,
         calls             INTEGER NOT NULL,
         ok_calls          INTEGER NOT NULL,
         total_duration_ms INTEGER NOT NULL,
         max_duration_ms   INTEGER NOT NULL,
         PRIMARY KEY (day, surface, tool)
     ) STRICT;",
    ),
    (
        2,
        // Nullable: legacy rows stay NULL and read back as 'main'. STRICT
        // permits ADD COLUMN of a typed, non-NOT-NULL column with no default.
        "ALTER TABLE events ADD COLUMN origin TEXT;    -- branch, or 'main' (FR-OB-08)",
    ),
];

/// Open (creating if absent) and migrate `telemetry.db` at `path`.
///
/// Applies the same pragma contract as the graph store: WAL journalling so the
/// background writer never blocks a concurrent `stats` reader, and a busy
/// timeout so two short-lived CLI processes flushing at once retry instead of
/// failing ([NFR-OO-02] best-effort still applies — a timeout is dropped).
///
/// # Errors
/// Returns an error if the file cannot be opened or a migration fails.
pub(crate) fn open(path: &Path) -> Result<Connection> {
    let mut conn = Connection::open(path)
        .with_context(|| format!("opening telemetry store at {}", path.display()))?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA busy_timeout = 250;",
    )
    .context("applying the telemetry pragma contract")?;
    apply_migrations(&mut conn)?;
    Ok(conn)
}

/// Open `telemetry.db` read-only for `stats` ([FR-OB-04]).
///
/// # Errors
/// Returns an error if the file does not exist or cannot be opened.
pub(crate) fn open_readonly(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("opening telemetry store read-only at {}", path.display()))?;
    conn.execute_batch("PRAGMA busy_timeout = 250;")
        .context("applying the telemetry read pragma")?;
    Ok(conn)
}

/// Apply every embedded migration newer than the store's `user_version` —
/// one transaction per migration, all or nothing (the graph-store discipline).
fn apply_migrations(conn: &mut Connection) -> Result<()> {
    let current: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .context("reading telemetry user_version")?;

    for &(version, sql) in MIGRATIONS {
        if version <= current {
            continue;
        }
        let tx = conn
            .transaction()
            .with_context(|| format!("opening transaction for telemetry migration {version}"))?;
        tx.execute_batch(sql)
            .with_context(|| format!("applying telemetry migration {version}"))?;
        tx.execute(
            "INSERT INTO schema_versions (version, applied_at) VALUES (?1, unixepoch())",
            [version],
        )
        .with_context(|| format!("recording telemetry migration {version}"))?;
        tx.pragma_update(None, "user_version", version)
            .with_context(|| format!("advancing telemetry user_version to {version}"))?;
        tx.commit()
            .with_context(|| format!("committing telemetry migration {version}"))?;
    }
    Ok(())
}

/// A migrated in-memory telemetry store — the unit-test seam.
#[cfg(test)]
pub(crate) fn open_in_memory() -> Connection {
    let mut conn = Connection::open_in_memory().expect("in-memory telemetry store");
    apply_migrations(&mut conn).expect("telemetry migrations apply");
    conn
}

/// An in-memory telemetry store migrated **only through v1** — the pre-`origin`
/// (legacy) shape, so a test can drive the v2 forward migration ([FR-OB-08])
/// over it and prove legacy rows survive and read as `'main'`.
///
/// [FR-OB-08]: ../../../docs/specs/requirements/FR-OB-08.md
#[cfg(test)]
pub(crate) fn open_in_memory_v1() -> Connection {
    let mut conn = Connection::open_in_memory().expect("in-memory telemetry store");
    let tx = conn.transaction().expect("v1 migration transaction");
    let (version, sql) = MIGRATIONS[0];
    tx.execute_batch(sql).expect("v1 schema");
    tx.execute(
        "INSERT INTO schema_versions (version, applied_at) VALUES (?1, unixepoch())",
        [version],
    )
    .expect("record v1");
    tx.pragma_update(None, "user_version", version)
        .expect("set v1 user_version");
    tx.commit().expect("commit v1");
    conn
}

/// Apply every pending migration to `conn` — the test seam for driving the
/// forward migration ledger directly ([FR-OB-08]).
#[cfg(test)]
pub(crate) fn migrate(conn: &mut Connection) -> Result<()> {
    apply_migrations(conn)
}

/// Insert a drained batch of event records in **one** transaction.
///
/// One transaction per batch is the whole point of batching ([NFR-OO-02]):
/// the per-write fsync cost is paid once per flush, not once per event.
///
/// # Errors
/// Returns an error if the transaction cannot commit — the caller (the
/// background writer) drops the batch, never propagates (best-effort,
/// [NFR-CC-03]).
pub(crate) fn write_batch(conn: &mut Connection, batch: &[EventRecord]) -> Result<()> {
    let tx = conn
        .transaction()
        .context("opening the telemetry batch transaction")?;
    {
        let mut stmt = tx
            .prepare_cached(
                "INSERT INTO events (at, surface, tool, duration_ms, ok, origin)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .context("preparing the telemetry insert")?;
        for e in batch {
            stmt.execute(rusqlite::params![
                e.at,
                e.surface,
                e.tool,
                e.duration_ms as i64,
                e.ok as i64,
                e.origin,
            ])
            .context("inserting a telemetry event")?;
        }
    }
    tx.commit().context("committing the telemetry batch")
}

/// Fold raw events older than `retention_days` into `daily_rollup`, then
/// delete them ([NFR-OO-04]).
///
/// The rollup rows are permanent; conflicts accumulate (a second prune over an
/// already-rolled-up day adds, never double-counts, because the raw rows it
/// reads are deleted in the same transaction).
///
/// # Errors
/// Returns an error if the rollup transaction cannot commit.
pub(crate) fn rollup_and_prune(
    conn: &mut Connection,
    now_unix: i64,
    retention_days: u32,
) -> Result<()> {
    let cutoff = now_unix - i64::from(retention_days) * 86_400;
    let tx = conn
        .transaction()
        .context("opening the telemetry rollup transaction")?;
    tx.execute(
        "INSERT INTO daily_rollup (day, surface, tool, calls, ok_calls,
                                   total_duration_ms, max_duration_ms)
         SELECT date(at, 'unixepoch'), surface, tool,
                count(*), sum(ok), sum(duration_ms), max(duration_ms)
         FROM events WHERE at < ?1
         GROUP BY 1, 2, 3
         ON CONFLICT (day, surface, tool) DO UPDATE SET
             calls             = calls + excluded.calls,
             ok_calls          = ok_calls + excluded.ok_calls,
             total_duration_ms = total_duration_ms + excluded.total_duration_ms,
             max_duration_ms   = max(max_duration_ms, excluded.max_duration_ms)",
        [cutoff],
    )
    .context("rolling up aged telemetry events")?;
    tx.execute("DELETE FROM events WHERE at < ?1", [cutoff])
        .context("pruning aged telemetry events")?;
    tx.commit().context("committing the telemetry rollup")
}
