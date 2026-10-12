//! The wiki's read routes ([FR-UI-06], [FR-WK-05], [FR-WK-27]): the Wiki tab's
//! status, search, page, menu and doc-asset reads under `/api/v1/wiki`.
//!
//! They were the web adapter's own (`api_v1`) until the wiki became an extension
//! (S-644), and moved here unchanged: each resolves its member through the web
//! adapter's [`MemberEngine`] extractor, crosses into the core through its
//! [`bridge`] under the same view label, and answers with its JSON helpers
//! ([`json`]) — so a wiki route scopes, reports and fails exactly as a core route
//! does. Every one is a `GET` and writes no store ([ADR-28]).
//!
//! [FR-UI-06]: ../../../docs/specs/requirements/FR-UI-06.md
//! [FR-WK-05]: ../../../docs/specs/requirements/FR-WK-05.md
//! [FR-WK-27]: ../../../docs/specs/requirements/FR-WK-27.md
//! [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md

use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};

use axum::extract::{Path, Query};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use logos_core::observability::Surface;
use serde::Serialize;
use web::{bridge, json, MemberEngine};

use crate::{AnchorProvenance, DocCategory, WikiEngine as _, WikiHit, WikiStatus};

// ── Wiki (index / search / page, [FR-UI-06]/[FR-WK-05]) ───────────────────────

/// `GET /api/v1/wiki` — the dual-axis `wiki status` read-model ([FR-UI-06],
/// [FR-WK-12]): per-anchor freshness counts, the revision-stale count, and the
/// revision they were computed at. A documented pure read that never prunes
/// ([ADR-28]).
pub(super) async fn wiki_index(MemberEngine(engine): MemberEngine) -> Response {
    let model = bridge(engine, "api_v1_wiki", Surface::Web, |e| -> anyhow::Result<WikiStatus> {
        e.wiki_status()
    })
    .await;
    json::respond(model)
}

/// `GET /api/v1/wiki/search?q=<term>` — the FTS search over the wiki ([FR-WK-05]).
/// Each hit carries its staleness flag exactly as `wiki search` reports it;
/// read-only — no `wiki.db` write ([ADR-28]). An empty `q` is an honest empty list
/// (`200`), never an error.
pub(super) async fn wiki_search(
    MemberEngine(engine): MemberEngine,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let term = q.get("q").cloned().unwrap_or_default().trim().to_string();
    let model = bridge(engine, "api_v1_wiki_search", Surface::Web, move |e| -> anyhow::Result<Vec<WikiHit>> {
        if term.is_empty() {
            Ok(Vec::new())
        } else {
            e.wiki_search(&term, false)
        }
    })
    .await;
    json::respond(model)
}

/// The agent wiki-page **presentation bundle** ([FR-UI-06], S-189) the migrated SPA
/// reader mounts. It carries the page's provenance + per-anchor freshness verbatim
/// from the [`WikiPage`](crate::WikiPage) read-model, **plus the server-rendered, already-safe HTML
/// body** (`rendered_html`) and the derived freshness verdict (`regen_pending`).
///
/// The Markdown→HTML render is done **server-side** by the same [`super::markdown`]
/// (comrak) the legacy view uses — so the XSS-neutralization boundary stays on the
/// server (raw HTML / dangerous URLs dropped; ` ```mermaid ` fences rewritten to
/// `.mermaid` blocks the client renders) and the SPA mounts a string it can trust
/// (`dangerouslySetInnerHTML`) without shipping a client Markdown engine. The
/// single page `h1` is rendered from `title`; a body that opens by repeating the
/// title is suppressed before render, so the title shows exactly once
/// ([FR-UI-06] single-title).
#[derive(Debug, Serialize)]
pub(super) struct WikiPageView {
    slug: String,
    title: String,
    /// The server-rendered, comrak-sanitized HTML body — empty for a `placeholder`.
    rendered_html: String,
    /// A known scaffold slug with no agent prose yet renders the honest "not yet
    /// generated" placeholder (`200`), never a fabricated body ([NFR-CC-04]).
    placeholder: bool,
    /// Provenance — `null` on a placeholder (no page written yet).
    generator: Option<String>,
    written_head: Option<String>,
    marker: Option<&'static str>,
    built_at_revision: Option<u64>,
    /// Per-anchor freshness; empty for an unanchored (overview) or placeholder page.
    anchors: Vec<AnchorProvenance>,
    stale: bool,
    has_missing: bool,
    /// Derived "stale — regeneration pending": the graph advanced past the page's
    /// built-at revision ([FR-WK-12]). Computed on read, written nowhere ([ADR-28]).
    regen_pending: bool,
    /// The graph revision the verdict was derived against (the banner's "graph now").
    current_revision: u64,
}

/// `GET /api/v1/wiki/page/*slug` — the agent wiki-page presentation bundle
/// ([FR-UI-06], S-189): provenance, per-anchor freshness, and the **server-rendered,
/// already-safe HTML body** the SPA reader mounts (comrak does the XSS
/// neutralization server-side; the SPA renders the `.mermaid` blocks client-side as
/// today). A present page is `200`; a known **scaffold** slug with no prose yet is a
/// `200` placeholder (the honest "not yet generated" state, [NFR-CC-04]); any other
/// unknown slug is an honest `404` — never a fabricated body. Read-only ([ADR-28]).
pub(super) async fn wiki_page(
    MemberEngine(engine): MemberEngine,
    Path(slug): Path<String>,
) -> Response {
    let model = bridge(engine, "api_v1_wiki_page", Surface::Web, move |e| -> anyhow::Result<WikiPageOutcome> {
        // Only the revision is needed, so it is read directly: `status()` would
        // also re-walk every unbound call for its per-language residue (S-589).
        // `0` when it cannot be read, as `status()`'s degraded read-model says.
        let current_revision = e
            .runtime()
            .and_then(|rt| rt.submit_read(|store| store.graph_revision()).ok())
            .unwrap_or(0);
        match e.wiki_read(&slug)? {
            Some(page) => {
                let regen_pending =
                    crate::revision_pending(page.built_at_revision, current_revision);
                // Render the title-suppressed body server-side (the comrak safety
                // boundary, [FR-UI-06] single-title); the SPA mounts the result.
                let rendered_html = super::markdown::render(
                    &super::markdown::suppress_leading_title_heading(&page.body, &page.title),
                );
                Ok(WikiPageOutcome::Page(Box::new(WikiPageView {
                    slug: page.slug,
                    title: page.title,
                    rendered_html,
                    placeholder: false,
                    generator: Some(page.generator),
                    written_head: Some(page.written_head),
                    marker: Some(page.marker),
                    built_at_revision: Some(page.built_at_revision),
                    anchors: page.anchors,
                    stale: page.stale,
                    has_missing: page.has_missing,
                    regen_pending,
                    current_revision,
                })))
            }
            // A known scaffold slug with no prose yet is the honest placeholder
            // (200), mirroring the server-rendered reader; any other slug is 404.
            // A User Guide `guide/*` slug ([FR-WK-23]) is not in the fixed
            // `scaffold_label` set (its file set is dynamic, per project), so it is
            // checked separately against the current `docs/howto/*.md` files before
            // falling back to 404 — a page `wiki materialize` has not yet written
            // still lands on an honest placeholder, never a 404 ([NFR-CC-04]).
            None => {
                let guide_label = e
                    .wiki_guide_pages()
                    .into_iter()
                    .find(|(guide_slug, _)| *guide_slug == slug)
                    .map(|(_, label)| label);
                match guide_label.or_else(|| super::menu::scaffold_label(&slug).map(str::to_string)) {
                    Some(label) => Ok(WikiPageOutcome::Placeholder(Box::new(WikiPageView {
                        slug,
                        title: label,
                        rendered_html: String::new(),
                        placeholder: true,
                        generator: None,
                        written_head: None,
                        marker: None,
                        built_at_revision: None,
                        anchors: Vec::new(),
                        stale: false,
                        has_missing: false,
                        regen_pending: false,
                        current_revision,
                    }))),
                    None => Ok(WikiPageOutcome::Missing(slug)),
                }
            }
        }
    })
    .await;
    match model {
        Ok(WikiPageOutcome::Page(view)) | Ok(WikiPageOutcome::Placeholder(view)) => json::ok(*view),
        Ok(WikiPageOutcome::Missing(slug)) => (
            StatusCode::NOT_FOUND,
            Json(json::ApiError { error: format!("no wiki page at `{slug}`") }),
        )
            .into_response(),
        Err(err) => json::fail(err),
    }
}

/// The three outcomes of a wiki-page read: a present page, an honest placeholder
/// for a known scaffold slug, or a genuine miss (`404`). Boxed views keep the
/// variants size-balanced (clippy `large_enum_variant`).
enum WikiPageOutcome {
    Page(Box<WikiPageView>),
    Placeholder(Box<WikiPageView>),
    Missing(String),
}

// ── Wiki doc-asset serving (same-origin, path-sandboxed, [FR-WK-27]/[ADR-58]) ──

/// The repo-relative **doc roots** the asset route serves image files from — the
/// only directories a request may resolve into ([FR-WK-27]). Their *canonicalized*
/// form is the structural containment boundary ([NFR-SE-04], the read-only source
/// sandbox's posture).
const DOC_ASSET_ROOTS: &[&str] = &["docs/specs", "docs/howto"];

/// Map a filename extension to the image content-type the asset route serves, or
/// `None` for a non-image file ([FR-WK-27]: "only image content-types are served").
/// Kept in lockstep with the transform's allow-list
/// (`crate::present::is_image_path`). An actual `.<ext>` is required — a
/// dotless name is never an image.
fn image_content_type(path: &str) -> Option<&'static str> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        _ => return None,
    })
}

/// The outcome of resolving a doc-asset request: a served image, or one of the two
/// distinct refusals the [`wiki_asset`] route reports.
enum AssetOutcome {
    /// The sandboxed, image-typed file — its content-type and bytes.
    Image(&'static str, Vec<u8>),
    /// The request is not an image file — refused (`415`, only images are served).
    NotImage,
    /// The path escapes the doc roots, or the file is absent/unreadable — refused
    /// (`404`, leaking nothing about what lies outside the sandbox).
    Denied,
}

/// Resolve a doc-asset request **structurally** ([FR-WK-27], [NFR-SE-04]): serve an
/// image file only when it canonicalizes to a path **inside** a canonicalized doc
/// root.
///
/// The containment check is a **canonicalized-prefix** test — never string matching.
/// [`std::fs::canonicalize`] resolves every `.`/`..` segment and symlink and fails on
/// an absent file, so the real target path is what [`FsPath::starts_with`]
/// (component-wise) is tested against a real doc root: a `..` traversal, an absolute
/// path, or a symlink pointing outside all resolve away *before* the prefix test and
/// fail it. The doc roots are canonicalized too, so a symlink in the repo prefix
/// (e.g. macOS `/tmp` → `/private/tmp`) cannot cause a spurious mismatch.
fn resolve_doc_asset(root: &FsPath, rel_path: &str) -> AssetOutcome {
    // Only image files are ever served — a cheap gate before any filesystem work.
    let Some(mime) = image_content_type(rel_path) else {
        return AssetOutcome::NotImage;
    };
    // Canonicalize the allowed roots, skipping one that is absent in this project.
    let allowed: Vec<PathBuf> =
        DOC_ASSET_ROOTS.iter().filter_map(|r| std::fs::canonicalize(root.join(r)).ok()).collect();
    // Canonicalize the requested target — resolves `..`/`.`/symlinks, errors if absent.
    let Ok(full) = std::fs::canonicalize(root.join(rel_path)) else {
        return AssetOutcome::Denied;
    };
    // Structural containment: the real target must sit within a real doc root.
    if !allowed.iter().any(|base| full.starts_with(base)) {
        return AssetOutcome::Denied;
    }
    match std::fs::read(&full) {
        Ok(bytes) => AssetOutcome::Image(mime, bytes),
        // A directory or unreadable node with an image-looking name → refused.
        Err(_) => AssetOutcome::Denied,
    }
}

/// `GET /api/v1/wiki/asset/*path` — the same-origin, read-only **doc-image asset
/// route** ([FR-WK-27], [ADR-58]) that presented pages' `<img src>` values resolve to
/// (rewritten by the [FR-WK-25] transform, `crate::present::rewrite_refs`).
/// It serves image files from the doc roots (`docs/specs/**`, `docs/howto/**`) only,
/// **path-sandboxed** by canonicalized-prefix containment ([`resolve_doc_asset`],
/// [NFR-SE-04]): a `..`/absolute/symlink escape is an honest `404`, a non-image path
/// a `415`, and only image content-types are served. A served image also carries
/// `X-Content-Type-Options: nosniff`, so a browser honors the declared image type and
/// never MIME-sniffs a doc file into an active document (defense in depth for the
/// `image/svg+xml` type, atop the unchanged self-only CSP). Read-only — the fetch
/// mutates no store ([ADR-28]); the assets are **same-origin**, so the self-only CSP
/// is byte identical (no `data:` inlining, no external host).
///
/// The blocking filesystem read runs on the pool via the [`bridge`]
/// ([ADR-03]), never the current-thread serve loop.
///
/// [FR-WK-27]: ../../docs/specs/requirements/FR-WK-27.md
/// [FR-WK-25]: ../../docs/specs/requirements/FR-WK-25.md
/// [NFR-SE-04]: ../../docs/specs/requirements/NFR-SE-04.md
/// [ADR-58]: ../../docs/specs/architecture/decisions/ADR-58.md
/// [ADR-28]: ../../docs/specs/architecture/decisions/ADR-28.md
/// [ADR-03]: ../../docs/specs/architecture/decisions/ADR-03.md
pub(super) async fn wiki_asset(
    MemberEngine(engine): MemberEngine,
    Path(rel_path): Path<String>,
) -> Response {
    let outcome =
        bridge(engine, "api_v1_wiki_asset", Surface::Web, move |e| resolve_doc_asset(e.root(), &rel_path)).await;
    match outcome {
        AssetOutcome::Image(mime, bytes) => (
            [
                (header::CONTENT_TYPE, mime),
                (header::CACHE_CONTROL, "no-cache"),
                // Honor the declared image type; never sniff a doc file into an active
                // document (belt-and-suspenders for `image/svg+xml`, atop the CSP).
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            bytes,
        )
            .into_response(),
        AssetOutcome::NotImage => (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "the wiki asset route serves image files only",
        )
            .into_response(),
        AssetOutcome::Denied => (StatusCode::NOT_FOUND, "no such wiki asset").into_response(),
    }
}

// ── Wiki navigation IA (four-tier menu, [FR-UI-06]) ───────────────────────────

/// One leaf of a {@link WikiNavTier}: a page link by its fixed slug + label.
#[derive(Debug, Serialize)]
pub(super) struct WikiNavItem {
    slug: String,
    label: String,
}

/// One tier of the wiki menu — its title and its discrete page links.
#[derive(Debug, Serialize)]
pub(super) struct WikiNavTier {
    title: String,
    items: Vec<WikiNavItem>,
}

/// The wiki menu **information architecture** ([FR-UI-06], CR-034/CR-035/CR-039/
/// CR-062) the SPA renders: **Summary / User Guide / Design / Specs** tiers plus a
/// top-level Search link — the User Guide tier present only when
/// [`Engine::wiki_guide_pages`](crate::WikiEngine::wiki_guide_pages) is non-empty ([FR-WK-23]). Composed from the
/// **same** [`super::menu`] constants the server-rendered menu uses (the
/// `DocCategory` slug/title contract + the Summary scaffold), so the two menus can
/// never disagree; Frontend Design appears in Design only when its source doc
/// exists (read through the engine, never a filesystem scan).
#[derive(Debug, Serialize)]
pub(super) struct WikiNav {
    tiers: Vec<WikiNavTier>,
    /// The top-level Search link label (a sibling of the tiers, CR-039).
    search_label: String,
}

/// `GET /api/v1/wiki/nav` — the wiki menu IA ([FR-UI-06], S-189, CR-062). A pure
/// read: the fixed Summary/Design/Specs slug contracts, a single engine read for
/// Frontend-Design presence, and a single engine read for the dynamic User Guide
/// page set; it touches no store ([ADR-28]).
pub(super) async fn wiki_nav(MemberEngine(engine): MemberEngine) -> Response {
    use super::menu::{
        DESIGN_DOCS, GUIDED_TOUR, OVERVIEW_ARCHITECTURE, OVERVIEW_ARCHITECTURE_LABEL, SPECS_DOCS,
        SPECS_SRS, SPECS_SRS_LABEL, USER_GUIDE_TIER_TITLE,
    };
    let nav = bridge(engine, "api_v1_wiki_nav", Surface::Web, |e| {
        let item = |slug: &str, label: &str| WikiNavItem { slug: slug.into(), label: label.into() };
        let doc_item = |c: DocCategory| WikiNavItem { slug: c.slug().into(), label: c.title().into() };

        // Summary — the agent-tier Overview scaffold. The architecture narrative
        // left this tier (CR-062/ADR-57): it is now the presented
        // docs/specs/architecture.md under Design, so GUIDED_TOUR no longer carries
        // it and no filter is needed here.
        let summary = GUIDED_TOUR.iter().map(|(slug, label)| item(slug, label)).collect();

        // User Guide — one page per docs/howto/*.md file (CR-062, [FR-WK-23]);
        // absent entirely when docs/howto/ has no files, never an empty tier.
        let guide_items: Vec<WikiNavItem> =
            e.wiki_guide_pages().into_iter().map(|(slug, label)| item(&slug, &label)).collect();
        let user_guide = (!guide_items.is_empty())
            .then(|| WikiNavTier { title: USER_GUIDE_TIER_TITLE.into(), items: guide_items });

        // Design — the presented Architecture page, the consolidated ADRs/Components/
        // Integrations docs, and Frontend Design only when its source doc exists.
        let mut design = vec![item(OVERVIEW_ARCHITECTURE, OVERVIEW_ARCHITECTURE_LABEL)];
        design.extend(DESIGN_DOCS.iter().map(|&c| doc_item(c)));
        if e.wiki_doc_category_present(DocCategory::FrontendDesign) {
            design.push(doc_item(DocCategory::FrontendDesign));
        }

        // Specs — the presented SRS hub (CR-064, [FR-WK-26]) first, then the
        // consolidated requirement / UAT documents.
        let mut specs = vec![item(SPECS_SRS, SPECS_SRS_LABEL)];
        specs.extend(SPECS_DOCS.iter().map(|&c| doc_item(c)));

        let mut tiers = vec![WikiNavTier { title: "Summary".into(), items: summary }];
        tiers.extend(user_guide);
        tiers.push(WikiNavTier { title: "Design".into(), items: design });
        tiers.push(WikiNavTier { title: "Specs".into(), items: specs });

        WikiNav { tiers, search_label: "Search".into() }
    })
    .await;
    json::ok(nav)
}
