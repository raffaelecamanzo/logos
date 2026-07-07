//! wiki-engine — the `.logos/wiki.db` agent-generated wiki store ([wiki-engine],
//! [ADR-24], [CR-008]).
//!
//! This module owns the **fourth-store substrate** [S-052] establishes: a
//! separate `wiki.db` on its own forward-only migration track ([db]) that
//! survives a full `index`, the slug-upsert **write path** (byte-verbatim body
//! with the documented 1 MiB cap, write-time anchor resolution to content hashes,
//! write-time HEAD tag, mandatory generator label, loud rejection of unknown
//! anchors), the **read-time per-anchor freshness** computation, and the
//! **orphan lifecycle** (flag missing anchors; auto-delete + log a page when
//! every anchor is gone; explicit delete).
//!
//! # The capability split that defines the tier ([ADR-24])
//! Logos **stores, anchors, and serves**; the coding agent **generates**.
//! Generated prose is the one artifact Logos cannot reproduce deterministically
//! ([NFR-RA-06]) or offline ([NFR-SE-01]), so it lives quarantined here:
//!
//! - **Never gated.** Nothing in the metric/gate/governance path holds a
//!   connection to `wiki.db` or `ATTACH`-es it, so "the gate cannot see the wiki"
//!   is physical, not conventional ([BR-29], [UAT-WK-02]). The freshness read is
//!   reached only from a wiki surface — never from `gate`/`sync`/`index`.
//! - **Survives `index`.** A full rebuild wipes `logos.db`; `wiki.db` is a
//!   separate file the rebuild never opens ([FR-WK-01]).
//! - **Honest by construction.** Every served page carries the generator label,
//!   the written-at HEAD, per-anchor freshness, and the fixed
//!   [`GENERATED_CONTENT_MARKER`] — wiki text never masquerades as extracted
//!   fact ([FR-WK-04], [NFR-RA-05]).
//!
//! # Freshness is computed against the working tree, at read time ([FR-WK-03])
//! Anchors store the content hash captured at write; freshness re-hashes the
//! entity's current content and compares. Per the [ADR-23] coverage precedent
//! this hash is taken from the **working tree** (a file's bytes on disk), so an
//! edit flips the anchor stale at the next read with **no intervening `sync`** —
//! and `sync`/`index` never touch `wiki.db`. Freshness means "the anchor is
//! unchanged since write", never "the prose was verified" ([NFR-CC-04]).
//!
//! [wiki-engine]: ../../../docs/specs/architecture/components/wiki-engine.md
//! [ADR-23]: ../../../docs/specs/architecture/decisions/ADR-23.md
//! [ADR-24]: ../../../docs/specs/architecture/decisions/ADR-24.md
//! [CR-008]: ../../../docs/requests/CR-008-wiki-store-and-serve.md
//! [BR-29]: ../../../docs/specs/software-spec.md#324-source-wiki
//! [FR-WK-01]: ../../../docs/specs/requirements/FR-WK-01.md
//! [FR-WK-02]: ../../../docs/specs/requirements/FR-WK-02.md
//! [FR-WK-03]: ../../../docs/specs/requirements/FR-WK-03.md
//! [FR-WK-04]: ../../../docs/specs/requirements/FR-WK-04.md
//! [FR-WK-07]: ../../../docs/specs/requirements/FR-WK-07.md
//! [FR-WK-19]: ../../../docs/specs/requirements/FR-WK-19.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
//! [UAT-WK-02]: ../../../docs/specs/requirements/UAT-WK-02.md
//! [S-052]: ../../../docs/planning/journal.md#s-052-wiki-store-write-path-and-page-lifecycle
//! [CR-059]: ../../../docs/requests/CR-059-wiki-generation-grounding-and-write-guard.md

mod db;
mod hook;
pub mod native;
mod present;
mod skill;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};

use crate::graph_store::GraphStore;
use crate::model::NodeKind;

pub(crate) use db::db_path;
pub use db::PrunedPage;
pub use native::{
    native_label, render as render_native, FileEntry, FileSymbol, NativeWiki, StructureKind,
    StructureNode,
};
pub use hook::{
    materialize as materialize_hook, materialize_quality_report as materialize_quality_report_hook,
    HookEmitSummary,
};
pub use hook::{HOOK_SCRIPT_REL, QUALITY_REPORT_HOOK_SCRIPT_REL, SETTINGS_REL};
pub use skill::{
    materialize as materialize_skill, rendered_skill, EmitAction, EmitSummary, LinkKind,
};
pub use skill::{SKILL_DIR_REL, SKILL_LINK_REL};

/// Open (creating if absent) and migrate the wiki store under `root`.
///
/// Creates the `.logos/` parent if needed so a first wiki write on a brand-new
/// project works; the store is on its **own** forward-only migration track,
/// never attached to `logos.db` ([FR-WK-01]).
///
/// # Errors
/// Returns an error if the directory or file cannot be created/opened, or a
/// migration fails.
pub fn open(root: &Path) -> Result<rusqlite::Connection> {
    let path = db_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {} for the wiki store", parent.display()))?;
    }
    db::open(&path)
}

/// The newest migration version a fully migrated `wiki.db` reports — on its own
/// track, independent of `logos.db` ([FR-WK-01]). The rebuild-survival test
/// asserts the store reports this after a full `index`.
pub fn latest_version() -> i64 {
    db::latest_version()
}

/// The documented hard cap on a stored body: **1 MiB** of UTF-8 bytes ([FR-WK-02]).
/// A write whose body exceeds this is rejected loudly with the store untouched.
pub const MAX_BODY_BYTES: usize = 1024 * 1024;

/// The fixed generated-content marker every read carries ([FR-WK-04], [BR-29]).
/// Its text is part of the contract — surfaces render it verbatim so wiki prose
/// can never be mistaken for fact Logos extracted.
pub const GENERATED_CONTENT_MARKER: &str = "generated content — not extracted by Logos";

/// The generator label the deterministic **presented tier** ([FR-WK-20],
/// [ADR-57], [CR-062]) stamps on every page it assembles from authored
/// `docs/specs/**` sources — the third provenance class beside the extracted
/// native tier and the generated agent tier ([BR-29] refined to three tiers). Two
/// contracts key on it: the served provenance marker (`marker_for`) reports a
/// presented page as [`PRESENTED_CONTENT_MARKER`], never model-generated; and the
/// write-path prose guard (`validate_prose`, [FR-WK-19]) **trusts** it — a
/// presented body is a verbatim copy of an authored document, never agent noise,
/// so it is never rejected. `materialize` is the sole producer.
pub const PRESENTED_GENERATOR: &str = "logos:doc-present";

/// The served provenance marker a **presented** page carries ([FR-WK-20],
/// [BR-29]) — the tier-correct counterpart to [`GENERATED_CONTENT_MARKER`]. A
/// presented page is a faithful, deterministic rendering of an authored
/// `docs/specs/**` document, so its served provenance must never read as
/// model-generated prose nor as an extracted graph fact. Its text is part of the
/// contract — surfaces render it verbatim.
pub const PRESENTED_CONTENT_MARKER: &str = "presented from docs/specs/… — not model-generated";

// ── Anchor model ────────────────────────────────────────────────────────────

/// What kind of graph entity an anchor binds to.
///
/// A page anchors to entities by their **stable** identity, never a storage
/// rowid — rowids are reassigned by the `index` rebuild this store outlives
/// ([FR-WK-01]). `File` binds by repo-relative path; `Symbol` binds by canonical
/// [`LogosSymbol`](crate::model::LogosSymbol) string and covers code symbols,
/// `DocSection`s, and doc-graph Requirement/Adr/Story nodes — every anchorable
/// node carries a symbol ([FR-WK-02]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorKind {
    /// A file, bound by repo-relative path. Freshness/existence is the file's
    /// bytes on disk ([ADR-23] precedent).
    File,
    /// A graph node, bound by canonical symbol string. Existence is resolved in
    /// the graph; content is the node's defining file on disk.
    Symbol,
}

impl AnchorKind {
    /// The wire/storage token for this kind.
    pub fn as_str(self) -> &'static str {
        match self {
            AnchorKind::File => "file",
            AnchorKind::Symbol => "symbol",
        }
    }
}

/// A page anchor: a kind plus the stable entity key it binds to.
///
/// The wire form an agent/surface passes is `"<kind>:<key>"` — `file:src/foo.rs`
/// or `symbol:<canonical symbol>`. The key may itself contain `:` (symbol
/// strings do); only the first `:` separates the kind prefix from the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    /// Which kind of entity this binds to.
    pub kind: AnchorKind,
    /// The stable entity key (repo-relative path, or canonical symbol string).
    pub entity_id: String,
}

impl Anchor {
    /// Parse the `"<kind>:<key>"` wire form, rejecting an unknown prefix or an
    /// empty key — never guessing a kind ([NFR-RA-05]).
    ///
    /// # Errors
    /// Returns an error when the string has no recognized `file:`/`symbol:`
    /// prefix or an empty key — surfaced as a loud write rejection.
    pub fn parse(raw: &str) -> Result<Anchor> {
        let (prefix, key) = raw.split_once(':').ok_or_else(|| {
            anyhow!(
                "unknown anchor {raw:?}: expected a `file:<path>` or `symbol:<symbol>` \
                 entity id (anchors are never guessed)"
            )
        })?;
        if key.is_empty() {
            bail!("unknown anchor {raw:?}: the entity id after `{prefix}:` is empty");
        }
        Anchor::from_parts(prefix, key.to_string())
    }

    /// Build an anchor from a stored/parsed `(kind, key)` pair.
    ///
    /// A `file` anchor's key must be a **repo-relative, non-traversing** path:
    /// it is joined onto the worktree root and `blake3`-hashed for freshness, so
    /// an absolute path or a `..` segment would escape the repo and let a write
    /// (and every later read) hash an arbitrary file on disk. Such keys are
    /// rejected, here — on both the write (`parse`) and read (`load_anchors`)
    /// paths — never guessed ([NFR-RA-05], [NFR-SE-01] "writes/reads stay within
    /// the project").
    ///
    /// # Errors
    /// Returns an error on an unrecognized kind token or an unsafe `file` path —
    /// at write time a loud rejection, at read time a corrupt-store signal.
    pub(crate) fn from_parts(kind: &str, entity_id: String) -> Result<Anchor> {
        let kind = match kind {
            "file" => AnchorKind::File,
            "symbol" => AnchorKind::Symbol,
            other => bail!(
                "unknown anchor kind {other:?}: expected `file` or `symbol` \
                 (anchors are never guessed)"
            ),
        };
        if kind == AnchorKind::File {
            validate_file_anchor(&entity_id)?;
        }
        Ok(Anchor { kind, entity_id })
    }

    /// The `"<kind>:<key>"` wire form — the round-trip of [`Anchor::parse`].
    pub fn as_id(&self) -> String {
        format!("{}:{}", self.kind.as_str(), self.entity_id)
    }
}

/// An anchor as stored, paired with the content hash captured at its write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredAnchor {
    pub(crate) anchor: Anchor,
    pub(crate) content_hash: String,
}

/// Resolves an anchor against the current state of the world — the seam the
/// write path (capture) and the read path (re-check) share.
///
/// Returns the entity's **current** content hash, or `None` when the entity is
/// gone (a write rejects; a read flags the anchor `missing`). Implementations
/// never fabricate a hash for an entity they cannot find ([NFR-RA-05]).
pub trait AnchorResolver {
    /// The current content hash of `anchor`'s entity, or `None` if it is gone.
    ///
    /// # Errors
    /// Returns an error only on an unexpected backing-store failure — a
    /// not-found entity is `Ok(None)`, never an error.
    fn resolve(&self, anchor: &Anchor) -> Result<Option<String>>;
}

/// What kind of page-worthy entity the `wiki status` work-list discovered
/// ([FR-WK-06]). Each maps to the anchor wire form an agent would use to write
/// the missing page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityKind {
    /// A module/package/namespace node — anchored by `symbol:<symbol>`.
    Module,
    /// A whole indexed file — anchored by `file:<repo-relative-path>`.
    File,
    /// A typed `FR-*`/`NFR-*` requirement node ([FR-DG-07]) — `symbol:<symbol>`.
    Requirement,
    /// A typed `ADR-NN` decision node ([FR-DG-07]) — `symbol:<symbol>`.
    Adr,
    /// A typed `S-NNN` story node ([FR-DG-07]) — `symbol:<symbol>`.
    Story,
}

impl EntityKind {
    /// The [`AnchorKind`] this entity is addressed by: a `File` entity is bound
    /// `file:<path>`, every other class `symbol:<symbol>`.
    pub fn as_anchor_kind(self) -> AnchorKind {
        match self {
            EntityKind::File => AnchorKind::File,
            _ => AnchorKind::Symbol,
        }
    }

    /// The anchor kind token (`file`/`symbol`) this entity is addressed by — the
    /// `kind` half of the `(kind, entity_id)` "already anchored" key. Delegates
    /// to [`AnchorKind::as_str`] so the token table lives in exactly one place.
    pub fn anchor_kind(self) -> &'static str {
        self.as_anchor_kind().as_str()
    }

    /// The human-/wire-facing label surfaces render for this entity class.
    pub fn label(self) -> &'static str {
        match self {
            EntityKind::Module => "module",
            EntityKind::File => "file",
            EntityKind::Requirement => "requirement",
            EntityKind::Adr => "adr",
            EntityKind::Story => "story",
        }
    }
}

/// A fixed **consolidated documentation category** ([FR-WK-06] as modified by
/// [CR-034]). Each summarizes one `docs/` glob into a *single* agent-tier page
/// at a canonical slug — replacing the fragmented per-Requirement/Adr/component/
/// integration pages (and dropping Story/CR pages entirely) with one readable
/// document per category. The seven slugs are a **fixed contract** the
/// generation work-list (this module) and the wiki menu ([FR-WK-11], S-133)
/// both key on, so the producer and the menu never disagree on where a
/// consolidated page lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocCategory {
    /// Architecture decision records — `docs/specs/architecture/decisions/*.md`.
    Adrs,
    /// Architecture components — `docs/specs/architecture/components/*.md`.
    Components,
    /// External integrations — `docs/specs/architecture/integrations/*.md`.
    Integrations,
    /// Functional requirements — `docs/specs/requirements/FR-*.md`.
    FunctionalRequirements,
    /// Non-functional requirements — `docs/specs/requirements/NFR-*.md`.
    NonFunctionalRequirements,
    /// User acceptance tests — `docs/specs/requirements/UAT-*.md`.
    UserAcceptanceTests,
    /// Frontend design — the single `docs/specs/frontend-design.md`.
    FrontendDesign,
}

impl DocCategory {
    /// All seven categories in fixed menu order: the three **Architecture**-tier
    /// documents, then the four **Specs**-tier documents ([FR-WK-11]).
    pub const ALL: [DocCategory; 7] = [
        DocCategory::Adrs,
        DocCategory::Components,
        DocCategory::Integrations,
        DocCategory::FunctionalRequirements,
        DocCategory::NonFunctionalRequirements,
        DocCategory::UserAcceptanceTests,
        DocCategory::FrontendDesign,
    ];

    /// The canonical wiki slug this category's consolidated page lives at — the
    /// fixed contract S-133's menu renders and the work-list/queue seed.
    pub fn slug(self) -> &'static str {
        match self {
            DocCategory::Adrs => "architecture/adrs",
            DocCategory::Components => "architecture/components",
            DocCategory::Integrations => "architecture/integrations",
            DocCategory::FunctionalRequirements => "specs/functional-requirements",
            DocCategory::NonFunctionalRequirements => "specs/non-functional-requirements",
            DocCategory::UserAcceptanceTests => "specs/user-acceptance-tests",
            DocCategory::FrontendDesign => "specs/frontend-design",
        }
    }

    /// The page title the skill gives the generated consolidated document.
    pub fn title(self) -> &'static str {
        match self {
            DocCategory::Adrs => "ADRs",
            DocCategory::Components => "Components",
            DocCategory::Integrations => "Integrations",
            DocCategory::FunctionalRequirements => "Functional Requirements",
            DocCategory::NonFunctionalRequirements => "Non-Functional Requirements",
            DocCategory::UserAcceptanceTests => "User Acceptance Tests",
            DocCategory::FrontendDesign => "Frontend Design",
        }
    }

    /// The work-list section label (the [`StructuredSection::section`] token) —
    /// the single discriminator the generation queue keys on to route this row
    /// to [`GenerationCategory::ConsolidatedDoc`]. Distinct from every
    /// [`OverviewSection`] label.
    pub fn label(self) -> &'static str {
        match self {
            DocCategory::Adrs => "adrs",
            DocCategory::Components => "components",
            DocCategory::Integrations => "integrations",
            DocCategory::FunctionalRequirements => "functional-requirements",
            DocCategory::NonFunctionalRequirements => "non-functional-requirements",
            DocCategory::UserAcceptanceTests => "user-acceptance-tests",
            DocCategory::FrontendDesign => "frontend-design",
        }
    }

    /// The `docs/`-relative source glob the doc-grounding directive names — the
    /// files the agent reads and summarizes into the one consolidated page.
    pub fn source_glob(self) -> &'static str {
        match self {
            DocCategory::Adrs => "docs/specs/architecture/decisions/*.md",
            DocCategory::Components => "docs/specs/architecture/components/*.md",
            DocCategory::Integrations => "docs/specs/architecture/integrations/*.md",
            DocCategory::FunctionalRequirements => "docs/specs/requirements/FR-*.md",
            DocCategory::NonFunctionalRequirements => "docs/specs/requirements/NFR-*.md",
            DocCategory::UserAcceptanceTests => "docs/specs/requirements/UAT-*.md",
            DocCategory::FrontendDesign => "docs/specs/frontend-design.md",
        }
    }

    /// Map a [`StructuredSection::section`] label back to its category, if it is
    /// one — the generation queue routes consolidated rows with this.
    fn from_label(label: &str) -> Option<DocCategory> {
        DocCategory::ALL.into_iter().find(|c| c.label() == label)
    }

    /// Whether at least one source file for this category exists under `root` —
    /// a pure local filesystem read: no `wiki.db` write, no LLM, no network
    /// ([NFR-SE-01]). A category whose source files are absent is **never**
    /// seeded into the work-list — reported (by its absence), never fabricated
    /// ([FR-WK-06] as modified by [CR-034]).
    ///
    /// `pub` so the read-only [`Engine`](crate::Engine) presence accessor can
    /// answer the wiki menu's "show the optional Frontend Design entry?" question
    /// ([FR-WK-11] as modified by [CR-034], S-133) without the web surface
    /// reaching into the filesystem itself.
    pub fn present_under(self, root: &Path) -> bool {
        match self {
            DocCategory::FrontendDesign => root.join("docs/specs/frontend-design.md").is_file(),
            DocCategory::Adrs => dir_has(root, "docs/specs/architecture/decisions", "", ".md"),
            DocCategory::Components => {
                dir_has(root, "docs/specs/architecture/components", "", ".md")
            }
            DocCategory::Integrations => {
                dir_has(root, "docs/specs/architecture/integrations", "", ".md")
            }
            DocCategory::FunctionalRequirements => {
                dir_has(root, "docs/specs/requirements", "FR-", ".md")
            }
            DocCategory::NonFunctionalRequirements => {
                dir_has(root, "docs/specs/requirements", "NFR-", ".md")
            }
            DocCategory::UserAcceptanceTests => {
                dir_has(root, "docs/specs/requirements", "UAT-", ".md")
            }
        }
    }
}

/// `true` when `<root>/<rel_dir>` holds at least one file whose name starts with
/// `prefix` and ends with `suffix`. A pure local-FS read backing
/// [`DocCategory::present_under`]; a missing directory reads as "no files".
/// `FR-` never matches an `NFR-`/`UAT-` file (distinct first character), so the
/// three `requirements/` categories partition cleanly.
fn dir_has(root: &Path, rel_dir: &str, prefix: &str, suffix: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(root.join(rel_dir)) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        name.starts_with(prefix) && name.ends_with(suffix) && entry.path().is_file()
    })
}

/// A page-worthy entity the work-list considers — its kind plus the stable key
/// the anchor binds to (a repo-relative path for `File`, a canonical symbol
/// string otherwise). [`EntityCatalog`] yields these; `wiki status` keeps only
/// the ones no page anchors yet ([FR-WK-06]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateEntity {
    /// The entity class.
    pub kind: EntityKind,
    /// The stable key (repo-relative path, or canonical symbol string).
    pub entity_id: String,
    /// A human-facing name for the work-list row.
    pub name: String,
}

/// Enumerates the raw **page-worthy** entity candidates in the current
/// graph/tree — modules, top-level files, and (only when the documentation
/// graph is present) typed Requirement/Adr/Story nodes ([FR-WK-06],
/// [FR-DG-07]). `wiki status` ([`status`]) decides which of these it actually
/// seeds into the regeneration work-list — File and Module are excluded
/// ([CR-056]/[S-221]), same as Requirement/Adr/Story already are ([CR-034]).
/// The discovery seam the write path's [`AnchorResolver`] is to freshness: a
/// trait so `wiki status` is unit-testable with a fake catalog and the
/// production read stays behind the read-only graph pool.
pub trait EntityCatalog {
    /// Every page-worthy entity in the current graph/tree, in any order
    /// (`wiki status` sorts deterministically). Never fabricates an entity that
    /// is not in the graph ([NFR-RA-05]); absent the doc graph, the typed-node
    /// classes are simply not yielded — never invented.
    ///
    /// # Errors
    /// Returns an error only on an unexpected backing-store failure.
    fn page_worthy_entities(&self) -> Result<Vec<CandidateEntity>>;

    /// The consolidated documentation categories ([CR-034]) whose `docs/` source
    /// files are present under the project root, in fixed [`DocCategory::ALL`]
    /// menu order — the work-list seeds one entry per present category, and a
    /// category with no source files is omitted (reported by absence, never
    /// fabricated, [FR-WK-06]). A pure local-FS read; the default yields none so
    /// an off-disk fake catalog opts in explicitly.
    ///
    /// # Errors
    /// Returns an error only on an unexpected backing-store failure.
    fn present_doc_categories(&self) -> Result<Vec<DocCategory>> {
        Ok(Vec::new())
    }

    /// Whether a repo-relative `docs/` file exists under the project root — the
    /// seam the Summary overview item uses to choose its doc-grounding directive
    /// vs. the code-reading fallback ([FR-WK-13] as modified by [CR-034], [CRA-01]),
    /// and the seam the **SRS-mode gate** ([`wiki_srs_mode`], [FR-WK-21]) reads
    /// `docs/specs/architecture.md` through. A pure local-FS read; the default
    /// reports absent so an off-disk fake catalog opts in explicitly.
    fn doc_file_present(&self, _repo_relative: &str) -> bool {
        false
    }
}

/// The production [`AnchorResolver`]: existence + content against the canonical
/// graph and the working tree, with no `ATTACH` and no write ([wiki-engine]
/// graph-store dependency).
///
/// - **File** anchors hash the file's bytes on disk — so an edit flips the
///   anchor stale at the next read with no `sync`, and a rename (the path gone)
///   reads as missing ([FR-WK-03], the [ADR-23] precedent).
/// - **Symbol** anchors resolve the node in the graph (gone → missing) and hash
///   its **defining file** on disk, so editing the file flips the symbol anchor
///   stale without a `sync`. A node with no defining file has no content to
///   anchor and reads as missing — never fabricated fresh.
pub(crate) struct GraphAnchorResolver<'a> {
    store: &'a dyn GraphStore,
    root: &'a Path,
}

impl<'a> GraphAnchorResolver<'a> {
    pub(crate) fn new(store: &'a dyn GraphStore, root: &'a Path) -> Self {
        GraphAnchorResolver { store, root }
    }
}

impl AnchorResolver for GraphAnchorResolver<'_> {
    fn resolve(&self, anchor: &Anchor) -> Result<Option<String>> {
        match anchor.kind {
            AnchorKind::File => Ok(content_hash(&self.root.join(&anchor.entity_id))),
            AnchorKind::Symbol => match self.store.node_by_symbol(&anchor.entity_id)? {
                Some(node) => match node.file_path {
                    Some(file_path) => Ok(content_hash(&self.root.join(file_path))),
                    // A node with no defining file carries no content to anchor:
                    // missing, never a fabricated fresh hash ([NFR-RA-05]).
                    None => Ok(None),
                },
                None => Ok(None),
            },
        }
    }
}

impl EntityCatalog for GraphAnchorResolver<'_> {
    /// The page-worthy entities of the **Modules** tier: modules and indexed
    /// top-level files ([FR-WK-11]). The typed Requirement/Adr/Story doc-graph
    /// nodes ([FR-DG-07]) are **no longer page-worthy** ([FR-WK-06] as modified
    /// by [CR-034]) — documentation is consolidated per-category via
    /// [`present_doc_categories`](EntityCatalog::present_doc_categories) instead
    /// of fragmented one-page-per-node, and Story/CR nodes are dropped from the
    /// wiki entirely. The doc-graph nodes themselves are untouched; only their
    /// wiki page-worthiness changes.
    ///
    /// Still enumerates every file and module as a raw candidate — `wiki status`
    /// ([`status`]) is the layer that excludes File and Module from the
    /// generation work-list ([CR-056]/[S-221]); this catalog stays a plain "what
    /// exists" enumeration, independent of that seeding policy.
    fn page_worthy_entities(&self) -> Result<Vec<CandidateEntity>> {
        let mut out = Vec::new();
        // Top-level files — one candidate per indexed file (`file:<path>`).
        for file in self.store.indexed_files()? {
            out.push(CandidateEntity {
                kind: EntityKind::File,
                entity_id: file.path.clone(),
                name: file.path,
            });
        }
        // Modules — the per-source-file Modules tier, unchanged by [CR-034].
        for node in self.store.all_nodes()? {
            if node.kind == NodeKind::Module {
                out.push(CandidateEntity {
                    kind: EntityKind::Module,
                    entity_id: node.symbol.as_str().to_string(),
                    name: node.name,
                });
            }
        }
        Ok(out)
    }

    /// The consolidated doc categories whose source files exist under the
    /// worktree root, in fixed menu order — a pure local-FS scan, no graph read
    /// and no `wiki.db` touch ([CR-034], [NFR-SE-01]).
    fn present_doc_categories(&self) -> Result<Vec<DocCategory>> {
        Ok(DocCategory::ALL
            .into_iter()
            .filter(|category| category.present_under(self.root))
            .collect())
    }

    /// A repo-relative `docs/` file's existence on disk — a pure local-FS read.
    fn doc_file_present(&self, repo_relative: &str) -> bool {
        self.root.join(repo_relative).is_file()
    }
}

/// blake3 of a file's bytes on disk — the per-anchor freshness hash ([FR-WK-03],
/// the [ADR-23] coverage anchor). `None` if the file cannot be read (deleted /
/// moved), which the caller treats as a missing entity, never fabricating a hash.
pub(crate) fn content_hash(path: &Path) -> Option<String> {
    std::fs::read(path)
        .ok()
        .map(|bytes| blake3::hash(&bytes).to_hex().to_string())
}

/// The write-time HEAD commit of the repo at `root`, or `None` outside a git
/// repo / on an unborn HEAD ([FR-WK-02] write-time HEAD tag). Read-only
/// `git rev-parse HEAD`; the wiki stays functional out of the box without a HEAD
/// ([ADR-24]) — the surface records an empty tag rather than failing the write.
pub fn head_sha(root: &Path) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

// ── Read-models ([ADR-01] Serialize payloads) ───────────────────────────────

/// One anchor's freshness verdict at read time ([FR-WK-03]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Freshness {
    /// The anchored entity's content hash is unchanged since write.
    Fresh,
    /// The entity is present but its content changed since write.
    Stale,
    /// The entity is gone from the working tree / graph.
    Missing,
}

impl Freshness {
    /// The fixed lowercase label surfaces render.
    pub fn as_str(self) -> &'static str {
        match self {
            Freshness::Fresh => "fresh",
            Freshness::Stale => "stale",
            Freshness::Missing => "missing",
        }
    }
}

/// One anchor's provenance on a served page ([FR-WK-04]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AnchorProvenance {
    /// `"file"` or `"symbol"`.
    pub kind: &'static str,
    /// The stable entity key the page anchors to.
    pub entity_id: String,
    /// This anchor's freshness verdict at this read.
    pub freshness: Freshness,
}

/// A served wiki page — the verbatim body plus the mandatory provenance no
/// surface may omit ([FR-WK-04], [BR-29]). The shape makes a provenance-less
/// page **unconstructible**: `generator`, `written_head`, `anchors` (per-anchor
/// freshness), and the fixed `marker` are all required fields.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WikiPage {
    /// The validated path-like slug.
    pub slug: String,
    /// The page title.
    pub title: String,
    /// The body, byte-identical to the input of the write that stored it
    /// ([FR-WK-02] round-trip).
    pub body: String,
    /// The mandatory generator label.
    pub generator: String,
    /// The write-time HEAD commit; empty when the repo had no resolvable HEAD.
    pub written_head: String,
    /// The served provenance marker, tier-correct by generator (`marker_for`,
    /// [BR-29] refined): [`PRESENTED_CONTENT_MARKER`] for a presented page,
    /// [`GENERATED_CONTENT_MARKER`] for a generated agent page.
    pub marker: &'static str,
    /// The persisted graph revision this page was built at ([FR-WK-12],
    /// [FR-SY-09]). The two-tier view compares it against the current revision to
    /// derive the "stale — regeneration pending" verdict, with no write on the
    /// page view ([ADR-32], [ADR-28]); `0` for a page written before any `index`.
    pub built_at_revision: u64,
    /// Per-anchor freshness, in stored order ([FR-WK-03]).
    pub anchors: Vec<AnchorProvenance>,
    /// `true` when at least one anchor is stale — the page is presented as stale
    /// ([FR-WK-03]). A zero-anchor (overview) page is never stale.
    pub stale: bool,
    /// `true` when at least one anchor is missing — the page survives flagged
    /// ([FR-WK-07]) until *every* anchor is gone.
    pub has_missing: bool,
}

/// The outcome of a write ([FR-WK-02]) — a `Serialize` read-model the S-053
/// surfaces render.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WriteSummary {
    /// The slug written.
    pub slug: String,
    /// `true` when this write replaced an existing page (last-write-wins).
    pub replaced: bool,
    /// The write-time HEAD recorded; empty when none was resolvable.
    pub written_head: String,
    /// Number of anchors resolved and stored.
    pub anchor_count: usize,
}

/// Whether an agent page is **"stale — regeneration pending"** ([FR-WK-12]): the
/// graph has advanced past the revision the page was built at. The single
/// source of truth for the revision-pending verdict — the search read-model
/// ([`WikiHit`]), the `wiki status` work-list ([`section_state`]), and the web
/// page view all derive the verdict through this one predicate, so a page can
/// never read "regeneration pending" on one surface and "Fresh" on another
/// ([FR-WK-05] as modified by [CR-039]).
///
/// Vacuously `false` before the first `index` (`current == 0`) and for a page
/// built at the current revision — both honest, neither masquerading as stale
/// ([NFR-CC-04]). Computed purely from the persisted revisions, with no
/// `wiki.db` write ([ADR-28]).
pub fn revision_pending(built_at_revision: u64, current_revision: u64) -> bool {
    current_revision > 0 && built_at_revision < current_revision
}

/// One search hit / enumerated page ([FR-WK-05]) — the staleness flag and the
/// provenance summary every result carries. Reused by the `wiki status`
/// work-list for its stale- and missing-anchor-page sections ([FR-WK-06]) so the
/// two surfaces describe a page the same way.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WikiHit {
    /// The page slug.
    pub slug: String,
    /// The page title.
    pub title: String,
    /// The generator label (provenance summary, [FR-WK-04]).
    pub generator: String,
    /// The write-time HEAD commit; empty when none was resolvable.
    pub written_head: String,
    /// `true` when at least one anchor is stale at this read ([FR-WK-03]).
    pub stale: bool,
    /// `true` when at least one anchor is missing at this read ([FR-WK-07]).
    pub has_missing: bool,
    /// The persisted graph revision this page was built at ([FR-WK-12],
    /// [FR-SY-09]) — the raw provenance value, mirroring
    /// [`WikiPage::built_at_revision`]. Carried so a surface that holds its **own**
    /// current revision (the wiki landing, which must keep its derived
    /// regeneration-pending count coherent with the revision it displays) can
    /// re-derive the verdict through [`revision_pending`] against that exact
    /// revision, rather than against a second, independently-read one ([ADR-28]).
    /// `0` for a page written before any `index`.
    pub built_at_revision: u64,
    /// `true` when the page is **"stale — regeneration pending"**: the graph has
    /// advanced past the revision the page was built at ([FR-WK-12], derived by
    /// [`revision_pending`] against the current graph revision **at search time**).
    /// Carried on the hit so the search "State" column renders the **same** verdict
    /// the page view shows — including the revision-pending dimension search
    /// formerly omitted — without a second engine call ([FR-WK-05] as modified by
    /// [CR-039]). Computed on read, written nowhere ([ADR-28]).
    pub revision_pending: bool,
}

/// A page-worthy entity with no anchored page yet — one regeneration work-list
/// row ([FR-WK-06]). Carries the ready-to-use anchor wire form so the generating
/// agent can write the missing page directly.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UnanchoredEntity {
    /// The entity class label (`module`/`file`/`requirement`/`adr`/`story`).
    pub kind: &'static str,
    /// The stable entity key (repo-relative path, or canonical symbol string).
    pub entity_id: String,
    /// A human-facing name for the row.
    pub name: String,
    /// The `"<kind>:<key>"` anchor an agent would pass to `wiki write`.
    pub anchor: String,
}

/// One of the five fixed **Overview** prose children of the structured wiki's
/// information architecture ([FR-WK-11]) — the agent-tier sections only the
/// embedded skill can synthesize ([ADR-32]). Each is a single page at a
/// **canonical slug** so the two-tier view ([FR-WK-12]) and the work-list agree
/// on where it lives. The slug/title set is kept **byte-identical with the web
/// `GUIDED_TOUR`** (`web/src/wiki.rs`) — the menu and the generation work-list
/// must seed the same Guided-Tour pages, or one queues a page the menu never
/// shows (the [CR-028] divergence this fixes). The deterministic native tier
/// ([FR-WK-10]) — Codebase structure, the Files view, the dependency Mermaid — is
/// **never** represented here: it is always live-rendered and needs no
/// regeneration.
///
/// **The synthesized `overview/architecture` page is retired** ([CR-062],
/// [ADR-57]): the Design-tier "Architecture" entry is now the **presented**
/// `docs/specs/architecture.md` ([FR-WK-20]), assembled deterministically rather
/// than paraphrased by the agent, so it is no longer an agent-authored Overview
/// child. The slug lives on as a Design-tier presented page (`web/src/wiki.rs`'s
/// `OVERVIEW_ARCHITECTURE`); it simply leaves this Summary-tier set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverviewSection {
    /// "Project Overview" — the at-a-glance orientation page.
    ProjectOverview,
    /// "Getting Started" — the first-steps onboarding page.
    GettingStarted,
    /// "Key concepts" — the handful of ideas a newcomer needs.
    KeyConcepts,
    /// "How It Works" — the runtime/data-flow walkthrough.
    HowItWorks,
    /// "Known issues" — the known-issues prose section.
    KnownIssues,
}

impl OverviewSection {
    /// The five Overview prose children, in menu order — the iteration source the
    /// work-list seeds from so the set is fixed and exhaustive ([FR-WK-11]). The
    /// order matches the web `GUIDED_TOUR` (`web/src/wiki.rs`) exactly. The
    /// Architecture narrative was retired from this set by [CR-062] (now the
    /// presented `docs/specs/architecture.md`, [FR-WK-20]).
    pub const ALL: [OverviewSection; 5] = [
        OverviewSection::ProjectOverview,
        OverviewSection::GettingStarted,
        OverviewSection::KeyConcepts,
        OverviewSection::HowItWorks,
        OverviewSection::KnownIssues,
    ];

    /// The canonical slug the embedded skill writes this section to — a fixed
    /// contract so the work-list (absence/staleness detection), the view
    /// (lookup), and the web `GUIDED_TOUR` menu never disagree.
    pub fn slug(self) -> &'static str {
        match self {
            OverviewSection::ProjectOverview => "overview/project-overview",
            OverviewSection::GettingStarted => "overview/getting-started",
            OverviewSection::KeyConcepts => "overview/key-concepts",
            OverviewSection::HowItWorks => "overview/how-it-works",
            OverviewSection::KnownIssues => "overview/known-issues",
        }
    }

    /// The work-list section label surfaces render ([FR-WK-06]).
    pub fn label(self) -> &'static str {
        match self {
            OverviewSection::ProjectOverview => "project overview",
            OverviewSection::GettingStarted => "getting started",
            OverviewSection::KeyConcepts => "key concepts",
            OverviewSection::HowItWorks => "how it works",
            OverviewSection::KnownIssues => "known-issues prose",
        }
    }

    /// The human page title the skill gives the generated section — matching the
    /// web `GUIDED_TOUR` label so a written page resolves to its menu entry.
    pub fn title(self) -> &'static str {
        match self {
            OverviewSection::ProjectOverview => "Project Overview",
            OverviewSection::GettingStarted => "Getting Started",
            OverviewSection::KeyConcepts => "Key concepts",
            OverviewSection::HowItWorks => "How It Works",
            OverviewSection::KnownIssues => "Known issues",
        }
    }
}

/// Why an agent-tier structured section is on the regeneration work-list
/// ([FR-WK-06] as modified by [CR-027], [FR-WK-12]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SectionState {
    /// No page has been written for this section yet — it needs first authoring.
    Absent,
    /// A page exists but was built at an **older** graph revision than the
    /// current one: "stale — regeneration pending" ([FR-WK-12]). The verdict is
    /// derived purely by comparing the page's built-at revision to the current
    /// graph revision — no anchor re-hash, no write on read ([ADR-28]).
    RevisionStale,
}

impl SectionState {
    /// The fixed wire label.
    pub fn as_str(self) -> &'static str {
        match self {
            SectionState::Absent => "absent",
            SectionState::RevisionStale => "revision-stale",
        }
    }
}

/// One agent-tier structured-wiki section the regeneration work-list seeds
/// ([FR-WK-06] as modified by [CR-027], [CR-034], [FR-WK-11], [FR-WK-12]) — the
/// **prose** sections only the embedded skill can synthesize: the five Overview
/// children (Project overview, Getting started, Key concepts, How it works,
/// Known-issues prose — the Architecture narrative retired by [CR-062]) and, in
/// Case 2 ([FR-WK-21]), the consolidated documentation category pages
/// ([`DocCategory`]). Per-file objectives are no
/// longer seeded here ([CR-056]/[S-221]) — they had no navigational entry point
/// in the wiki menu ([FR-WK-11]) or the native Files view ([FR-WK-10]), the same
/// unnavigable class [CR-034] removed for documentation nodes. Carries the
/// slug + anchor an agent passes straight to `wiki write`, plus the optional
/// doc-grounding directive. The deterministic native tier ([FR-WK-10]) is never
/// listed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StructuredSection {
    /// The section class label: one of the five Overview labels (`project
    /// overview` / `getting started` / `key concepts` / `how it works` /
    /// `known-issues prose` — `architecture narrative` retired by [CR-062]), or —
    /// added by [CR-034], Case 2 only ([FR-WK-21]) — a
    /// consolidated [`DocCategory::label`] (`adrs` / `components` /
    /// `integrations` / `functional-requirements` / `non-functional-requirements`
    /// / `user-acceptance-tests` / `frontend-design`).
    pub section: &'static str,
    /// The canonical slug the skill writes the page to.
    pub slug: String,
    /// The page title.
    pub title: String,
    /// The `"<kind>:<key>"` anchor to bind — **always empty** for the current
    /// section classes, the zero-anchor Overview and consolidated category
    /// sections (codebase-/docs-wide synthesis tied to no single entity, kept
    /// fresh by the built-at-revision comparison rather than per-anchor
    /// staleness).
    pub anchor: String,
    /// Whether the section is absent or revision-stale.
    pub state: SectionState,
    /// The doc-grounding directive ([CR-034], [FR-WK-21], [FR-WK-24]): for a
    /// Case-2 consolidated category and for four of the five Overview children
    /// (Project Overview, Getting Started, How It Works, Key Concepts — the
    /// mapping is identical in both modes), names the user-facing `docs/`
    /// source(s) the agent grounds on. `None` for Known Issues, which stays free
    /// synthesis. The binary only *names* the source — it reads no doc content and
    /// makes no LLM/network call ([NFR-SE-01]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grounding: Option<DocGrounding>,
}

/// A **doc-grounding directive** ([FR-WK-13] as modified by [CR-034],
/// [FR-WK-24]): the `docs/`/user-facing source(s) the agent reads and
/// summarizes to ground a wiki page in the project's own documentation instead
/// of synthesizing fresh, drift-prone prose. The binary only *names* the
/// source — it reads no doc content and makes no LLM or network call
/// ([NFR-SE-01]); the connected agent does the synthesis off the request path.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DocGrounding {
    /// The `docs/`-relative source file(s)/glob to read and summarize.
    pub sources: Vec<String>,
    /// `true` when a named source is **absent** for this item so the agent
    /// reads the code graph instead — a precise per-instance fact, not an
    /// item-type marker ([CRA-01]). Consolidated category items are seeded only
    /// when their source files exist, so it is always `false` for those.
    pub fallback_to_code: bool,
    /// The one-line human directive the `wiki generate` prompt block renders.
    pub directive: String,
}

impl DocGrounding {
    /// The directive for a consolidated category page ([CR-034]): summarize every
    /// file matching the category's glob into one document, with an in-page
    /// sub-section (heading + anchor) per source file so deep links survive.
    fn for_category(category: DocCategory) -> DocGrounding {
        let glob = category.source_glob();
        DocGrounding {
            sources: vec![glob.to_string()],
            fallback_to_code: false,
            directive: format!(
                "Doc-grounded: read every file matching `{glob}` and summarize them into this \
                 one page, with an in-page sub-section (heading + anchor) per source document. \
                 State only what the docs say; do not invent content."
            ),
        }
    }

    /// The directive for a doc-grounded overview item ([CR-034] as modified by
    /// [FR-WK-24], strengthened by [CR-064]): read the mapped user-facing
    /// doc(s) when **all** are present, else fall back to reading the code
    /// graph ([CRA-01]) — under the same user-facing framing, never the
    /// symbol-centric one, and always opening with a concrete reader outcome
    /// before structural detail. Applies identically in both SRS and
    /// inference mode ([FR-WK-24] Decision Log: the re-pitch is not
    /// Case-1-only). `present` is whether every named source exists on disk.
    fn for_overview(sources: &[&str], present: bool) -> DocGrounding {
        let list = sources.join("`, `");
        let directive = if present {
            format!(
                "Doc-grounded: read `{list}` and write this page from the reader's \
                 perspective — open with a concrete outcome (what the reader can do and \
                 why) and name the actual commands/workflows those docs document, not \
                 internal symbols/types."
            )
        } else {
            format!(
                "Doc-grounded with code fallback: `{list}` is absent — summarize from the code \
                 graph (`logos context`/`node`/`explore`) instead, keeping the same \
                 reader-facing, concrete-outcome-first framing."
            )
        };
        DocGrounding {
            sources: sources.iter().map(|s| (*s).to_string()).collect(),
            // The code fallback is the operative path for *this* item only when
            // a mapped source is absent — a precise per-instance fact, so the
            // boolean matches the directive text rather than marking the item type
            // ([CRA-01], [NFR-CC-04] honesty-by-construction).
            fallback_to_code: !present,
            directive,
        }
    }
}

/// The project's own README — grounds the **Project Overview** and **Key
/// Concepts** Overview pages alongside [`SUMMARY_DOC`] ([FR-WK-24]): the
/// reader-facing "what is this and why" framing the SRS hub doc alone does not
/// carry.
const README_DOC: &str = "README.md";

/// The doc the **Project Overview**, **How It Works**, and **Key Concepts**
/// Overview pages ground in alongside their other mapped source(s) ([FR-WK-24]);
/// absent → the code-reading fallback fires.
const SUMMARY_DOC: &str = "docs/specs/software-spec.md";

/// The authored User Guide's onboarding page — grounds the **Getting Started**
/// Overview page together with [`HOWTO_INSTALLATION_DOC`] and
/// [`HOWTO_USAGE_DOC`] ([FR-WK-24]), so the wiki's first-steps page and the
/// authored `docs/howto/` corpus never drift apart.
const HOWTO_README_DOC: &str = "docs/howto/README.md";

/// The authored installation guide — grounds **Getting Started** alongside
/// [`HOWTO_README_DOC`] and [`HOWTO_USAGE_DOC`] ([FR-WK-24]).
const HOWTO_INSTALLATION_DOC: &str = "docs/howto/installation.md";

/// The authored usage guide — grounds **Getting Started** (with
/// [`HOWTO_README_DOC`]/[`HOWTO_INSTALLATION_DOC`]) and, separately, **How It
/// Works** (with [`SUMMARY_DOC`]) ([FR-WK-24]): the behavioral, task-oriented
/// doc both pages draw on.
const HOWTO_USAGE_DOC: &str = "docs/howto/usage.md";

/// The `docs/`-relative glob [`present::guide_pages`] presents — every
/// `docs/howto/*.md` file, one page each ([FR-WK-23]).
const HOWTO_GUIDE_GLOB: &str = "docs/howto/*.md";

/// The wiki slug prefix the **User Guide** tier's per-file pages live under
/// ([FR-WK-23], [FR-WK-11]) — `guide/<name>`, `README.md` at `guide/overview`.
pub(crate) const GUIDE_SLUG_PREFIX: &str = "guide";

/// The authored `docs/specs/architecture.md` — the load-bearing swe-skills
/// artifact the **SRS-mode gate** ([`is_srs_mode`], [FR-WK-21]) keys on, and the
/// document the Design-tier Architecture page is **presented** from ([FR-WK-20]).
/// (Before [CR-062] this doubled as the retired `overview/architecture` overview's
/// grounding doc; that agent-authored page is retired in favor of presentation.)
const ARCHITECTURE_DOC: &str = "docs/specs/architecture.md";

/// The wiki slug the **presented Architecture page** lives at ([FR-WK-20],
/// [CR-062]) — the deterministic presentation of `docs/specs/architecture.md`. Retained
/// at the slug the retired agent-authored `overview/architecture` page used, so
/// the Design-tier menu link and reader route are unchanged (the web
/// `OVERVIEW_ARCHITECTURE` const mirrors it, kept in lockstep by a drift guard in
/// `web/src/wiki.rs`). It is neither an [`OverviewSection`] slug (the Architecture
/// narrative was retired from that set by [CR-062]) nor a [`DocCategory`] slug, so
/// `reconciliation_valid_slugs` adds it **explicitly** — gated on the source
/// doc's presence — so the sweep never purges the page `materialize` just wrote.
pub const PRESENTED_ARCHITECTURE_SLUG: &str = "overview/architecture";

/// The title the presented Architecture page carries — matching the web
/// Design-tier label (`OVERVIEW_ARCHITECTURE_LABEL`). `pub` so the web drift
/// guard can assert the two stay byte-identical (`logos-core` cannot depend on
/// `web`, so the pair is duplicated by convention and pinned by that guard).
pub const PRESENTED_ARCHITECTURE_TITLE: &str = "Architecture";

/// The authored `docs/specs/software-spec.md` — the **SRS hub**, presented at
/// [`PRESENTED_SRS_SLUG`] ([FR-WK-26], [CR-064]). Named as a single no-wildcard
/// glob so [`present::srs_page`] presents exactly this hub, never the stray
/// top-level `docs/specs/*.md` analyst intermediates (`analyst-frs.md`,
/// `writer-nfr.md`, …) that share the directory.
const SOFTWARE_SPEC_DOC: &str = "docs/specs/software-spec.md";

/// The wiki slug the **presented SRS hub page** lives at ([FR-WK-26], [FR-WK-11],
/// [CR-064]) — the deterministic presentation of `docs/specs/software-spec.md`,
/// listed first under the **Specs** tier (`web`'s `SPECS_SRS`). It is neither an
/// [`OverviewSection`] slug nor a [`DocCategory`] slug, so
/// `reconciliation_valid_slugs` adds it **explicitly** — gated on the source
/// doc's presence — so the sweep never purges the page `materialize` just wrote,
/// while a stale leftover in a repo without an authored SRS is still purged. The
/// web `SPECS_SRS` const mirrors it, kept in lockstep by a drift guard in
/// `web/src/wiki.rs`.
pub const PRESENTED_SRS_SLUG: &str = "specs/srs";

/// The title the presented SRS hub page carries and the **Specs**-tier menu label
/// (`web`'s `SPECS_SRS_LABEL`). `pub` so the web drift guard can assert the two
/// stay byte-identical (`logos-core` cannot depend on `web`, so the pair is
/// duplicated by convention and pinned by that guard).
pub const PRESENTED_SRS_TITLE: &str = "Software Requirements Specification";

/// The presence of each **user-facing doc** an Overview grounding directive may
/// name ([FR-WK-24]) — gathered once per `status` call (pure local-FS reads,
/// [NFR-SE-01]) so building all four directives costs exactly five
/// [`EntityCatalog::doc_file_present`] calls, not one per section.
#[derive(Debug, Clone, Copy, Default)]
struct OverviewDocPresence {
    readme: bool,
    howto_readme: bool,
    howto_installation: bool,
    howto_usage: bool,
    summary: bool,
}

/// The **SRS-mode gate** predicate ([FR-WK-21], [ADR-57]): `true` (Case 1 — SRS
/// present) when the project carries both load-bearing swe-skills artifacts — the
/// authored `docs/specs/architecture.md` **and** at least one
/// `FR-*`/`NFR-*`/`UAT-*` file under `docs/specs/requirements/` — else `false`
/// (Case 2 — inference mode). Expressed purely over the resolved presence inputs
/// so it is a deterministic function of the on-disk `docs/specs/` layout
/// ([NFR-RA-06]); the inputs are pure local-FS reads, so evaluating the gate does
/// **no** `wiki.db` write, no LLM, and no network call ([NFR-SE-01]).
///
/// In Case 1 the generation work-list / queue restricts the agent to the
/// **Summary/Overview tier** — the Design/Specs categories are produced by the
/// deterministic presented tier, not the agent. In Case 2 the agent infers the
/// full set (Overview + present consolidated categories) from the code graph and
/// discoverable docs, the pre-[CR-062] behavior.
fn is_srs_mode(architecture_present: bool, present_categories: &[DocCategory]) -> bool {
    architecture_present
        && present_categories.iter().any(|c| {
            matches!(
                c,
                DocCategory::FunctionalRequirements
                    | DocCategory::NonFunctionalRequirements
                    | DocCategory::UserAcceptanceTests
            )
        })
}

/// The **SRS-mode gate** as a pure local-FS read on the project `root`
/// ([FR-WK-21], [ADR-57]) — the canonical entry the [`Engine`](crate::Engine)
/// façade and its surfaces ([FR-WK-20] presented tier, [FR-WK-13] queue) call to
/// decide Case 1 vs Case 2. Reuses the same [`DocCategory::present_under`] scan
/// and the [`is_srs_mode`] predicate the [`status`] work-list applies through the
/// catalog seam, so the two agree by construction. No graph read, no `wiki.db`
/// write, no LLM, no network — a deterministic function of the on-disk layout
/// ([NFR-SE-01], [NFR-RA-06]).
pub(crate) fn wiki_srs_mode(root: &Path) -> bool {
    let present_categories: Vec<DocCategory> = DocCategory::ALL
        .into_iter()
        .filter(|category| category.present_under(root))
        .collect();
    is_srs_mode(root.join(ARCHITECTURE_DOC).is_file(), &present_categories)
}

/// The active-mode valid-slug set the reconciliation sweep purges against
/// ([FR-WK-22]): the five Overview/Summary slugs ([`OverviewSection::ALL`]) ∪
/// the present consolidated-category slugs ([`DocCategory::present_under`]) ∪
/// the present User Guide `guide/*` slugs ([`present::guide_pages`],
/// [FR-WK-23]). Independent of the [`wiki_srs_mode`] gate — Case 1 and Case 2
/// both route through the same Overview/Summary tier and the same
/// present-category set (Case 1 materializes the categories and guides
/// deterministically, Case 2 has the agent infer the categories; either way
/// the slug is the one this function names), so the sweep needs no mode branch
/// of its own. A pure local-FS read, no `wiki.db` touch ([NFR-SE-01]).
fn reconciliation_valid_slugs(root: &Path) -> HashSet<String> {
    let mut slugs: HashSet<String> = OverviewSection::ALL
        .into_iter()
        .map(|section| section.slug().to_string())
        .collect();
    slugs.extend(
        DocCategory::ALL
            .into_iter()
            .filter(|category| category.present_under(root))
            .map(|category| category.slug().to_string()),
    );
    // The presented Architecture page ([FR-WK-20], [CR-062]) reuses the retired
    // `overview/architecture` slug, which is neither an OverviewSection slug nor a
    // DocCategory slug — add it when (and only when) its source doc is present, so
    // the page `materialize` writes survives the sweep, while a stale leftover in a
    // repo without an authored architecture.md is still purged.
    if root.join(ARCHITECTURE_DOC).is_file() {
        slugs.insert(PRESENTED_ARCHITECTURE_SLUG.to_string());
    }
    // The presented SRS hub ([FR-WK-26], [CR-064], S-269) reuses the `specs/srs`
    // slug, which is neither an OverviewSection slug nor a DocCategory slug — add it
    // when (and only when) `docs/specs/software-spec.md` is present, so the page
    // `materialize` writes survives the sweep, while a stale leftover in a repo
    // without an authored SRS is still purged.
    if root.join(SOFTWARE_SPEC_DOC).is_file() {
        slugs.insert(PRESENTED_SRS_SLUG.to_string());
    }
    // The User Guide tier's per-file slugs ([FR-WK-23]) — keyed on exactly the
    // `docs/howto/*.md` files present today, so a guide whose source file was
    // deleted is purged like any other orphan, while every guide `materialize`
    // just (re-)wrote survives.
    slugs.extend(present::guide_pages(root).into_iter().map(|page| page.slug));
    slugs
}

/// The **User Guide** tier's page set ([FR-WK-23], [FR-WK-11]) — one
/// `(slug, title)` pair per `docs/howto/*.md` file, in the same order
/// [`materialize`] writes them ([`present::guide_pages`]; `README.md` first at
/// `guide/overview`). The seam [`crate::Engine::wiki_guide_pages`] exposes to
/// the web menu ([FR-UI-06]) so the User Guide tier's dynamic file set never
/// needs its own fixed enum, unlike [`DocCategory`]. **Gated on
/// [`wiki_srs_mode`]** ([FR-WK-21]) — empty in Case 2 even when `docs/howto/`
/// has files, since [FR-WK-23] gates the tier on "`docs/howto/` presence **in
/// SRS mode**": `materialize` never writes a Case-2 guide page (`Engine::
/// wiki_materialize` returns before opening `wiki.db`), so a Case-2 menu
/// listing would be a permanent, un-fulfillable placeholder. Also empty when
/// `docs/howto/` has no `*.md` file, so the tier is simply absent rather than
/// an empty group. A pure local-FS read, no `wiki.db` touch ([NFR-SE-01]).
pub(crate) fn wiki_guide_pages(root: &Path) -> Vec<(String, String)> {
    if !wiki_srs_mode(root) {
        return Vec::new();
    }
    present::guide_pages(root).into_iter().map(|page| (page.slug, page.title)).collect()
}

/// The reconciliation sweep ([FR-WK-22]): bulk-purge any stored page whose slug
/// falls outside [`reconciliation_valid_slugs`], logging each removal to the
/// pruned-log via [`db::delete_pages_not_in`]. Retires orphaned pages left by a
/// prior or superseded generation run — unreachable from the menu ([FR-WK-11])
/// and un-regenerable by the work-list ([FR-WK-06]) — that the lazy
/// all-anchors-gone orphan lifecycle ([FR-WK-07]) never reaches, since that
/// lifecycle fires only on read and only once every anchor is gone, so it never
/// touches a page nothing reads anymore. Idempotent: once every stored slug is
/// valid, a re-run purges nothing. A pure store + local-FS operation — no LLM,
/// no network call ([NFR-SE-01]).
///
/// Invoked from the [FR-WK-20] `wiki materialize` path (S-262); exposed here so
/// it is independently unit-testable against a fixture store ahead of that
/// wiring.
///
/// Returns the purged slugs, in slug order.
///
/// # Errors
/// Returns an error only on an unexpected store failure.
pub(crate) fn reconcile(conn: &mut rusqlite::Connection, root: &Path) -> Result<Vec<String>> {
    db::delete_pages_not_in(conn, &reconciliation_valid_slugs(root))
}

/// The outcome of `wiki materialize` ([FR-WK-20], [CR-062]) — a `Serialize`
/// read-model the CLI/MCP/UI surfaces (wired by S-263) render. In Case 2
/// (non-SRS) it is the empty summary with `srs_mode = false`: presentation is a
/// Case-1 operation, so the binary writes nothing and the connected agent infers
/// the full set from the code graph as before ([FR-WK-21]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct MaterializeSummary {
    /// `true` when the SRS-mode gate ([FR-WK-21]) selected presentation (Case 1).
    pub srs_mode: bool,
    /// The slugs presented into `wiki.db`, in write order (the Architecture page,
    /// then the present categories in [`DocCategory::ALL`] menu order, then the
    /// present User Guide pages in [`present::guide_pages`] order, [FR-WK-23]).
    pub materialized: Vec<String>,
    /// The slugs the reconciliation sweep ([FR-WK-22]) removed, in slug order.
    pub pruned: Vec<String>,
}

/// The deterministic presented-tier build ([FR-WK-20], [ADR-57], [CR-062]) — the
/// store-writing half of [`crate::Engine::wiki_materialize`]. Assembles and
/// upserts one page per **present** Design/Specs category (glob → sorted
/// section-per-source-document consolidated Markdown, [`present::consolidated_page`])
/// plus the single-file Architecture page ([`PRESENTED_ARCHITECTURE_SLUG`] ←
/// [`ARCHITECTURE_DOC`], [`present::architecture_page`]) plus, when present, the
/// **User Guide** tier's per-file pages ([`present::guide_pages`], [FR-WK-23]) —
/// each stamped `generator =` [`PRESENTED_GENERATOR`], with one `file:<path>`
/// anchor per source document and the current `built_at_revision`. A category
/// whose source glob matches no file yields no page (reported by absence,
/// [FR-WK-06]); likewise an empty `docs/howto/` yields no guide pages.
///
/// The presented generator is trusted by the prose guard ([`write`],
/// [`validate_prose`]), so a faithfully-presented body is never itself rejected;
/// `file:` anchors resolve by hashing the source doc on disk, so the write always
/// resolves for a document that exists. Pure local-FS reads + `wiki.db` writes —
/// no LLM, no network ([NFR-SE-01]); re-running with unchanged sources rewrites
/// byte-identical bodies ([FR-WK-20]). The caller runs [`reconcile`] afterward so
/// orphaned prior-run pages are swept in the same operation.
///
/// Returns the materialized slugs, in write order.
///
/// # Errors
/// Propagates a write-path rejection ([`write`]) or an unexpected store failure.
pub(crate) fn materialize(
    conn: &mut rusqlite::Connection,
    resolver: &dyn AnchorResolver,
    root: &Path,
    written_head: &str,
    built_at_revision: u64,
) -> Result<Vec<String>> {
    // Architecture leads the Design tier; the SRS hub leads the Specs tier
    // ([FR-WK-26], S-269); then the present categories in fixed menu order; then
    // the User Guide per-file pages, if any. A category/page with no source file
    // is simply skipped (the SRS hub via `srs_page` returning `None`).
    let mut pages: Vec<present::PresentedPage> = present::architecture_page(root)
        .into_iter()
        .chain(present::srs_page(root))
        .chain(DocCategory::ALL.into_iter().filter_map(|category| present::consolidated_page(root, category)))
        .chain(present::guide_pages(root))
        .collect();

    // FR-WK-25/ADR-58 presentation-layer transform: build the resolution manifest
    // from the assembled page set (so it lists exactly what was presented), then
    // rewrite each body's in-body reference targets against it before the write, so
    // authored relative links resolve onto the wiki routing instead of 404-ing.
    // Deterministic and offline — a pure function of the source set (NFR-SE-01),
    // byte-identical on re-run (NFR-RA-06).
    let manifest = present::Manifest::from_pages(&pages);
    for page in &mut pages {
        page.body = present::rewrite_refs(&page.body, &page.source_dir, &page.slug, &manifest);
    }

    let mut written = Vec::with_capacity(pages.len());
    for page in &pages {
        let draft = PageDraft {
            slug: &page.slug,
            title: &page.title,
            body: &page.body,
            anchors: &page.anchors,
            generator: PRESENTED_GENERATOR,
            written_head,
            built_at_revision,
        };
        write(conn, resolver, &draft)?;
        written.push(page.slug.clone());
    }
    Ok(written)
}

/// The regeneration work-list ([FR-WK-06]) — what the generating agent acts on:
/// pages gone stale, pages with a missing anchor, page-worthy entities that have
/// no anchored page yet, and the structured agent-tier sections that are absent
/// or revision-stale. Logos discovers; the agent writes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WorkList {
    /// Pages with at least one stale anchor — candidates for a refresh
    /// ([FR-WK-03]), slug-ordered.
    pub stale_pages: Vec<WikiHit>,
    /// Pages with at least one missing anchor but still surviving ([FR-WK-07]),
    /// slug-ordered.
    pub missing_anchor_pages: Vec<WikiHit>,
    /// Page-worthy entities lacking any anchored page yet, in deterministic
    /// `(kind, entity_id)` order ([FR-WK-06]). **File and Module entities are
    /// excluded** ([FR-WK-06] as modified by [CR-056]/[S-221]) — their
    /// per-entity pages have no navigational entry point in the wiki menu
    /// ([FR-WK-11]) or the native Files view ([FR-WK-10]), the same unnavigable
    /// class [CR-034] removed for documentation nodes. Currently always empty in
    /// production (File/Module were the only page-worthy kinds the catalog ever
    /// yielded); the mechanism is retained generically for a future page-worthy
    /// entity class.
    pub unanchored_entities: Vec<UnanchoredEntity>,
    /// Agent-tier structured-wiki sections that are absent or revision-stale
    /// ([FR-WK-06] as modified by [CR-027], [CR-034], [FR-WK-12]) — the prose the
    /// embedded skill regenerates off the request path. **Empty before the first
    /// `index`** (no graph revision yet), so `init` is never blocked by wiki work;
    /// the deterministic native tier ([FR-WK-10]) is never listed. Ordered
    /// deterministically: the five Overview children in menu order, then — in Case
    /// 2 only ([FR-WK-21]) — the present consolidated documentation categories in
    /// [`DocCategory::ALL`] order ([NFR-RA-06]). Per-file objectives are **no longer seeded**
    /// ([CR-056]/[S-221]) — the same unnavigable-page-class removal [CR-034]
    /// applied to documentation nodes.
    pub structured_sections: Vec<StructuredSection>,
}

/// The `wiki status` read-model ([FR-WK-06]): the store summary plus the
/// regeneration work-list. A deterministic function of `wiki.db` + the current
/// graph/tree ([NFR-RA-06]).
///
/// `PartialEq` (not `Eq`): `freshness_fraction` is an `f64`, the same posture
/// `Thresholds`/`Constraints` take for their float fields.
///
/// Freshness is **dual-axis** ([CR-044], [FR-WK-12]): a page is fresh only when its
/// anchors are all fresh **and** it was built at the current graph revision. So
/// `fresh_count` is the count of pages clean on *both* axes — it excludes a
/// zero-anchor prose page that is merely revision-stale, which earlier counted as
/// fresh and let the landing read "all fresh" while the banner read STALE.
///
/// `fresh_count` is mutually exclusive with the union of the staleness counts, but
/// `stale_count`, `missing_anchor_count`, and `revision_stale_count` **overlap by
/// design**: one page can be anchor-stale, missing-anchor, *and* revision-stale at
/// once (it appears in every applicable work-list section — each issue needs
/// addressing). So those three may sum past `page_count`; only `fresh_count` feeds
/// `freshness_fraction`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct WikiStatus {
    /// Total stored pages.
    pub page_count: usize,
    /// Pages clean on **both** axes — every anchor fresh (none stale, none missing)
    /// **and** built at the current graph revision ([CR-044]). Mutually exclusive
    /// with the union of `stale_count`/`missing_anchor_count`/`revision_stale_count`.
    pub fresh_count: usize,
    /// Pages with at least one stale anchor (may also be in `missing_anchor_count`
    /// and/or `revision_stale_count`).
    pub stale_count: usize,
    /// Pages with at least one missing anchor (may also be in `stale_count` and/or
    /// `revision_stale_count`).
    pub missing_anchor_count: usize,
    /// Pages built at a graph revision the current revision has advanced past —
    /// "stale — regeneration pending" ([FR-WK-12], [FR-SY-09], [ADR-32]). Counts
    /// every page regardless of anchors, so zero-anchor prose pages are honestly
    /// reported here rather than masquerading as fresh ([CR-044]).
    pub revision_stale_count: usize,
    /// The graph revision the counts were computed against ([FR-SY-09]); `0` before
    /// the first `index`. Surfaced so every consumer derives its "regeneration
    /// pending" verdict from the same revision these counts used — no second read,
    /// no one-tick skew ([CR-044]).
    pub current_revision: u64,
    /// Fraction of pages that are fully fresh (dual-axis), in `[0,1]`; `1.0` for an
    /// empty store (vacuously — `page_count` is `0`).
    pub freshness_fraction: f64,
    /// The auto-deletion log, newest first ([FR-WK-07]).
    pub pruned: Vec<PrunedPage>,
    /// The regeneration work-list.
    pub work_list: WorkList,
}

// ── Write path ([FR-WK-02]) ──────────────────────────────────────────────────

/// The data for one page write ([FR-WK-02]) — the caller-supplied content plus
/// the engine-resolved `written_head` tag, grouped so the write contract is one
/// cohesive value rather than a long positional argument list.
#[derive(Debug, Clone, Copy)]
pub struct PageDraft<'a> {
    /// The validated path-like slug (the upsert key).
    pub slug: &'a str,
    /// The page title.
    pub title: &'a str,
    /// The markdown body, stored byte-verbatim (1 MiB cap).
    pub body: &'a str,
    /// The `"<kind>:<key>"` anchor wire forms — zero or more.
    pub anchors: &'a [String],
    /// The mandatory non-empty generator label.
    pub generator: &'a str,
    /// The write-time HEAD commit; empty when the repo has no resolvable HEAD.
    pub written_head: &'a str,
    /// The persisted graph revision ([FR-SY-09]) captured at write — the
    /// **built-at revision** the two-tier view later compares against the current
    /// revision to derive the agent page's "stale — regeneration pending" verdict,
    /// with no write on the page view ([FR-WK-12], [ADR-32]). `0` when no graph
    /// revision exists yet (a page written before the first `index`).
    pub built_at_revision: u64,
}

/// Upsert a page by slug ([FR-WK-02]).
///
/// Validates the slug, the mandatory non-empty generator, and the 1 MiB body
/// cap, then resolves **every** anchor to its current content hash via
/// `resolver` — an unknown anchor (bad grammar, unknown kind, or an entity the
/// resolver cannot find) rejects the whole write. Finally runs the
/// [`validate_prose`] content-validity guard ([FR-WK-19]), rejecting a body
/// that is agent-noise rather than page prose. Every check runs **before** any
/// store write, so a rejected write leaves `wiki.db` byte-identical. On success
/// the page, its anchors (with captured hashes), and the `written_head` tag are
/// persisted last-write-wins.
///
/// # Errors
/// Returns an error (mapped by the surface to a non-zero exit, store unchanged)
/// on an invalid slug, an empty generator, an over-cap body, an unknown
/// anchor, or a body that fails the [FR-WK-19] content-validity guard.
pub(crate) fn write(
    conn: &mut rusqlite::Connection,
    resolver: &dyn AnchorResolver,
    draft: &PageDraft<'_>,
) -> Result<WriteSummary> {
    validate_slug(draft.slug)?;
    if draft.generator.trim().is_empty() {
        bail!("the generator label is mandatory and must be non-empty ([FR-WK-02])");
    }
    if draft.body.len() > MAX_BODY_BYTES {
        bail!(
            "wiki body is {} bytes, over the {} byte (1 MiB) cap ([FR-WK-02])",
            draft.body.len(),
            MAX_BODY_BYTES
        );
    }

    // Resolve every anchor to its current content hash BEFORE any write — an
    // unknown anchor rejects the whole write with the store byte-identical
    // ([FR-WK-02], [NFR-RA-05]).
    let mut resolved: Vec<(Anchor, String)> = Vec::with_capacity(draft.anchors.len());
    for raw in draft.anchors {
        let anchor = Anchor::parse(raw)?;
        match resolver.resolve(&anchor)? {
            Some(hash) => resolved.push((anchor, hash)),
            None => bail!(
                "unknown anchor {:?}: no such entity in the current graph/tree \
                 (anchors are never guessed, [NFR-RA-05])",
                anchor.as_id()
            ),
        }
    }

    // Content-validity guard ([FR-WK-19]): a structural signature match run
    // last, after every other check has passed, so a slug/generator/cap/anchor
    // rejection keeps its own specific message. Placed here rather than first
    // so a page that is *otherwise* invalid is never mistaken for agent-noise.
    //
    // The presented tier ([FR-WK-20], [CR-062]) is **exempt**: its bodies are
    // verbatim copies of authored `docs/specs/**` documents, never agent prose, so
    // the agent-noise guard must trust the `logos:doc-present` generator and never
    // reject a faithfully-presented body — an authored doc may legitimately embed a
    // `<tool_call`/`Error:` code sample or be a short stub the guard would flag.
    if draft.generator != PRESENTED_GENERATOR {
        validate_prose(draft.body)?;
    }

    let replaced = db::load_page(conn, draft.slug)?.is_some();
    db::upsert_page(conn, draft, &resolved)?;

    tracing::info!(
        slug = draft.slug,
        anchors = resolved.len(),
        replaced,
        "wiki page written"
    );

    Ok(WriteSummary {
        slug: draft.slug.to_string(),
        replaced,
        written_head: draft.written_head.to_string(),
        anchor_count: resolved.len(),
    })
}

// ── Read path + orphan lifecycle ([FR-WK-03], [FR-WK-07]) ────────────────────

/// Read one page by slug, computing per-anchor freshness against the current
/// tree and running the orphan lifecycle ([FR-WK-03], [FR-WK-07]).
///
/// Returns `Ok(None)` when the slug does not exist **or** when every anchor of
/// the page is gone — in the latter case the page is auto-deleted and recorded
/// in the pruned log before returning, so the next `wiki status` surfaces the
/// pruning ([FR-WK-07]). A page with at least one surviving anchor is returned
/// with the missing ones flagged. A zero-anchor page is never pruned and never
/// stale ([FR-WK-03]).
///
/// # Errors
/// Returns an error only on an unexpected store failure.
pub(crate) fn read(
    conn: &mut rusqlite::Connection,
    resolver: &dyn AnchorResolver,
    slug: &str,
) -> Result<Option<WikiPage>> {
    let Some(page) = db::load_page(conn, slug)? else {
        return Ok(None);
    };

    let verdicts = freshness_of(resolver, &page.anchors)?;

    // Orphan lifecycle: a page with anchors, ALL of them gone, is auto-deleted
    // and logged ([FR-WK-07]). A zero-anchor page has nothing to lose and is
    // never pruned.
    if !page.anchors.is_empty() && verdicts.iter().all(|f| *f == Freshness::Missing) {
        db::prune_page(conn, &page.slug, &page.title)?;
        tracing::info!(
            slug = page.slug,
            anchors = page.anchors.len(),
            "wiki page pruned — every anchor is gone"
        );
        return Ok(None);
    }

    Ok(Some(into_page(page, &verdicts)))
}

/// Explicitly delete a page by slug ([FR-WK-07], CLI-only surface).
///
/// # Errors
/// Returns an error when no page with that slug exists — a non-zero exit so a
/// typo'd delete is loud, never a silent no-op ([NFR-UX-02]).
pub(crate) fn delete(conn: &rusqlite::Connection, slug: &str) -> Result<()> {
    if db::delete_page(conn, slug)? {
        tracing::info!(slug, "wiki page deleted");
        Ok(())
    } else {
        bail!("no wiki page with slug {slug:?} to delete")
    }
}

/// The recorded auto-deletions, newest first ([FR-WK-07]).
///
/// # Errors
/// Returns an error only on an unexpected store failure.
pub(crate) fn pruned_log(conn: &rusqlite::Connection) -> Result<Vec<PrunedPage>> {
    db::pruned_log(conn)
}

// ── Search + enumeration ([FR-WK-05]) ────────────────────────────────────────

/// Build a complete, safe FTS5 MATCH expression from raw user `query`, or `None`
/// for an empty/whitespace query — a well-defined no-op rather than an opaque
/// FTS5 `syntax error`. The query is wrapped as one quoted phrase (embedded `"`
/// doubled per FTS5 string quoting), so arbitrary punctuation in the query can
/// never be misread as an FTS operator and the whole expression is bound as a
/// parameter, never interpolated ([NFR-SE-02]; the [graph_store] `suggest`
/// precedent). A quoted phrase also gives the [FR-WK-05] "phrase appearing only
/// in one page" its natural adjacency semantics.
pub(crate) fn fts_phrase_query(query: &str) -> Option<String> {
    if query.trim().is_empty() {
        return None;
    }
    Some(format!("\"{}\"", query.replace('"', "\"\"")))
}

/// FTS5 bm25 search over page titles + bodies, or enumerate every page in `list`
/// mode ([FR-WK-05]). Each hit carries its staleness flag and provenance summary
/// ([FR-WK-04]), computed against the current tree via `resolver`. Search hits
/// are bm25-then-slug ordered; list mode is slug-ordered. A **pure read** — it
/// never prunes (the orphan lifecycle stays on the [`read`] path), so a `search`
/// has no surprising store-mutating side effect.
///
/// `current_revision` is the persisted graph revision ([FR-SY-09]) each hit's
/// built-at revision is compared against to derive the **revision-pending**
/// verdict ([FR-WK-12], [`revision_pending`]) — so a result reports the **same**
/// staleness verdict the page view shows, never plain "Fresh" while its page
/// view reads "stale — regeneration pending" ([FR-WK-05] as modified by
/// [CR-039]). `0` (no `index` yet) yields no pending verdict.
///
/// # Errors
/// Returns an error only on an unexpected store failure.
pub(crate) fn search(
    conn: &rusqlite::Connection,
    resolver: &dyn AnchorResolver,
    query: &str,
    list: bool,
    current_revision: u64,
) -> Result<Vec<WikiHit>> {
    let slugs = if list {
        db::all_slugs(conn)?
    } else {
        match fts_phrase_query(query) {
            // LIMIT -1 is SQLite's "no limit" — search never silently truncates.
            Some(fts) => db::search_slugs(conn, &fts, -1)?,
            None => return Ok(Vec::new()),
        }
    };
    let mut hits = Vec::with_capacity(slugs.len());
    for slug in slugs {
        if let Some(page) = db::load_page(conn, &slug)? {
            let verdicts = freshness_of(resolver, &page.anchors)?;
            hits.push(hit_of(&page, &verdicts, current_revision));
        }
    }
    Ok(hits)
}

// ── Status + regeneration work-list ([FR-WK-06]) ─────────────────────────────

/// The store summary + regeneration work-list ([FR-WK-06]). Counts pages by
/// freshness, partitions stale / missing-anchor pages (slug-ordered), surfaces
/// the pruned-orphan log, lists page-worthy entities (`catalog`) that no page
/// anchors yet — excluding File and Module, which have no navigational entry
/// point ([FR-WK-11]) or the native Files view ([FR-WK-10]) instead
/// ([CR-056]/[S-221]) — and seeds the agent-tier structured sections that are
/// absent or revision-stale against `revision` (the current persisted graph
/// revision, [FR-SY-09]) — a deterministic function of `wiki.db` + the current
/// graph/tree ([NFR-RA-06]). A **pure read** — never prunes, never regenerates.
///
/// `revision_stale_threshold` ([FR-WK-17]) dampens **re-queue cadence only**:
/// a structured section already built at least once re-enters
/// `work_list.structured_sections` (and so the `wiki generate` queue,
/// [FR-WK-13]) only once `revision` has advanced at least this many revisions
/// past the section's built-at revision — not on every single revision
/// advance. `revision_stale_count` below is computed independently via
/// [`revision_pending`] and stays truthful regardless of this threshold
/// ([NFR-CC-04]): dampening never masks the honest staleness signal, it only
/// bounds how often the agent is asked to act on it.
///
/// # Errors
/// Returns an error only on an unexpected store failure.
pub(crate) fn status(
    conn: &rusqlite::Connection,
    resolver: &dyn AnchorResolver,
    catalog: &dyn EntityCatalog,
    revision: u64,
    revision_stale_threshold: u64,
) -> Result<WikiStatus> {
    // [FR-WK-22]: `files/%` pages are retired outright — the forward-only
    // `wiki.db` migration ([`db`]'s migration 4) deletes every such row on
    // store open. Skip any that linger here too, so the work-list (and the
    // `wiki generate` queue it feeds, via `generation_queue`) can never
    // surface a per-file page for refresh regardless of how it reached the
    // store — a lingering `files/%` page never re-enters the queue.
    let slugs: Vec<String> = db::all_slugs(conn)?
        .into_iter()
        .filter(|slug| !is_retired_files_slug(slug))
        .collect();
    let page_count = slugs.len();
    let mut stale_pages = Vec::new();
    let mut missing_anchor_pages = Vec::new();
    let mut fresh_count = 0usize;
    let mut revision_stale_count = 0usize;
    // Built-at revision per slug, gathered in this single page pass so the
    // structured-section seeding ([CR-027]) costs no second load. Both the fixed
    // Overview singletons and the per-file objectives are addressed by their
    // **canonical slug**, so one slug→revision map serves the whole work-list.
    let mut built_at_by_slug: HashMap<String, u64> = HashMap::new();
    for slug in &slugs {
        let Some(page) = db::load_page(conn, slug)? else {
            continue;
        };
        built_at_by_slug.insert(page.slug.clone(), page.built_at_revision);
        let verdicts = freshness_of(resolver, &page.anchors)?;
        let hit = hit_of(&page, &verdicts, revision);
        // Dual-axis freshness ([CR-044]): the per-anchor verdict (stale/missing)
        // and the revision verdict are independent — a zero-anchor prose page can
        // never be anchor-stale yet still be revision-stale once the graph
        // advances past its built-at. Count it on every axis it fails so the
        // landing can never read "all fresh" while a page awaits regeneration.
        let rev_pending = revision_pending(page.built_at_revision, revision);
        if rev_pending {
            revision_stale_count += 1;
        }
        // `stale_pages` and `missing_anchor_pages` overlap by design — a page
        // with both a stale and a missing anchor belongs in both. Push into the
        // missing list first so the one case that needs a clone is explicit; a
        // page clean on both axes lands only in the fresh count.
        if !hit.stale && !hit.has_missing && !rev_pending {
            fresh_count += 1;
        } else {
            if hit.has_missing {
                missing_anchor_pages.push(hit.clone());
            }
            if hit.stale {
                stale_pages.push(hit);
            }
        }
    }
    let freshness_fraction = if page_count == 0 {
        1.0
    } else {
        fresh_count as f64 / page_count as f64
    };

    // Page-worthy entities, fetched once and shared by the unanchored-entity
    // seeding below and consulted for the present doc categories further down
    // (one graph read, not two).
    let candidates = catalog.page_worthy_entities()?;
    let anchored = db::anchored_entities(conn)?;
    let mut unanchored_entities: Vec<UnanchoredEntity> = candidates
        .iter()
        // File and Module are never seeded as unanchored work-list entries
        // ([FR-WK-06] as modified by [CR-056]/[S-221]): a generic per-entity page
        // for either has no navigational entry point in the wiki menu
        // ([FR-WK-11]) or the native Files view ([FR-WK-10]) — the same
        // unnavigable class [CR-034] removed for documentation nodes. The native
        // Files view still enumerates every file/module live, independent of this
        // work-list.
        .filter(|c| !matches!(c.kind, EntityKind::File | EntityKind::Module))
        .filter_map(|c| {
            let anchor_kind = c.kind.anchor_kind();
            // Skip entities a page already anchors (matched on the stored
            // `(kind, entity_id)` key); keep the rest as a ready-to-use anchor.
            if anchored.contains(&(anchor_kind.to_string(), c.entity_id.clone())) {
                return None;
            }
            Some(UnanchoredEntity {
                kind: c.kind.label(),
                anchor: format!("{anchor_kind}:{}", c.entity_id),
                entity_id: c.entity_id.clone(),
                name: c.name.clone(),
            })
        })
        .collect();
    unanchored_entities
        .sort_by(|a, b| (a.kind, a.entity_id.as_str()).cmp(&(b.kind, b.entity_id.as_str())));

    // The doc-grounding inputs ([CR-034] as modified by [FR-WK-24]): which
    // consolidated categories have source files on disk, and which user-facing
    // docs each Overview page's grounding directive may name (drives the
    // read-the-doc directive vs. the code-reading fallback, in both SRS and
    // inference mode). All pure local-FS reads — no `wiki.db` write, no LLM, no
    // network ([NFR-SE-01]).
    let present_categories = catalog.present_doc_categories()?;
    let doc_presence = OverviewDocPresence {
        readme: catalog.doc_file_present(README_DOC),
        howto_readme: catalog.doc_file_present(HOWTO_README_DOC),
        howto_installation: catalog.doc_file_present(HOWTO_INSTALLATION_DOC),
        howto_usage: catalog.doc_file_present(HOWTO_USAGE_DOC),
        summary: catalog.doc_file_present(SUMMARY_DOC),
    };
    // The SRS-mode gate ([FR-WK-21], [ADR-57]) — architecture.md present AND a
    // requirement present — decides whether the work-list is bimodal: in Case 1
    // the Design/Specs categories are produced by the deterministic presented tier
    // ([FR-WK-20]) and leave the agent queue entirely; in Case 2 they stay
    // (Overview + present categories). A pure local-FS read via the catalog seam.
    let srs_mode = is_srs_mode(
        catalog.doc_file_present(ARCHITECTURE_DOC),
        &present_categories,
    );
    let structured_sections = structured_sections(
        &present_categories,
        doc_presence,
        srs_mode,
        revision,
        revision_stale_threshold,
        &built_at_by_slug,
    );

    Ok(WikiStatus {
        page_count,
        fresh_count,
        stale_count: stale_pages.len(),
        missing_anchor_count: missing_anchor_pages.len(),
        revision_stale_count,
        current_revision: revision,
        freshness_fraction,
        pruned: db::pruned_log(conn)?,
        work_list: WorkList {
            stale_pages,
            missing_anchor_pages,
            unanchored_entities,
            structured_sections,
        },
    })
}

/// Seed the agent-tier structured-wiki sections that need regeneration ([FR-WK-06]
/// as modified by [CR-027], [CR-034], [CR-062], [FR-WK-12]): the five fixed
/// Overview prose children and — **in Case 2 only** ([FR-WK-21]) — the
/// **consolidated documentation category** pages, each that is **absent** or
/// **revision-stale** (built at least `revision_stale_threshold` revisions before
/// `revision`, [FR-WK-17]). In Case 1 (SRS mode) the categories are produced by
/// the deterministic presented tier ([FR-WK-20]) and omitted here. The
/// deterministic native tier ([FR-WK-10]) is never produced here. Per-file
/// objectives are **no longer seeded** ([CR-056]/[S-221]) — a generic per-file
/// page has no navigational entry point in the wiki menu ([FR-WK-11]) or the
/// native Files view ([FR-WK-10]), the same unnavigable class [CR-034] removed for
/// documentation nodes.
///
/// **First availability is the first `index`** ([FR-WK-12]): with no graph
/// revision yet (`revision == 0`) this is empty — an honest seed state — so
/// `init` is never blocked by wiki work and generation is prompted only once a
/// graph exists.
///
/// Output is deterministic ([NFR-RA-06]): the five Overview children in fixed
/// menu order, then (Case 2 only) the present consolidated categories in
/// [`DocCategory::ALL`] order.
///
/// Every section is addressed by its **canonical slug** — the Overview
/// singletons and consolidated categories by their fixed slugs. Presence/
/// freshness is a slug lookup against `built_at_by_slug`, so the work-list hands
/// the agent the exact slug to write and detects the page it writes there.
///
/// Bimodal by the **SRS-mode gate** ([FR-WK-21], [ADR-57]): in **Case 1**
/// (`srs_mode`) the Design/Specs consolidated categories are produced by the
/// deterministic presented tier ([FR-WK-20]), so they are **omitted** here and
/// the work-list yields only the Overview/Summary children. In **Case 2** the
/// categories stay (Overview + present consolidated categories), the
/// pre-[CR-062] behavior.
///
/// [CR-034] grounding as re-pitched by [FR-WK-24]: four of the five Overview
/// children (Project Overview, Getting Started, How It Works, Key Concepts)
/// carry a doc-grounding directive naming their mapped user-facing doc(s)
/// (`doc_presence` decides read-the-doc vs. the code-reading fallback) —
/// **identically in both modes**, never conditioned on `srs_mode`; each Case-2
/// consolidated category names its `docs/` glob. `present_categories` already
/// excludes categories whose source files are absent (reported by absence,
/// never fabricated).
fn structured_sections(
    present_categories: &[DocCategory],
    doc_presence: OverviewDocPresence,
    srs_mode: bool,
    revision: u64,
    revision_stale_threshold: u64,
    built_at_by_slug: &HashMap<String, u64>,
) -> Vec<StructuredSection> {
    // Before the first `index` there is no graph to narrate — no structured work
    // ([FR-WK-12] first-build-after-index).
    if revision == 0 {
        return Vec::new();
    }

    let mut sections = Vec::new();

    // The five fixed Overview prose children — zero-anchor singletons at canonical
    // slugs whose only freshness signal is the built-at revision. Four of the
    // five carry a doc-grounding directive ([CR-034], [FR-WK-24]).
    for section in OverviewSection::ALL {
        if let Some(state) = section_state(
            built_at_by_slug.get(section.slug()),
            revision,
            revision_stale_threshold,
        ) {
            sections.push(StructuredSection {
                section: section.label(),
                slug: section.slug().to_string(),
                title: section.title().to_string(),
                anchor: String::new(),
                state,
                grounding: overview_grounding(section, doc_presence),
            });
        }
    }

    // Case 2 only: the consolidated documentation category pages ([CR-034]) —
    // zero-anchor singletons at fixed slugs, only the categories whose source
    // files exist, each naming its `docs/` glob. In SRS mode (Case 1) these are
    // materialized deterministically by the presented tier ([FR-WK-20]) and never
    // queued to the agent ([FR-WK-21], [FR-WK-13]).
    if !srs_mode {
        for &category in present_categories {
            let slug = category.slug().to_string();
            if let Some(state) = section_state(
                built_at_by_slug.get(&slug),
                revision,
                revision_stale_threshold,
            ) {
                sections.push(StructuredSection {
                    section: category.label(),
                    slug,
                    title: category.title().to_string(),
                    anchor: String::new(),
                    state,
                    grounding: Some(DocGrounding::for_category(category)),
                });
            }
        }
    }

    sections
}

/// The doc-grounding directive for an Overview child ([CR-034] as re-pitched by
/// [FR-WK-24]): four of the five children ground in the project's own
/// **user-facing docs** — Project Overview and Key Concepts in
/// [`README_DOC`] + [`SUMMARY_DOC`]; Getting Started in [`HOWTO_README_DOC`] +
/// [`HOWTO_INSTALLATION_DOC`] + [`HOWTO_USAGE_DOC`]; How It Works in
/// [`HOWTO_USAGE_DOC`] + [`SUMMARY_DOC`] — falling back to the code graph
/// ([CRA-01]) only when a mapped doc is absent, under the same user-facing
/// framing. Known Issues stays free synthesis (`None`). This mapping is
/// **identical in both SRS and inference mode** — the [FR-WK-24] re-pitch is not
/// gated on `srs_mode` (its Decision Log rejected a Case-1-only scope). (The
/// Architecture overview's grounding was retired with the page itself —
/// [CR-062].)
fn overview_grounding(section: OverviewSection, docs: OverviewDocPresence) -> Option<DocGrounding> {
    match section {
        OverviewSection::ProjectOverview => Some(DocGrounding::for_overview(
            &[README_DOC, SUMMARY_DOC],
            docs.readme && docs.summary,
        )),
        OverviewSection::GettingStarted => Some(DocGrounding::for_overview(
            &[HOWTO_README_DOC, HOWTO_INSTALLATION_DOC, HOWTO_USAGE_DOC],
            docs.howto_readme && docs.howto_installation && docs.howto_usage,
        )),
        OverviewSection::HowItWorks => Some(DocGrounding::for_overview(
            &[HOWTO_USAGE_DOC, SUMMARY_DOC],
            docs.howto_usage && docs.summary,
        )),
        OverviewSection::KeyConcepts => Some(DocGrounding::for_overview(
            &[README_DOC, SUMMARY_DOC],
            docs.readme && docs.summary,
        )),
        OverviewSection::KnownIssues => None,
    }
}

/// Classify a structured section from its stored page's built-at revision
/// against the current graph `revision` ([FR-WK-12]), **dampened** by
/// `revision_stale_threshold` ([FR-WK-17]) — the single rule both the Overview
/// singletons and the consolidated category sections share: no page → `Absent`
/// (never dampened — an unwritten page always needs first authoring); a page
/// built `>= revision_stale_threshold` revisions ago → `RevisionStale`; anything
/// closer (including a page built at the current revision) → `None` (off the
/// work-list).
///
/// This is deliberately **not** [`revision_pending`] — that predicate remains
/// the honest per-page/search "is this page behind at all" verdict
/// ([FR-WK-12], [CR-039]) and continues to drive `revision_stale_count`
/// ([FR-WK-06]) untouched. This function only decides **work-list/queue
/// membership** — re-queue cadence — so a page can be `revision_pending` (and
/// counted as such in `wiki status`) while sitting below the dampening
/// threshold and therefore absent from `structured_sections`/`wiki generate`
/// ([NFR-CC-04]: reporting is never masked by dampening).
///
/// At `revision_stale_threshold == 1` this reduces exactly to
/// `revision_pending(built_at, revision)` ([FR-WK-17] AC: the minimum
/// threshold is "re-queue whenever revision-pending").
fn section_state(
    built_at: Option<&u64>,
    revision: u64,
    revision_stale_threshold: u64,
) -> Option<SectionState> {
    match built_at {
        None => Some(SectionState::Absent),
        Some(&built_at) if revision.saturating_sub(built_at) >= revision_stale_threshold => {
            Some(SectionState::RevisionStale)
        }
        Some(_) => None,
    }
}

/// The canonical slug an existing per-file objectives page lives at:
/// `files/<path>`. Per-file objectives are no longer **seeded**
/// ([CR-056]/[S-221]), but a page already written to this slug (before the
/// change) is left in place and still served/searched; a would-be `file`-kind
/// unanchored entity ([`entity_target_slug`]) resolves to this same slug so the
/// two would de-duplicate against each other if either were ever seeded again
/// ([FR-WK-13]).
fn file_objectives_slug(path: &str) -> String {
    slug_under("files", path)
}

/// Whether `slug` is under the retired per-file objectives prefix
/// ([FR-WK-22]) — every page [`file_objectives_slug`] ever addressed. The
/// forward-only migration deletes these rows on store open; [`status`]'s
/// work-list filter is the independent, defense-in-depth check that a
/// lingering `files/%` page can never re-enter the stale/missing refresh
/// queue no matter how it reached the store.
fn is_retired_files_slug(slug: &str) -> bool {
    slug.starts_with("files/")
}

/// Build a valid wiki slug ([FR-WK-02]) of the form `<prefix>/<sanitized raw>`:
/// `raw` is split on `/`, empty segments dropped, and within each segment any
/// character outside the slug alphabet is lowercased or mapped to `-`, so a
/// repo-relative path or a canonical symbol string (which carry `.`, uppercase,
/// spaces, `#`, etc.) becomes a deterministic, always-valid slug. Each non-empty
/// input segment maps to a non-empty slug segment (every character produces
/// exactly one output character), so the result never has an empty segment.
fn slug_under(prefix: &str, raw: &str) -> String {
    let mut slug = String::from(prefix);
    for segment in raw.split('/').filter(|s| !s.is_empty()) {
        slug.push('/');
        for ch in segment.chars() {
            if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_' {
                slug.push(ch);
            } else if ch.is_ascii_uppercase() {
                slug.push(ch.to_ascii_lowercase());
            } else {
                slug.push('-');
            }
        }
    }
    slug
}

/// Per-anchor freshness verdicts, in stored order ([FR-WK-03]).
fn freshness_of(resolver: &dyn AnchorResolver, anchors: &[StoredAnchor]) -> Result<Vec<Freshness>> {
    anchors
        .iter()
        .map(|stored| {
            Ok(match resolver.resolve(&stored.anchor)? {
                None => Freshness::Missing,
                Some(current) if current == stored.content_hash => Freshness::Fresh,
                Some(_) => Freshness::Stale,
            })
        })
        .collect()
}

/// The `(stale, has_missing)` page-level flags for a set of per-anchor verdicts:
/// a page is stale if any anchor is stale, missing-flagged if any is missing
/// ([FR-WK-03], [FR-WK-07]). The single definition both [`hit_of`] and
/// [`into_page`] share, so the staleness rule lives in one place.
fn stale_flags(verdicts: &[Freshness]) -> (bool, bool) {
    (
        verdicts.contains(&Freshness::Stale),
        verdicts.contains(&Freshness::Missing),
    )
}

/// Assemble a [`WikiHit`] (search result / work-list page row) from a stored
/// page and its read-time freshness verdicts ([FR-WK-05], [FR-WK-06]).
///
/// `current_revision` is the persisted graph revision the page's built-at
/// revision is compared against to derive the **revision-pending** verdict
/// ([FR-WK-12], via [`revision_pending`]) — so the search row reports the same
/// staleness verdict the page view shows ([FR-WK-05] as modified by [CR-039]).
fn hit_of(page: &db::StoredPage, verdicts: &[Freshness], current_revision: u64) -> WikiHit {
    let (stale, has_missing) = stale_flags(verdicts);
    WikiHit {
        slug: page.slug.clone(),
        title: page.title.clone(),
        generator: page.generator.clone(),
        written_head: page.written_head.clone(),
        stale,
        has_missing,
        built_at_revision: page.built_at_revision,
        revision_pending: revision_pending(page.built_at_revision, current_revision),
    }
}

/// The served provenance marker for a page with this `generator` — tier-correct
/// per [BR-29] (refined to three tiers by [CR-062]): the presented-tier marker
/// for a [`PRESENTED_GENERATOR`] page ([FR-WK-20]), else the generated-content
/// marker every agent page carries ([FR-WK-04]). Keeps the served [`WikiPage`]
/// honest — a presented page never reads as model-generated, and a generated page
/// never reads as presented.
fn marker_for(generator: &str) -> &'static str {
    if generator == PRESENTED_GENERATOR {
        PRESENTED_CONTENT_MARKER
    } else {
        GENERATED_CONTENT_MARKER
    }
}

/// Assemble the [`WikiPage`] read-model from a stored page and its verdicts.
fn into_page(page: db::StoredPage, verdicts: &[Freshness]) -> WikiPage {
    let anchors: Vec<AnchorProvenance> = page
        .anchors
        .iter()
        .zip(verdicts)
        .map(|(stored, freshness)| AnchorProvenance {
            kind: stored.anchor.kind.as_str(),
            entity_id: stored.anchor.entity_id.clone(),
            freshness: *freshness,
        })
        .collect();
    let (stale, has_missing) = stale_flags(verdicts);
    WikiPage {
        slug: page.slug,
        title: page.title,
        body: page.body,
        marker: marker_for(&page.generator),
        generator: page.generator,
        written_head: page.written_head,
        built_at_revision: page.built_at_revision,
        stale,
        has_missing,
        anchors,
    }
}

// ── Generation queue ([FR-WK-13]) ─────────────────────────────────────────────

/// Which `wiki status` work-list ([FR-WK-06]) category a [`GenerationItem`] was
/// formatted from ([FR-WK-13]) — the fixed Overview prose children and the
/// consolidated documentation category pages are the two agent-tier structured
/// section groups, so the queue's fixed order is self-describing. The
/// deterministic native tier ([FR-WK-10]) has no variant — it is never queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GenerationCategory {
    /// One of the five fixed Overview prose children ([FR-WK-11]).
    Overview,
    /// A consolidated documentation category page ([CR-034]) — one document per
    /// [`DocCategory`], grounded in its `docs/` source glob.
    ConsolidatedDoc,
    /// A page-worthy entity ([FR-WK-06]) that no page anchors yet — excluding
    /// File and Module, which are never seeded ([CR-056]/[S-221]).
    UnanchoredEntity,
    /// An existing page whose anchored content went stale ([FR-WK-03]).
    StalePage,
    /// An existing, surviving page with at least one missing anchor ([FR-WK-07]).
    MissingAnchorPage,
}

impl GenerationCategory {
    /// The fixed kebab-case label surfaces render.
    pub fn as_str(self) -> &'static str {
        match self {
            GenerationCategory::Overview => "overview",
            GenerationCategory::ConsolidatedDoc => "consolidated-doc",
            GenerationCategory::UnanchoredEntity => "unanchored-entity",
            GenerationCategory::StalePage => "stale-page",
            GenerationCategory::MissingAnchorPage => "missing-anchor-page",
        }
    }
}

/// One ready-to-run entry in the `wiki generate` queue ([FR-WK-13]): a single
/// agent-tier page to (re)generate, carrying its target slug, the anchor to bind,
/// the reason it is queued, and the exact runnable `wiki write` skeleton.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GenerationItem {
    /// The work-list category this entry was formatted from.
    pub category: GenerationCategory,
    /// The target wiki slug the agent writes the page to.
    pub slug: String,
    /// The suggested page title.
    pub title: String,
    /// The `"<kind>:<key>"` anchor to bind, or **empty** when the page anchors
    /// nothing (the Overview singletons) or its anchors are re-supplied by the
    /// agent on an existing-page refresh.
    pub anchor: String,
    /// Why this page is queued: `absent` / `revision-stale` (structured
    /// sections), `no-page` (unanchored entity), `stale` / `missing` (refreshes).
    pub reason: &'static str,
    /// The exact runnable `logos wiki write …` command skeleton — the agent fills
    /// the generator id and pipes the prose body on stdin.
    pub command: String,
    /// The doc-grounding directive ([CR-034], [FR-WK-24]) for four of the five
    /// Overview children and the Case-2 consolidated category items — names the
    /// user-facing `docs/` source(s) to ground on, identically in both modes;
    /// `None` for Known Issues, the sole free-synthesis item. Carried
    /// from the work-list's [`StructuredSection::grounding`]; the binary makes no
    /// LLM/network call ([NFR-SE-01]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grounding: Option<DocGrounding>,
}

/// The ordered `wiki generate` generation queue ([FR-WK-13]) — the `wiki status`
/// work-list reformatted into a stable, ready-to-run plan for the connected
/// agent. A pure, deterministic function of the work-list ([NFR-RA-06]); building
/// it performs no `wiki.db` write, no LLM call, and no network call ([NFR-SE-01]).
/// An empty `items` is the honest "nothing to generate" result ([NFR-CC-04]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WikiGenerationQueue {
    /// The generation items, in the fixed [FR-WK-13]/[CR-034]/[FR-WK-21] order:
    /// the five Overview sections, then (Case 2 only) the consolidated
    /// documentation category pages,
    /// then unanchored page-worthy entities (currently always empty — File and
    /// Module are excluded, [CR-056]/[S-221]), then stale then missing-anchor
    /// existing-page refreshes. The count is the array length — never stored
    /// separately, so it can never drift from `items`.
    pub items: Vec<GenerationItem>,
}

impl WikiGenerationQueue {
    /// Render the default human-readable prompt block ([FR-WK-13]) — the prose an
    /// agent reads to drive the generation loop. Deterministic ([NFR-RA-06]); an
    /// empty queue renders the explicit "nothing to generate" line ([NFR-CC-04]).
    pub fn render_prompt_block(&self) -> String {
        if self.items.is_empty() {
            return "Nothing to generate — the wiki work-list is empty.\n".to_string();
        }
        let n = self.items.len();
        let mut out = format!(
            "Wiki generation queue — {n} page{} to generate.\n",
            if n == 1 { "" } else { "s" }
        );
        out.push_str(
            "Logos drives this loop and stays offline — you generate the prose. Run each \
             command below, replacing <generator> with your model id and piping the page \
             body on stdin.\n",
        );
        for (i, item) in self.items.iter().enumerate() {
            out.push_str(&format!(
                "\n{}. [{} · {}] {}\n   slug: {}\n",
                i + 1,
                item.category.as_str(),
                item.reason,
                item.title,
                item.slug
            ));
            if !item.anchor.is_empty() {
                out.push_str(&format!("   anchor: {}\n", item.anchor));
            }
            if let Some(grounding) = &item.grounding {
                out.push_str(&format!("   grounding: {}\n", grounding.directive));
            }
            out.push_str(&format!("   {}\n", item.command));
        }
        out
    }
}

/// Format the `wiki status` work-list ([FR-WK-06]) into the ordered generation
/// queue ([FR-WK-13]). Pure formatting — a deterministic function of `status`
/// ([NFR-RA-06]) that performs no I/O, so `wiki generate`'s offline, write-free
/// posture follows directly from `wiki status`'s ([NFR-SE-01]).
///
/// Order is fixed: the five Overview prose children, then (Case 2 only,
/// [FR-WK-21]) the consolidated documentation category pages ([CR-034]), then
/// unanchored page-worthy entities (currently always empty — File and Module are
/// excluded, [CR-056]/[S-221]), then stale then missing-anchor existing-page
/// refreshes — exactly the agent-tier work-list, never any native tier content
/// ([FR-WK-10]). Each **target slug is emitted at most once**, the earliest
/// category winning: a page that is both revision-stale and anchor-stale
/// appears once.
pub fn generation_queue(status: &WikiStatus) -> WikiGenerationQueue {
    let work = &status.work_list;
    let mut items: Vec<GenerationItem> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // 1 + 2: the agent-tier structured sections — the five Overview children then
    // (Case 2 only, [FR-WK-21]) the consolidated documentation category pages,
    // already in the work-list's fixed order ([FR-WK-12], [CR-034]). The grounding
    // directive (if any) rides along from the work-list row.
    for section in &work.structured_sections {
        let category = if DocCategory::from_label(section.section).is_some() {
            GenerationCategory::ConsolidatedDoc
        } else {
            GenerationCategory::Overview
        };
        push_item(
            &mut items,
            &mut seen,
            category,
            section.slug.clone(),
            section.title.clone(),
            section.anchor.clone(),
            section.state.as_str(),
            section.grounding.clone(),
        );
    }

    // 3: unanchored page-worthy entities, in their `(kind, entity_id)` order.
    for entity in &work.unanchored_entities {
        push_item(
            &mut items,
            &mut seen,
            GenerationCategory::UnanchoredEntity,
            entity_target_slug(entity.kind, &entity.entity_id),
            entity.name.clone(),
            entity.anchor.clone(),
            "no-page",
            None,
        );
    }

    // 4: existing-page refreshes — stale then missing-anchor, each slug-ordered.
    // A refresh re-writes the page in place; its anchors are re-supplied by the
    // agent (a work-list page row carries no anchor list), so the anchor is empty.
    // The two work-list sections overlap by design (a page with both a stale and
    // a missing anchor is in both); the stale row is queued first, so it carries a
    // combined `stale+missing` reason — the missing signal is surfaced, not lost
    // to the slug dedup that drops the duplicate missing-anchor row.
    let also_missing: HashSet<&str> = work
        .missing_anchor_pages
        .iter()
        .map(|page| page.slug.as_str())
        .collect();
    for page in &work.stale_pages {
        let reason = if also_missing.contains(page.slug.as_str()) {
            "stale+missing"
        } else {
            "stale"
        };
        push_item(
            &mut items,
            &mut seen,
            GenerationCategory::StalePage,
            page.slug.clone(),
            page.title.clone(),
            String::new(),
            reason,
            None,
        );
    }
    for page in &work.missing_anchor_pages {
        push_item(
            &mut items,
            &mut seen,
            GenerationCategory::MissingAnchorPage,
            page.slug.clone(),
            page.title.clone(),
            String::new(),
            "missing",
            None,
        );
    }

    WikiGenerationQueue { items }
}

/// Push one generation item, skipping a target slug already queued — the earliest
/// category wins, so the queue lists each slug exactly once ([FR-WK-13]).
#[allow(clippy::too_many_arguments)]
fn push_item(
    items: &mut Vec<GenerationItem>,
    seen: &mut HashSet<String>,
    category: GenerationCategory,
    slug: String,
    title: String,
    anchor: String,
    reason: &'static str,
    grounding: Option<DocGrounding>,
) {
    if !seen.insert(slug.clone()) {
        return;
    }
    let command = write_skeleton(&slug, &title, &anchor);
    items.push(GenerationItem {
        category,
        slug,
        title,
        anchor,
        reason,
        command,
        grounding,
    });
}

/// The suggested target slug for an unanchored page-worthy entity ([FR-WK-13]) —
/// a deterministic, always-valid wiki slug ([FR-WK-02]) under a per-class prefix:
/// `files/<path>` for a file, `modules/<symbol>` for a module, `reference/<symbol>`
/// otherwise. File and Module entities are never seeded into the work-list this
/// formats ([CR-056]/[S-221]), so only the fallback arm is reachable today; the
/// per-class mapping is kept for a future page-worthy entity class and so a
/// `file`-kind entity, if ever seeded again, would resolve to the same slug an
/// existing per-file objectives page occupies. The agent may choose a different
/// slug; this is the ready-to-run default.
fn entity_target_slug(kind: &str, entity_id: &str) -> String {
    match kind {
        "file" => file_objectives_slug(entity_id),
        "module" => slug_under("modules", entity_id),
        _ => slug_under("reference", entity_id),
    }
}

/// Build the exact runnable `logos wiki write …` command skeleton for one queue
/// item ([FR-WK-13]). The slug is a validated slug (safe bare); the title and
/// anchor are shell-single-quoted so spaces, em-dashes, or the `:`-bearing symbol
/// anchors pass as one literal argument. The body comes from stdin (`--body-file
/// -`) and the generator is a `<generator>` placeholder — the two parts only the
/// agent can supply.
fn write_skeleton(slug: &str, title: &str, anchor: &str) -> String {
    // The slug is interpolated bare (it is a validated slug — safe characters
    // only), unlike the single-quoted free-text title/anchor. Assert the
    // invariant so a future caller passing an unvalidated slug fails loudly in
    // tests/debug rather than emitting a paste-into-shell command with an
    // unescaped slug ([FR-WK-02], [NFR-SE-01]).
    debug_assert!(
        validate_slug(slug).is_ok(),
        "write_skeleton requires a validated slug, got {slug:?}"
    );
    let mut command = format!(
        "logos wiki write {slug} --title {} --generator '<generator>'",
        shell_single_quote(title)
    );
    if !anchor.is_empty() {
        command.push_str(&format!(" --anchor {}", shell_single_quote(anchor)));
    }
    command.push_str(" --body-file -");
    command
}

/// POSIX single-quote `s` for safe inclusion as one shell argument, escaping an
/// embedded single quote as `'\''`. Keeps the `wiki write` skeleton runnable even
/// when a title or symbol anchor carries spaces or shell metacharacters.
fn shell_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

// ── Content-validity guard ([FR-WK-19]) ──────────────────────────────────────

/// The minimum body length, in trimmed bytes, below which a page cannot be
/// real prose ([FR-WK-19]) — a fence-only or single-line stub trips this even
/// when it happens to carry a heading.
const MIN_BODY_BYTES: usize = 40;

/// First-person planning/refusal preambles a tool-less agent emits when its
/// prompt tells it to "read" or "explore" something it has no tool to reach
/// ([CR-059] §5.1) — matched only against the body's opening text so mid-page
/// prose that happens to use "I" is never affected.
const PLANNING_PREAMBLE_PREFIXES: &[&str] = &[
    "i need to",
    "i'll need to",
    "i will need to",
    "let me",
    "i can't",
    "i cannot",
    "i'm unable to",
    "i am unable to",
    "i don't have",
    "i do not have",
    "i'm going to",
    "i am going to",
];

/// Reject a body that is agent-noise rather than page prose ([FR-WK-19]): it
/// contains a tool-call token, a command-error transcript, opens with a
/// first-person planning/refusal preamble, carries no Markdown heading, or
/// falls below the minimum length. Every check is a **structural** signature
/// match — none of them fire on the mere presence of a fenced code block, so a
/// page that legitimately shows a ` ```bash ` example is unaffected.
///
/// The two agent-noise *content* signatures (a `<tool_call` token and an
/// `Error:`/`cmd:` transcript) are matched against a copy of the body with
/// fenced code blocks stripped ([`strip_fenced_code_blocks`]), so a page that
/// legitimately *quotes* one of those patterns inside a ` ``` ` fence — e.g.
/// documentation of this very guard — is accepted rather than mistaken for the
/// noise it describes ([CR-059] §5.1). The structural signatures (heading,
/// opening preamble, minimum length) are whole-body properties a fence neither
/// adds nor removes, so they run against the raw body.
///
/// # Errors
/// Returns an error naming which signature matched; the caller ([`write`])
/// surfaces this as a non-zero CLI exit or an honest per-page failure in the
/// in-process run, with the store left byte-identical either way ([NFR-CC-04]).
fn validate_prose(body: &str) -> Result<()> {
    let scanned = strip_fenced_code_blocks(body);
    if scanned.contains("<tool_call") {
        bail!(
            "wiki body looks like a tool-call transcript, not page prose \
             (contains `<tool_call` outside a code fence) ([FR-WK-19])"
        );
    }
    if contains_error_transcript(&scanned) {
        bail!(
            "wiki body looks like a command-error transcript, not page prose \
             (an `Error:` line followed by a `cmd:` line, outside a code fence) ([FR-WK-19])"
        );
    }
    let opening = body.trim_start().to_ascii_lowercase();
    if let Some(prefix) = PLANNING_PREAMBLE_PREFIXES
        .iter()
        .find(|prefix| opening.starts_with(**prefix))
    {
        bail!(
            "wiki body opens with a first-person planning/refusal preamble \
             ({prefix:?}), not page prose ([FR-WK-19])"
        );
    }
    if !has_markdown_heading(body) {
        bail!("wiki body has no Markdown heading — it does not read as a page ([FR-WK-19])");
    }
    let trimmed_len = body.trim().len();
    if trimmed_len < MIN_BODY_BYTES {
        bail!(
            "wiki body is {trimmed_len} bytes, under the {MIN_BODY_BYTES} byte minimum \
             for real page prose ([FR-WK-19])"
        );
    }
    Ok(())
}

/// Return a copy of `body` with every fenced code block — its content **and**
/// its fence markers — replaced by empty lines, preserving the total line count
/// so line-window scans ([`contains_error_transcript`]) keep identical
/// semantics over the surviving prose. Feeds the agent-noise *content*
/// signatures in [`validate_prose`]: a page that legitimately quotes a
/// `<tool_call` token or an `Error:`/`cmd:` pair inside a fence must not be
/// mistaken for the noise itself ([FR-WK-19], [CR-059] §5.1).
///
/// Recognises CommonMark fenced blocks: an opener is a line with up to three
/// leading spaces followed by a run of ≥3 backticks or ≥3 tildes; for backtick
/// fences the info string must not contain a backtick (so an inline
/// ` ```code``` ` fragment never opens a block). The block closes at the first
/// line that is a bare run of ≥ the opener's length of the same fence
/// character. An unterminated fence runs to the end of the body.
fn strip_fenced_code_blocks(body: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    for line in body.lines() {
        let dedented = line.trim_start_matches(' ');
        let leading = line.len() - dedented.len();
        match fence {
            Some((fchar, flen)) => {
                if leading <= 3 {
                    let run = dedented.chars().take_while(|&c| c == fchar).count();
                    if run >= flen && dedented[run..].trim().is_empty() {
                        fence = None;
                    }
                }
                out.push("");
            }
            None => {
                let first = dedented.chars().next();
                if leading <= 3 && (first == Some('`') || first == Some('~')) {
                    let fchar = first.unwrap();
                    let run = dedented.chars().take_while(|&c| c == fchar).count();
                    let info = &dedented[run..];
                    if run >= 3 && (fchar != '`' || !info.contains('`')) {
                        fence = Some((fchar, run));
                        out.push("");
                        continue;
                    }
                }
                out.push(line);
            }
        }
    }
    out.join("\n")
}

/// `true` when a `cmd:` line appears within a
/// [`ERROR_TRANSCRIPT_LOOKAHEAD_LINES`]-line window starting at (and
/// including) an `Error:` line — the shape of an invented command-error
/// transcript ([CR-059] §5.1), whether the two fragments share one line
/// (`Error: … cmd: …`) or appear on adjacent lines. Line-bounded (rather than
/// a raw substring window) so the match is safe over multi-byte UTF-8 and
/// does not fire on an "Error:" mention that is unrelated to any nearby `cmd:`.
const ERROR_TRANSCRIPT_LOOKAHEAD_LINES: usize = 5;

fn contains_error_transcript(body: &str) -> bool {
    let lines: Vec<&str> = body.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if line.contains("Error:") {
            let end = (i + ERROR_TRANSCRIPT_LOOKAHEAD_LINES).min(lines.len());
            if lines[i..end].iter().any(|l| l.contains("cmd:")) {
                return true;
            }
        }
    }
    false
}

/// `true` when `body` contains at least one ATX-style Markdown heading line
/// (`#` through `######`, followed by a space and non-empty text) — the
/// minimal structural signal that distinguishes a page from a noise dump.
fn has_markdown_heading(body: &str) -> bool {
    body.lines().any(|line| {
        let trimmed = line.trim_start();
        let hashes = trimmed.chars().take_while(|&c| c == '#').count();
        (1..=6).contains(&hashes)
            && trimmed[hashes..].starts_with(' ')
            && !trimmed[hashes..].trim().is_empty()
    })
}

// ── Slug validation ──────────────────────────────────────────────────────────

/// Validate a path-like slug ([FR-WK-02]).
///
/// A slug is one or more `/`-separated segments; each segment is non-empty and
/// made of lowercase ASCII letters, digits, `-`, or `_`. No leading/trailing
/// slash, no empty segment (`//`), no `.`/`..` traversal, no absolute path, no
/// whitespace — so a slug is a safe, stable, deterministic key and never escapes
/// the store namespace.
///
/// # Errors
/// Returns an error describing the first violation — a loud write rejection.
fn validate_slug(slug: &str) -> Result<()> {
    if slug.is_empty() {
        bail!("wiki slug is empty ([FR-WK-02])");
    }
    if slug.starts_with('/') || slug.ends_with('/') {
        bail!("wiki slug {slug:?} must not start or end with `/` ([FR-WK-02])");
    }
    for segment in slug.split('/') {
        if segment.is_empty() {
            bail!("wiki slug {slug:?} has an empty path segment ([FR-WK-02])");
        }
        if segment == "." || segment == ".." {
            bail!("wiki slug {slug:?} must not contain `.`/`..` path traversal ([FR-WK-02])");
        }
        if !segment
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        {
            bail!(
                "wiki slug segment {segment:?} is invalid: use lowercase letters, digits, \
                 `-`, or `_` ([FR-WK-02])"
            );
        }
    }
    Ok(())
}

/// Validate a `file:` anchor key — a repo-relative, non-traversing path.
///
/// The key is joined onto the worktree root and hashed for freshness, so it must
/// not be empty, absolute (`/…`), backslash/NUL-bearing, or contain a `..`
/// segment — any of which would let the anchor reach a file outside the repo
/// ([NFR-SE-01], [NFR-RA-05]). Symbol keys are graph identities and are not
/// path-joined, so they need no such check.
///
/// # Errors
/// Returns an error describing the first violation.
fn validate_file_anchor(path: &str) -> Result<()> {
    if path.is_empty() {
        bail!("file anchor path is empty ([FR-WK-02])");
    }
    if path.starts_with('/') {
        bail!("file anchor {path:?} must be repo-relative, not absolute ([NFR-SE-01])");
    }
    if path.contains('\\') || path.contains('\0') {
        bail!("file anchor {path:?} must not contain backslashes or NUL bytes ([NFR-SE-01])");
    }
    if path.split('/').any(|segment| segment == "..") {
        bail!("file anchor {path:?} must not contain a `..` path-traversal segment ([NFR-SE-01])");
    }
    Ok(())
}
