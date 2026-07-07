//! `history.db` — the separate git-history evidence store ([FR-GH-01],
//! [ADR-22], [ADR-13] reapplied).
//!
//! Deliberately a **third SQLite file** under `.logos/`, beside `logos.db` and
//! `telemetry.db`. The reasons are the [ADR-13] reasons one more time:
//!
//! - A full `index` rebuilds `logos.db` wholesale; the mined history must
//!   survive that ([FR-GH-01]) — a separate file simply isn't touched.
//! - The gated metric path holds no connection to this file and the store is
//!   **never `ATTACH`-ed** to `logos.db`, so "the gate cannot read history" is a
//!   physical property, not a coding convention ([BR-26], [UAT-GH-02]).
//! - Its migration track is **independent**: `history.db` starts at
//!   `user_version = 1` and advances on its own, regardless of `logos.db`'s
//!   version ([FR-GH-01]).
//!
//! Schema (migration 1 — the [S-046] substrate every CR-006/CR-007 story rides):
//! - `commits` — one row per mined commit within the HEAD-anchored window, with
//!   the committer timestamp (the determinism clock, [BR-27]) and the
//!   `.mailmap`-coalesced author identity. Migration 2 ([S-047]) adds `subject`
//!   for the defect heuristic ([FR-GH-05]).
//! - `file_changes` — one row per (commit, file) numstat fact, rename-aware
//!   (`old_path`), binary-aware (`added`/`deleted` NULL).
//! - `mine_state` — the single-row mining cursor: the HEAD the store was mined
//!   through and the local `git --version` (recorded for golden reproducibility,
//!   [ADR-22] — `git log --numstat -M` output is git-version-sensitive).
//!
//! Migration 2 ([S-047]) adds `commits.subject` and the append-only
//! `temporal_snapshots` series ([FR-GH-09]) on this same independent track.
//!
//! Migrations follow the same forward-only `user_version` discipline as
//! [`crate::graph_store::migrate`] and [`crate::observability::db`] — one
//! transaction per migration, all-or-nothing ([NFR-RA-07]).
//!
//! [FR-GH-01]: ../../../docs/specs/requirements/FR-GH-01.md
//! [BR-26]: ../../../docs/specs/software-spec.md#322-git-history-analytics
//! [BR-27]: ../../../docs/specs/software-spec.md#322-git-history-analytics
//! [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
//! [UAT-GH-02]: ../../../docs/specs/requirements/UAT-GH-02.md
//! [ADR-13]: ../../../docs/specs/architecture/decisions/ADR-13.md
//! [ADR-22]: ../../../docs/specs/architecture/decisions/ADR-22.md
//! [S-046]: ../../../docs/planning/journal.md#s-046-history-store-and-incremental-git-miner

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;

use super::miner::{CommitFact, FileChange};

/// Forward-only migration ledger for `history.db` — dense, 1-based, strictly
/// increasing (mirrors `graph_store::schema::MIGRATIONS` and
/// `observability::db::MIGRATIONS`). [S-047] appends migration 2 (the commit
/// subject + the temporal snapshot series); [S-049] appends migration 3+ for the
/// coverage tables on this same track. Migration 1 is **never** edited.
const MIGRATIONS: &[(i64, &str)] = &[
    (
        1,
        "CREATE TABLE schema_versions (
         version    INTEGER PRIMARY KEY,
         applied_at INTEGER NOT NULL
     ) STRICT;

     -- One row per mined commit within the HEAD-anchored window. `committed_at`
     -- is the committer timestamp — the determinism clock every temporal
     -- formula anchors to (BR-27); wall clock never enters. `author_name` /
     -- `author_email` are the `.mailmap`-coalesced identity (%aN/%aE).
     CREATE TABLE commits (
         sha          TEXT    PRIMARY KEY,        -- full 40-hex commit id
         committed_at INTEGER NOT NULL,           -- committer unix seconds
         author_name  TEXT    NOT NULL,           -- mailmapped (%aN)
         author_email TEXT    NOT NULL,           -- mailmapped (%aE)
         file_count   INTEGER NOT NULL            -- distinct files touched (co-change cap, FR-GH-04)
     ) STRICT;
     CREATE INDEX commits_committed_at ON commits (committed_at);

     -- One row per (commit, file) numstat fact. `added`/`deleted` are NULL for
     -- binary files (numstat '-'); `old_path` is the pre-rename path when `-M`
     -- followed a rename, else NULL (FR-GH-02).
     CREATE TABLE file_changes (
         commit_sha TEXT    NOT NULL REFERENCES commits(sha) ON DELETE CASCADE,
         path       TEXT    NOT NULL,             -- current (post-rename) repo-relative path
         added      INTEGER,                      -- NULL = binary
         deleted    INTEGER,                      -- NULL = binary
         old_path   TEXT,                         -- pre-rename path, else NULL
         PRIMARY KEY (commit_sha, path)
     ) STRICT;
     CREATE INDEX file_changes_path ON file_changes (path);

     -- The single-row mining cursor (id pinned to 1). `mined_through` is the
     -- newest commit incorporated; the next mine reads only `mined_through..HEAD`
     -- (FR-GH-02). `git_version` pins the rename-following output shape for
     -- golden reproducibility (ADR-22).
     CREATE TABLE mine_state (
         id            INTEGER PRIMARY KEY CHECK (id = 1),
         head_sha      TEXT NOT NULL,
         mined_through TEXT NOT NULL,
         git_version   TEXT NOT NULL
     ) STRICT;",
    ),
    (
        2,
        // [S-047] — the temporal-metric layer on the same forward-only track.
        //
        // (a) `commits.subject`: the defect-history heuristic (FR-GH-05) matches
        //     `defect_patterns` against the commit subject. ADD COLUMN on a STRICT
        //     table is supported; nullable so a row mined by an older binary stays
        //     coherent (a NULL subject simply never matches — never fabricated).
        //
        // (b) `temporal_snapshots`: the append-only series (FR-GH-09). One row per
        //     temporal evaluation carrying project-level metric aggregates plus the
        //     provenance every snapshot pins (BR-27): the HEAD SHA, the
        //     mined-through SHA, the local git version, the HEAD committer
        //     timestamp (the determinism clock), and the effective `[history]`
        //     config hash. Append-only, mirroring `metric_snapshots` discipline
        //     (FR-QM-07); `created_at` is recording-time provenance only and is
        //     NEVER an input to any metric formula. `logos.db` is untouched.
        "ALTER TABLE commits ADD COLUMN subject TEXT;

         CREATE TABLE temporal_snapshots (
             id                INTEGER PRIMARY KEY,
             created_at        INTEGER NOT NULL,   -- wall-clock recording time (provenance only)
             head_sha          TEXT    NOT NULL,   -- the evaluated HEAD (FR-GH-09)
             mined_through     TEXT    NOT NULL,   -- newest commit incorporated
             git_version       TEXT    NOT NULL,   -- local git (ADR-22 reproducibility)
             config_hash       TEXT    NOT NULL,   -- EffectiveHistory::hash (BR-27, FR-GH-09)
             window_months     INTEGER NOT NULL,   -- the resolved window
             head_committed_at INTEGER NOT NULL,   -- the determinism clock (BR-27)
             file_count        INTEGER NOT NULL,   -- files with in-window activity
             total_commits     INTEGER NOT NULL,   -- distinct in-window commits
             total_added       INTEGER NOT NULL,   -- summed added lines (binary = 0)
             total_deleted     INTEGER NOT NULL,   -- summed deleted lines
             defect_commits    INTEGER NOT NULL,   -- distinct in-window fix-commits (heuristic)
             max_churn_commits INTEGER NOT NULL,   -- max per-file commit count
             mean_ownership_dispersion_bp INTEGER NOT NULL, -- mean over files, /10000
             mean_change_entropy_bp       INTEGER NOT NULL  -- mean over files, /10000
         ) STRICT;
         CREATE INDEX temporal_snapshots_created ON temporal_snapshots (created_at);",
    ),
    // ── Migration 3 (S-049, CR-007, ADR-23): the coverage half of the evidence
    // store. Coverage-namespaced tables on this SAME forward-only history.db
    // track — `logos.db` is untouched (BR-28) and never ATTACH-ed. All four
    // survive a full `index` because history.db is a separate file the graph
    // rebuild never opens (FR-CV-02). Appended as migration 3 after S-047's
    // migration 2 (the temporal series) when the two parallel I2 sessions were
    // assembled — the track stays dense and 1-based.
    (
        3,
        // One snapshot per ingest-time HEAD SHA (FR-CV-02, FR-CV-04). Same-HEAD
        // ingests merge into the open snapshot; a new HEAD opens a new one and
        // prior snapshots are retained as queryable provenance. `config_hash` is
        // the effective [coverage] hash recorded at snapshot creation (FR-CV-09).
        "CREATE TABLE coverage_snapshots (
             id          INTEGER PRIMARY KEY,
             head_sha    TEXT    NOT NULL UNIQUE,  -- ingest-time HEAD (provenance anchor)
             config_hash TEXT    NOT NULL,         -- effective [coverage] hash (FR-CV-09)
             created_at  INTEGER NOT NULL          -- unixepoch() at first ingest into this snapshot
         ) STRICT;

         -- One row per ingested report merged into a snapshot (FR-CV-04 merge
         -- provenance; FR-CV-06 'report count'). `report_hash` makes a re-ingest
         -- of the SAME report a no-op (UAT-CV-01 idempotency) while DIFFERENT
         -- reports at the same HEAD merge (summed line hits).
         CREATE TABLE coverage_reports (
             snapshot_id INTEGER NOT NULL REFERENCES coverage_snapshots(id) ON DELETE CASCADE,
             report_hash TEXT    NOT NULL,         -- blake3 of the report file bytes
             format      TEXT    NOT NULL,         -- 'lcov' | 'cobertura' (detected/forced)
             ingested_at INTEGER NOT NULL,
             PRIMARY KEY (snapshot_id, report_hash)
         ) STRICT, WITHOUT ROWID;

         -- One row per covered repo file in a snapshot, anchored by the file's
         -- content hash at ingest (FR-CV-02 per-file anchoring; the FR-CV-05
         -- freshness read compares the current hash against this). `path` is the
         -- repo-relative indexed path the report path mapped to (FR-CV-03).
         CREATE TABLE coverage_files (
             id           INTEGER PRIMARY KEY,
             snapshot_id  INTEGER NOT NULL REFERENCES coverage_snapshots(id) ON DELETE CASCADE,
             path         TEXT    NOT NULL,        -- repo-relative indexed path
             content_hash TEXT    NOT NULL,        -- blake3 of file content at ingest (anchor)
             UNIQUE (snapshot_id, path)
         ) STRICT;
         CREATE INDEX coverage_files_path ON coverage_files (path);

         -- One row per instrumented line of a covered file. `hits` SUM on a
         -- same-HEAD merge (FR-CV-04, LCOV's own merge rule). File-level coverage
         -- % is derived on read: instrumented = COUNT(*), covered = COUNT(hits>0).
         CREATE TABLE coverage_lines (
             file_id INTEGER NOT NULL REFERENCES coverage_files(id) ON DELETE CASCADE,
             line_no INTEGER NOT NULL,
             hits    INTEGER NOT NULL,
             PRIMARY KEY (file_id, line_no)
         ) STRICT, WITHOUT ROWID;",
    ),
];

/// Open (creating if absent) and migrate `history.db` at `path`.
///
/// Applies the same pragma contract as the other two stores: WAL journalling
/// and a busy timeout. Foreign keys are enabled so the `file_changes → commits`
/// cascade is live (a re-mine that replaces a commit's facts is one delete +
/// re-insert).
///
/// # Errors
/// Returns an error if the file cannot be opened or a migration fails.
pub(crate) fn open(path: &Path) -> Result<Connection> {
    let mut conn = Connection::open(path)
        .with_context(|| format!("opening history store at {}", path.display()))?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 250;",
    )
    .context("applying the history pragma contract")?;
    apply_migrations(&mut conn)?;
    Ok(conn)
}

/// Apply every embedded migration newer than the store's `user_version` — one
/// transaction per migration, all or nothing (the shared store discipline).
///
/// `user_version` (a database-header value, not a table) is the authoritative
/// gate, so it can be read before migration 1 creates `schema_versions`, and a
/// rolled-back migration reverts the version pointer with the schema.
fn apply_migrations(conn: &mut Connection) -> Result<()> {
    let current: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .context("reading history user_version")?;

    for &(version, sql) in MIGRATIONS {
        if version <= current {
            continue;
        }
        let tx = conn
            .transaction()
            .with_context(|| format!("opening transaction for history migration {version}"))?;
        tx.execute_batch(sql)
            .with_context(|| format!("applying history migration {version}"))?;
        tx.execute(
            "INSERT INTO schema_versions (version, applied_at) VALUES (?1, unixepoch())",
            [version],
        )
        .with_context(|| format!("recording history migration {version}"))?;
        tx.pragma_update(None, "user_version", version)
            .with_context(|| format!("advancing history user_version to {version}"))?;
        tx.commit()
            .with_context(|| format!("committing history migration {version}"))?;
    }
    Ok(())
}

/// The newest schema version the embedded ledger knows about — the version a
/// fully migrated `history.db` reports on its own, independent track. Test-only
/// for now (consumed by [`super::latest_schema_version`] and the migration
/// tests); [S-047] promotes it alongside its reader.
///
/// [S-047]: ../../../docs/planning/journal.md#s-047-temporal-metrics-co-change-and-defect-heuristic
#[cfg(test)]
pub(crate) fn latest_version() -> i64 {
    MIGRATIONS.last().map(|&(version, _)| version).unwrap_or(0)
}

/// The store's current schema version (`PRAGMA user_version`). Test-only for now
/// (see [`latest_version`]).
///
/// # Errors
/// Returns an error if the pragma cannot be read.
#[cfg(test)]
pub(crate) fn current_version(conn: &Connection) -> Result<i64> {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
        .context("reading history user_version")
}

/// The HEAD the store was last mined through, or `None` if nothing was mined yet
/// (a fresh store). The newest mined commit is `mined_through`; an unchanged
/// HEAD short-circuits the next mine to zero commits ([FR-GH-02]).
///
/// # Errors
/// Returns an error if the cursor row cannot be read.
pub(crate) fn mine_cursor(conn: &Connection) -> Result<Option<MineCursor>> {
    conn.query_row(
        "SELECT head_sha, mined_through FROM mine_state WHERE id = 1",
        [],
        |row| {
            Ok(MineCursor {
                head_sha: row.get(0)?,
                mined_through: row.get(1)?,
            })
        },
    )
    .map(Some)
    .or_else(|err| match err {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other).context("reading the history mine cursor"),
    })
}

/// The local `git --version` recorded by the last mine, or `"unknown"` if the
/// store was never mined — the value [`super::temporal`] pins in each snapshot
/// for golden reproducibility ([ADR-22], [FR-GH-09]).
///
/// # Errors
/// Returns an error if the cursor row cannot be read.
pub(crate) fn stored_git_version(conn: &Connection) -> Result<String> {
    conn.query_row(
        "SELECT git_version FROM mine_state WHERE id = 1",
        [],
        |row| row.get(0),
    )
    .or_else(|err| match err {
        rusqlite::Error::QueryReturnedNoRows => Ok("unknown".to_string()),
        other => Err(other).context("reading the stored git version"),
    })
}

/// The persisted mining cursor: the HEAD the store was mined through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MineCursor {
    /// HEAD commit id at the last mine.
    pub(crate) head_sha: String,
    /// Newest commit incorporated — the lower bound of the next incremental mine.
    pub(crate) mined_through: String,
}

/// Persist a batch of mined commits and their file-change facts, then advance
/// the cursor to `head_sha` / `mined_through`, all in **one** transaction
/// ([NFR-RA-07] atomic write — a crash mid-mine leaves the store at its prior
/// coherent state, and the next mine re-reads from the old cursor).
///
/// Commit rows upsert by `sha` (immutable, so `DO NOTHING`); file-change rows
/// upsert by `(commit_sha, path)`. Idempotent: re-mining a commit already in the
/// store changes nothing, which is what makes the ancestor-fallback full re-mine
/// safe.
///
/// # Errors
/// Returns an error if the transaction cannot commit.
pub(crate) fn persist_mine(
    conn: &mut Connection,
    commits: &[CommitFact],
    changes: &[FileChange],
    head_sha: &str,
    mined_through: &str,
    git_version: &str,
) -> Result<()> {
    let tx = conn
        .transaction()
        .context("opening the history mine transaction")?;
    {
        let mut commit_stmt = tx
            .prepare_cached(
                "INSERT INTO commits (sha, committed_at, author_name, author_email, subject, file_count)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (sha) DO NOTHING",
            )
            .context("preparing the commit insert")?;
        for c in commits {
            commit_stmt
                .execute(rusqlite::params![
                    c.sha,
                    c.committed_at,
                    c.author_name,
                    c.author_email,
                    c.subject,
                    c.file_count as i64,
                ])
                .with_context(|| format!("inserting commit {}", c.sha))?;
        }

        let mut change_stmt = tx
            .prepare_cached(
                "INSERT INTO file_changes (commit_sha, path, added, deleted, old_path)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (commit_sha, path) DO UPDATE SET
                     added    = excluded.added,
                     deleted  = excluded.deleted,
                     old_path = excluded.old_path",
            )
            .context("preparing the file-change insert")?;
        for ch in changes {
            change_stmt
                .execute(rusqlite::params![
                    ch.commit_sha,
                    ch.path,
                    ch.added,
                    ch.deleted,
                    ch.old_path,
                ])
                .with_context(|| {
                    format!("inserting file change {} in {}", ch.path, ch.commit_sha)
                })?;
        }
    }
    tx.execute(
        "INSERT INTO mine_state (id, head_sha, mined_through, git_version)
         VALUES (1, ?1, ?2, ?3)
         ON CONFLICT (id) DO UPDATE SET
             head_sha      = excluded.head_sha,
             mined_through = excluded.mined_through,
             git_version   = excluded.git_version",
        rusqlite::params![head_sha, mined_through, git_version],
    )
    .context("advancing the history mine cursor")?;
    tx.commit().context("committing the history mine")
}

// ── temporal-metric read & snapshot persist ([S-047]) ───────────────────────

/// One in-window commit, loaded for temporal computation ([FR-GH-03]..[FR-GH-05]).
/// A projection of the `commits` row: only the columns the metrics read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WindowCommit {
    /// Commit id — joins to its [`WindowChange`] rows.
    pub(crate) sha: String,
    /// Committer timestamp (the determinism clock, [BR-27]).
    pub(crate) committed_at: i64,
    /// `.mailmap`-coalesced author email — the ownership/entropy identity.
    pub(crate) author_email: String,
    /// Commit subject — the defect-heuristic match input ([FR-GH-05]). Empty for
    /// a row mined before migration 2 added the column (a NULL → `""`).
    pub(crate) subject: String,
    /// Distinct files this commit touched — the mega-commit cap input ([FR-GH-04]).
    pub(crate) file_count: i64,
}

/// One in-window (commit, file) change, loaded for temporal computation. A
/// projection of the `file_changes` row joined to its in-window commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WindowChange {
    /// The owning commit's sha.
    pub(crate) commit_sha: String,
    /// Current (post-rename) repo-relative path — the metric key.
    pub(crate) path: String,
    /// Lines added (`None` = binary; counted as 0 toward churn).
    pub(crate) added: Option<i64>,
    /// Lines deleted (`None` = binary).
    pub(crate) deleted: Option<i64>,
}

/// Load every commit with `committed_at >= cutoff` (the HEAD-anchored window),
/// ordered canonically by `(committed_at, sha)` so the computation is
/// order-stable ([ADR-08], [NFR-RA-06]). `cutoff = None` means "no lower bound"
/// (the whole store is in-window — a window so large the cutoff is unrepresentable).
///
/// # Errors
/// Returns an error if the query fails.
pub(crate) fn load_window_commits(
    conn: &Connection,
    cutoff: Option<i64>,
) -> Result<Vec<WindowCommit>> {
    let min = cutoff.unwrap_or(i64::MIN);
    let mut stmt = conn
        .prepare_cached(
            "SELECT sha, committed_at, author_email, COALESCE(subject, ''), file_count
               FROM commits
              WHERE committed_at >= ?1
              ORDER BY committed_at ASC, sha ASC",
        )
        .context("preparing the in-window commit load")?;
    let rows = stmt
        .query_map([min], |row| {
            Ok(WindowCommit {
                sha: row.get(0)?,
                committed_at: row.get(1)?,
                author_email: row.get(2)?,
                subject: row.get(3)?,
                file_count: row.get(4)?,
            })
        })
        .context("loading in-window commits")?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("collecting in-window commits")
}

/// Load every file change belonging to an in-window commit, ordered canonically
/// by `(path, commit_sha)` ([ADR-08]).
///
/// # Errors
/// Returns an error if the query fails.
pub(crate) fn load_window_changes(
    conn: &Connection,
    cutoff: Option<i64>,
) -> Result<Vec<WindowChange>> {
    let min = cutoff.unwrap_or(i64::MIN);
    let mut stmt = conn
        .prepare_cached(
            "SELECT fc.commit_sha, fc.path, fc.added, fc.deleted
               FROM file_changes fc
               JOIN commits c ON c.sha = fc.commit_sha
              WHERE c.committed_at >= ?1
              ORDER BY fc.path ASC, fc.commit_sha ASC",
        )
        .context("preparing the in-window change load")?;
    let rows = stmt
        .query_map([min], |row| {
            Ok(WindowChange {
                commit_sha: row.get(0)?,
                path: row.get(1)?,
                added: row.get(2)?,
                deleted: row.get(3)?,
            })
        })
        .context("loading in-window changes")?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("collecting in-window changes")
}

/// The committer timestamp of `sha` from the `commits` table — the temporal
/// "now" anchor ([BR-27]), read from a mined fact (no git subprocess). `None` if
/// the commit is not in the store (e.g. an empty repo, or HEAD predates nothing).
///
/// # Errors
/// Returns an error if the query fails for a reason other than no-rows.
pub(crate) fn committed_at_of(conn: &Connection, sha: &str) -> Result<Option<i64>> {
    conn.query_row(
        "SELECT committed_at FROM commits WHERE sha = ?1",
        [sha],
        |row| row.get(0),
    )
    .map(Some)
    .or_else(|err| match err {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other).context("reading a commit's committer timestamp"),
    })
}

/// The project-level aggregates + provenance of one temporal evaluation — the
/// `temporal_snapshots` row shape ([FR-GH-09]). Built by the computation in
/// [`super::temporal`] and appended by [`persist_temporal_snapshot`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TemporalSnapshotRow {
    pub(crate) head_sha: String,
    pub(crate) mined_through: String,
    pub(crate) git_version: String,
    pub(crate) config_hash: String,
    pub(crate) window_months: u32,
    pub(crate) head_committed_at: i64,
    pub(crate) file_count: i64,
    pub(crate) total_commits: i64,
    pub(crate) total_added: i64,
    pub(crate) total_deleted: i64,
    pub(crate) defect_commits: i64,
    pub(crate) max_churn_commits: i64,
    pub(crate) mean_ownership_dispersion_bp: i64,
    pub(crate) mean_change_entropy_bp: i64,
}

/// Append one row to the `temporal_snapshots` series ([FR-GH-09]). Append-only:
/// the function only ever `INSERT`s — no prior row is mutated, mirroring the
/// `metric_snapshots` discipline ([FR-QM-07]). `created_at` is recording-time
/// provenance (`unixepoch()`), never a metric input ([BR-27]).
///
/// # Errors
/// Returns an error if the insert fails.
pub(crate) fn persist_temporal_snapshot(
    conn: &Connection,
    row: &TemporalSnapshotRow,
) -> Result<()> {
    conn.execute(
        "INSERT INTO temporal_snapshots (
             created_at, head_sha, mined_through, git_version, config_hash,
             window_months, head_committed_at, file_count, total_commits,
             total_added, total_deleted, defect_commits, max_churn_commits,
             mean_ownership_dispersion_bp, mean_change_entropy_bp
         ) VALUES (
             unixepoch(), ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14
         )",
        rusqlite::params![
            row.head_sha,
            row.mined_through,
            row.git_version,
            row.config_hash,
            row.window_months,
            row.head_committed_at,
            row.file_count,
            row.total_commits,
            row.total_added,
            row.total_deleted,
            row.defect_commits,
            row.max_churn_commits,
            row.mean_ownership_dispersion_bp,
            row.mean_change_entropy_bp,
        ],
    )
    .context("appending a temporal snapshot")?;
    Ok(())
}

/// A migrated in-memory `history.db` — the unit-test seam (mirrors
/// `observability::db::open_in_memory`).
#[cfg(test)]
pub(crate) fn open_in_memory() -> Connection {
    let mut conn = Connection::open_in_memory().expect("in-memory history store");
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .expect("history fk pragma");
    apply_migrations(&mut conn).expect("history migrations apply");
    conn
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh store migrates to the latest version on its own track and starts
    /// with an empty cursor — the [FR-GH-01] "independent migration version".
    #[test]
    fn fresh_store_migrates_and_has_no_cursor() {
        let conn = open_in_memory();
        assert_eq!(current_version(&conn).unwrap(), latest_version());
        assert!(latest_version() >= 1, "the ledger has at least migration 1");
        assert_eq!(
            mine_cursor(&conn).unwrap(),
            None,
            "a never-mined store has no cursor"
        );
    }

    /// Persisting commits + file changes and advancing the cursor round-trips,
    /// and the `file_changes → commits` cascade is live (FK on).
    #[test]
    fn persist_round_trips_and_cascades() {
        let mut conn = open_in_memory();
        let commits = vec![CommitFact {
            sha: "a".repeat(40),
            committed_at: 1_000,
            author_name: "Ada".to_string(),
            author_email: "ada@example.com".to_string(),
            subject: "fix: round trip".to_string(),
            file_count: 2,
        }];
        let changes = vec![
            FileChange {
                commit_sha: "a".repeat(40),
                path: "src/lib.rs".to_string(),
                added: Some(10),
                deleted: Some(2),
                old_path: None,
            },
            FileChange {
                commit_sha: "a".repeat(40),
                path: "src/bin.rs".to_string(),
                added: None, // binary
                deleted: None,
                old_path: Some("src/old.rs".to_string()),
            },
        ];
        persist_mine(
            &mut conn,
            &commits,
            &changes,
            &"a".repeat(40),
            &"a".repeat(40),
            "git 2.x",
        )
        .unwrap();

        assert_eq!(
            mine_cursor(&conn).unwrap(),
            Some(MineCursor {
                head_sha: "a".repeat(40),
                mined_through: "a".repeat(40),
            })
        );
        let change_count: i64 = conn
            .query_row("SELECT count(*) FROM file_changes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(change_count, 2);

        // Deleting the commit cascades its file changes (FK ON DELETE CASCADE).
        conn.execute("DELETE FROM commits WHERE sha = ?1", [&"a".repeat(40)])
            .unwrap();
        let after: i64 = conn
            .query_row("SELECT count(*) FROM file_changes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after, 0, "file_changes cascade on commit delete");
    }

    /// Re-persisting the same commit is idempotent (the ancestor-fallback full
    /// re-mine relies on this): commit `DO NOTHING`, file-change upsert.
    #[test]
    fn persist_is_idempotent() {
        let mut conn = open_in_memory();
        let commits = vec![CommitFact {
            sha: "b".repeat(40),
            committed_at: 2_000,
            author_name: "Bo".to_string(),
            author_email: "bo@example.com".to_string(),
            subject: "second".to_string(),
            file_count: 1,
        }];
        let changes = vec![FileChange {
            commit_sha: "b".repeat(40),
            path: "a.rs".to_string(),
            added: Some(1),
            deleted: Some(0),
            old_path: None,
        }];
        let head = "b".repeat(40);
        persist_mine(&mut conn, &commits, &changes, &head, &head, "git").unwrap();
        persist_mine(&mut conn, &commits, &changes, &head, &head, "git").unwrap();

        let commit_count: i64 = conn
            .query_row("SELECT count(*) FROM commits", [], |r| r.get(0))
            .unwrap();
        let change_count: i64 = conn
            .query_row("SELECT count(*) FROM file_changes", [], |r| r.get(0))
            .unwrap();
        assert_eq!((commit_count, change_count), (1, 1), "no duplicate rows");
    }

    /// A `TemporalSnapshotRow` round-trips through `persist_temporal_snapshot` —
    /// every one of the 14 bound columns reads back exactly, so a positional
    /// binding offset in the insert is caught here ([FR-GH-09]).
    #[test]
    fn temporal_snapshot_round_trips_every_column() {
        let conn = open_in_memory();
        let row = TemporalSnapshotRow {
            head_sha: "h".repeat(40),
            mined_through: "m".repeat(40),
            git_version: "git version 2.50.1".to_string(),
            config_hash: "deadbeef".to_string(),
            window_months: 9,
            head_committed_at: 1_735_992_000,
            file_count: 7,
            total_commits: 11,
            total_added: 123,
            total_deleted: 45,
            defect_commits: 3,
            max_churn_commits: 6,
            mean_ownership_dispersion_bp: 4321,
            mean_change_entropy_bp: 8765,
        };
        persist_temporal_snapshot(&conn, &row).unwrap();

        let got = conn
            .query_row(
                "SELECT head_sha, mined_through, git_version, config_hash, window_months,
                        head_committed_at, file_count, total_commits, total_added,
                        total_deleted, defect_commits, max_churn_commits,
                        mean_ownership_dispersion_bp, mean_change_entropy_bp
                   FROM temporal_snapshots WHERE id = 1",
                [],
                |r| {
                    Ok(TemporalSnapshotRow {
                        head_sha: r.get(0)?,
                        mined_through: r.get(1)?,
                        git_version: r.get(2)?,
                        config_hash: r.get(3)?,
                        window_months: r.get(4)?,
                        head_committed_at: r.get(5)?,
                        file_count: r.get(6)?,
                        total_commits: r.get(7)?,
                        total_added: r.get(8)?,
                        total_deleted: r.get(9)?,
                        defect_commits: r.get(10)?,
                        max_churn_commits: r.get(11)?,
                        mean_ownership_dispersion_bp: r.get(12)?,
                        mean_change_entropy_bp: r.get(13)?,
                    })
                },
            )
            .unwrap();
        assert_eq!(got, row, "every snapshot column round-trips without offset");

        // created_at was stamped (provenance), and the row is the only one.
        let count: i64 = conn
            .query_row("SELECT count(*) FROM temporal_snapshots", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}
