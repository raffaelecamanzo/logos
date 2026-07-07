//! Store-level unit tests for the wiki write path, read-time freshness, and the
//! orphan lifecycle ([S-052], [FR-WK-02], [FR-WK-03], [FR-WK-07]).
//!
//! These drive the store through an in-memory `wiki.db` with a **fake**
//! [`AnchorResolver`] — a `(anchor id → current hash)` map — so the write
//! contract, the three freshness verdicts, and the prune lifecycle are exercised
//! deterministically without git or disk. The rebuild-survival and gate-immunity
//! fixtures, which need a real `Engine` + tree, live in `tests/wiki_store.rs`.
//!
//! [S-052]: ../../../docs/planning/journal.md#s-052-wiki-store-write-path-and-page-lifecycle

use std::collections::HashMap;

use anyhow::Result;

use super::*;

/// A map-backed resolver: present-with-`Some` → that hash; present-with-`None`
/// or absent → the entity is gone (missing). Mutated between a write and a read
/// to simulate edits/renames.
#[derive(Default)]
struct FakeResolver {
    hashes: HashMap<String, Option<String>>,
}

impl FakeResolver {
    fn with(anchor: &str, hash: Option<&str>) -> Self {
        let mut r = FakeResolver::default();
        r.set(anchor, hash);
        r
    }
    fn set(&mut self, anchor: &str, hash: Option<&str>) {
        self.hashes
            .insert(anchor.to_string(), hash.map(str::to_string));
    }
}

impl AnchorResolver for FakeResolver {
    fn resolve(&self, anchor: &Anchor) -> Result<Option<String>> {
        Ok(self.hashes.get(&anchor.as_id()).cloned().flatten())
    }
}

/// A combined fake resolver + catalog for the `wiki status` work-list tests:
/// the hash map drives freshness, the candidate list drives the page-worthy
/// discovery — both without a real graph.
#[derive(Default)]
struct FakeWorld {
    resolver: FakeResolver,
    candidates: Vec<CandidateEntity>,
    /// CR-034: the consolidated doc categories whose source files "exist" — the
    /// off-disk stand-in for [`EntityCatalog::present_doc_categories`].
    doc_categories: Vec<DocCategory>,
    /// CR-034: the repo-relative `docs/` files that "exist" — the off-disk
    /// stand-in for [`EntityCatalog::doc_file_present`].
    present_docs: std::collections::HashSet<String>,
}

impl FakeWorld {
    fn module(mut self, symbol: &str, name: &str) -> Self {
        self.candidates.push(CandidateEntity {
            kind: EntityKind::Module,
            entity_id: symbol.to_string(),
            name: name.to_string(),
        });
        self
    }
    fn file(mut self, path: &str) -> Self {
        self.candidates.push(CandidateEntity {
            kind: EntityKind::File,
            entity_id: path.to_string(),
            name: path.to_string(),
        });
        self
    }
    fn fresh(mut self, anchor: &str, hash: &str) -> Self {
        self.resolver.set(anchor, Some(hash));
        self
    }
    /// CR-034: mark a consolidated doc category as having present source files.
    fn doc_category(mut self, category: DocCategory) -> Self {
        self.doc_categories.push(category);
        self
    }
    /// CR-034: mark a repo-relative `docs/` file as present (overview grounding).
    fn doc_present(mut self, repo_relative: &str) -> Self {
        self.present_docs.insert(repo_relative.to_string());
        self
    }
}

impl AnchorResolver for FakeWorld {
    fn resolve(&self, anchor: &Anchor) -> Result<Option<String>> {
        self.resolver.resolve(anchor)
    }
}

impl EntityCatalog for FakeWorld {
    fn page_worthy_entities(&self) -> Result<Vec<CandidateEntity>> {
        Ok(self.candidates.clone())
    }
    fn present_doc_categories(&self) -> Result<Vec<DocCategory>> {
        // Yield in the canonical DocCategory::ALL order regardless of insertion
        // order, mirroring the production resolver's deterministic filter.
        Ok(DocCategory::ALL
            .into_iter()
            .filter(|c| self.doc_categories.contains(c))
            .collect())
    }
    fn doc_file_present(&self, repo_relative: &str) -> bool {
        self.present_docs.contains(repo_relative)
    }
}

/// A minimal well-formed body — a heading plus enough prose — for tests whose
/// focus is not the page content itself, so a plain write clears the
/// [FR-WK-19] content-validity guard without the test needing to care.
const PAGE: &str = "# Test Page\n\nPlaceholder prose long enough to satisfy the write-path content-validity guard.";

fn page_count(conn: &rusqlite::Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM pages", [], |r| r.get(0))
        .unwrap()
}

/// Positional-argument wrapper over [`write`] so the store-contract tests read
/// compactly; it just packs the fields into a [`PageDraft`].
#[allow(clippy::too_many_arguments)]
fn wr(
    conn: &mut rusqlite::Connection,
    resolver: &dyn AnchorResolver,
    head: &str,
    slug: &str,
    title: &str,
    body: &str,
    anchors: &[String],
    generator: &str,
) -> Result<WriteSummary> {
    write(
        conn,
        resolver,
        &PageDraft {
            slug,
            title,
            body,
            anchors,
            generator,
            written_head: head,
            built_at_revision: 0,
        },
    )
}

/// A [`write`] wrapper that also pins the built-at revision — the seam the
/// built-at-revision capture tests drive ([FR-WK-12]).
#[allow(clippy::too_many_arguments)]
fn wr_at(
    conn: &mut rusqlite::Connection,
    resolver: &dyn AnchorResolver,
    head: &str,
    slug: &str,
    title: &str,
    body: &str,
    anchors: &[String],
    generator: &str,
    built_at_revision: u64,
) -> Result<WriteSummary> {
    write(
        conn,
        resolver,
        &PageDraft {
            slug,
            title,
            body,
            anchors,
            generator,
            written_head: head,
            built_at_revision,
        },
    )
}

// ── FR-WK-02: write contract ─────────────────────────────────────────────────

#[test]
fn read_after_write_returns_byte_identical_body_with_full_provenance() {
    let mut conn = db::open_in_memory();
    let resolver = FakeResolver::with("file:src/lib.rs", Some("h1"));
    // A body with unicode, blank lines, trailing whitespace and no trailing
    // newline — every byte must round-trip ([FR-WK-02] verbatim).
    let body = "# Title\n\nA paragraph — with em‑dash.\n\n    indented\ntrailing  ";

    let summary = wr(
        &mut conn,
        &resolver,
        "deadbeef",
        "guide/overview",
        "Overview",
        body,
        &["file:src/lib.rs".to_string()],
        "claude-opus",
    )
    .unwrap();
    assert!(!summary.replaced);
    assert_eq!(summary.anchor_count, 1);
    assert_eq!(summary.written_head, "deadbeef");

    let page = read(&mut conn, &resolver, "guide/overview")
        .unwrap()
        .expect("page reads back");
    assert_eq!(page.body, body, "body is byte-identical to the input");
    assert_eq!(page.generator, "claude-opus");
    assert_eq!(page.written_head, "deadbeef");
    assert_eq!(
        page.marker, GENERATED_CONTENT_MARKER,
        "fixed marker present"
    );
    assert_eq!(page.anchors.len(), 1);
    assert_eq!(page.anchors[0].kind, "file");
    assert_eq!(page.anchors[0].freshness, Freshness::Fresh);
    assert!(!page.stale && !page.has_missing);
}

#[test]
fn unknown_anchor_rejects_write_and_leaves_the_store_byte_identical() {
    let mut conn = db::open_in_memory();
    // The resolver knows nothing about this anchor → it resolves to missing.
    let resolver = FakeResolver::default();

    let err = wr(
        &mut conn,
        &resolver,
        "head",
        "p",
        "T",
        "body",
        &["file:src/gone.rs".to_string()],
        "gen",
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("unknown anchor"),
        "rejection names the unknown anchor: {err}"
    );
    assert_eq!(page_count(&conn), 0, "no page was written");
}

#[test]
fn empty_generator_rejects_write() {
    let mut conn = db::open_in_memory();
    let resolver = FakeResolver::with("file:a.rs", Some("h"));
    for empty in ["", "   ", "\t\n"] {
        let err = wr(
            &mut conn,
            &resolver,
            "head",
            "p",
            "T",
            "body",
            &["file:a.rs".to_string()],
            empty,
        )
        .unwrap_err();
        assert!(err.to_string().contains("generator"), "got: {err}");
    }
    assert_eq!(page_count(&conn), 0);
}

#[test]
fn over_cap_body_rejects_write_byte_identical() {
    let mut conn = db::open_in_memory();
    let resolver = FakeResolver::with("file:a.rs", Some("h"));
    let body = "x".repeat(MAX_BODY_BYTES + 1);
    let err = wr(
        &mut conn,
        &resolver,
        "head",
        "p",
        "T",
        &body,
        &["file:a.rs".to_string()],
        "gen",
    )
    .unwrap_err();
    assert!(err.to_string().contains("cap"), "got: {err}");
    assert_eq!(page_count(&conn), 0);

    // Exactly at the cap is accepted — padded out from a heading so the body
    // also clears the [FR-WK-19] content-validity guard at the exact byte
    // count under test.
    let heading = "# Cap Test\n\n";
    let at_cap_body = format!("{heading}{}", "x".repeat(MAX_BODY_BYTES - heading.len()));
    assert_eq!(at_cap_body.len(), MAX_BODY_BYTES);
    let ok = wr(
        &mut conn,
        &resolver,
        "head",
        "p",
        "T",
        &at_cap_body,
        &["file:a.rs".to_string()],
        "gen",
    );
    assert!(ok.is_ok(), "a body exactly at the 1 MiB cap is accepted");
}

#[test]
fn invalid_slugs_reject_write() {
    let mut conn = db::open_in_memory();
    let resolver = FakeResolver::default();
    for bad in [
        "",
        "/leading",
        "trailing/",
        "double//slash",
        "../escape",
        "has space",
        "Upper",
        "weird*char",
    ] {
        let err = wr(&mut conn, &resolver, "h", bad, "T", "b", &[], "gen").unwrap_err();
        assert!(
            err.to_string().contains("slug"),
            "slug {bad:?} should be rejected with a slug message: {err}"
        );
    }
    // A valid multi-segment slug with the allowed character classes is accepted.
    assert!(wr(&mut conn, &resolver, "h", "a/b-1/c_2", "T", PAGE, &[], "gen").is_ok());
}

#[test]
fn second_write_to_a_slug_replaces_last_write_wins() {
    let mut conn = db::open_in_memory();
    let mut resolver = FakeResolver::with("file:a.rs", Some("h1"));

    wr(
        &mut conn,
        &resolver,
        "head1",
        "p",
        "First",
        "# First\n\nplaceholder prose long enough to satisfy the write-path guard, v1.",
        &["file:a.rs".to_string()],
        "gen1",
    )
    .unwrap();

    // The second write changes the body, head, generator, and anchor set.
    resolver.set("symbol:scip foo#", Some("h2"));
    let summary = wr(
        &mut conn,
        &resolver,
        "head2",
        "p",
        "Second",
        "# Second\n\nplaceholder prose long enough to satisfy the write-path guard, v2.",
        &["symbol:scip foo#".to_string()],
        "gen2",
    )
    .unwrap();
    assert!(summary.replaced, "the second write replaced the first");

    let page = read(&mut conn, &resolver, "p").unwrap().unwrap();
    assert_eq!(page.title, "Second");
    assert_eq!(
        page.body,
        "# Second\n\nplaceholder prose long enough to satisfy the write-path guard, v2."
    );
    assert_eq!(page.written_head, "head2");
    assert_eq!(page.generator, "gen2");
    assert_eq!(page.anchors.len(), 1);
    assert_eq!(page.anchors[0].kind, "symbol");
    assert_eq!(page.anchors[0].entity_id, "scip foo#");
    assert_eq!(page_count(&conn), 1, "still one page — upsert, not insert");
}

// ── FR-WK-12: built-at graph revision capture ────────────────────────────────

#[test]
fn write_captures_the_built_at_revision_and_it_round_trips() {
    let mut conn = db::open_in_memory();
    let resolver = FakeResolver::with("file:a.rs", Some("h1"));
    wr_at(
        &mut conn,
        &resolver,
        "head",
        "p",
        "T",
        PAGE,
        &["file:a.rs".to_string()],
        "gen",
        7,
    )
    .unwrap();

    // The built-at revision is recorded at write and surfaced on the read-model,
    // so the two-tier view can derive freshness against the current revision
    // ([FR-WK-12], [ADR-32]) — never an input to per-anchor freshness.
    let page = read(&mut conn, &resolver, "p").unwrap().unwrap();
    assert_eq!(page.built_at_revision, 7);
    assert!(!page.stale, "the built-at revision does not affect anchor freshness");
}

#[test]
fn a_re_write_updates_the_built_at_revision_last_write_wins() {
    let mut conn = db::open_in_memory();
    let resolver = FakeResolver::with("file:a.rs", Some("h1"));
    wr_at(&mut conn, &resolver, "h", "p", "T", PAGE, &["file:a.rs".to_string()], "gen", 3).unwrap();
    assert_eq!(read(&mut conn, &resolver, "p").unwrap().unwrap().built_at_revision, 3);

    // A second write at a later revision overwrites the built-at value.
    wr_at(&mut conn, &resolver, "h", "p", "T", PAGE, &["file:a.rs".to_string()], "gen", 9).unwrap();
    assert_eq!(
        read(&mut conn, &resolver, "p").unwrap().unwrap().built_at_revision,
        9,
        "the latest write's built-at revision wins"
    );
}

#[test]
fn a_page_written_before_any_index_records_revision_zero() {
    // The honest default: a page written with no graph revision yet records 0 —
    // it reads as "built before revision 1", never masquerading as fresh.
    let mut conn = db::open_in_memory();
    let resolver = FakeResolver::with("file:a.rs", Some("h1"));
    wr(&mut conn, &resolver, "h", "p", "T", PAGE, &["file:a.rs".to_string()], "gen").unwrap();
    assert_eq!(read(&mut conn, &resolver, "p").unwrap().unwrap().built_at_revision, 0);
}

// ── FR-WK-03: read-time freshness ────────────────────────────────────────────

#[test]
fn anchor_flips_fresh_to_stale_when_content_changes() {
    let mut conn = db::open_in_memory();
    let mut resolver = FakeResolver::with("file:a.rs", Some("h1"));
    wr(
        &mut conn,
        &resolver,
        "head",
        "p",
        "T",
        PAGE,
        &["file:a.rs".to_string()],
        "gen",
    )
    .unwrap();
    assert_eq!(
        read(&mut conn, &resolver, "p").unwrap().unwrap().anchors[0].freshness,
        Freshness::Fresh
    );

    // The file's content moved → the anchor is stale at the next read, with no
    // store write in between (freshness is computed at read time, FR-WK-03).
    resolver.set("file:a.rs", Some("h2"));
    let page = read(&mut conn, &resolver, "p").unwrap().unwrap();
    assert_eq!(page.anchors[0].freshness, Freshness::Stale);
    assert!(
        page.stale,
        "a page with a stale anchor is presented as stale"
    );
    assert!(!page.has_missing);
}

#[test]
fn zero_anchor_page_is_never_stale() {
    let mut conn = db::open_in_memory();
    let resolver = FakeResolver::default();
    wr(
        &mut conn,
        &resolver,
        "head",
        "overview",
        "T",
        PAGE,
        &[],
        "gen",
    )
    .unwrap();
    let page = read(&mut conn, &resolver, "overview").unwrap().unwrap();
    assert!(page.anchors.is_empty());
    assert!(
        !page.stale,
        "an overview page (zero anchors) is never stale"
    );
    assert!(!page.has_missing);
}

// ── FR-WK-07: orphan lifecycle ───────────────────────────────────────────────

#[test]
fn single_anchor_page_is_pruned_and_logged_when_its_anchor_is_gone() {
    let mut conn = db::open_in_memory();
    let mut resolver = FakeResolver::with("file:a.rs", Some("h1"));
    wr(
        &mut conn,
        &resolver,
        "head",
        "doomed",
        "Doomed",
        PAGE,
        &["file:a.rs".to_string()],
        "gen",
    )
    .unwrap();

    // The anchored file is renamed away — its only anchor is gone.
    resolver.set("file:a.rs", None);
    let gone = read(&mut conn, &resolver, "doomed").unwrap();
    assert!(gone.is_none(), "the all-anchors-gone page is auto-deleted");
    assert_eq!(page_count(&conn), 0);

    let log = pruned_log(&conn).unwrap();
    assert_eq!(log.len(), 1, "the pruning is recorded");
    assert_eq!(log[0].slug, "doomed");
    assert_eq!(log[0].title, "Doomed");
}

#[test]
fn two_anchor_page_survives_with_the_gone_anchor_flagged_missing() {
    let mut conn = db::open_in_memory();
    let mut resolver = FakeResolver::default();
    resolver.set("file:a.rs", Some("h1"));
    resolver.set("file:b.rs", Some("h2"));
    wr(
        &mut conn,
        &resolver,
        "head",
        "module",
        "Module",
        PAGE,
        &["file:a.rs".to_string(), "file:b.rs".to_string()],
        "gen",
    )
    .unwrap();

    // a.rs is renamed away; b.rs survives → the page survives, a.rs flagged.
    resolver.set("file:a.rs", None);
    let page = read(&mut conn, &resolver, "module")
        .unwrap()
        .expect("multi-anchor page survives a single missing anchor");
    assert!(page.has_missing, "the gone anchor is flagged missing");
    assert!(
        !page.stale,
        "a missing anchor is not the same as a stale one"
    );
    assert_eq!(page.anchors[0].freshness, Freshness::Missing);
    assert_eq!(page.anchors[1].freshness, Freshness::Fresh);
    assert_eq!(page_count(&conn), 1, "the page was not pruned");
    assert!(pruned_log(&conn).unwrap().is_empty(), "nothing was pruned");
}

#[test]
fn explicit_delete_removes_the_page_and_a_nonexistent_slug_errs() {
    let mut conn = db::open_in_memory();
    let resolver = FakeResolver::default();
    wr(&mut conn, &resolver, "head", "p", "T", PAGE, &[], "gen").unwrap();

    delete(&conn, "p").unwrap();
    assert_eq!(page_count(&conn), 0);
    assert!(
        read(&mut conn, &resolver, "p").unwrap().is_none(),
        "the deleted page no longer reads"
    );

    let err = delete(&conn, "nope").unwrap_err();
    assert!(err.to_string().contains("nope"), "got: {err}");
}

// ── Anchor grammar ───────────────────────────────────────────────────────────

#[test]
fn anchor_parse_round_trips_and_rejects_bad_forms() {
    let file = Anchor::parse("file:src/foo.rs").unwrap();
    assert_eq!(file.kind, AnchorKind::File);
    assert_eq!(file.entity_id, "src/foo.rs");
    assert_eq!(file.as_id(), "file:src/foo.rs");

    // A symbol key may itself contain `:` — only the first `:` splits the kind.
    let sym = Anchor::parse("symbol:scip rust foo:bar#").unwrap();
    assert_eq!(sym.kind, AnchorKind::Symbol);
    assert_eq!(sym.entity_id, "scip rust foo:bar#");
    assert_eq!(sym.as_id(), "symbol:scip rust foo:bar#");

    for bad in ["noprefix", "file:", "bogus:thing", ""] {
        assert!(Anchor::parse(bad).is_err(), "{bad:?} should not parse");
    }
}

#[test]
fn file_anchor_rejects_path_traversal_and_absolute_paths() {
    // A `file:` anchor is joined onto the worktree root and hashed, so a
    // traversing or absolute key would reach a file outside the repo — rejected
    // on the write (parse) path ([NFR-SE-01], [NFR-RA-05]).
    for bad in [
        "file:../../etc/passwd",
        "file:/etc/shadow",
        "file:a/../../b",
        "file:bad\\path",
    ] {
        let err = Anchor::parse(bad).unwrap_err();
        assert!(
            err.to_string().contains("file anchor"),
            "{bad:?} should be rejected as an unsafe file anchor: {err}"
        );
    }
    // A nested repo-relative path is fine.
    assert!(Anchor::parse("file:src/a/b/c.rs").is_ok());

    // And the rejection holds end-to-end on the write path with the store
    // left byte-identical (no page written).
    let mut conn = db::open_in_memory();
    let resolver = FakeResolver::with("file:../escape", Some("h"));
    let err = wr(
        &mut conn,
        &resolver,
        "head",
        "p",
        "T",
        "b",
        &["file:../escape".to_string()],
        "gen",
    )
    .unwrap_err();
    assert!(err.to_string().contains("file anchor"), "got: {err}");
    assert_eq!(page_count(&conn), 0);
}

#[test]
fn reverting_the_edit_restores_fresh() {
    // FR-WK-03 acceptance: reverting the edit restores fresh.
    let mut conn = db::open_in_memory();
    let mut resolver = FakeResolver::with("file:a.rs", Some("h1"));
    wr(
        &mut conn,
        &resolver,
        "head",
        "p",
        "T",
        PAGE,
        &["file:a.rs".to_string()],
        "gen",
    )
    .unwrap();
    resolver.set("file:a.rs", Some("h2")); // edited → stale
    assert!(read(&mut conn, &resolver, "p").unwrap().unwrap().stale);
    resolver.set("file:a.rs", Some("h1")); // reverted → fresh again
    let page = read(&mut conn, &resolver, "p").unwrap().unwrap();
    assert_eq!(page.anchors[0].freshness, Freshness::Fresh);
    assert!(!page.stale, "reverting the edit restores fresh (FR-WK-03)");
}

#[test]
fn empty_written_head_round_trips() {
    // A repo with no resolvable HEAD records an empty tag, not a failure
    // ([ADR-24] out-of-the-box); it must round-trip as "".
    let mut conn = db::open_in_memory();
    let resolver = FakeResolver::default();
    let summary = wr(&mut conn, &resolver, "", "p", "T", PAGE, &[], "gen").unwrap();
    assert_eq!(summary.written_head, "");
    let page = read(&mut conn, &resolver, "p").unwrap().unwrap();
    assert_eq!(page.written_head, "", "empty HEAD round-trips");
}

#[test]
fn a_rejected_write_leaves_an_existing_page_byte_identical() {
    // FR-WK-02: a rejected write leaves the store byte-identical — proven on a
    // POPULATED store (the upsert of an existing slug must not be half-applied).
    let mut conn = db::open_in_memory();
    let resolver = FakeResolver::with("file:a.rs", Some("h1"));
    wr(
        &mut conn,
        &resolver,
        "h1",
        "p",
        "Original",
        "# Original\n\nThe original page body, long enough to satisfy the guard.",
        &["file:a.rs".to_string()],
        "gen1",
    )
    .unwrap();

    // Over-cap overwrite of the same slug → rejected.
    let big = "x".repeat(MAX_BODY_BYTES + 1);
    assert!(wr(
        &mut conn,
        &resolver,
        "h2",
        "p",
        "New",
        &big,
        &["file:a.rs".to_string()],
        "gen2"
    )
    .is_err());
    // Unknown-anchor overwrite of the same slug → rejected.
    assert!(wr(
        &mut conn,
        &resolver,
        "h2",
        "p",
        "New",
        "new body",
        &["file:gone.rs".to_string()],
        "gen2"
    )
    .is_err());

    // The original page is untouched by either rejected overwrite.
    let page = read(&mut conn, &resolver, "p").unwrap().unwrap();
    assert_eq!(page.title, "Original");
    assert_eq!(
        page.body,
        "# Original\n\nThe original page body, long enough to satisfy the guard."
    );
    assert_eq!(page.written_head, "h1");
    assert_eq!(page.generator, "gen1");
    assert_eq!(page_count(&conn), 1);
}

#[test]
fn pruned_log_is_ordered_newest_first() {
    // The pruned log is the work-list S-053 reads; its newest-first ordering is
    // a named contract ([FR-WK-07]).
    let mut conn = db::open_in_memory();
    let mut resolver = FakeResolver::default();
    resolver.set("file:a.rs", Some("h1"));
    resolver.set("file:b.rs", Some("h2"));
    wr(
        &mut conn,
        &resolver,
        "h",
        "alpha",
        "Alpha",
        PAGE,
        &["file:a.rs".to_string()],
        "gen",
    )
    .unwrap();
    wr(
        &mut conn,
        &resolver,
        "h",
        "beta",
        "Beta",
        PAGE,
        &["file:b.rs".to_string()],
        "gen",
    )
    .unwrap();

    resolver.set("file:a.rs", None); // alpha prunes first
    read(&mut conn, &resolver, "alpha").unwrap();
    resolver.set("file:b.rs", None); // beta prunes second
    read(&mut conn, &resolver, "beta").unwrap();

    let log = pruned_log(&conn).unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].slug, "beta", "newest pruning first");
    assert_eq!(log[1].slug, "alpha");
}

// ── FR-WK-01: independent migration track ────────────────────────────────────

#[test]
fn fresh_store_migrates_to_the_latest_version_on_its_own_track() {
    let conn = db::open_in_memory();
    assert_eq!(db::current_version(&conn).unwrap(), db::latest_version());
    assert!(
        db::latest_version() >= 1,
        "the ledger has at least migration 1"
    );
}

// ── FR-WK-22: per-file page retirement migration ──────────────────────────────

/// Migration 4 deletes every `files/%` slug on open, records each removal in
/// the pruned-log, and is idempotent — a second run finds nothing left to
/// prune. A non-`files/%` page is untouched — including one that merely
/// shares the `files` substring without the `files/` prefix boundary, so the
/// `LIKE 'files/%'` pattern is proven not to over-match.
#[test]
fn migration_4_retires_all_files_percent_pages_and_logs_each_removal() {
    // Seed a store on the pre-retirement track (version 3) with two files/%
    // pages and two ordinary pages (one of which, `filesystem/overview`,
    // shares the `files` substring but not the `files/` prefix), inserted
    // directly — the migration acts on rows already in the store, not
    // through the write path.
    let mut conn = db::open_in_memory_before_migration_4();
    conn.execute_batch(
        "INSERT INTO pages (slug, title, body, generator, written_head, written_at, built_at_revision)
         VALUES
            ('files/src/a-rs',      'a.rs — objectives',   'body', 'gen', 'h', 0, 0),
            ('files/src/b-rs',      'b.rs — objectives',   'body', 'gen', 'h', 0, 0),
            ('guide/intro',         'Intro',               'body', 'gen', 'h', 0, 0),
            ('filesystem/overview', 'Filesystem Overview', 'body', 'gen', 'h', 0, 0)",
    )
    .unwrap();
    assert_eq!(page_count(&conn), 4);

    db::migrate(&mut conn).unwrap();

    assert_eq!(
        db::current_version(&conn).unwrap(),
        db::latest_version(),
        "migration 4 advances the store to the latest version"
    );
    assert_eq!(
        db::all_slugs(&conn).unwrap(),
        vec!["filesystem/overview".to_string(), "guide/intro".to_string()],
        "both files/% rows are gone; ordinary pages survive untouched, \
         including one that only shares the files substring"
    );

    let log = db::pruned_log(&conn).unwrap();
    let mut pruned: Vec<(&str, &str)> = log.iter().map(|p| (p.slug.as_str(), p.title.as_str())).collect();
    pruned.sort_unstable();
    assert_eq!(
        pruned,
        vec![
            ("files/src/a-rs", "a.rs — objectives"),
            ("files/src/b-rs", "b.rs — objectives"),
        ],
        "each removal is recorded in the pruned-log with its title, not just its slug"
    );

    // Idempotent: a second run on an already-migrated store finds nothing left
    // to prune — byte-identical (no-op).
    db::migrate(&mut conn).unwrap();
    assert_eq!(page_count(&conn), 2);
    assert_eq!(
        db::pruned_log(&conn).unwrap().len(),
        2,
        "a second run logs no duplicate prunings"
    );
}

/// [`is_retired_files_slug`] matches exactly the `files/%` prefix boundary —
/// a bare `files` segment (no trailing slash) and a `filesystem/...` slug
/// that merely shares the `files` substring are never mistaken for a
/// retired per-file page, while `files/` and any `files/<path>` are. Pins
/// the Rust-side check against the same boundary the migration's SQL
/// `LIKE 'files/%'` pattern enforces.
#[test]
fn is_retired_files_slug_matches_exactly_the_prefix_boundary() {
    assert!(!is_retired_files_slug("files"));
    assert!(!is_retired_files_slug("filesystem/overview"));
    assert!(is_retired_files_slug("files/"));
    assert!(is_retired_files_slug("files/src/a-rs"));
}

/// A store with no per-file pages migrates to a no-op: no pruned-log rows are
/// written, and no page is touched.
#[test]
fn migration_4_is_a_no_op_when_no_files_percent_pages_exist() {
    let mut conn = db::open_in_memory_before_migration_4();
    conn.execute_batch(
        "INSERT INTO pages (slug, title, body, generator, written_head, written_at, built_at_revision)
         VALUES ('guide/intro', 'Intro', 'body', 'gen', 'h', 0, 0)",
    )
    .unwrap();

    db::migrate(&mut conn).unwrap();

    assert_eq!(page_count(&conn), 1);
    assert_eq!(db::all_slugs(&conn).unwrap(), vec!["guide/intro".to_string()]);
    assert!(
        db::pruned_log(&conn).unwrap().is_empty(),
        "no files/% rows existed, so nothing is logged as pruned"
    );
}

// ── FR-WK-22: bulk `delete_pages_not_in` + reconciliation sweep ───────────────

/// `delete_pages_not_in` removes exactly the pages whose slug is absent from
/// the supplied valid set, logs each removal to the pruned-log, leaves every
/// valid page untouched, and is idempotent — a second run against the already-
/// reconciled store removes nothing and logs no duplicate.
#[test]
fn delete_pages_not_in_removes_exactly_the_out_of_set_slugs_and_logs_each() {
    let mut conn = db::open_in_memory();
    conn.execute_batch(
        "INSERT INTO pages (slug, title, body, generator, written_head, written_at, built_at_revision)
         VALUES
            ('overview/project-overview', 'Project Overview', 'body', 'gen', 'h', 0, 0),
            ('architecture/adrs',         'ADRs',             'body', 'gen', 'h', 0, 0),
            ('architecture/components',   'Components',       'body', 'gen', 'h', 0, 0)",
    )
    .unwrap();

    let valid: HashSet<String> = ["overview/project-overview", "architecture/adrs"]
        .into_iter()
        .map(str::to_string)
        .collect();

    let removed = db::delete_pages_not_in(&mut conn, &valid).unwrap();
    assert_eq!(
        removed,
        vec!["architecture/components".to_string()],
        "only the out-of-set slug is reported removed"
    );
    assert_eq!(
        db::all_slugs(&conn).unwrap(),
        vec!["architecture/adrs".to_string(), "overview/project-overview".to_string()],
        "both valid pages survive untouched"
    );
    let log = db::pruned_log(&conn).unwrap();
    assert_eq!(log.len(), 1, "exactly one removal is logged");
    assert_eq!(log[0].slug, "architecture/components");
    assert_eq!(log[0].title, "Components", "the pruned-log records the title too");

    // Idempotent: a second run against the already-reconciled store removes
    // nothing and logs no duplicate.
    let removed_again = db::delete_pages_not_in(&mut conn, &valid).unwrap();
    assert!(removed_again.is_empty(), "a reconciled store has nothing left to remove");
    assert_eq!(
        db::pruned_log(&conn).unwrap().len(),
        1,
        "a second run logs no duplicate pruning"
    );
}

/// [`reconciliation_valid_slugs`] is the five fixed Overview/Summary slugs ∪
/// only the consolidated-category slugs whose source files actually exist
/// under `root` — an absent category (no source files) never contributes a
/// slug to the valid set, mirroring [`DocCategory::present_under`].
#[test]
fn reconciliation_valid_slugs_is_overview_union_present_categories_only() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("docs/specs/architecture/decisions")).unwrap();
    std::fs::write(
        root.join("docs/specs/architecture/decisions/ADR-01.md"),
        "# ADR-01",
    )
    .unwrap();

    let valid = reconciliation_valid_slugs(root);

    for section in OverviewSection::ALL {
        assert!(
            valid.contains(section.slug()),
            "every fixed Overview/Summary slug is always valid"
        );
    }
    assert!(
        valid.contains(DocCategory::Adrs.slug()),
        "the present Adrs category contributes its slug"
    );
    assert!(
        !valid.contains(DocCategory::Components.slug()),
        "an absent category (no source files under root) contributes no slug"
    );
}

/// The reconciliation sweep purges a stored page whose slug is outside the
/// active-mode valid set, never removes a valid Overview or present-category
/// page, and is idempotent — a second run on the reconciled store removes
/// nothing. Mirrors the [FR-WK-22] acceptance criteria end to end through
/// [`reconcile`], not just the underlying store primitive.
#[test]
fn reconcile_purges_orphans_and_is_idempotent() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("docs/specs/architecture/decisions")).unwrap();
    std::fs::write(
        root.join("docs/specs/architecture/decisions/ADR-01.md"),
        "# ADR-01",
    )
    .unwrap();

    let mut conn = db::open_in_memory();
    conn.execute_batch(
        "INSERT INTO pages (slug, title, body, generator, written_head, written_at, built_at_revision)
         VALUES
            ('overview/project-overview', 'Project Overview', 'body', 'gen', 'h', 0, 0),
            ('architecture/adrs',         'ADRs',             'body', 'gen', 'h', 0, 0),
            ('architecture/components',   'Components',       'body', 'gen', 'h', 0, 0),
            ('requirements/fr-wk-01',     'FR-WK-01',         'body', 'gen', 'h', 0, 0)",
    )
    .unwrap();

    let removed = reconcile(&mut conn, root).unwrap();
    let mut removed_sorted = removed.clone();
    removed_sorted.sort_unstable();
    assert_eq!(
        removed_sorted,
        vec!["architecture/components".to_string(), "requirements/fr-wk-01".to_string()],
        "the sweep purges exactly the two out-of-set orphans"
    );
    assert_eq!(
        db::all_slugs(&conn).unwrap(),
        vec!["architecture/adrs".to_string(), "overview/project-overview".to_string()],
        "the valid Overview and present-category pages survive"
    );
    assert_eq!(db::pruned_log(&conn).unwrap().len(), 2, "each purge is logged");

    // Idempotent: a second run on the reconciled store removes nothing.
    let removed_again = reconcile(&mut conn, root).unwrap();
    assert!(removed_again.is_empty(), "a reconciled store has nothing left to purge");
    assert_eq!(
        db::pruned_log(&conn).unwrap().len(),
        2,
        "a second sweep logs no duplicate pruning"
    );
}

// ── FR-WK-05: FTS5 search + enumeration ───────────────────────────────────────

#[test]
fn search_ranks_the_page_with_the_unique_phrase_first_and_flags_staleness() {
    let mut conn = db::open_in_memory();
    let mut resolver = FakeResolver::with("file:a.rs", Some("h1"));
    resolver.set("file:b.rs", Some("h2"));
    // Two pages; only `alpha`'s body contains the unique phrase.
    wr(
        &mut conn,
        &resolver,
        "h",
        "alpha",
        "Alpha Guide",
        "# Alpha Guide\n\nthe quux subsystem orchestrates widgets",
        &["file:a.rs".to_string()],
        "gen",
    )
    .unwrap();
    wr(
        &mut conn,
        &resolver,
        "h",
        "beta",
        "Beta Guide",
        "# Beta Guide\n\nan unrelated page about pipelines",
        &["file:b.rs".to_string()],
        "gen",
    )
    .unwrap();

    let hits = search(&conn, &resolver, "quux subsystem", false, 0).unwrap();
    assert_eq!(hits.len(), 1, "only the page with the phrase matches");
    assert_eq!(hits[0].slug, "alpha", "the matching page ranks first");
    assert!(!hits[0].stale, "fresh anchor → not stale");
    assert_eq!(hits[0].generator, "gen", "provenance summary present");

    // Editing alpha's anchored file flips its hit to stale at the next search.
    resolver.set("file:a.rs", Some("h1-edited"));
    let hits = search(&conn, &resolver, "quux", false, 0).unwrap();
    assert!(hits[0].stale, "the staleness flag rides on the result row");
}

/// [FR-WK-12] predicate boundary cases — the single source of truth the search
/// read-model, the `wiki status` work-list, and the web page view all derive the
/// "stale — regeneration pending" verdict from ([CR-039]).
#[test]
fn revision_pending_boundary_cases() {
    assert!(!revision_pending(0, 0), "pre-index (current 0) → never pending");
    assert!(!revision_pending(5, 0), "current 0 → never pending");
    assert!(!revision_pending(3, 3), "built at the current revision → not pending");
    assert!(revision_pending(2, 3), "graph advanced past built-at → pending");
    assert!(!revision_pending(10, 3), "built-at ahead of current → not pending (no u64 underflow)");
}

/// [FR-WK-05] as modified by [CR-039]: a search hit carries the **revision-pending**
/// verdict so the search "State" agrees with the page view — a page built at an
/// older graph revision reads "regeneration pending" in search **even when its
/// anchors are fresh** (the exact contradiction CR-039 fixes: search formerly
/// reported it plain "Fresh"). The verdict rides on every hit, computed against
/// the current revision via the same predicate the page view uses.
#[test]
fn search_hit_carries_the_revision_pending_verdict() {
    let mut conn = db::open_in_memory();
    let resolver = FakeResolver::with("file:a.rs", Some("h1"));
    // A page built at revision 5 whose anchor stays fresh.
    wr_at(
        &mut conn,
        &resolver,
        "h",
        "alpha",
        "Alpha",
        "# Alpha\n\nthe quux subsystem, described at enough length to clear the guard.",
        &["file:a.rs".to_string()],
        "gen",
        5,
    )
    .unwrap();

    // Graph advanced to 7 → the page is "stale — regeneration pending" though its
    // anchor is unchanged (fresh): the very case search formerly mislabelled Fresh.
    let pending = search(&conn, &resolver, "quux", false, 7).unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].revision_pending, "graph advanced past built-at → pending");
    assert!(!pending[0].stale, "the anchor is fresh — only the revision advanced");
    // The raw built-at revision rides on the hit too (the landing re-derives the
    // verdict against its own displayed revision through it).
    assert_eq!(pending[0].built_at_revision, 5, "the hit carries the page's built-at revision");

    // List mode carries the same verdict + built-at — the wiki landing enumerates
    // via list mode, so its pending count depends on these being populated.
    let listed = search(&conn, &resolver, "", true, 7).unwrap();
    assert!(listed[0].revision_pending, "list-mode hits carry the revision-pending verdict");
    assert_eq!(listed[0].built_at_revision, 5, "list-mode hits carry the built-at revision");

    // Built at the current revision → not pending; pre-index (current 0) → not pending.
    assert!(!search(&conn, &resolver, "quux", false, 5).unwrap()[0].revision_pending);
    assert!(!search(&conn, &resolver, "quux", false, 0).unwrap()[0].revision_pending);
}

#[test]
fn list_mode_enumerates_all_pages_slug_ordered_with_provenance() {
    let mut conn = db::open_in_memory();
    let resolver = FakeResolver::with("file:a.rs", Some("h1"));
    wr(
        &mut conn,
        &resolver,
        "head1",
        "zeta",
        "Zeta",
        PAGE,
        &[],
        "g1",
    )
    .unwrap();
    wr(
        &mut conn,
        &resolver,
        "head2",
        "alpha",
        "Alpha",
        PAGE,
        &["file:a.rs".to_string()],
        "g2",
    )
    .unwrap();

    // List mode ignores the query and returns every page, slug-ordered.
    let all = search(&conn, &resolver, "", true, 0).unwrap();
    assert_eq!(
        all.iter().map(|h| h.slug.as_str()).collect::<Vec<_>>(),
        ["alpha", "zeta"],
        "list mode is slug-ordered and complete"
    );
    assert_eq!(all[0].title, "Alpha");
    assert_eq!(all[0].generator, "g2");
    assert_eq!(all[0].written_head, "head2");
    assert!(
        !all[0].stale,
        "alpha is fresh while its anchor is unchanged"
    );

    // Editing alpha's anchored file flips its list-mode row to stale (the
    // freshness flag rides on every enumerated page, FR-WK-05 AC2); the
    // zero-anchor overview page is never stale.
    let mut resolver = resolver;
    resolver.set("file:a.rs", Some("h1-edited"));
    let all = search(&conn, &resolver, "", true, 0).unwrap();
    assert!(
        all[0].stale,
        "the edited anchor makes alpha stale in list mode"
    );
    assert!(
        !all[1].stale,
        "the zero-anchor overview page is never stale"
    );
}

#[test]
fn empty_query_is_a_no_op_not_a_syntax_error() {
    let mut conn = db::open_in_memory();
    let resolver = FakeResolver::with("file:a.rs", Some("h1"));
    wr(
        &mut conn,
        &resolver,
        "h",
        "p",
        "T",
        PAGE,
        &["file:a.rs".to_string()],
        "g",
    )
    .unwrap();
    // A blank search box yields no hits, never an FTS5 syntax error.
    assert!(search(&conn, &resolver, "   ", false, 0).unwrap().is_empty());
    assert!(fts_phrase_query("  \t ").is_none());
    // Punctuation that would be FTS operators is safely phrase-quoted.
    assert_eq!(
        fts_phrase_query("a \"b\" OR c").unwrap(),
        "\"a \"\"b\"\" OR c\""
    );
}

#[test]
fn a_re_write_keeps_the_fts_index_in_sync() {
    // The UPDATE trigger must retract the old body's postings — a stale posting
    // would let an old phrase keep matching (the NFR-RA-09 desync trap).
    let mut conn = db::open_in_memory();
    let resolver = FakeResolver::with("file:a.rs", Some("h1"));
    wr(
        &mut conn,
        &resolver,
        "h",
        "p",
        "T",
        "# Notes\n\nthe original subsystem text, described at enough length to clear the guard.",
        &["file:a.rs".to_string()],
        "g",
    )
    .unwrap();
    assert_eq!(
        search(&conn, &resolver, "original", false, 0).unwrap().len(),
        1
    );

    // Re-write the same slug with a different body.
    wr(
        &mut conn,
        &resolver,
        "h",
        "p",
        "T",
        "# Notes\n\na completely replaced corpus, described at enough length to clear the guard.",
        &["file:a.rs".to_string()],
        "g",
    )
    .unwrap();
    assert!(
        search(&conn, &resolver, "original", false, 0)
            .unwrap()
            .is_empty(),
        "the old body no longer matches after the re-write"
    );
    assert_eq!(
        search(&conn, &resolver, "replaced corpus", false, 0)
            .unwrap()
            .len(),
        1,
        "the new body matches"
    );
}

#[test]
fn a_pruned_page_drops_out_of_the_fts_index() {
    // The DELETE trigger must retract postings so a pruned page stops matching.
    let mut conn = db::open_in_memory();
    let mut resolver = FakeResolver::with("file:a.rs", Some("h1"));
    wr(
        &mut conn,
        &resolver,
        "h",
        "doomed",
        "Doomed",
        "# Notes\n\nfindable phrase here, described at enough length to clear the guard.",
        &["file:a.rs".to_string()],
        "g",
    )
    .unwrap();
    assert_eq!(
        search(&conn, &resolver, "findable phrase", false, 0)
            .unwrap()
            .len(),
        1
    );
    // Anchor gone → the page is pruned on its next read.
    resolver.set("file:a.rs", None);
    assert!(read(&mut conn, &resolver, "doomed").unwrap().is_none());
    assert!(
        search(&conn, &resolver, "findable phrase", false, 0)
            .unwrap()
            .is_empty(),
        "the pruned page is gone from the FTS index too"
    );
}

// ── FR-WK-06: status summary + regeneration work-list ─────────────────────────

#[test]
fn status_work_list_excludes_file_and_module_entities_but_still_tracks_a_stale_page() {
    // [CR-056]/[S-221]: File and Module entities are no longer page-worthy — a
    // module lacking any anchored page must NOT appear in the unanchored
    // work-list ([FR-WK-06]), while a genuinely anchor-stale existing page still
    // does (the generic existing-page mechanism is unaffected).
    let mut conn = db::open_in_memory();
    let world = FakeWorld::default()
        .fresh("file:a.rs", "h1")
        .file("a.rs") // a.rs IS anchored below → would never appear unanchored anyway
        // The catalog yields RAW entity keys (as the production GraphAnchorResolver
        // does: `node.symbol`), not pre-prefixed wire forms — `status` prepends the
        // kind to build the anchor.
        .module("crate::widgets", "widgets");
    wr(
        &mut conn,
        &world,
        "h",
        "guide/a",
        "About a",
        PAGE,
        &["file:a.rs".to_string()],
        "gen",
    )
    .unwrap();

    // The module has no anchored page, but File/Module are excluded from the
    // unanchored work-list ([CR-056]/[S-221]) — it never appears. revision 0
    // isolates this CR-008 work-list assertion from the structured section
    // seeding (covered by dedicated tests below).
    let st = status(&conn, &world, &world, 0, 1).unwrap();
    assert_eq!(st.page_count, 1);
    assert_eq!(st.stale_count, 0);
    assert!((st.freshness_fraction - 1.0).abs() < f64::EPSILON);
    assert!(st.work_list.stale_pages.is_empty());
    assert!(
        st.work_list.unanchored_entities.is_empty(),
        "File and Module are never seeded as unanchored entities: {:?}",
        st.work_list.unanchored_entities
    );

    // Edit the anchored file → the page joins the work-list as stale.
    let world = FakeWorld {
        candidates: world.candidates.clone(),
        resolver: {
            let mut r = FakeResolver::default();
            r.set("file:a.rs", Some("h1-edited"));
            r
        },
        ..Default::default()
    };
    let st = status(&conn, &world, &world, 0, 1).unwrap();
    assert_eq!(st.stale_count, 1);
    assert_eq!(st.work_list.stale_pages.len(), 1, "exactly one stale page");
    assert_eq!(st.work_list.stale_pages[0].slug, "guide/a");
    assert!(st.work_list.stale_pages[0].stale);
    assert!((st.freshness_fraction - 0.0).abs() < f64::EPSILON);
    // The module is still excluded after the edit.
    assert!(st.work_list.unanchored_entities.is_empty());
}

#[test]
fn status_surfaces_missing_anchor_pages_and_the_pruned_log() {
    let mut conn = db::open_in_memory();
    let mut world = FakeWorld::default();
    world.resolver.set("file:a.rs", Some("h1"));
    world.resolver.set("file:b.rs", Some("h2"));
    // A two-anchor page (survives flagged) and a single-anchor page (will prune).
    wr(
        &mut conn,
        &world,
        "h",
        "module/ab",
        "AB",
        PAGE,
        &["file:a.rs".to_string(), "file:b.rs".to_string()],
        "gen",
    )
    .unwrap();
    wr(
        &mut conn,
        &world,
        "h",
        "page/a",
        "A",
        PAGE,
        &["file:a.rs".to_string()],
        "gen",
    )
    .unwrap();

    // a.rs is renamed away: page/a will prune on read, module/ab survives flagged.
    world.resolver.set("file:a.rs", None);
    assert!(read(&mut conn, &world, "page/a").unwrap().is_none());

    let st = status(&conn, &world, &world, 0, 1).unwrap();
    assert_eq!(st.page_count, 1, "only module/ab remains");
    assert_eq!(st.missing_anchor_count, 1);
    assert_eq!(
        st.work_list.missing_anchor_pages.len(),
        1,
        "exactly one missing-anchor page"
    );
    assert_eq!(st.work_list.missing_anchor_pages[0].slug, "module/ab");
    assert!(st.work_list.missing_anchor_pages[0].has_missing);
    assert_eq!(st.pruned.len(), 1, "the prune is surfaced in status");
    assert_eq!(st.pruned[0].slug, "page/a");
}

/// [CR-034]: documentation page-worthiness is **consolidated**, one entry per
/// present category (each naming its `docs/` glob), never one page per typed
/// node. A category whose source files are absent is not seeded (reported by
/// absence, never fabricated). Story/CR are never page-worthy — the production
/// resolver no longer yields them at all (covered over a real graph in
/// `tests/wiki_store.rs`).
#[test]
fn status_seeds_one_consolidated_entry_per_present_category() {
    let conn = db::open_in_memory();
    // Two categories present (ADRs, Functional Requirements); the rest absent.
    // Revision 1 so the structured work-list seeds (post-first-`index`).
    let world = FakeWorld::default()
        .doc_category(DocCategory::Adrs)
        .doc_category(DocCategory::FunctionalRequirements);
    let st = status(&conn, &world, &world, 1, 1).unwrap();

    let consolidated: Vec<(&str, &str)> = st
        .work_list
        .structured_sections
        .iter()
        .filter(|s| DocCategory::ALL.iter().any(|c| c.label() == s.section))
        .map(|s| (s.section, s.slug.as_str()))
        .collect();
    assert_eq!(
        consolidated,
        vec![
            ("adrs", "architecture/adrs"),
            ("functional-requirements", "specs/functional-requirements"),
        ],
        "exactly one consolidated entry per present category, in menu order"
    );

    // Each consolidated entry is zero-anchor, absent (no page yet), and carries a
    // doc-grounding directive naming its source glob — never a per-node page.
    let adrs = st
        .work_list
        .structured_sections
        .iter()
        .find(|s| s.slug == "architecture/adrs")
        .unwrap();
    assert!(adrs.anchor.is_empty(), "consolidated pages are zero-anchor");
    assert_eq!(adrs.state, SectionState::Absent);
    let grounding = adrs.grounding.as_ref().expect("consolidated entry is doc-grounded");
    assert_eq!(
        grounding.sources,
        vec!["docs/specs/architecture/decisions/*.md".to_string()],
        "the grounding names the category source glob"
    );
    assert!(!grounding.fallback_to_code, "categories are only seeded when present — no fallback");

    // An absent category (no source files) is not seeded — never fabricated.
    assert!(
        st.work_list
            .structured_sections
            .iter()
            .all(|s| s.slug != "specs/frontend-design"),
        "a category with no source files is absent from the work-list"
    );

    // No typed-node unanchored entities are produced from the consolidated path
    // (the FakeWorld injects none here); the work-list carries documents, not nodes.
    assert!(
        st.work_list
            .unanchored_entities
            .iter()
            .all(|e| e.kind != "adr" && e.kind != "requirement" && e.kind != "story"),
        "documentation is consolidated, never per-node"
    );
}

// ── FR-WK-21 / CR-062: SRS-mode gate and bimodal generation queue ─────────────

/// [FR-WK-21]/[ADR-57]: the SRS-mode predicate is Case 1 **only** when
/// `architecture.md` is present AND at least one `FR-*`/`NFR-*`/`UAT-*` file is
/// present; removing either condition is Case 2. A pure function of the resolved
/// presence inputs — no I/O in the predicate itself ([NFR-RA-06]).
#[test]
fn is_srs_mode_requires_architecture_and_a_requirement() {
    use DocCategory::*;

    // Case 1: architecture.md + any one requirement family.
    assert!(is_srs_mode(true, &[FunctionalRequirements]), "architecture.md + FR → Case 1");
    assert!(is_srs_mode(true, &[NonFunctionalRequirements]), "architecture.md + NFR → Case 1");
    assert!(is_srs_mode(true, &[UserAcceptanceTests]), "architecture.md + UAT → Case 1");
    assert!(
        is_srs_mode(true, &[Adrs, Components, FunctionalRequirements]),
        "a requirement anywhere in the present set → Case 1"
    );

    // Case 2: architecture.md present but no requirement family (Design docs only).
    assert!(
        !is_srs_mode(true, &[Adrs, Components, Integrations, FrontendDesign]),
        "architecture.md but no FR/NFR/UAT → Case 2"
    );
    // Case 2: a requirement present but architecture.md absent.
    assert!(!is_srs_mode(false, &[FunctionalRequirements]), "requirement but no architecture.md → Case 2");
    // Case 2: neither.
    assert!(!is_srs_mode(false, &[]), "empty layout → Case 2");
}

/// The [`wiki_srs_mode`] root gate reads `docs/specs/architecture.md` + a
/// requirement family off disk and agrees with [`is_srs_mode`] ([FR-WK-21]) — a
/// pure local-FS read over a real directory layout, no `wiki.db` / LLM / network.
#[test]
fn wiki_srs_mode_gate_reads_architecture_and_requirements_from_disk() {
    // Build a temp `docs/specs/` layout and toggle the two load-bearing artifacts.
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    let specs = root.join("docs/specs");
    let reqs = specs.join("requirements");
    std::fs::create_dir_all(&reqs).unwrap();

    // Case 2c: empty layout — neither artifact present.
    assert!(!wiki_srs_mode(root), "empty layout → Case 2");

    // Case 2b: a requirement present, but architecture.md absent.
    std::fs::write(reqs.join("FR-WK-01.md"), "# FR-WK-01").unwrap();
    assert!(!wiki_srs_mode(root), "requirement but no architecture.md → Case 2");

    // Case 1: architecture.md added alongside the requirement.
    std::fs::write(specs.join("architecture.md"), "# Architecture").unwrap();
    assert!(wiki_srs_mode(root), "architecture.md + FR present → Case 1");

    // Case 2a: architecture.md present, but only a Design-tier doc (no FR/NFR/UAT).
    std::fs::remove_file(reqs.join("FR-WK-01.md")).unwrap();
    std::fs::create_dir_all(specs.join("architecture/decisions")).unwrap();
    std::fs::write(specs.join("architecture/decisions/ADR-01.md"), "# ADR-01").unwrap();
    assert!(!wiki_srs_mode(root), "architecture.md + ADRs but no requirement → Case 2");
}

/// [FR-WK-21]: in **Case 1** the work-list / queue lists **only** the Overview
/// items — the consolidated Design/Specs categories are produced by the presented
/// tier ([FR-WK-20]) and never queued to the agent. Evaluating the gate and
/// building the work-list mutates nothing (a pure read, no `wiki.db` write).
#[test]
fn srs_mode_work_list_is_overview_only_and_omits_consolidated_categories() {
    let conn = db::open_in_memory();
    // Case 1 fixture: architecture.md + FR present, plus other Design/Specs
    // categories present — all of which must stay OUT of the agent queue.
    let world = FakeWorld::default()
        .doc_present("docs/specs/architecture.md")
        .doc_present("docs/specs/software-spec.md")
        .doc_category(DocCategory::Adrs)
        .doc_category(DocCategory::Components)
        .doc_category(DocCategory::FunctionalRequirements)
        .doc_category(DocCategory::NonFunctionalRequirements);
    let st = status(&conn, &world, &world, 1, 1).unwrap();

    // Exactly the five Overview children, in menu order — no consolidated category.
    let sections: Vec<&str> = st
        .work_list
        .structured_sections
        .iter()
        .map(|s| s.slug.as_str())
        .collect();
    assert_eq!(
        sections,
        vec![
            "overview/project-overview",
            "overview/getting-started",
            "overview/key-concepts",
            "overview/how-it-works",
            "overview/known-issues",
        ],
        "SRS mode: the work-list is Overview-only"
    );
    // No consolidated Design/Specs category is queued in Case 1.
    assert!(
        st.work_list
            .structured_sections
            .iter()
            .all(|s| DocCategory::from_label(s.section).is_none()),
        "SRS mode: consolidated categories are presented, never queued to the agent"
    );

    // The generation queue is likewise Overview-only.
    let queue = generation_queue(&st);
    assert!(
        queue.items.iter().all(|i| i.category == GenerationCategory::Overview),
        "SRS mode: the queue lists only Overview items"
    );
    assert_eq!(queue.items.len(), 5, "five Overview items, no consolidated docs");

    // The gate/work-list is a pure read — nothing was written to the store.
    assert_eq!(page_count(&conn), 0, "building the work-list writes no page");
}

/// [FR-WK-21]: in **Case 2** (no SRS) the queue is unchanged — Overview PLUS the
/// present consolidated categories, the pre-[CR-062] behavior. The same fixture
/// as Case 1 but with `architecture.md` absent flips the mode and re-admits the
/// categories, proving the gate is the sole discriminator.
#[test]
fn non_srs_mode_work_list_keeps_the_present_consolidated_categories() {
    let conn = db::open_in_memory();
    // architecture.md ABSENT → Case 2, even though requirements are present.
    let world = FakeWorld::default()
        .doc_present("docs/specs/software-spec.md")
        .doc_category(DocCategory::Adrs)
        .doc_category(DocCategory::FunctionalRequirements);
    let st = status(&conn, &world, &world, 1, 1).unwrap();

    let slugs: Vec<&str> = st
        .work_list
        .structured_sections
        .iter()
        .map(|s| s.slug.as_str())
        .collect();
    // Overview children first, then the two present consolidated categories.
    assert_eq!(
        slugs,
        vec![
            "overview/project-overview",
            "overview/getting-started",
            "overview/key-concepts",
            "overview/how-it-works",
            "overview/known-issues",
            "architecture/adrs",
            "specs/functional-requirements",
        ],
        "Case 2: Overview + present consolidated categories (pre-CR-062 behavior)"
    );
}

/// [FR-WK-24] AC: the Overview grounding re-pitch applies **identically in
/// both modes**, for **all four** mapped Overview children — SRS mode no
/// longer special-cases the Project Overview's grounding source list the way
/// the pre-[FR-WK-24] presented-pages directive did in Case 1 ([FR-WK-21]).
/// With every mapped user-facing doc present, Case 1 and Case 2 produce
/// byte-identical `DocGrounding` values for Project Overview, Getting
/// Started, How It Works, and Key Concepts, and none falls back to code.
#[test]
fn srs_mode_grounds_every_overview_child_in_the_same_user_facing_docs_as_case_2() {
    let conn = db::open_in_memory();
    let case1 = FakeWorld::default()
        .doc_present("docs/specs/architecture.md")
        .doc_present("docs/specs/software-spec.md")
        .doc_present("README.md")
        .doc_present("docs/howto/README.md")
        .doc_present("docs/howto/installation.md")
        .doc_present("docs/howto/usage.md")
        .doc_category(DocCategory::FunctionalRequirements);
    let st1 = status(&conn, &case1, &case1, 1, 1).unwrap();

    let case2 = FakeWorld::default()
        .doc_present("docs/specs/software-spec.md")
        .doc_present("README.md")
        .doc_present("docs/howto/README.md")
        .doc_present("docs/howto/installation.md")
        .doc_present("docs/howto/usage.md");
    let st2 = status(&conn, &case2, &case2, 1, 1).unwrap();

    for slug in [
        "overview/project-overview",
        "overview/getting-started",
        "overview/how-it-works",
        "overview/key-concepts",
    ] {
        let g1 = st1
            .work_list
            .structured_sections
            .iter()
            .find(|s| s.slug == slug)
            .and_then(|s| s.grounding.clone())
            .unwrap_or_else(|| panic!("{slug} is grounded in SRS mode"));
        let g2 = st2
            .work_list
            .structured_sections
            .iter()
            .find(|s| s.slug == slug)
            .and_then(|s| s.grounding.clone())
            .unwrap_or_else(|| panic!("{slug} is grounded in Case 2"));

        assert_eq!(
            g1, g2,
            "{slug}: the grounding directive is identical in both modes ([FR-WK-24])"
        );
        assert!(
            !g1.fallback_to_code,
            "{slug}: every mapped doc is present — no code fallback"
        );
    }

    // Project Overview specifically grounds in README.md + software-spec.md, not
    // the retired presented-pages directive.
    let project = st1
        .work_list
        .structured_sections
        .iter()
        .find(|s| s.slug == "overview/project-overview")
        .and_then(|s| s.grounding.clone())
        .unwrap();
    assert_eq!(
        project.sources,
        vec!["README.md".to_string(), "docs/specs/software-spec.md".to_string()],
        "grounds in README.md + software-spec.md, not the presented Design/Specs pages"
    );
}

/// [CR-034]: a consolidated category page written at an older graph revision
/// reads **revision-stale** once the graph advances — the same derived freshness
/// the Overview pages get, applied to the consolidated documents S-133 renders a
/// REGEN badge for. Authoring at the current revision clears it.
#[test]
fn consolidated_category_page_goes_revision_stale_then_clears() {
    let mut conn = db::open_in_memory();
    // Author the ADRs consolidated page at the older revision 1 (zero-anchor).
    wr_at(
        &mut conn,
        &FakeWorld::default(),
        "h",
        "architecture/adrs",
        "ADRs",
        PAGE,
        &[],
        "gen",
        1,
    )
    .unwrap();

    // The graph has advanced to revision 2 → the page built at 1 is revision-stale.
    let world = FakeWorld::default().doc_category(DocCategory::Adrs);
    let st = status(&conn, &world, &world, 2, 1).unwrap();
    let adrs = st
        .work_list
        .structured_sections
        .iter()
        .find(|s| s.slug == "architecture/adrs")
        .expect("the consolidated ADRs page is on the work-list");
    assert_eq!(adrs.state, SectionState::RevisionStale);
    assert!(
        adrs.grounding.is_some(),
        "a revision-stale consolidated page still carries its grounding directive"
    );

    // Re-author at the current revision 2 → it clears from the work-list.
    wr_at(
        &mut conn,
        &FakeWorld::default(),
        "h",
        "architecture/adrs",
        "ADRs",
        PAGE,
        &[],
        "gen",
        2,
    )
    .unwrap();
    let st = status(&conn, &world, &world, 2, 1).unwrap();
    assert!(
        st.work_list
            .structured_sections
            .iter()
            .all(|s| s.slug != "architecture/adrs"),
        "a consolidated page rebuilt at the current revision clears from the work-list"
    );
}

#[test]
fn status_on_an_empty_store_is_well_formed_and_fraction_is_one() {
    // The freshness_fraction guard: 0 pages must not divide by zero — it reads as
    // vacuously fresh (1.0), with empty work-list sections and pruned log.
    let conn = db::open_in_memory();
    let world = FakeWorld::default();
    let st = status(&conn, &world, &world, 0, 1).unwrap();
    assert_eq!(st.page_count, 0);
    assert_eq!(st.fresh_count, 0);
    assert!(
        (st.freshness_fraction - 1.0).abs() < f64::EPSILON,
        "empty store is vacuously fresh (1.0), never a div-by-zero"
    );
    assert!(st.work_list.stale_pages.is_empty());
    assert!(st.work_list.missing_anchor_pages.is_empty());
    assert!(st.work_list.unanchored_entities.is_empty());
    assert!(st.work_list.structured_sections.is_empty());
    assert!(st.pruned.is_empty());
}

// ── FR-WK-06 / CR-027: agent-tier structured-section seeding ──────────────────

/// Before the first `index` (`revision == 0`) the structured-section work-list
/// is empty — an honest seed state so `init` is never blocked by wiki work
/// ([FR-WK-12] first-build-after-index).
#[test]
fn structured_sections_are_empty_before_the_first_index() {
    let conn = db::open_in_memory();
    // Even with admitted files in the catalog, revision 0 means no graph yet.
    let world = FakeWorld::default().file("src/a.rs").file("src/b.rs");
    let st = status(&conn, &world, &world, 0, 1).unwrap();
    assert!(
        st.work_list.structured_sections.is_empty(),
        "no structured work is seeded before the first index"
    );
}

/// After the first `index`, an empty wiki seeds exactly the five fixed Overview
/// prose-child sections — all `absent`, never any native-tier content, and
/// never a per-file objectives entry ([FR-WK-06] as modified by [CR-027],
/// [CR-056]/[S-221], [CR-062]). The synthesized `overview/architecture` page is
/// retired ([CR-062]) so it is absent from the seeded set.
#[test]
fn structured_sections_seed_the_absent_agent_sections_after_index() {
    let conn = db::open_in_memory();
    let world = FakeWorld::default()
        .file("src/a.rs")
        .file("src/b.rs")
        // Files/modules are no longer structured sections ([CR-056]/[S-221]) —
        // they must not leak into the structured-section seeding.
        .module("crate::widgets", "widgets");
    let st = status(&conn, &world, &world, 1, 1).unwrap();
    let sections = &st.work_list.structured_sections;

    // Every seeded section is absent (nothing is written yet).
    assert!(sections.iter().all(|s| s.state == SectionState::Absent));

    // The five Overview singletons, in fixed menu order, at their canonical
    // slugs — exactly, with no per-file objectives and no retired Architecture.
    let overview: Vec<(&str, &str)> = sections
        .iter()
        .map(|s| (s.section, s.slug.as_str()))
        .collect();
    assert_eq!(
        overview,
        vec![
            ("project overview", "overview/project-overview"),
            ("getting started", "overview/getting-started"),
            ("key concepts", "overview/key-concepts"),
            ("how it works", "overview/how-it-works"),
            ("known-issues prose", "overview/known-issues"),
        ],
        "the five Overview children seed in menu order at canonical slugs, and nothing else"
    );

    // The retired Architecture narrative ([CR-062]) is never seeded.
    assert!(
        sections.iter().all(|s| s.slug != "overview/architecture"),
        "the synthesized overview/architecture page is retired"
    );

    // The titles are part of the wire contract the view ([S-104]) consumes.
    let project = sections
        .iter()
        .find(|s| s.slug == "overview/project-overview")
        .unwrap();
    assert_eq!(project.title, "Project Overview");

    // No per-file objectives section is ever produced ([CR-056]/[S-221]).
    assert!(
        sections.iter().all(|s| s.section != "file objectives"),
        "per-file objectives are no longer seeded"
    );

    // The native tier is never listed — no extracted section class appears.
    assert!(
        sections.iter().all(|s| {
            !matches!(
                s.section,
                "codebase structure" | "configuration" | "files view" | "dependency mermaid"
            )
        }),
        "the deterministic native tier is never in the work-list"
    );
}

/// A present singleton section built at an older revision reads `revision-stale`;
/// rebuilt at the current revision it drops off the work-list. The verdict is a
/// pure built-at-vs-current comparison — no anchor, no write ([FR-WK-12]).
#[test]
fn structured_sections_flag_revision_stale_then_clear_on_regeneration() {
    let mut conn = db::open_in_memory();
    let world = FakeWorld::default(); // no admitted files → only the singletons

    // Author the Project Overview at revision 1 (a zero-anchor overview page).
    wr_at(
        &mut conn,
        &world,
        "h",
        OverviewSection::ProjectOverview.slug(),
        "Project Overview",
        PAGE,
        &[],
        "gen",
        1,
    )
    .unwrap();

    // The graph advanced to revision 2 → the page is revision-stale; the other
    // four singletons are still absent.
    let st = status(&conn, &world, &world, 2, 1).unwrap();
    let project: Vec<&StructuredSection> = st
        .work_list
        .structured_sections
        .iter()
        .filter(|s| s.slug == "overview/project-overview")
        .collect();
    assert_eq!(project.len(), 1);
    assert_eq!(project[0].state, SectionState::RevisionStale);
    assert_eq!(
        st.work_list
            .structured_sections
            .iter()
            .filter(|s| s.state == SectionState::Absent)
            .count(),
        4,
        "the four un-authored Overview children are still absent"
    );

    // Regenerate it at the current revision 2 → it clears from the work-list.
    wr_at(
        &mut conn,
        &world,
        "h",
        OverviewSection::ProjectOverview.slug(),
        "Project Overview",
        PAGE,
        &[],
        "gen",
        2,
    )
    .unwrap();
    let st = status(&conn, &world, &world, 2, 1).unwrap();
    assert!(
        st.work_list
            .structured_sections
            .iter()
            .all(|s| s.slug != "overview/project-overview"),
        "a section rebuilt at the current revision is no longer on the work-list"
    );
}

// ── FR-WK-17: revision-stale regeneration cadence dampening ──────────────────

/// The core [FR-WK-17] fixture: with a dampening threshold of 3, a page
/// regenerated at revision 10 does **not** re-queue on the next two single
/// revision advances (delta 1, delta 2) — only once the graph has advanced 3
/// revisions past the built-at revision does it reappear on the work-list. AC:
/// "repeated single-revision advances do not re-queue it; it re-queues only
/// once the configured threshold is crossed."
#[test]
fn revision_stale_dampening_suppresses_requeue_until_threshold_crossed() {
    let mut conn = db::open_in_memory();
    let world = FakeWorld::default();
    let threshold = 3;

    wr_at(
        &mut conn,
        &world,
        "h",
        OverviewSection::ProjectOverview.slug(),
        "Project Overview",
        PAGE,
        &[],
        "gen",
        10,
    )
    .unwrap();

    let on_work_list = |revision: u64| -> bool {
        status(&conn, &world, &world, revision, threshold)
            .unwrap()
            .work_list
            .structured_sections
            .iter()
            .any(|s| s.slug == "overview/project-overview")
    };

    // Delta 0 (still at the built-at revision): fresh, never queued.
    assert!(!on_work_list(10), "delta 0 is current, not revision-stale");
    // Delta 1 and delta 2: revision-pending in truth, but dampened out of the
    // work-list — repeated single-revision advances must not re-queue it.
    assert!(!on_work_list(11), "delta 1 is dampened below the threshold of 3");
    assert!(!on_work_list(12), "delta 2 is dampened below the threshold of 3");
    // Dampening governs re-queue of an ALREADY-REGENERATED page only — it must
    // never suppress first-time authoring. The other four Overview singletons
    // (never built, `built_at: None`) stay `Absent` regardless of the high
    // threshold, at the very revision where the regenerated page is dampened
    // out ([FR-WK-17]: dampening bounds re-queue cadence, not initial seeding).
    let st = status(&conn, &world, &world, 11, threshold).unwrap();
    assert_eq!(
        st.work_list
            .structured_sections
            .iter()
            .filter(|s| s.state == SectionState::Absent)
            .count(),
        4,
        "the four never-built Overview singletons still seed absent under a high threshold"
    );
    // Delta 3 crosses the configured threshold — it re-queues.
    assert!(on_work_list(13), "delta 3 crosses the threshold and re-queues");
    let st = status(&conn, &world, &world, 13, threshold).unwrap();
    let section = st
        .work_list
        .structured_sections
        .iter()
        .find(|s| s.slug == "overview/project-overview")
        .unwrap();
    assert_eq!(section.state, SectionState::RevisionStale);
}

/// [FR-WK-17] AC: at its minimum (`1`), the dampening threshold reduces exactly
/// to "re-queue whenever revision-pending" — the pre-dampening behavior. A
/// single revision advance is enough to re-queue.
#[test]
fn revision_stale_threshold_minimum_of_one_requeues_on_the_first_revision_advance() {
    let mut conn = db::open_in_memory();
    let world = FakeWorld::default();

    wr_at(
        &mut conn,
        &world,
        "h",
        OverviewSection::ProjectOverview.slug(),
        "Project Overview",
        PAGE,
        &[],
        "gen",
        10,
    )
    .unwrap();

    let st = status(&conn, &world, &world, 11, 1).unwrap();
    let section = st
        .work_list
        .structured_sections
        .iter()
        .find(|s| s.slug == "overview/project-overview")
        .expect("threshold 1 re-queues on the very next revision advance");
    assert_eq!(section.state, SectionState::RevisionStale);
}

/// [FR-WK-17] AC: reporting is never masked by dampening — `revision_stale_count`
/// counts the page throughout, even on the single-revision advances a high
/// threshold dampens out of `work_list.structured_sections` entirely.
#[test]
fn revision_stale_count_stays_truthful_while_the_work_list_is_dampened() {
    let mut conn = db::open_in_memory();
    let world = FakeWorld::default();
    let threshold = 10;

    wr_at(
        &mut conn,
        &world,
        "h",
        OverviewSection::ProjectOverview.slug(),
        "Project Overview",
        PAGE,
        &[],
        "gen",
        10,
    )
    .unwrap();

    for revision in [11u64, 12, 13, 15] {
        let st = status(&conn, &world, &world, revision, threshold).unwrap();
        assert_eq!(
            st.revision_stale_count, 1,
            "revision_stale_count stays honest at revision {revision} regardless of dampening"
        );
        assert!(
            st.work_list
                .structured_sections
                .iter()
                .all(|s| s.slug != "overview/project-overview"),
            "the high threshold dampens the page out of the work-list at revision {revision}"
        );
    }
}

/// A pre-existing per-file objectives page (written before [CR-056]/[S-221], at
/// the old canonical `files/<path>` slug) is no longer tracked by
/// [`structured_sections`] at all — not even as `revision-stale` — because the
/// per-file objectives seeding is removed entirely, not merely left to age. The
/// page itself is untouched (not deleted) and stays readable.
#[test]
fn a_preexisting_file_objectives_page_is_no_longer_a_structured_section() {
    let mut conn = db::open_in_memory();
    let world = FakeWorld::default()
        .file("src/a.rs")
        .fresh("file:src/a.rs", "ha");

    // A page at the old canonical per-file-objectives slug, built at an older
    // revision than the current one.
    wr_at(
        &mut conn,
        &world,
        "h",
        "files/src/a-rs",
        "a.rs — objectives",
        PAGE,
        &["file:src/a.rs".to_string()],
        "gen",
        1,
    )
    .unwrap();

    let st = status(&conn, &world, &world, 2, 1).unwrap();
    assert!(
        st.work_list
            .structured_sections
            .iter()
            .all(|s| s.slug != "files/src/a-rs"),
        "a pre-existing per-file objectives page is no longer tracked as a \
         structured section, revision-stale or otherwise"
    );
    // The page itself is untouched — still readable, not deleted.
    assert!(read(&mut conn, &world, "files/src/a-rs").unwrap().is_some());
}

/// The same guarantee for a pre-existing **module**-anchored page (a `symbol:`
/// anchor, not `file:`): untouched by [CR-056]/[S-221], still readable, and
/// still tracked for anchor staleness by the generic existing-page mechanism —
/// which is anchor-kind-agnostic and unaffected by the `EntityKind` catalog
/// filter this change added.
#[test]
fn a_preexisting_module_page_is_untouched_and_still_tracked_for_anchor_staleness() {
    let mut conn = db::open_in_memory();
    let world = FakeWorld::default()
        .module("crate::widgets", "widgets")
        .fresh("symbol:crate::widgets", "h1");

    wr(
        &mut conn,
        &world,
        "h",
        "modules/crate--widgets",
        "widgets",
        PAGE,
        &["symbol:crate::widgets".to_string()],
        "gen",
    )
    .unwrap();

    // Untouched by the change — still readable, not deleted, and not surfaced
    // as an unanchored entity (it IS anchored, but Module is excluded anyway).
    let st = status(&conn, &world, &world, 0, 1).unwrap();
    assert!(st.work_list.unanchored_entities.is_empty());
    assert!(read(&mut conn, &world, "modules/crate--widgets").unwrap().is_some());

    // Edit the anchored symbol's content → the page joins the work-list as
    // stale, exactly like a file-anchored page — the mechanism is generic.
    let world = FakeWorld {
        candidates: world.candidates.clone(),
        resolver: {
            let mut r = FakeResolver::default();
            r.set("symbol:crate::widgets", Some("h1-edited"));
            r
        },
        ..Default::default()
    };
    let st = status(&conn, &world, &world, 0, 1).unwrap();
    assert_eq!(st.work_list.stale_pages.len(), 1);
    assert_eq!(st.work_list.stale_pages[0].slug, "modules/crate--widgets");
}

/// The per-file objectives slug is always a valid wiki slug ([FR-WK-02]) — the
/// path's `.`, uppercase, and other non-slug characters are sanitized so the
/// skill can write to it without rejection.
#[test]
fn file_objectives_slug_is_always_a_valid_slug() {
    for path in [
        "src/a.rs",
        "logos-core/src/Wiki/mod.rs",
        "Cargo.toml",
        ".gitignore",
        "a/b.c/d.e",
        // A segment of all out-of-alphabet characters maps each to `-` — still a
        // non-empty, valid segment (never an empty `files//a-rs`).
        "!!!/a.rs",
    ] {
        let slug = file_objectives_slug(path);
        assert!(
            validate_slug(&slug).is_ok(),
            "slug {slug:?} for path {path:?} must validate"
        );
        assert!(slug.starts_with("files/"), "objectives live under files/");
    }
    // Each out-of-alphabet character maps to one `-`, so the segment stays
    // non-empty and the slug validates.
    assert_eq!(file_objectives_slug("!!!/a.rs"), "files/---/a-rs");
}

// ── FR-WK-13: `wiki generate` offline generation queue ────────────────────────

/// The queue reformats the work-list into the fixed [FR-WK-13] order — the six
/// Overview children, then unanchored page-worthy entities — each carrying its
/// target slug and a runnable `wiki write` skeleton; the native tier is never
/// present. Files and modules never appear, seeded or not ([CR-056]/[S-221]).
#[test]
fn generation_queue_orders_overview_children_and_excludes_file_and_module_entities() {
    let mut conn = db::open_in_memory();
    let world = FakeWorld::default()
        .file("src/a.rs")
        .fresh("file:src/a.rs", "ha")
        .module("crate::widgets", "widgets");
    // a.rs is anchored by an (unrelated, fresh) page; the module has no page at
    // all. Neither ever reaches the queue ([CR-056]/[S-221]).
    wr_at(
        &mut conn,
        &world,
        "h",
        "guide/intro",
        "Intro",
        PAGE,
        &["file:src/a.rs".to_string()],
        "gen",
        1,
    )
    .unwrap();

    let st = status(&conn, &world, &world, 1, 1).unwrap();
    let queue = generation_queue(&st);

    assert_eq!(
        queue
            .items
            .iter()
            .map(|i| (i.category, i.slug.as_str(), i.reason))
            .collect::<Vec<_>>(),
        vec![
            (GenerationCategory::Overview, "overview/project-overview", "absent"),
            (GenerationCategory::Overview, "overview/getting-started", "absent"),
            (GenerationCategory::Overview, "overview/key-concepts", "absent"),
            (GenerationCategory::Overview, "overview/how-it-works", "absent"),
            (GenerationCategory::Overview, "overview/known-issues", "absent"),
        ],
        "only the five Overview children are queued — no file or module entries"
    );
    assert_eq!(queue.items.len(), 5);
    // The fresh, in-work-list-free `guide/intro` page is not queued.
    assert!(queue.items.iter().all(|i| i.slug != "guide/intro"));
    // Neither the anchored file nor the unanchored module ever surfaces.
    assert!(queue.items.iter().all(|i| i.slug != "files/src/a-rs"));
    assert!(queue.items.iter().all(|i| i.category != GenerationCategory::UnanchoredEntity));
    // No item ever carries native-tier content — every category is agent-tier.
    assert!(queue.items.iter().all(|i| matches!(
        i.category,
        GenerationCategory::Overview
            | GenerationCategory::ConsolidatedDoc
            | GenerationCategory::UnanchoredEntity
            | GenerationCategory::StalePage
            | GenerationCategory::MissingAnchorPage
    )));
}

/// Each item carries a runnable `wiki write` skeleton: the slug bare, the title
/// single-quoted, the body on stdin, the generator a placeholder ([FR-WK-13]).
#[test]
fn generation_queue_items_carry_a_runnable_wiki_write_skeleton() {
    let conn = db::open_in_memory();
    let world = FakeWorld::default();
    let st = status(&conn, &world, &world, 1, 1).unwrap();
    let queue = generation_queue(&st);

    // A zero-anchor Overview singleton: no `--anchor`, body on stdin.
    let overview = queue
        .items
        .iter()
        .find(|i| i.slug == "overview/project-overview")
        .unwrap();
    assert_eq!(
        overview.command,
        "logos wiki write overview/project-overview --title 'Project Overview' \
         --generator '<generator>' --body-file -"
    );
    assert!(overview.anchor.is_empty());
}

/// The runnable skeleton single-quotes a `:`-bearing, space-bearing symbol
/// anchor so it stays one shell argument. Exercised directly against
/// [`write_skeleton`] rather than through the full pipeline — no current
/// work-list category seeds a non-empty anchor into the queue
/// ([CR-056]/[S-221] excludes the only category that used to, `file`/`module`
/// unanchored entities).
#[test]
fn write_skeleton_single_quotes_a_space_and_colon_bearing_anchor() {
    let slug = entity_target_slug("module", "scip rust widgets#");
    assert_eq!(slug, "modules/scip-rust-widgets-");
    assert!(validate_slug(&slug).is_ok());

    let command = write_skeleton(&slug, "widgets", "symbol:scip rust widgets#");
    assert_eq!(
        command,
        "logos wiki write modules/scip-rust-widgets- --title 'widgets' \
         --generator '<generator>' --anchor 'symbol:scip rust widgets#' --body-file -"
    );
}

/// A pre-existing per-file page is never queued for refresh, stale anchor or
/// not — the work-list skips every `files/%` slug outright ([FR-WK-22],
/// [CR-062]), reversing the earlier [CR-056] "served but no longer
/// regenerated" disposition that let 220 such unreachable pages accumulate.
/// The page itself is untouched — still readable — only its presence in the
/// regeneration work-list/queue changes. A file with no page at all is still
/// not page-worthy either ([FR-WK-13]).
#[test]
fn generation_queue_never_refreshes_a_preexisting_files_percent_page() {
    let mut conn = db::open_in_memory();
    // a.rs's pre-existing per-file page, from before per-file pages were
    // retired.
    let writer = FakeWorld::default().fresh("file:src/a.rs", "h1");
    wr_at(
        &mut conn,
        &writer,
        "h",
        "files/src/a-rs",
        "src/a.rs — objectives",
        PAGE,
        &["file:src/a.rs".to_string()],
        "gen",
        1,
    )
    .unwrap();

    // At revision 2: a.rs edited (its page would otherwise go anchor-stale);
    // b.rs has no page.
    let world = FakeWorld::default()
        .file("src/a.rs")
        .file("src/b.rs")
        .fresh("file:src/a.rs", "h1-edited")
        .fresh("file:src/b.rs", "hb");
    let st = status(&conn, &world, &world, 2, 1).unwrap();
    assert!(
        st.work_list
            .stale_pages
            .iter()
            .all(|p| p.slug != "files/src/a-rs"),
        "a files/% page is never surfaced for refresh, stale anchor or not (FR-WK-22)"
    );
    assert_eq!(st.page_count, 0, "a files/% page is invisible to the store summary too");

    let queue = generation_queue(&st);
    assert!(
        queue.items.iter().all(|i| i.slug != "files/src/a-rs"),
        "a files/% page never re-enters the generation queue"
    );

    // The page itself is untouched — still readable, not deleted.
    assert!(read(&mut conn, &world, "files/src/a-rs").unwrap().is_some());

    // b.rs has no page at all — a File is no longer page-worthy
    // ([CR-056]/[S-221]), so it never appears in the queue.
    assert!(
        queue.items.iter().all(|i| i.slug != "files/src/b-rs"),
        "a file with no existing page is never seeded into the queue"
    );
}

/// Stale and missing-anchor existing pages (that are not at a structured slug)
/// are queued as `stale-page` then `missing-anchor-page` refreshes, after the
/// structured sections ([FR-WK-13]).
#[test]
fn generation_queue_lists_stale_then_missing_existing_page_refreshes() {
    let mut conn = db::open_in_memory();
    let writer = FakeWorld::default()
        .fresh("file:src/x.rs", "h1")
        .fresh("file:src/z.rs", "h2");
    wr_at(&mut conn, &writer, "h", "guide/intro", "Intro", PAGE, &["file:src/x.rs".to_string()], "gen", 2).unwrap();
    wr_at(&mut conn, &writer, "h", "guide/orphan", "Orphan", PAGE, &["file:src/z.rs".to_string()], "gen", 2).unwrap();

    // x.rs edited → guide/intro stale; z.rs gone → guide/orphan missing-anchor.
    let world = FakeWorld::default().fresh("file:src/x.rs", "h1-edited");
    let st = status(&conn, &world, &world, 2, 1).unwrap();
    let queue = generation_queue(&st);

    // The six Overview children come first; the two refreshes follow, stale
    // before missing.
    let refreshes: Vec<(GenerationCategory, &str, &str, &str)> = queue
        .items
        .iter()
        .filter(|i| {
            matches!(
                i.category,
                GenerationCategory::StalePage | GenerationCategory::MissingAnchorPage
            )
        })
        .map(|i| (i.category, i.slug.as_str(), i.reason, i.anchor.as_str()))
        .collect();
    assert_eq!(
        refreshes,
        vec![
            (GenerationCategory::StalePage, "guide/intro", "stale", ""),
            (GenerationCategory::MissingAnchorPage, "guide/orphan", "missing", ""),
        ],
        "stale refresh before missing refresh, each with an empty (agent-supplied) anchor"
    );
    // A refresh skeleton omits `--anchor` (re-supplied by the agent) and reads
    // the body from stdin.
    let intro = queue.items.iter().find(|i| i.slug == "guide/intro").unwrap();
    assert_eq!(
        intro.command,
        "logos wiki write guide/intro --title 'Intro' --generator '<generator>' --body-file -"
    );
}

/// A page that is in BOTH the stale and the missing-anchor work-list sections is
/// queued once (the stale row wins the slug dedup) with a combined `stale+missing`
/// reason, so the missing-anchor signal is surfaced rather than dropped ([FR-WK-13]).
#[test]
fn generation_queue_marks_a_stale_and_missing_page_with_a_combined_reason() {
    let mut conn = db::open_in_memory();
    let writer = FakeWorld::default()
        .fresh("file:a.rs", "h1")
        .fresh("file:b.rs", "h2");
    wr_at(
        &mut conn,
        &writer,
        "h",
        "guide/both",
        "Both",
        PAGE,
        &["file:a.rs".to_string(), "file:b.rs".to_string()],
        "gen",
        1,
    )
    .unwrap();

    // a.rs edited (→ stale anchor); b.rs gone (→ missing anchor). The page lands
    // in both work-list sections.
    let world = FakeWorld::default().fresh("file:a.rs", "h1-edited");
    let st = status(&conn, &world, &world, 1, 1).unwrap();
    assert!(st.work_list.stale_pages.iter().any(|p| p.slug == "guide/both"));
    assert!(st
        .work_list
        .missing_anchor_pages
        .iter()
        .any(|p| p.slug == "guide/both"));

    let queue = generation_queue(&st);
    let both: Vec<&GenerationItem> =
        queue.items.iter().filter(|i| i.slug == "guide/both").collect();
    assert_eq!(both.len(), 1, "queued once despite being in both sections");
    assert_eq!(both[0].category, GenerationCategory::StalePage);
    assert_eq!(
        both[0].reason, "stale+missing",
        "the combined reason surfaces the missing-anchor signal"
    );
}

/// An empty work-list yields an empty queue and the explicit "nothing to
/// generate" prompt block ([FR-WK-13], [NFR-CC-04]) — never a fabricated queue.
#[test]
fn generation_queue_is_empty_when_the_work_list_is_empty() {
    let conn = db::open_in_memory();
    // Revision 0: no graph yet → no structured work; no pages; no candidates.
    let world = FakeWorld::default();
    let st = status(&conn, &world, &world, 0, 1).unwrap();
    let queue = generation_queue(&st);
    assert!(queue.items.is_empty());
    assert_eq!(
        queue.render_prompt_block(),
        "Nothing to generate — the wiki work-list is empty.\n"
    );
}

/// [FR-WK-17]: the `wiki generate` queue is a pure reformatting of the
/// dampened work-list, so a page dampened out of
/// `work_list.structured_sections` is also absent from the queue — repeated
/// single-revision advances after a regeneration must not add it back until
/// the configured threshold is crossed.
#[test]
fn generation_queue_omits_a_dampened_revision_stale_page_until_threshold_crossed() {
    let mut conn = db::open_in_memory();
    let world = FakeWorld::default();
    let threshold = 4;

    wr_at(
        &mut conn,
        &world,
        "h",
        OverviewSection::ProjectOverview.slug(),
        "Project Overview",
        PAGE,
        &[],
        "gen",
        20,
    )
    .unwrap();

    // Delta 1: revision-pending in truth, but dampened out of the queue.
    let st = status(&conn, &world, &world, 21, threshold).unwrap();
    let queue = generation_queue(&st);
    assert!(
        queue
            .items
            .iter()
            .all(|i| i.slug != "overview/project-overview"),
        "a single-revision advance must not re-add the page to the queue"
    );

    // Delta 4 crosses the threshold — the queue lists it again, revision-stale.
    let st = status(&conn, &world, &world, 24, threshold).unwrap();
    let queue = generation_queue(&st);
    let item = queue
        .items
        .iter()
        .find(|i| i.slug == "overview/project-overview")
        .expect("the page re-enters the queue once the threshold is crossed");
    assert_eq!(item.reason, "revision-stale");
}

/// The queue is a deterministic function of `wiki.db` + the revision
/// ([NFR-RA-06]): two `status` reads of the same store at the same revision
/// serialize to byte-identical `--json` — the [FR-WK-13] AC2 guarantee.
#[test]
fn generation_queue_json_is_byte_identical_for_a_fixed_store_and_revision() {
    let conn = db::open_in_memory();
    let world = FakeWorld::default()
        .doc_category(DocCategory::Adrs)
        .doc_category(DocCategory::FunctionalRequirements)
        .doc_present("docs/specs/software-spec.md");

    let j1 = serde_json::to_string(&generation_queue(&status(&conn, &world, &world, 3, 1).unwrap())).unwrap();
    let j2 = serde_json::to_string(&generation_queue(&status(&conn, &world, &world, 3, 1).unwrap())).unwrap();
    assert_eq!(j1, j2, "the --json queue is byte-identical across two runs");
    // The serialized shape is the documented contract: a single `items` array
    // (the count is the array length, never a separate field).
    assert!(j1.starts_with("{\"items\":["), "json shape: {j1}");

    // Determinism is structural, not statistical: the queue leads with the fixed
    // Overview order and the consolidated category pages sit at their fixed slugs,
    // regardless of the HashSet/HashMap seed.
    let parsed: serde_json::Value = serde_json::from_str(&j1).unwrap();
    let slugs: Vec<&str> = parsed["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["slug"].as_str().unwrap())
        .collect();
    assert_eq!(slugs[0], "overview/project-overview", "fixed lead order: {slugs:?}");
    assert!(
        slugs.contains(&"architecture/adrs") && slugs.contains(&"specs/functional-requirements"),
        "the consolidated category pages are queued at their fixed slugs: {slugs:?}"
    );
}

/// The human prompt block leads with a header and renders each item with its
/// slug + runnable command ([FR-WK-13]); it is deterministic. Anchor rendering
/// is exercised against a synthetic item — no current work-list category seeds
/// a non-empty anchor into the queue ([CR-056]/[S-221]).
#[test]
fn render_prompt_block_leads_with_a_header_and_lists_each_command() {
    let queue = WikiGenerationQueue {
        items: vec![GenerationItem {
            category: GenerationCategory::UnanchoredEntity,
            slug: "modules/crate--widgets".to_string(),
            title: "widgets".to_string(),
            anchor: "symbol:crate::widgets".to_string(),
            reason: "no-page",
            command: write_skeleton("modules/crate--widgets", "widgets", "symbol:crate::widgets"),
            grounding: None,
        }],
    };
    let block = queue.render_prompt_block();
    assert!(
        block.starts_with("Wiki generation queue — "),
        "the block leads with a header: {block}"
    );
    assert!(block.contains("logos wiki write modules/crate--widgets"), "block: {block}");
    // The `anchor:` line is rendered for an item with a non-empty anchor.
    assert!(
        block.contains("anchor: symbol:crate::widgets"),
        "a non-empty anchor is shown on its own line: {block}"
    );
    assert_eq!(block, queue.render_prompt_block(), "rendering is deterministic");
}

/// The header uses the singular "1 page" when exactly one item is queued
/// ([FR-WK-13]). Authoring four of the five Overview children at the current
/// revision leaves exactly one absent — the only item left to queue.
#[test]
fn render_prompt_block_uses_singular_for_a_single_item() {
    let mut conn = db::open_in_memory();
    let world = FakeWorld::default();
    for slug in [
        OverviewSection::GettingStarted.slug(),
        OverviewSection::KeyConcepts.slug(),
        OverviewSection::HowItWorks.slug(),
        OverviewSection::KnownIssues.slug(),
    ] {
        wr_at(&mut conn, &world, "h", slug, "t", PAGE, &[], "gen", 1).unwrap();
    }
    let st = status(&conn, &world, &world, 1, 1).unwrap();
    let queue = generation_queue(&st);
    assert_eq!(queue.items.len(), 1, "exactly the one un-authored Overview child is queued");
    assert!(
        queue
            .render_prompt_block()
            .starts_with("Wiki generation queue — 1 page to generate.\n"),
        "the header is singular for one item: {}",
        queue.render_prompt_block()
    );
}

/// Every `GenerationCategory::as_str` label matches its serde (`--json`)
/// serialization — the human prompt block and the machine queue agree on the
/// kebab-case wire contract, and a future rename cannot silently diverge the two.
#[test]
fn generation_category_as_str_matches_its_serde_serialization() {
    for category in [
        GenerationCategory::Overview,
        GenerationCategory::ConsolidatedDoc,
        GenerationCategory::UnanchoredEntity,
        GenerationCategory::StalePage,
        GenerationCategory::MissingAnchorPage,
    ] {
        // serde serializes the unit variant to a bare JSON string ("overview"…).
        let serialized = serde_json::to_string(&category).unwrap();
        assert_eq!(
            serialized,
            format!("\"{}\"", category.as_str()),
            "as_str and serde must agree for {category:?}"
        );
    }
}

// ── FR-WK-13 / CR-034: doc-grounded, consolidated generation ──────────────────

/// [FR-WK-24]: four of the five Overview children carry a doc-grounding
/// directive naming their mapped user-facing doc(s); when every mapped source
/// is present the directive reads them and `fallback_to_code` is `false`, when
/// any is absent it falls back to code-reading (`fallback_to_code` is `true`)
/// ([CR-034], [CRA-01]). Known Issues stays free synthesis (no grounding) in
/// both cases. The retired Architecture overview ([CR-062]) is never present.
#[test]
fn overview_children_carry_doc_grounding_with_code_fallback_in_case_2() {
    let conn = db::open_in_memory();
    let section = |st: &WikiStatus, slug: &str| {
        st.work_list
            .structured_sections
            .iter()
            .find(|s| s.slug == slug)
            .unwrap_or_else(|| panic!("section {slug} present"))
            .clone()
    };

    // Every mapped user-facing doc present, architecture.md absent → Case 2
    // (no SRS gate), and every grounded section reads its mapped doc(s).
    let world = FakeWorld::default()
        .doc_present("README.md")
        .doc_present("docs/specs/software-spec.md")
        .doc_present("docs/howto/README.md")
        .doc_present("docs/howto/installation.md")
        .doc_present("docs/howto/usage.md");
    let st = status(&conn, &world, &world, 1, 1).unwrap();

    let project = section(&st, "overview/project-overview")
        .grounding
        .expect("Project Overview is doc-grounded");
    assert_eq!(
        project.sources,
        vec!["README.md".to_string(), "docs/specs/software-spec.md".to_string()]
    );
    assert!(!project.fallback_to_code, "present docs → no code fallback");
    // [FR-WK-24] strengthened by [CR-064]: the present-branch directive itself
    // (not just the SKILL.md template) carries the reader-facing,
    // concrete-outcome-first framing — mirrors the fallback-branch check below.
    assert!(
        project.directive.contains("reader's perspective") && project.directive.contains("concrete outcome"),
        "present-doc directive should open with a concrete-outcome, reader-facing framing: {}",
        project.directive
    );
    assert!(
        project.directive.contains("not internal symbols/types"),
        "present-doc directive should steer away from internal symbols/types: {}",
        project.directive
    );

    let getting_started = section(&st, "overview/getting-started")
        .grounding
        .expect("Getting Started is doc-grounded");
    assert_eq!(
        getting_started.sources,
        vec![
            "docs/howto/README.md".to_string(),
            "docs/howto/installation.md".to_string(),
            "docs/howto/usage.md".to_string(),
        ]
    );
    assert!(!getting_started.fallback_to_code, "present docs → no code fallback");

    let how_it_works = section(&st, "overview/how-it-works")
        .grounding
        .expect("How It Works is doc-grounded");
    assert_eq!(
        how_it_works.sources,
        vec!["docs/howto/usage.md".to_string(), "docs/specs/software-spec.md".to_string()]
    );
    assert!(!how_it_works.fallback_to_code, "present docs → no code fallback");

    let key_concepts = section(&st, "overview/key-concepts")
        .grounding
        .expect("Key Concepts is doc-grounded");
    assert_eq!(
        key_concepts.sources,
        vec!["README.md".to_string(), "docs/specs/software-spec.md".to_string()]
    );
    assert!(!key_concepts.fallback_to_code, "present docs → no code fallback");

    // Known Issues is the sole Overview child that stays free synthesis.
    assert!(
        section(&st, "overview/known-issues").grounding.is_none(),
        "Known Issues stays free synthesis"
    );

    // The retired Architecture overview page is never in the produced set.
    assert!(
        st.work_list
            .structured_sections
            .iter()
            .all(|s| s.slug != "overview/architecture"),
        "the synthesized overview/architecture page is retired ([CR-062])"
    );

    // With every mapped doc absent (still Case 2 — no architecture.md), every
    // grounded section falls back to code-reading, under the same framing.
    let bare = FakeWorld::default();
    let st = status(&conn, &bare, &bare, 1, 1).unwrap();
    for slug in [
        "overview/project-overview",
        "overview/getting-started",
        "overview/how-it-works",
        "overview/key-concepts",
    ] {
        let g = section(&st, slug)
            .grounding
            .expect("still grounded, now falling back to code");
        assert!(g.fallback_to_code, "{slug}: absent docs → code fallback flag set");
        assert!(
            g.directive.contains("absent") && g.directive.contains("code"),
            "{slug}: absent doc → code fallback directive: {}",
            g.directive
        );
    }
}

/// The generation queue routes a consolidated category row to
/// `ConsolidatedDoc`, places it after the six Overview children, and carries
/// the grounding directive through to the item ([CR-034], [FR-WK-13]). A file
/// with no page is never seeded alongside it ([CR-056]/[S-221]).
#[test]
fn generation_queue_routes_and_orders_consolidated_categories() {
    let conn = db::open_in_memory();
    let world = FakeWorld::default()
        .file("src/b.rs")
        .doc_category(DocCategory::Adrs)
        .doc_category(DocCategory::FrontendDesign);
    let st = status(&conn, &world, &world, 1, 1).unwrap();
    let queue = generation_queue(&st);

    let categories: Vec<(GenerationCategory, &str)> = queue
        .items
        .iter()
        .map(|i| (i.category, i.slug.as_str()))
        .collect();
    assert_eq!(
        categories,
        vec![
            (GenerationCategory::Overview, "overview/project-overview"),
            (GenerationCategory::Overview, "overview/getting-started"),
            (GenerationCategory::Overview, "overview/key-concepts"),
            (GenerationCategory::Overview, "overview/how-it-works"),
            (GenerationCategory::Overview, "overview/known-issues"),
            (GenerationCategory::ConsolidatedDoc, "architecture/adrs"),
            (GenerationCategory::ConsolidatedDoc, "specs/frontend-design"),
        ],
        "Overview → consolidated (menu order, Case 2); the unanchored file is never seeded"
    );

    // The consolidated item carries its grounding directive and a runnable,
    // zero-anchor write skeleton.
    let adrs = queue
        .items
        .iter()
        .find(|i| i.slug == "architecture/adrs")
        .unwrap();
    assert!(adrs.anchor.is_empty());
    let g = adrs.grounding.as_ref().expect("consolidated item is doc-grounded");
    assert_eq!(g.sources, vec!["docs/specs/architecture/decisions/*.md".to_string()]);
    assert_eq!(
        adrs.command,
        "logos wiki write architecture/adrs --title 'ADRs' --generator '<generator>' --body-file -"
    );

    // The grounding directive surfaces in the human prompt block.
    assert!(
        queue.render_prompt_block().contains("grounding:"),
        "the prompt block surfaces the doc-grounding directive"
    );
}

/// The fixed consolidated-page slug contract ([CR-034]) — the producer's half of
/// the slug agreement S-133's menu consumes. Pinned here so a slug rename is a
/// loud test failure, not a silent menu/work-list divergence.
#[test]
fn consolidated_category_slugs_are_the_fixed_contract() {
    assert_eq!(DocCategory::Adrs.slug(), "architecture/adrs");
    assert_eq!(DocCategory::Components.slug(), "architecture/components");
    assert_eq!(DocCategory::Integrations.slug(), "architecture/integrations");
    assert_eq!(
        DocCategory::FunctionalRequirements.slug(),
        "specs/functional-requirements"
    );
    assert_eq!(
        DocCategory::NonFunctionalRequirements.slug(),
        "specs/non-functional-requirements"
    );
    assert_eq!(
        DocCategory::UserAcceptanceTests.slug(),
        "specs/user-acceptance-tests"
    );
    assert_eq!(DocCategory::FrontendDesign.slug(), "specs/frontend-design");
    // Every category slug is a valid wiki slug and its label round-trips.
    for category in DocCategory::ALL {
        assert!(validate_slug(category.slug()).is_ok(), "{:?}", category);
        assert_eq!(DocCategory::from_label(category.label()), Some(category));
    }
}

/// `shell_single_quote` wraps an argument in single quotes and escapes an
/// embedded single quote as `'\''`, so the `wiki write` skeleton stays runnable.
#[test]
fn shell_single_quote_escapes_embedded_quotes() {
    assert_eq!(shell_single_quote("plain"), "'plain'");
    assert_eq!(shell_single_quote("a b — c"), "'a b — c'");
    assert_eq!(shell_single_quote("it's"), "'it'\\''s'");
}

// ── FR-WK-19: content-validity guard ─────────────────────────────────────────

/// Captured-garbage fixtures ([CR-059] §5.1) — each is agent-noise, not page
/// prose, and must be rejected with the store left byte-identical. Every
/// fixture trips **exactly** the signature its name describes — the expected
/// reason fragment is asserted (not just the generic `FR-WK-19` marker every
/// branch shares), so a fixture caught by the wrong check is a test failure,
/// not a silent pass.
const GARBAGE_FIXTURES: &[(&str, &str, &str)] = &[
    (
        "tool-call dump",
        "<tool_call>\n{\"name\": \"read_file\", \"arguments\": {\"path\": \"docs/specs/software-spec.md\"}}\n</tool_call>",
        "tool-call transcript",
    ),
    (
        "command-error transcript",
        "I attempted to read the source.\nError: command not found\ncmd: cat docs/specs/software-spec.md",
        "command-error transcript",
    ),
    (
        "first-person planning preamble",
        "I need to gather information about this project before I can write the page.",
        "planning/refusal preamble",
    ),
    (
        "first-person refusal preamble",
        "I can't generate this page without reading the source files first.",
        "planning/refusal preamble",
    ),
    (
        "no heading, over the minimum length",
        "Just a paragraph of plain prose with no heading at all, repeated to clear forty bytes.",
        "no Markdown heading",
    ),
    ("fence-only, no heading", "```bash\necho hello\n```", "no Markdown heading"),
    ("below the minimum length", "# T\n\nshort", "byte minimum"),
];

#[test]
fn write_rejects_every_captured_garbage_fixture_byte_identical() {
    for (name, body, reason) in GARBAGE_FIXTURES {
        let mut conn = db::open_in_memory();
        let resolver = FakeResolver::default();
        let err = wr(&mut conn, &resolver, "h", "p", "T", body, &[], "gen")
            .expect_err(&format!("fixture {name:?} should be rejected: {body:?}"));
        assert!(
            err.to_string().contains(reason),
            "fixture {name:?} is rejected by the {reason:?} signature, not a different one: {err}"
        );
        assert_eq!(page_count(&conn), 0, "fixture {name:?} leaves the store byte-identical");
    }
}

/// [FR-WK-20]/[CR-062]: the write-path prose guard **trusts** the presented
/// generator — a body that would be rejected as agent-noise for any other
/// generator is admitted when it is stamped `logos:doc-present`, because a
/// presented body is a verbatim copy of an authored document (which may itself
/// embed a `<tool_call` sample or be a short stub). The served page then carries
/// the tier-correct **presented** marker, never the generated one.
#[test]
fn a_presented_generator_bypasses_the_prose_guard_and_carries_the_presented_marker() {
    for (name, body, _reason) in GARBAGE_FIXTURES {
        let mut conn = db::open_in_memory();
        let resolver = FakeResolver::default();
        wr(&mut conn, &resolver, "h", "specs/functional-requirements", "FR", body, &[], PRESENTED_GENERATOR)
            .unwrap_or_else(|e| panic!("presented generator admits the {name:?} body: {e}"));
        let page = read(&mut conn, &resolver, "specs/functional-requirements")
            .unwrap()
            .expect("the presented page is stored");
        assert_eq!(page.body, *body, "the presented body round-trips byte-identical");
        assert_eq!(
            page.marker, PRESENTED_CONTENT_MARKER,
            "a presented page's served provenance is the presented marker, not the generated one",
        );
        assert_ne!(page.marker, GENERATED_CONTENT_MARKER);
    }
}

/// The valid set the reconciliation sweep keys on ([FR-WK-22]) includes the
/// presented Architecture slug **only when** `docs/specs/architecture.md` is
/// present — so the sweep never purges the page `materialize` just wrote in
/// Case 1, yet still purges a stale `overview/architecture` leftover in a repo
/// that carries no authored architecture.md ([CR-062]).
#[test]
fn reconciliation_valid_set_gates_the_presented_architecture_slug_on_its_source() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    assert!(
        !reconciliation_valid_slugs(root).contains(PRESENTED_ARCHITECTURE_SLUG),
        "absent architecture.md → the presented Architecture slug is not valid (a leftover is swept)"
    );
    std::fs::create_dir_all(root.join("docs/specs")).unwrap();
    std::fs::write(root.join(ARCHITECTURE_DOC), "# Architecture\n").unwrap();
    assert!(
        reconciliation_valid_slugs(root).contains(PRESENTED_ARCHITECTURE_SLUG),
        "present architecture.md → the presented Architecture page survives the sweep"
    );
}

/// The valid set the reconciliation sweep keys on ([FR-WK-22]) includes the
/// presented SRS hub slug **only when** `docs/specs/software-spec.md` is present —
/// so the sweep never purges the `specs/srs` page `materialize` just wrote in
/// Case 1, yet still purges a stale `specs/srs` leftover in a repo that carries no
/// authored SRS hub ([CR-064], S-269). Mirrors the presented-Architecture gate.
#[test]
fn reconciliation_valid_set_gates_the_presented_srs_slug_on_its_source() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    assert!(
        !reconciliation_valid_slugs(root).contains(PRESENTED_SRS_SLUG),
        "absent software-spec.md → the presented SRS slug is not valid (a leftover is swept)"
    );
    std::fs::create_dir_all(root.join("docs/specs")).unwrap();
    std::fs::write(root.join(SOFTWARE_SPEC_DOC), "# Software Requirements Specification\n").unwrap();
    assert!(
        reconciliation_valid_slugs(root).contains(PRESENTED_SRS_SLUG),
        "present software-spec.md → the presented SRS page survives the sweep"
    );
}

/// The byte-identical guarantee on a **populated** store, mirroring the
/// [FR-WK-02] precedent
/// ([`a_rejected_write_leaves_an_existing_page_byte_identical`]): an empty
/// store staying empty is a weak proxy for "byte-identical" (it holds
/// trivially). A rejected overwrite of an existing slug must leave that page's
/// title/body/generator/head completely untouched, not just the row count.
#[test]
fn write_rejects_every_garbage_fixture_as_an_overwrite_leaving_the_existing_page_untouched() {
    let original_body = "# Original\n\nThe original page body, long enough to satisfy the guard.";
    for (name, body, _reason) in GARBAGE_FIXTURES {
        let mut conn = db::open_in_memory();
        let resolver = FakeResolver::default();
        wr(&mut conn, &resolver, "h1", "p", "Original", original_body, &[], "gen1")
            .expect("the original page write succeeds");

        let err = wr(&mut conn, &resolver, "h2", "p", "New", body, &[], "gen2")
            .expect_err(&format!("fixture {name:?} should reject the overwrite: {body:?}"));
        assert!(err.to_string().contains("FR-WK-19"), "fixture {name:?}: {err}");

        let page = read(&mut conn, &resolver, "p").unwrap().unwrap();
        assert_eq!(page.title, "Original", "fixture {name:?} left the title untouched");
        assert_eq!(page.body, original_body, "fixture {name:?} left the body untouched");
        assert_eq!(page.generator, "gen1", "fixture {name:?} left the generator untouched");
        assert_eq!(page.written_head, "h1", "fixture {name:?} left the head untouched");
        assert_eq!(page_count(&conn), 1, "fixture {name:?} did not add or remove a page");
    }
}

/// A well-formed page — including one that legitimately contains a ` ```bash `
/// fenced example — is stored unchanged: the guard is a structural signature
/// match, never triggered by the mere presence of a fenced code block
/// ([FR-WK-19]).
#[test]
fn write_accepts_a_well_formed_page_with_a_bash_code_fence() {
    let mut conn = db::open_in_memory();
    let resolver = FakeResolver::default();
    let body = "# Getting Started\n\n\
                Run the CLI locally with the following command:\n\n\
                ```bash\n\
                logos index && logos wiki generate\n\
                ```\n\n\
                The command indexes the repository and lists the pending wiki work.";

    let summary = wr(&mut conn, &resolver, "h", "guide/start", "Getting Started", body, &[], "gen")
        .expect("a well-formed page with a bash fence is accepted");
    assert!(!summary.replaced);
    let page = read(&mut conn, &resolver, "guide/start").unwrap().unwrap();
    assert_eq!(page.body, body, "the fenced example is stored byte-identical");
}

/// A page that legitimately *quotes* agent-noise patterns inside fenced code
/// blocks — e.g. documentation of the guard itself — is accepted and stored
/// byte-identical: the content signatures are scanned against a fence-stripped
/// copy, so a `<tool_call` token or an `Error:`/`cmd:` pair inside a ` ``` `
/// fence never trips the guard ([FR-WK-19], [CR-059] §5.1).
#[test]
fn write_accepts_a_page_that_quotes_noise_patterns_inside_code_fences() {
    let mut conn = db::open_in_memory();
    let resolver = FakeResolver::default();
    let body = "# The Content-Validity Guard\n\n\
                The guard rejects a body that carries a tool-call token, such as:\n\n\
                ```text\n\
                <tool_call>{\"name\": \"read_file\"}</tool_call>\n\
                ```\n\n\
                It also rejects an invented command-error transcript:\n\n\
                ```text\n\
                Error: command not found\n\
                cmd: cat docs/specs/software-spec.md\n\
                ```\n\n\
                Both examples above are quoted, not emitted, so this page is valid prose.";

    let summary = wr(&mut conn, &resolver, "h", "guard/doc", "The Guard", body, &[], "gen")
        .expect("a page quoting noise patterns inside code fences is accepted");
    assert!(!summary.replaced);
    let page = read(&mut conn, &resolver, "guard/doc").unwrap().unwrap();
    assert_eq!(page.body, body, "the fenced examples are stored byte-identical");
}

/// `strip_fenced_code_blocks` blanks fenced content (and its markers) while
/// preserving line count and leaving surrounding prose intact — the exact
/// contract [`validate_prose`] relies on to exempt quoted noise.
#[test]
fn strip_fenced_code_blocks_blanks_fences_and_preserves_prose() {
    // Backtick fence: the quoted token is removed, the prose survives.
    let stripped = strip_fenced_code_blocks("before\n```\n<tool_call>x\n```\nafter");
    assert!(!stripped.contains("<tool_call"), "quoted token is stripped");
    assert!(stripped.contains("before") && stripped.contains("after"), "prose survives");
    assert_eq!(stripped.lines().count(), 5, "line count is preserved");

    // Tilde fence closes only on a matching tilde run.
    let tilde = strip_fenced_code_blocks("~~~\n<tool_call>\n~~~");
    assert!(!tilde.contains("<tool_call"), "tilde-fenced token is stripped");

    // An inline ```code``` fragment must NOT open a block that swallows prose.
    let inline = strip_fenced_code_blocks("see ```x``` here\n<tool_call>y");
    assert!(inline.contains("<tool_call"), "inline fence does not open a block");

    // A bare, unfenced token is left intact for the guard to reject.
    let bare = strip_fenced_code_blocks("<tool_call>{}</tool_call>");
    assert!(bare.contains("<tool_call"), "unfenced token is preserved");
}

/// `has_markdown_heading` / `contains_error_transcript` unit-level pins for the
/// exact boundary the table-driven fixture test exercises end to end.
#[test]
fn has_markdown_heading_recognizes_atx_headings_only() {
    assert!(has_markdown_heading("# Title\n\nbody"));
    assert!(has_markdown_heading("intro\n\n## Sub-heading\n\nbody"));
    assert!(!has_markdown_heading("no heading here at all"));
    assert!(!has_markdown_heading("#no-space-after-hash"));
    assert!(!has_markdown_heading("####### seven hashes is not ATX"));
}

#[test]
fn contains_error_transcript_requires_both_lines_near_each_other() {
    assert!(contains_error_transcript("Error: it failed\ncmd: ls -la"));
    assert!(contains_error_transcript("prose\nError: it failed\nmore prose\ncmd: ls -la\ntail"));
    assert!(
        contains_error_transcript("Error: it failed, cmd: ls -la"),
        "the two fragments sharing one line also matches"
    );
    assert!(
        !contains_error_transcript("Error: it failed"),
        "an Error: line with no nearby cmd: line does not match"
    );
    assert!(
        !contains_error_transcript("cmd: ls -la"),
        "a cmd: line with no Error: line does not match"
    );
}
