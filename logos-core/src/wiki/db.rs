//! `wiki.db` — the agent-generated wiki store ([FR-WK-01], [ADR-24], [ADR-13]
//! reapplied a fourth time).
//!
//! Deliberately a **fourth SQLite file** under `.logos/`, beside `logos.db`,
//! `telemetry.db`, and `history.db`. The reasons are the [ADR-13]/[ADR-22]
//! reasons one more time, sharpened by [ADR-24]:
//!
//! - A full `index` rebuilds `logos.db` wholesale; agent-written pages must
//!   survive that ([FR-WK-01]) — a separate file simply isn't touched. Unlike
//!   the graph, wiki content is **not re-derivable** by Logos, so the wipe would
//!   destroy user data ([ADR-24]).
//! - The gated metric path holds no connection to this file and the store is
//!   **never `ATTACH`-ed** to `logos.db`, so "the gate cannot see the wiki" is a
//!   physical property, not a coding convention ([BR-29], [UAT-WK-02]).
//! - Its migration track is **independent**: `wiki.db` starts at
//!   `user_version = 1` and advances on its own, regardless of `logos.db`'s
//!   version ([FR-WK-01]).
//!
//! Schema (migration 1 — the [S-052] substrate the wiki surfaces ride):
//! - `pages` — one row per slug. `body` is the markdown stored **byte-verbatim**
//!   (the [FR-WK-02] round-trip contract; the 1 MiB cap is enforced in
//!   [`super::write`] before any write). `generator` is the mandatory non-empty
//!   label; `written_head` is the write-time HEAD commit (empty when the repo has
//!   no resolvable HEAD — the wiki stays functional out of the box, [ADR-24]).
//! - `page_anchors` — zero or more rows per page, each anchoring the page to one
//!   graph entity by its **stable** id (`file:<path>` / `symbol:<symbol>`, never a
//!   rowid — rowids do not survive the `index` rebuild this store outlives) and
//!   the content hash captured at write ([FR-WK-02]). Read-time freshness
//!   ([FR-WK-03]) compares the stored hash against the current tree.
//! - `pruned_log` — append-only record of every all-anchors-gone auto-deletion
//!   ([FR-WK-07]) so `wiki status` ([FR-WK-06], S-053) can surface what the
//!   hygiene lifecycle removed and the agent regenerates deliberately. Migration
//!   4 ([FR-WK-22]) also logs here — the forward-only per-file `files/%`
//!   retirement runs the same durable trace as the lazy orphan prune.
//!
//! Migrations follow the same forward-only `user_version` discipline as
//! [`crate::graph_store`] and [`crate::history::db`] — one transaction per
//! migration, all-or-nothing ([NFR-RA-07]).
//!
//! [FR-WK-01]: ../../../docs/specs/requirements/FR-WK-01.md
//! [FR-WK-02]: ../../../docs/specs/requirements/FR-WK-02.md
//! [FR-WK-03]: ../../../docs/specs/requirements/FR-WK-03.md
//! [FR-WK-06]: ../../../docs/specs/requirements/FR-WK-06.md
//! [FR-WK-07]: ../../../docs/specs/requirements/FR-WK-07.md
//! [FR-WK-22]: ../../../docs/specs/requirements/FR-WK-22.md
//! [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
//! [BR-29]: ../../../docs/specs/software-spec.md#324-source-wiki
//! [UAT-WK-02]: ../../../docs/specs/requirements/UAT-WK-02.md
//! [ADR-13]: ../../../docs/specs/architecture/decisions/ADR-13.md
//! [ADR-22]: ../../../docs/specs/architecture/decisions/ADR-22.md
//! [ADR-24]: ../../../docs/specs/architecture/decisions/ADR-24.md
//! [S-052]: ../../../docs/planning/journal.md#s-052-wiki-store-write-path-and-page-lifecycle

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};

use super::{Anchor, PageDraft, StoredAnchor};

/// Forward-only migration ledger for `wiki.db` — dense, 1-based, strictly
/// increasing (mirrors `graph_store::schema::MIGRATIONS` and
/// `history::db::MIGRATIONS`). Migration 1 is **never** edited; later wiki
/// stories (FTS5 for [FR-WK-05], S-053) append migration 2+ on this same track.
const MIGRATIONS: &[(i64, &str)] = &[
    (
        1,
        "CREATE TABLE schema_versions (
         version    INTEGER PRIMARY KEY,
         applied_at INTEGER NOT NULL
     ) STRICT;

     -- One row per slug (last-write-wins, no revision history — FR-WK-02).
     -- `body` is the markdown stored byte-verbatim; the documented 1 MiB cap is
     -- enforced in the write path before this row is touched, so a rejected
     -- over-cap write never reaches the store. `generator` is the mandatory
     -- non-empty label; `written_head` is the write-time HEAD SHA ('' when the
     -- repo has no resolvable HEAD). `written_at` is recording-time provenance
     -- only, NEVER an input to freshness (freshness is a pure function of the
     -- anchor hashes + the current tree, FR-WK-03 / NFR-RA-06).
     CREATE TABLE pages (
         slug         TEXT    PRIMARY KEY,        -- validated path-like slug
         title        TEXT    NOT NULL,
         body         TEXT    NOT NULL,           -- byte-verbatim markdown
         generator    TEXT    NOT NULL,           -- mandatory non-empty label
         written_head TEXT    NOT NULL,           -- write-time HEAD SHA, '' if none
         written_at   INTEGER NOT NULL            -- unix seconds (provenance only)
     ) STRICT;

     -- Zero or more anchors per page, in stable input order (`ordinal`). Each
     -- anchors to a graph entity by its STABLE id — `file:<path>` or
     -- `symbol:<symbol>`, never a storage rowid (rowids are reassigned by the
     -- `index` rebuild this store survives). `content_hash` is captured at write
     -- and compared at read for per-anchor freshness (FR-WK-03). ON DELETE
     -- CASCADE so an explicit/auto delete of a page removes its anchors.
     CREATE TABLE page_anchors (
         slug         TEXT    NOT NULL REFERENCES pages(slug) ON DELETE CASCADE,
         ordinal      INTEGER NOT NULL,           -- stable input order
         kind         TEXT    NOT NULL,           -- 'file' | 'symbol'
         entity_id    TEXT    NOT NULL,           -- the stable key (path or symbol)
         content_hash TEXT    NOT NULL,           -- hash captured at write
         PRIMARY KEY (slug, ordinal)
     ) STRICT;
     CREATE INDEX page_anchors_entity ON page_anchors (kind, entity_id);

     -- Append-only record of every all-anchors-gone auto-deletion (FR-WK-07).
     -- `wiki status` (FR-WK-06, S-053) surfaces these so the agent regenerates
     -- deliberately. `pruned_at` is recording-time provenance only.
     CREATE TABLE pruned_log (
         id         INTEGER PRIMARY KEY,
         slug       TEXT    NOT NULL,
         title      TEXT    NOT NULL,
         pruned_at  INTEGER NOT NULL              -- unix seconds (provenance only)
     ) STRICT;",
    ),
    (
        2,
        // Migration 2 — FTS5 bm25 search over page titles + bodies, indexed
        // INSIDE wiki.db so search survives a full `index` exactly like the
        // pages it indexes (FR-WK-05). External-content (`content='pages'`,
        // content_rowid defaults to `rowid`) stores no second copy of the
        // body; the index reads `title`/`body` back from `pages` by rowid.
        // Sync is OUR responsibility, via the triggers below — the same
        // discipline `graph_store`'s `nodes_fts` follows (NFR-RA-09); a plain
        // DELETE without the special `('delete', …)` row would silently desync
        // the inverted index (SRS §16.8 trap). `unicode61` is the offline
        // tokenizer FR-WK-05 names; no vectors (NFR-SE-01, ADR-24).
        //
        // An upsert (`INSERT … ON CONFLICT DO UPDATE`) keeps a page's rowid, so
        // a re-write fires the UPDATE trigger (retract-then-reindex) and the
        // index tracks the new title/body. The backfill re-indexes any rows a
        // pre-upgrade (migration-1-only) build already wrote, so search is
        // complete on first use after the upgrade (forward-only discipline).
        "CREATE VIRTUAL TABLE pages_fts USING fts5(
             title,
             body,
             content='pages',
             content_rowid='rowid',
             tokenize='unicode61'
         );

         CREATE TRIGGER pages_fts_ai AFTER INSERT ON pages BEGIN
             INSERT INTO pages_fts(rowid, title, body)
             VALUES (new.rowid, new.title, new.body);
         END;

         CREATE TRIGGER pages_fts_ad AFTER DELETE ON pages BEGIN
             INSERT INTO pages_fts(pages_fts, rowid, title, body)
             VALUES ('delete', old.rowid, old.title, old.body);
         END;

         CREATE TRIGGER pages_fts_au AFTER UPDATE ON pages BEGIN
             INSERT INTO pages_fts(pages_fts, rowid, title, body)
             VALUES ('delete', old.rowid, old.title, old.body);
             INSERT INTO pages_fts(rowid, title, body)
             VALUES (new.rowid, new.title, new.body);
         END;

         -- Backfill rows written by a migration-1-only build, if any.
         INSERT INTO pages_fts(rowid, title, body)
             SELECT rowid, title, body FROM pages;",
    ),
    (
        3,
        // Migration 3 — the per-page **built-at graph revision** ([FR-WK-12],
        // [CR-027], [ADR-32]). Captured at write from the persisted graph
        // revision ([FR-SY-09]); the two-tier view derives the agent page's
        // "stale — regeneration pending" verdict by comparing it to the current
        // revision, with NO write on the page view ([ADR-28]). `0` is the
        // honest default for a page written by a pre-upgrade build that had no
        // revision to record — it reads as "built before revision 1", i.e.
        // stale once the graph has any revision, never masquerading as fresh
        // ([NFR-CC-04]). Provenance only — never an input to per-anchor
        // freshness ([FR-WK-03], [NFR-RA-06]).
        "ALTER TABLE pages ADD COLUMN built_at_revision INTEGER NOT NULL DEFAULT 0;",
    ),
    (
        4,
        // Migration 4 — retire every per-file `files/%` page outright
        // ([FR-WK-22], [CR-062]). Per-file objectives seeding was already
        // stopped ([CR-056]), but historical instances persisted and could
        // still be refreshed via the stale path; this is a store where 220 of
        // them had accumulated, unreachable from the menu and un-regenerable.
        // The `INSERT` runs first so every doomed row is logged to
        // `pruned_log` (the same durable trace [`prune_page`] writes) before
        // the `DELETE` removes it, in the one migration transaction — forward-
        // only and idempotent (a re-run finds no `files/%` rows left, so both
        // statements affect zero rows). Anchors cascade off the page delete.
        // No LLM, no network — a pure store operation ([NFR-SE-01]).
        "INSERT INTO pruned_log (slug, title, pruned_at)
             SELECT slug, title, unixepoch() FROM pages WHERE slug LIKE 'files/%';
         DELETE FROM pages WHERE slug LIKE 'files/%';",
    ),
];

/// The resolved `.logos/wiki.db` path for a worktree `root`.
pub(crate) fn db_path(root: &Path) -> std::path::PathBuf {
    root.join(".logos").join("wiki.db")
}

/// Open (creating if absent) and migrate `wiki.db` at `path`.
///
/// Applies the same pragma contract as the other stores: WAL journalling and a
/// busy timeout. Foreign keys are enabled so the `page_anchors → pages` cascade
/// is live (an upsert replaces a page's anchors as one delete + re-insert).
///
/// # Errors
/// Returns an error if the file cannot be opened or a migration fails.
pub(crate) fn open(path: &Path) -> Result<Connection> {
    let mut conn = Connection::open(path)
        .with_context(|| format!("opening the wiki store at {}", path.display()))?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 250;",
    )
    .context("applying the wiki pragma contract")?;
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
        .context("reading wiki user_version")?;

    for &(version, sql) in MIGRATIONS {
        if version <= current {
            continue;
        }
        let tx = conn
            .transaction()
            .with_context(|| format!("opening transaction for wiki migration {version}"))?;
        tx.execute_batch(sql)
            .with_context(|| format!("applying wiki migration {version}"))?;
        tx.execute(
            "INSERT INTO schema_versions (version, applied_at) VALUES (?1, unixepoch())",
            [version],
        )
        .with_context(|| format!("recording wiki migration {version}"))?;
        tx.pragma_update(None, "user_version", version)
            .with_context(|| format!("advancing wiki user_version to {version}"))?;
        tx.commit()
            .with_context(|| format!("committing wiki migration {version}"))?;
    }
    Ok(())
}

/// The newest schema version the embedded ledger knows about — the version a
/// fully migrated `wiki.db` reports on its own, independent track ([FR-WK-01]).
pub(crate) fn latest_version() -> i64 {
    MIGRATIONS.last().map(|&(version, _)| version).unwrap_or(0)
}

/// The store's current schema version (`PRAGMA user_version`) — the value the
/// migration test asserts equals [`latest_version`] on the wiki's own track
/// ([FR-WK-01]). Test-only (mirrors `history::db::current_version`); a production
/// reader promotes it when one lands.
///
/// # Errors
/// Returns an error if the pragma cannot be read.
#[cfg(test)]
pub(crate) fn current_version(conn: &Connection) -> Result<i64> {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
        .context("reading wiki user_version")
}

/// A page row plus its anchors, loaded for a read or freshness evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredPage {
    pub(crate) slug: String,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) generator: String,
    pub(crate) written_head: String,
    /// The persisted graph revision captured at write ([FR-WK-12], [FR-SY-09]).
    /// Provenance only — never an input to per-anchor freshness.
    pub(crate) built_at_revision: u64,
    pub(crate) anchors: Vec<StoredAnchor>,
}

/// Upsert a page and its anchors in **one** transaction (last-write-wins,
/// [FR-WK-02]). The page row is replaced by `slug`; its anchors are deleted and
/// re-inserted, so a second write to the same slug records the second write's
/// HEAD and anchor hashes ([FR-WK-02] acceptance). The caller has already
/// validated the slug/body/generator and resolved every anchor — this function
/// is reached only on a write that is guaranteed to succeed, so a rejected write
/// leaves the store byte-identical because it never gets here.
///
/// # Errors
/// Returns an error if the transaction cannot commit.
pub(crate) fn upsert_page(
    conn: &mut Connection,
    draft: &PageDraft<'_>,
    anchors: &[(Anchor, String)],
) -> Result<()> {
    let slug = draft.slug;
    let tx = conn
        .transaction()
        .context("opening the wiki upsert transaction")?;
    tx.execute(
        "INSERT INTO pages
             (slug, title, body, generator, written_head, written_at, built_at_revision)
         VALUES (?1, ?2, ?3, ?4, ?5, unixepoch(), ?6)
         ON CONFLICT (slug) DO UPDATE SET
             title             = excluded.title,
             body              = excluded.body,
             generator         = excluded.generator,
             written_head      = excluded.written_head,
             written_at        = excluded.written_at,
             built_at_revision = excluded.built_at_revision",
        rusqlite::params![
            slug,
            draft.title,
            draft.body,
            draft.generator,
            draft.written_head,
            draft.built_at_revision as i64,
        ],
    )
    .with_context(|| format!("upserting wiki page {slug}"))?;

    // Replace the anchor set wholesale: a re-write may add, drop, or reorder
    // anchors, and last-write-wins means the previous set carries nothing.
    tx.execute("DELETE FROM page_anchors WHERE slug = ?1", [slug])
        .with_context(|| format!("clearing prior anchors of {slug}"))?;
    {
        let mut stmt = tx
            .prepare_cached(
                "INSERT INTO page_anchors (slug, ordinal, kind, entity_id, content_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .context("preparing the anchor insert")?;
        for (ordinal, (anchor, content_hash)) in anchors.iter().enumerate() {
            stmt.execute(rusqlite::params![
                slug,
                ordinal as i64,
                anchor.kind.as_str(),
                anchor.entity_id,
                content_hash,
            ])
            .with_context(|| format!("inserting anchor {} of {slug}", anchor.entity_id))?;
        }
    }
    tx.commit().context("committing the wiki upsert")
}

/// Load one page and its anchors by slug, or `None` if no such page exists.
///
/// Anchors come back in stored `ordinal` order so the read-model is
/// deterministic ([NFR-RA-06]).
///
/// # Errors
/// Returns an error on an unexpected store failure.
pub(crate) fn load_page(conn: &Connection, slug: &str) -> Result<Option<StoredPage>> {
    let row = conn
        .query_row(
            "SELECT title, body, generator, written_head, built_at_revision
             FROM pages WHERE slug = ?1",
            [slug],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()
        .with_context(|| format!("loading wiki page {slug}"))?;
    let Some((title, body, generator, written_head, built_at_revision)) = row else {
        return Ok(None);
    };
    let anchors = load_anchors(conn, slug)?;
    Ok(Some(StoredPage {
        slug: slug.to_string(),
        title,
        body,
        generator,
        written_head,
        built_at_revision: built_at_revision as u64,
        anchors,
    }))
}

/// The anchors of one page, in stored `ordinal` order.
fn load_anchors(conn: &Connection, slug: &str) -> Result<Vec<StoredAnchor>> {
    let mut stmt = conn
        .prepare_cached(
            "SELECT kind, entity_id, content_hash FROM page_anchors
             WHERE slug = ?1 ORDER BY ordinal",
        )
        .context("preparing the anchor load")?;
    let rows = stmt
        .query_map([slug], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .with_context(|| format!("loading anchors of {slug}"))?;
    let mut anchors = Vec::new();
    for row in rows {
        let (kind, entity_id, content_hash) = row.context("reading an anchor row")?;
        anchors.push(StoredAnchor {
            anchor: Anchor::from_parts(&kind, entity_id)?,
            content_hash,
        });
    }
    Ok(anchors)
}

/// Delete a page by slug, returning `true` if a row was removed. Anchors cascade.
///
/// # Errors
/// Returns an error on an unexpected store failure.
pub(crate) fn delete_page(conn: &Connection, slug: &str) -> Result<bool> {
    let removed = conn
        .execute("DELETE FROM pages WHERE slug = ?1", [slug])
        .with_context(|| format!("deleting wiki page {slug}"))?;
    Ok(removed > 0)
}

/// Auto-delete an all-anchors-gone page and record the pruning, in one
/// transaction ([FR-WK-07]). The `pruned_log` row is the durable trace
/// `wiki status` surfaces; the delete cascades the page's anchors.
///
/// # Errors
/// Returns an error if the transaction cannot commit.
pub(crate) fn prune_page(conn: &mut Connection, slug: &str, title: &str) -> Result<()> {
    let tx = conn
        .transaction()
        .context("opening the wiki prune transaction")?;
    tx.execute(
        "INSERT INTO pruned_log (slug, title, pruned_at) VALUES (?1, ?2, unixepoch())",
        rusqlite::params![slug, title],
    )
    .with_context(|| format!("recording the prune of {slug}"))?;
    tx.execute("DELETE FROM pages WHERE slug = ?1", [slug])
        .with_context(|| format!("pruning wiki page {slug}"))?;
    tx.commit().context("committing the wiki prune")
}

/// Bulk-delete every stored page whose slug is **not** in `valid_slugs`,
/// logging each removal to the pruned-log in one transaction — the bulk form of
/// [`prune_page`] the [FR-WK-22] reconciliation sweep drives (`super::reconcile`).
/// Slug membership is an exact-set check, never a heuristic or a glob; a page
/// whose slug is present in `valid_slugs` is never touched, and every removal
/// lands in the same durable `pruned_log` trace `prune_page` writes ([FR-WK-07]).
/// Idempotent: once every remaining page's slug is in `valid_slugs`, a re-run
/// selects zero rows and commits a no-op transaction. A pure store operation —
/// no LLM, no network call ([NFR-SE-01]).
///
/// Returns the removed slugs, in slug order.
///
/// # Errors
/// Returns an error if the transaction cannot commit.
pub(crate) fn delete_pages_not_in(
    conn: &mut Connection,
    valid_slugs: &HashSet<String>,
) -> Result<Vec<String>> {
    let tx = conn
        .transaction()
        .context("opening the wiki reconciliation transaction")?;
    let doomed: Vec<(String, String)> = {
        let mut stmt = tx
            .prepare("SELECT slug, title FROM pages ORDER BY slug")
            .context("preparing the reconciliation scan")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .context("scanning pages for reconciliation")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting pages for reconciliation")?
    }
    .into_iter()
    .filter(|(slug, _)| !valid_slugs.contains(slug))
    .collect();

    for (slug, title) in &doomed {
        tx.execute(
            "INSERT INTO pruned_log (slug, title, pruned_at) VALUES (?1, ?2, unixepoch())",
            rusqlite::params![slug, title],
        )
        .with_context(|| format!("recording the reconciliation prune of {slug}"))?;
        tx.execute("DELETE FROM pages WHERE slug = ?1", [slug])
            .with_context(|| format!("reconciliation-pruning wiki page {slug}"))?;
    }
    tx.commit()
        .context("committing the wiki reconciliation sweep")?;

    Ok(doomed.into_iter().map(|(slug, _)| slug).collect())
}

/// One recorded auto-deletion ([FR-WK-07]) — the `wiki status` work-list reads
/// these (S-053). `pruned_at` is recording-time provenance only.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PrunedPage {
    /// The slug that was auto-deleted.
    pub slug: String,
    /// Its title at deletion time.
    pub title: String,
    /// Unix-seconds recording time (provenance only).
    pub pruned_at: i64,
}

/// Every recorded pruning, newest first ([FR-WK-07]). Test-and-S-053 reader.
///
/// # Errors
/// Returns an error on an unexpected store failure.
pub(crate) fn pruned_log(conn: &Connection) -> Result<Vec<PrunedPage>> {
    let mut stmt = conn
        .prepare_cached("SELECT slug, title, pruned_at FROM pruned_log ORDER BY id DESC")
        .context("preparing the pruned-log read")?;
    let rows = stmt
        .query_map([], |row| {
            Ok(PrunedPage {
                slug: row.get(0)?,
                title: row.get(1)?,
                pruned_at: row.get(2)?,
            })
        })
        .context("reading the pruned log")?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("collecting the pruned log")
}

/// Slugs of pages matching an FTS5 query, ranked by bm25 then slug ([FR-WK-05]).
///
/// `fts_query` is a complete, already-quoted FTS5 MATCH expression (see
/// [`super::fts_phrase_query`]) bound as a parameter — caller text never enters
/// the SQL string ([NFR-SE-02] injection boundary). `ORDER BY rank` is FTS5's
/// bm25 ordering; the `, slug` tiebreak keeps equally-ranked hits in a stable,
/// deterministic order ([NFR-RA-06]).
///
/// # Errors
/// Returns an error on an unexpected store failure.
pub(crate) fn search_slugs(conn: &Connection, fts_query: &str, limit: i64) -> Result<Vec<String>> {
    let mut stmt = conn
        .prepare_cached(
            "SELECT p.slug FROM pages_fts
             JOIN pages p ON p.rowid = pages_fts.rowid
             WHERE pages_fts MATCH ?1
             ORDER BY rank, p.slug
             LIMIT ?2",
        )
        .context("preparing the wiki search")?;
    let rows = stmt
        .query_map(rusqlite::params![fts_query, limit], |row| {
            row.get::<_, String>(0)
        })
        .context("running the wiki search")?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("collecting wiki search hits")
}

/// Every page slug, in deterministic slug order — the enumeration list mode
/// ([FR-WK-05]) and `wiki status` ([FR-WK-06]) iterate.
///
/// # Errors
/// Returns an error on an unexpected store failure.
pub(crate) fn all_slugs(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn
        .prepare_cached("SELECT slug FROM pages ORDER BY slug")
        .context("preparing the page enumeration")?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .context("enumerating pages")?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("collecting page slugs")
}

/// The `(kind, entity_id)` of every anchor across all pages — the "already has
/// an anchored page" set the `wiki status` work-list subtracts from the
/// page-worthy candidates ([FR-WK-06]).
///
/// # Errors
/// Returns an error on an unexpected store failure.
pub(crate) fn anchored_entities(
    conn: &Connection,
) -> Result<std::collections::HashSet<(String, String)>> {
    let mut stmt = conn
        .prepare_cached("SELECT DISTINCT kind, entity_id FROM page_anchors")
        .context("preparing the anchored-entity read")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .context("reading anchored entities")?;
    rows.collect::<rusqlite::Result<std::collections::HashSet<_>>>()
        .context("collecting anchored entities")
}

/// An in-memory `wiki.db` migrated only up to (and including) migration 3 —
/// i.e. **before** the [FR-WK-22] per-file retirement (migration 4) exists —
/// so a migration-behavior test can seed pre-retirement `files/%` rows
/// directly, then apply migration 4 in isolation via [`migrate`] and assert
/// its effect, mirroring what [`open`] does against a real pre-upgrade file.
#[cfg(test)]
pub(crate) fn open_in_memory_before_migration_4() -> Connection {
    let mut conn = Connection::open_in_memory().expect("in-memory wiki store");
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .expect("wiki fk pragma");
    for &(version, sql) in MIGRATIONS.iter().filter(|&&(v, _)| v < 4) {
        let tx = conn.transaction().expect("opening a seed migration tx");
        tx.execute_batch(sql).expect("applying a seed migration");
        tx.execute(
            "INSERT INTO schema_versions (version, applied_at) VALUES (?1, unixepoch())",
            [version],
        )
        .expect("recording a seed migration");
        tx.pragma_update(None, "user_version", version)
            .expect("advancing user_version");
        tx.commit().expect("committing a seed migration");
    }
    conn
}

/// Apply every embedded migration newer than the connection's current
/// version — the test-only seam over [`apply_migrations`], used to run a
/// specific migration (e.g. migration 4, [`open_in_memory_before_migration_4`])
/// against hand-seeded pre-migration state.
///
/// # Errors
/// Returns an error if a migration fails.
#[cfg(test)]
pub(crate) fn migrate(conn: &mut Connection) -> Result<()> {
    apply_migrations(conn)
}

/// A migrated in-memory `wiki.db` — the unit-test seam (mirrors
/// `history::db::open_in_memory`).
#[cfg(test)]
pub(crate) fn open_in_memory() -> Connection {
    let mut conn = Connection::open_in_memory().expect("in-memory wiki store");
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .expect("wiki fk pragma");
    apply_migrations(&mut conn).expect("wiki migrations apply");
    conn
}
