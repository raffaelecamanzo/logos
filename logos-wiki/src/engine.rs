//! The wiki's operations over `&Engine` ([wiki-engine], [ADR-74], S-644).
//!
//! These are the fourteen `Engine::wiki_*` methods that lived on the core's
//! façade until the wiki left `logos-core` for this crate. They are the same
//! methods with the same bodies: [`WikiEngine`] is implemented for
//! [`Engine`], so a caller that brings the trait into scope keeps writing
//! `engine.wiki_read(slug)`. Each call still goes through the core's single
//! observability chokepoint under its unchanged telemetry name — through the
//! extension door, as the `wiki` extension's own tool ([`crate::telemetry`]) —
//! so the CLI and MCP twins of every operation still share one entrypoint and
//! one telemetry record ([NFR-CC-01]).
//!
//! The one thing that could not move verbatim is the native tier's memo: it was
//! an `Engine` field, and an extension owns no field on the engine. It lives in
//! the engine's per-type extension state ([`Engine::extension_state`]) instead
//! — still one memo per engine, keyed by the graph revision.
//!
//! [wiki-engine]: ../../docs/specs/architecture/components/wiki-engine.md
//! [ADR-74]: ../../docs/specs/architecture/decisions/ADR-74.md
//! [NFR-CC-01]: ../../docs/specs/requirements/NFR-CC-01.md

use std::path::Path;
use std::sync::Mutex;

use anyhow::Result;
use logos_core::observability::{traced_extension, traced_extension_infallible};
use logos_core::Engine;

use crate::telemetry;

/// The native (extracted) wiki tier's render memo ([FR-WK-10], [ADR-32]),
/// keyed by the persisted graph revision ([FR-SY-09]): `Some((revision,
/// native))` when a render at `revision` is cached. A call at the same revision
/// is an O(1) hit; a revision advance is a miss that re-renders. The native tier
/// has no store — this in-memory memo is the entire memoization, nothing is
/// persisted to `wiki.db`. One per engine, through [`Engine::extension_state`].
///
/// [FR-WK-10]: ../../docs/specs/requirements/FR-WK-10.md
/// [FR-SY-09]: ../../docs/specs/requirements/FR-SY-09.md
/// [ADR-32]: ../../docs/specs/architecture/decisions/ADR-32.md
#[derive(Default)]
struct NativeMemo(Mutex<Option<(u64, crate::NativeWiki)>>);

/// The source wiki's operations over a [`Engine`] ([FR-WK-01]..[FR-WK-23]).
///
/// Bring it into scope (`use logos_wiki::WikiEngine;`) to call them as methods.
///
/// [FR-WK-01]: ../../docs/specs/requirements/FR-WK-01.md
/// [FR-WK-23]: ../../docs/specs/requirements/FR-WK-23.md
pub trait WikiEngine {
    /// Upsert a wiki page by slug ([FR-WK-02]): byte-verbatim body (1 MiB cap),
    /// write-time anchor resolution to content hashes, write-time HEAD tag, and a
    /// mandatory non-empty generator label, with loud rejection of an unknown
    /// anchor / empty generator / over-cap body (store left byte-identical).
    ///
    /// Anchors are the `"<kind>:<key>"` wire form — `file:<repo-relative-path>`
    /// or `symbol:<canonical symbol>`. They are resolved against whatever graph
    /// currently exists **without** forcing an index ([`Engine::nav_runtime_no_prologue`]):
    /// file anchors resolve by hashing the file on disk, symbol anchors by
    /// resolving the node in the graph. `wiki.db` is never `ATTACH`-ed to
    /// `logos.db` and never gated ([BR-29]).
    ///
    /// # Errors
    /// Returns an error (the surface maps to a non-zero exit, store unchanged) on
    /// an invalid slug, an empty generator, an over-cap body, or an unknown
    /// anchor; or for a transient [`Engine::open`] engine (no read-only pool).
    ///
    /// [FR-WK-02]: ../../../docs/specs/requirements/FR-WK-02.md
    /// [BR-29]: ../../../docs/specs/software-spec.md#324-source-wiki
    fn wiki_write(
        &self,
        slug: &str,
        title: &str,
        body: &str,
        anchors: &[String],
        generator: &str,
    ) -> Result<crate::WriteSummary>;

    /// Read a wiki page by slug with mandatory provenance and read-time per-anchor
    /// freshness ([FR-WK-03], [FR-WK-04]).
    ///
    /// Freshness is computed against the **current working tree** (no `sync`
    /// required, the [ADR-23] precedent): an edited anchor reads stale, a gone
    /// anchor reads missing. The orphan lifecycle runs here ([FR-WK-07]): a page
    /// whose every anchor is gone is auto-deleted and logged, and this read
    /// returns `Ok(None)`. A page with at least one surviving anchor is returned
    /// with the missing ones flagged.
    ///
    /// # Errors
    /// Returns an error only on an unexpected store failure, or for a transient
    /// [`Engine::open`] engine.
    ///
    /// [FR-WK-03]: ../../../docs/specs/requirements/FR-WK-03.md
    /// [FR-WK-04]: ../../../docs/specs/requirements/FR-WK-04.md
    /// [FR-WK-07]: ../../../docs/specs/requirements/FR-WK-07.md
    /// [ADR-23]: ../../../docs/specs/architecture/decisions/ADR-23.md
    fn wiki_read(&self, slug: &str) -> Result<Option<crate::WikiPage>>;

    /// Explicitly delete a wiki page by slug ([FR-WK-07], CLI-only surface).
    ///
    /// # Errors
    /// Returns an error when no page with that slug exists — a non-zero exit so a
    /// typo'd delete is loud, never a silent no-op.
    fn wiki_delete(&self, slug: &str) -> Result<()>;

    /// The recorded auto-deletions ([FR-WK-07]), newest first — the durable trace
    /// the `wiki status` work-list (S-053) surfaces.
    ///
    /// # Errors
    /// Returns an error only on an unexpected store failure.
    fn wiki_pruned_log(&self) -> Result<Vec<crate::PrunedPage>>;

    /// FTS5 bm25 search over wiki page titles + bodies, or enumerate every page
    /// in `list` mode ([FR-WK-05]). Each hit carries its staleness flag and
    /// provenance summary, computed against the current tree (no `sync`, the
    /// [ADR-23] precedent). The `logos wiki search` subcommand and the
    /// `wiki_search` MCP twin share this one `api` entrypoint byte-for-byte
    /// ([FR-WK-09], [NFR-CC-01]); a pure read that never prunes.
    ///
    /// The FTS5 index lives inside `wiki.db`, so search survives a full `index`
    /// like the pages it indexes; `wiki.db` is never `ATTACH`-ed to `logos.db`
    /// and never gated ([BR-29]).
    ///
    /// # Errors
    /// Returns an error only on an unexpected store failure, or for a transient
    /// [`Engine::open`] engine.
    ///
    /// [FR-WK-05]: ../../../docs/specs/requirements/FR-WK-05.md
    /// [FR-WK-09]: ../../../docs/specs/requirements/FR-WK-09.md
    /// [ADR-23]: ../../../docs/specs/architecture/decisions/ADR-23.md
    /// [BR-29]: ../../../docs/specs/software-spec.md#324-source-wiki
    fn wiki_search(&self, query: &str, list: bool) -> Result<Vec<crate::WikiHit>>;

    /// The wiki store summary + regeneration work-list ([FR-WK-06]): page/stale
    /// counts and the freshness fraction, the pruned-orphan log ([FR-WK-07]), and
    /// the work-list — stale pages, missing-anchor pages, and page-worthy
    /// entities that no page anchors yet (currently none: modules, top-level
    /// files, and the typed Requirement/Adr/Story nodes are all excluded,
    /// [CR-034]/[CR-056]), plus the agent-tier structured sections that are
    /// absent or revision-stale ([FR-WK-06] as modified by [CR-027], [FR-WK-12],
    /// [CR-056] — the six Overview prose children and the present consolidated
    /// documentation categories; per-file objectives are no longer seeded and
    /// the native tier is never listed). The `logos wiki status` subcommand and
    /// the `wiki_status` MCP twin share this one `api` entrypoint byte-for-byte
    /// ([FR-WK-09], [NFR-CC-01]); a deterministic function of `wiki.db` + the
    /// current graph/tree ([NFR-RA-06]), and a pure read that never prunes.
    /// Structured-section re-queue cadence is dampened by the configurable
    /// `[wiki].revision_stale_threshold` ([FR-WK-17]); `revision_stale_count`
    /// stays truthful regardless — dampening never masks reporting.
    ///
    /// # Errors
    /// Returns an error only on an unexpected store failure, or for a transient
    /// [`Engine::open`] engine.
    ///
    /// [FR-WK-06]: ../../../docs/specs/requirements/FR-WK-06.md
    /// [FR-WK-07]: ../../../docs/specs/requirements/FR-WK-07.md
    /// [FR-DG-07]: ../../../docs/specs/requirements/FR-DG-07.md
    /// [FR-WK-09]: ../../../docs/specs/requirements/FR-WK-09.md
    /// [FR-WK-17]: ../../../docs/specs/requirements/FR-WK-17.md
    fn wiki_status(&self) -> Result<crate::WikiStatus>;

    /// Format the `wiki status` work-list ([FR-WK-06]) into the ordered, offline
    /// **generation queue** ([FR-WK-13]) — the ready-to-run plan that drives the
    /// connected agent's off-request generation loop ([ADR-33]): the six Overview
    /// prose children, then the present consolidated documentation categories,
    /// then unanchored page-worthy entities (currently always empty, [CR-056]),
    /// then stale/missing existing-page refreshes, each carrying its target slug
    /// and a runnable `wiki write` skeleton; the deterministic native tier
    /// ([FR-WK-10]) is never queued. The `logos wiki generate` subcommand renders
    /// this as a human prompt block by default and the serialized queue under
    /// `--json`.
    ///
    /// A **pure read of `wiki.db` + the current graph revision**, sharing the
    /// exact [`wiki_status`](Self::wiki_status) read path: it performs no
    /// `wiki.db` write, no LLM call, and no network call ([NFR-SE-01]) — Logos
    /// drives the loop, the agent synthesizes the prose off the request path. A
    /// deterministic function of `wiki.db` + the revision ([NFR-RA-06]), so the
    /// `--json` queue is byte-identical for a fixed store + revision; an empty
    /// work-list yields an empty queue (the honest "nothing to generate",
    /// [NFR-CC-04]). One instrumented façade span — it dispatches the *inner*
    /// `wiki::status` read, never the already-traced public `wiki_status`, so it
    /// emits exactly one telemetry record and never nests a façade span
    /// ([NFR-CC-01]).
    ///
    /// # Errors
    /// Returns an error only on an unexpected store failure, or for a transient
    /// [`Engine::open`] engine.
    ///
    /// [FR-WK-06]: ../../../docs/specs/requirements/FR-WK-06.md
    /// [FR-WK-13]: ../../../docs/specs/requirements/FR-WK-13.md
    /// [FR-WK-10]: ../../../docs/specs/requirements/FR-WK-10.md
    /// [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    /// [NFR-CC-01]: ../../../docs/specs/requirements/NFR-CC-01.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    /// [ADR-33]: ../../../docs/specs/architecture/decisions/ADR-33.md
    fn wiki_generate(&self) -> Result<crate::WikiGenerationQueue>;

    /// The **native (extracted) wiki tier** ([FR-WK-10], [ADR-32]): the three
    /// deterministic sections live-rendered from the graph — Codebase structure,
    /// the Files view, and the dependency Mermaid diagram —
    /// carrying the "extracted — live from graph @revision N" provenance label.
    ///
    /// **Memoized by the persisted graph revision** ([FR-SY-09]): the first call
    /// at a revision renders from the graph and caches the result; subsequent
    /// calls at the same revision return the cached value (a cache hit is O(1),
    /// [NFR-PE-01]); a revision advance (after `index`/`sync`) is a cache miss
    /// that re-renders. **Nothing is ever written to `wiki.db`** — the native
    /// tier has no store ([ADR-32]); the render performs no second file walk and
    /// no LLM/network call ([NFR-SE-01]), so it is safe on the write-free-on-read
    /// GET path ([ADR-28], [BR-35]) and is never read by the gated metric path
    /// ([BR-29]).
    ///
    /// # Errors
    /// Returns an error only on an unexpected store failure, or for a transient
    /// [`Engine::open`] engine (no read-only pool).
    ///
    /// [FR-WK-10]: ../../../docs/specs/requirements/FR-WK-10.md
    /// [FR-SY-09]: ../../../docs/specs/requirements/FR-SY-09.md
    /// [NFR-PE-01]: ../../../docs/specs/requirements/NFR-PE-01.md
    /// [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
    /// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
    /// [ADR-32]: ../../../docs/specs/architecture/decisions/ADR-32.md
    /// [BR-29]: ../../../docs/specs/software-spec.md#324-source-wiki
    fn wiki_native(&self) -> Result<crate::NativeWiki>;

    /// Whether the `docs/` source file(s) for a consolidated documentation
    /// category ([`DocCategory`](crate::DocCategory), [CR-034]) exist under
    /// the project root — the presence signal the wiki menu's **Specs** tier uses
    /// to decide whether to show the optional **Frontend Design** entry
    /// ([FR-WK-11] as modified, S-133). Rendered against the **same fixed slug
    /// contract** the generation work-list ([FR-WK-06]) seeds, so the menu and the
    /// producer can never disagree on which category pages exist.
    ///
    /// A pure local-filesystem read (one `is_file`/`read_dir`) — no graph/store
    /// read, no `wiki.db` write, no LLM, and no network ([NFR-SE-01]) — so it is
    /// safe on the write-free-on-read GET path ([ADR-28]) and never moves the
    /// quality signal (the wiki store is gate-immune). Infallible by design: a
    /// missing directory reads as "absent", never an error.
    ///
    /// [FR-WK-11]: ../../../docs/specs/requirements/FR-WK-11.md
    /// [FR-WK-06]: ../../../docs/specs/requirements/FR-WK-06.md
    /// [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
    /// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
    fn wiki_doc_category_present(&self, category: crate::DocCategory) -> bool;

    /// The **SRS-mode gate** ([FR-WK-21], [ADR-57]): `true` (Case 1 — SRS present)
    /// when the project carries both load-bearing swe-skills artifacts —
    /// `docs/specs/architecture.md` **and** at least one `FR-*`/`NFR-*`/`UAT-*`
    /// file under `docs/specs/requirements/` — else `false` (Case 2). This is the
    /// façade half of the predicate the wiki work-list ([FR-WK-06]) / generation
    /// queue ([FR-WK-13]) branch on: in Case 1 the Design/Specs pages are produced
    /// by the deterministic presented tier ([FR-WK-20]) and the agent queue is
    /// restricted to the Summary tier; in Case 2 the agent infers the full set
    /// from the code graph. It is the surface accessor the CLI/MCP/hook/UI
    /// (wired later) consult to choose presentation over inference.
    ///
    /// A pure local-filesystem read (a handful of `is_file`/`read_dir`) — no
    /// graph/store read, no `wiki.db` write, no LLM, and no network ([NFR-SE-01]) —
    /// so it is safe on the write-free-on-read GET path ([ADR-28]) and never moves
    /// the quality signal. Infallible by design: a missing file/directory reads as
    /// Case 2, never an error; a deterministic function of the on-disk layout
    /// ([NFR-RA-06]).
    ///
    /// [FR-WK-21]: ../../../docs/specs/requirements/FR-WK-21.md
    /// [FR-WK-06]: ../../../docs/specs/requirements/FR-WK-06.md
    /// [FR-WK-13]: ../../../docs/specs/requirements/FR-WK-13.md
    /// [FR-WK-20]: ../../../docs/specs/requirements/FR-WK-20.md
    /// [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    /// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
    /// [ADR-57]: ../../../docs/specs/architecture/decisions/ADR-57.md
    fn wiki_srs_mode(&self) -> bool;

    /// The **User Guide** tier's page set ([FR-WK-23], [FR-WK-11]) — one
    /// `(slug, title)` pair per `docs/howto/*.md` file, in the same order
    /// [`Self::wiki_materialize`] writes them (`README.md` first, at
    /// `guide/overview`). The web menu ([FR-UI-06]) uses this to render (and
    /// gate the presence of) the User Guide tier without a fixed enum like
    /// [`DocCategory`](crate::DocCategory) — the guide file set is
    /// per-project. **Gated on [`Self::wiki_srs_mode`]** — empty in Case 2
    /// even when `docs/howto/` has files, since `wiki_materialize` never writes
    /// a Case-2 guide page and a Case-2 menu listing would be a permanent,
    /// un-fulfillable "not yet generated" placeholder. Also empty when
    /// `docs/howto/` has no `*.md` file, so the tier is simply absent rather
    /// than an empty group ([FR-WK-11]).
    ///
    /// A pure local-filesystem read (a `read_dir` + per-file read) — no
    /// graph/store read, no `wiki.db` write, no LLM, and no network
    /// ([NFR-SE-01]) — so it is safe on the write-free-on-read GET path
    /// ([ADR-28]) and never moves the quality signal. Infallible by design: a
    /// missing directory reads as an empty tier, never an error.
    ///
    /// [FR-WK-23]: ../../../docs/specs/requirements/FR-WK-23.md
    /// [FR-WK-11]: ../../../docs/specs/requirements/FR-WK-11.md
    /// [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
    /// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
    fn wiki_guide_pages(&self) -> Vec<(String, String)>;

    /// The reconciliation sweep ([FR-WK-22]): bulk-purge any stored page whose
    /// slug falls outside the active-mode valid set — the five Overview/Summary
    /// slugs ∪ the present consolidated-category slugs — logging each removal to
    /// the pruned-log ([`Self::wiki_pruned_log`] surfaces the trace). Retires
    /// orphaned pages left by a prior or superseded generation run that are
    /// unreachable from the menu ([FR-WK-11]) and un-regenerable by the work-list
    /// ([FR-WK-06]) — the lazy all-anchors-gone orphan lifecycle ([FR-WK-07])
    /// never reaches them, since it fires only on read.
    ///
    /// Idempotent: once every stored slug is valid, a re-run purges nothing. A
    /// pure store + local-FS operation — no LLM, no network call ([NFR-SE-01]).
    /// Runs at the end of the [FR-WK-20] `wiki materialize` path (S-262); exposed
    /// as its own façade operation so it is independently callable and testable
    /// ahead of that wiring.
    ///
    /// Returns the purged slugs, in slug order.
    ///
    /// # Errors
    /// Returns an error only on an unexpected store failure.
    ///
    /// [FR-WK-22]: ../../../docs/specs/requirements/FR-WK-22.md
    /// [FR-WK-11]: ../../../docs/specs/requirements/FR-WK-11.md
    /// [FR-WK-06]: ../../../docs/specs/requirements/FR-WK-06.md
    /// [FR-WK-07]: ../../../docs/specs/requirements/FR-WK-07.md
    /// [FR-WK-20]: ../../../docs/specs/requirements/FR-WK-20.md
    /// [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
    fn wiki_reconcile(&self) -> Result<Vec<String>>;

    /// The deterministic **presented tier** ([FR-WK-20], [ADR-57], [CR-062]): in
    /// SRS mode (Case 1, [`Self::wiki_srs_mode`]) assemble each present
    /// Design/Specs category — glob → sorted section-per-source-document
    /// consolidated Markdown — the single-file Architecture page
    /// (`docs/specs/architecture.md` → the `overview/architecture` slug), **and**
    /// the **User Guide** tier's per-file pages ([FR-WK-23]) — one page per
    /// `docs/howto/*.md` file, `README.md` → `guide/overview`, rendered
    /// verbatim — directly from the project's authored `docs/specs/**`/
    /// `docs/howto/**` sources, upsert each into `wiki.db` with `generator =
    /// "logos:doc-present"`, one source-file anchor per document, and the
    /// current built-at revision, then run the [FR-WK-22] reconciliation sweep
    /// so orphaned prior-run pages (including a `guide/*` page whose source file
    /// was deleted) are purged in the same operation. A category whose source
    /// glob matches no file, or an absent `docs/howto/`, yields no page.
    ///
    /// In **Case 2** (no authored SRS) it is a no-op returning the empty summary:
    /// presentation is reserved for projects carrying the load-bearing
    /// `architecture.md` + a requirement; the connected agent infers the full set
    /// from the code graph as before ([FR-WK-21]).
    ///
    /// Deterministic and offline — pure local-FS reads + `wiki.db` writes, no LLM
    /// and no network ([NFR-SE-01]); re-running with unchanged sources is
    /// byte-identical ([FR-WK-20]). This relaxes the [ADR-24] "binary never writes
    /// the store" property for faithful presentation while preserving the
    /// prose-authoring ban ([ADR-57]): the binary copies authored docs verbatim, it
    /// never *authors* prose.
    ///
    /// # Errors
    /// Returns an error on a write-path rejection or an unexpected store failure,
    /// or for a transient [`Engine::open`] engine (no read-only pool).
    ///
    /// [FR-WK-20]: ../../../docs/specs/requirements/FR-WK-20.md
    /// [FR-WK-21]: ../../../docs/specs/requirements/FR-WK-21.md
    /// [FR-WK-22]: ../../../docs/specs/requirements/FR-WK-22.md
    /// [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
    /// [ADR-24]: ../../../docs/specs/architecture/decisions/ADR-24.md
    /// [ADR-57]: ../../../docs/specs/architecture/decisions/ADR-57.md
    /// [CR-062]: ../../../docs/requests/CR-062-wiki-present-authored-docs.md
    fn wiki_materialize(&self) -> Result<crate::MaterializeSummary>;

    /// Materialize the embedded wiki-generation skill ([FR-WK-08], CLI-only
    /// surface) into `dir` (or the project root when `None`), restoring the
    /// embedded content when `force` is set and skipping an existing install
    /// otherwise. Pure local filesystem I/O — no network ([NFR-SE-01]).
    ///
    /// This is the post-install / post-upgrade refresh path behind
    /// `logos wiki skill --emit [dir] [--force]`; the `init -i` materialization
    /// runs the same engine through [`logos_core::init`].
    ///
    /// [FR-WK-08]: ../../../docs/specs/requirements/FR-WK-08.md
    /// [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
    fn wiki_skill_emit(
        &self,
        dir: Option<&Path>,
        force: bool,
    ) -> Result<crate::EmitSummary>;
}

impl WikiEngine for Engine {
    fn wiki_write(
        &self,
        slug: &str,
        title: &str,
        body: &str,
        anchors: &[String],
        generator: &str,
    ) -> Result<crate::WriteSummary> {
        traced_extension(telemetry::WIKI_WRITE, || {
            let runtime = self.nav_runtime_no_prologue()?;
            let head = crate::head_sha(self.root()).unwrap_or_default();
            let mut conn = crate::open(self.root())?;
            runtime.submit_read(|store| {
                // Capture the persisted graph revision the page is built at, so
                // the two-tier view can later derive its freshness with no write
                // on the page view ([FR-WK-12], [FR-SY-09], [ADR-32]).
                let built_at_revision = store.graph_revision()?;
                let draft = crate::PageDraft {
                    slug,
                    title,
                    body,
                    anchors,
                    generator,
                    written_head: &head,
                    built_at_revision,
                };
                let resolver = crate::GraphAnchorResolver::new(store, self.root());
                crate::write(&mut conn, &resolver, &draft)
            })
        })
    }

    fn wiki_read(&self, slug: &str) -> Result<Option<crate::WikiPage>> {
        traced_extension(telemetry::WIKI_READ, || {
            let runtime = self.nav_runtime_no_prologue()?;
            let mut conn = crate::open(self.root())?;
            runtime.submit_read(|store| {
                let resolver = crate::GraphAnchorResolver::new(store, self.root());
                crate::read(&mut conn, &resolver, slug)
            })
        })
    }

    fn wiki_delete(&self, slug: &str) -> Result<()> {
        traced_extension(telemetry::WIKI_DELETE, || {
            let conn = crate::open(self.root())?;
            crate::delete(&conn, slug)
        })
    }

    fn wiki_pruned_log(&self) -> Result<Vec<crate::PrunedPage>> {
        traced_extension(telemetry::WIKI_PRUNED_LOG, || {
            let conn = crate::open(self.root())?;
            crate::pruned_log(&conn)
        })
    }

    fn wiki_search(&self, query: &str, list: bool) -> Result<Vec<crate::WikiHit>> {
        traced_extension(telemetry::WIKI_SEARCH, || {
            let runtime = self.nav_runtime_no_prologue()?;
            let conn = crate::open(self.root())?;
            runtime.submit_read(|store| {
                // The current persisted graph revision ([FR-SY-09]) each hit's
                // built-at revision is compared against, so the search "State"
                // carries the same "stale — regeneration pending" verdict the
                // page view shows ([FR-WK-12], [CR-039]); `0` (no `index` yet)
                // yields no pending verdict. The exact seam `wiki_status` uses.
                let revision = store.graph_revision()?;
                let resolver = crate::GraphAnchorResolver::new(store, self.root());
                crate::search(&conn, &resolver, query, list, revision)
            })
        })
    }

    fn wiki_status(&self) -> Result<crate::WikiStatus> {
        traced_extension(telemetry::WIKI_STATUS, || {
            let runtime = self.nav_runtime_no_prologue()?;
            let conn = crate::open(self.root())?;
            // The revision-stale re-queue dampening threshold ([FR-WK-17],
            // [`WikiConfig::effective_revision_stale_threshold`]) — a plain
            // local `config.toml` read, no LLM/network call ([NFR-SE-01]).
            let threshold = logos_core::config::load_config_from_root(self.root())?
                .wiki
                .effective_revision_stale_threshold();
            runtime.submit_read(|store| {
                // The current persisted graph revision ([FR-SY-09]) seeds the
                // agent-tier structured sections: a page built at an older
                // revision is "stale — regeneration pending"; `0` (no `index`
                // yet) yields no structured work, so `init` is never blocked
                // ([FR-WK-12], [ADR-32]).
                let revision = store.graph_revision()?;
                let resolver = crate::GraphAnchorResolver::new(store, self.root());
                let status = crate::status(&conn, &resolver, &resolver, revision, threshold)?;
                Ok(status)
            })
        })
    }

    fn wiki_generate(&self) -> Result<crate::WikiGenerationQueue> {
        traced_extension(telemetry::WIKI_GENERATE, || {
            let runtime = self.nav_runtime_no_prologue()?;
            let conn = crate::open(self.root())?;
            // Same dampening threshold `wiki_status` resolves ([FR-WK-17]) — the
            // queue is a pure reformatting of the same work-list ([FR-WK-13]).
            let threshold = logos_core::config::load_config_from_root(self.root())?
                .wiki
                .effective_revision_stale_threshold();
            let status = runtime.submit_read(|store| {
                let revision = store.graph_revision()?;
                let resolver = crate::GraphAnchorResolver::new(store, self.root());
                crate::status(&conn, &resolver, &resolver, revision, threshold)
            })?;
            Ok(crate::generation_queue(&status))
        })
    }

    fn wiki_native(&self) -> Result<crate::NativeWiki> {
        traced_extension(telemetry::WIKI_NATIVE, || {
            let runtime = self.nav_runtime_no_prologue()?;
            let revision = runtime.submit_read(|store| store.graph_revision())?;
            let memo = self.extension_state::<NativeMemo>();

            // Cache hit: the render at this revision is already memoized.
            {
                let cache = memo.0.lock().expect("native-wiki cache lock");
                if let Some((cached_revision, native)) = cache.as_ref() {
                    if *cached_revision == revision {
                        return Ok(native.clone());
                    }
                }
            }

            // Cache miss: re-render from the graph at the current revision and
            // memoize. Nothing is written to `wiki.db` ([ADR-32]).
            let native =
                runtime.submit_read(|store| crate::render_native(store, revision))?;
            let mut cache = memo.0.lock().expect("native-wiki cache lock");
            *cache = Some((revision, native.clone()));
            Ok(native)
        })
    }

    fn wiki_doc_category_present(&self, category: crate::DocCategory) -> bool {
        // Through the single observability chokepoint ([ADR-13], [NFR-CC-01]) like
        // every other Engine accessor — `traced_infallible` because the presence
        // check cannot fail (a missing directory reads as `false`), matching the
        // `languages()` infallible-accessor precedent.
        traced_extension_infallible(telemetry::WIKI_DOC_CATEGORY_PRESENT, || {
            category.present_under(self.root())
        })
    }

    fn wiki_srs_mode(&self) -> bool {
        // `traced_infallible` like `wiki_doc_category_present` — the gate is a pure
        // local-FS read that cannot fail (a missing artifact reads as Case 2).
        traced_extension_infallible(telemetry::WIKI_SRS_MODE, || {
            crate::wiki_srs_mode(self.root())
        })
    }

    fn wiki_guide_pages(&self) -> Vec<(String, String)> {
        traced_extension_infallible(telemetry::WIKI_GUIDE_PAGES, || {
            crate::wiki_guide_pages(self.root())
        })
    }

    fn wiki_reconcile(&self) -> Result<Vec<String>> {
        traced_extension(telemetry::WIKI_RECONCILE, || {
            let mut conn = crate::open(self.root())?;
            crate::reconcile(&mut conn, self.root())
        })
    }

    fn wiki_materialize(&self) -> Result<crate::MaterializeSummary> {
        traced_extension(telemetry::WIKI_MATERIALIZE, || {
            // Case 2: presentation is a Case-1 operation — write nothing, sweep
            // nothing, so the agent-inference path is untouched ([FR-WK-21]).
            if !crate::wiki_srs_mode(self.root()) {
                return Ok(crate::MaterializeSummary {
                    srs_mode: false,
                    materialized: Vec::new(),
                    pruned: Vec::new(),
                });
            }
            let runtime = self.nav_runtime_no_prologue()?;
            let head = crate::head_sha(self.root()).unwrap_or_default();
            let mut conn = crate::open(self.root())?;
            // Capture the built-at revision from the same read the resolver uses,
            // so a presented page's freshness derives from one revision ([FR-SY-09]).
            let materialized = runtime.submit_read(|store| {
                let built_at_revision = store.graph_revision()?;
                let resolver = crate::GraphAnchorResolver::new(store, self.root());
                crate::materialize(&mut conn, &resolver, self.root(), &head, built_at_revision)
            })?;
            // Sweep orphaned prior-run pages once the present set is written
            // ([FR-WK-22]); the valid set includes the just-written slugs.
            let pruned = crate::reconcile(&mut conn, self.root())?;
            Ok(crate::MaterializeSummary { srs_mode: true, materialized, pruned })
        })
    }

    fn wiki_skill_emit(
        &self,
        dir: Option<&Path>,
        force: bool,
    ) -> Result<crate::EmitSummary> {
        traced_extension(telemetry::WIKI_SKILL_EMIT, || {
            crate::materialize_skill(dir.unwrap_or(self.root()), force)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;

    use logos_core::Engine;

    use super::{NativeMemo, WikiEngine};

    /// An indexed engine over a one-file git repository at `root` — a temp
    /// directory outside this checkout, so no enclosing repository is found.
    fn indexed(root: &Path) -> Engine {
        std::fs::create_dir_all(root.join("src")).expect("src dir");
        std::fs::write(root.join("src/a.rs"), "pub fn a() {}\n").expect("source file");
        let init = Command::new("git").arg("-C").arg(root).args(["init", "-q"]).output();
        assert!(init.expect("git on PATH").status.success(), "git init");
        let engine = Engine::start(root).expect("engine starts");
        engine.index();
        engine
    }

    /// The native tier answers a call at an unchanged revision from **this
    /// engine's** memo ([FR-WK-10], [ADR-32]) — the one piece of the S-644 move
    /// that could not stay verbatim (an `Engine` field became
    /// [`Engine::extension_state`]). The moved `wiki_store` tests compare two
    /// renders, which are equal whether or not the second was a hit; this
    /// plants a sentinel in the memo at the current revision and reads it back,
    /// then shows a fresh engine over the same root and revision renders anew.
    ///
    /// [FR-WK-10]: ../../docs/specs/requirements/FR-WK-10.md
    /// [ADR-32]: ../../docs/specs/architecture/decisions/ADR-32.md
    #[test]
    fn a_native_render_at_an_unchanged_revision_is_served_from_this_engines_memo() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let engine = indexed(tmp.path());

        let rendered = engine.wiki_native().expect("first render");
        let memo = engine.extension_state::<NativeMemo>();
        assert_eq!(
            memo.0.lock().unwrap().as_ref().map(|(revision, _)| *revision),
            Some(rendered.revision),
            "a miss memoizes the render under its revision"
        );

        let mut planted = rendered.clone();
        planted.label = "planted".to_string();
        *memo.0.lock().unwrap() = Some((rendered.revision, planted));
        assert_eq!(engine.wiki_native().expect("hit").label, "planted", "a hit is the memo's value");

        drop(memo);
        drop(engine);
        let fresh = Engine::start(tmp.path()).expect("a second engine starts");
        let rerendered = fresh.wiki_native().expect("fresh render");
        assert_eq!(rerendered.revision, rendered.revision, "same graph, same revision");
        assert_eq!(rerendered.label, rendered.label, "a new engine has its own memo, not the planted one");
    }
}
