//! present — the deterministic **presented-tier** assembler ([FR-WK-20],
//! [ADR-57], [CR-062]).
//!
//! Where the *generated* tier ([ADR-24]) has a model paraphrase a project's
//! authored specifications into wiki prose, the *presented* tier renders those
//! documents **faithfully and deterministically**. It has two modes:
//!
//! - **Consolidated** ([`consolidated_page`], [`architecture_page`]) — for each
//!   present Design/Specs category, glob the category's `docs/specs/**` source
//!   files, sort them, and concatenate them into one page with a
//!   heading-delimited section per source document.
//! - **Per-file** ([`guide_pages`], [FR-WK-23]) — for the **User Guide** tier,
//!   each `docs/howto/*.md` file becomes **its own** page, rendered verbatim
//!   with no added section wrapper: the guides are large, standalone,
//!   user-pitched documents, so a page each (with its own menu entry) is
//!   navigable, unlike the consolidated mode's per-category grouping.
//!
//! This module owns the **assembly** and the **presentation-layer reference
//! transform** ([FR-WK-25], [ADR-58]) — both pure functions of the on-disk source
//! files, with no `wiki.db` touch, no LLM, and no network ([NFR-SE-01]). The
//! store-writing orchestration ([`super::materialize`]) stamps the assembled
//! pages with the [`super::PRESENTED_GENERATOR`] label, a `file:<path>` anchor per
//! source document, and the current built-at revision, then hands them to the
//! shared write path ([`super::write`]). Because the body is a pure function of
//! the sorted source list and each file's bytes, re-running with unchanged
//! sources produces a byte-identical page ([FR-WK-20]).
//!
//! After assembly, [`materialize`](super::materialize) builds a [`Manifest`] —
//! every presented source path → its destination `(slug, section-anchor)` — and
//! [`rewrite_refs`] rewrites each body's in-body Markdown link targets against it so
//! authored *relative* references resolve onto the wiki's slug/anchor routing
//! instead of 404-ing. The transform touches only reference **targets**, never
//! prose, and is byte-identical on re-run ([FR-WK-25], [NFR-RA-06]).
//!
//! [FR-WK-20]: ../../../docs/specs/requirements/FR-WK-20.md
//! [FR-WK-23]: ../../../docs/specs/requirements/FR-WK-23.md
//! [FR-WK-25]: ../../../docs/specs/requirements/FR-WK-25.md
//! [ADR-24]: ../../../docs/specs/architecture/decisions/ADR-24.md
//! [ADR-57]: ../../../docs/specs/architecture/decisions/ADR-57.md
//! [ADR-58]: ../../../docs/specs/architecture/decisions/ADR-58.md
//! [CR-062]: ../../../docs/requests/CR-062-wiki-present-authored-docs.md
//! [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md

use std::collections::HashMap;
use std::path::Path;

use super::DocCategory;

/// One source document to present — its repo-relative path (the `file:` anchor
/// key, the sort key, and the [`stem_of`] source for the section heading/anchor) and
/// its bytes, presented with only reference targets rewritten ([`rewrite_refs`]).
struct SourceDoc {
    /// Repo-relative path, e.g. `docs/specs/architecture/decisions/ADR-01.md`.
    rel_path: String,
    /// The file's bytes, presented with only reference targets rewritten.
    body: String,
}

/// A fully assembled presented page ready for the write path — its slug, title,
/// consolidated Markdown body, and one `file:<path>` anchor per source document
/// (in sorted-source order, matching the body's sections).
pub(crate) struct PresentedPage {
    /// The canonical wiki slug this page upserts at.
    pub slug: String,
    /// The page title (the leading `# ` heading and the menu label).
    pub title: String,
    /// The consolidated Markdown body — a header line plus a section per source.
    pub body: String,
    /// One `file:<repo-relative-path>` anchor per source document.
    pub anchors: Vec<String>,
    /// The repo-relative directory the page's source documents live in — the base
    /// against which [`rewrite_refs`] resolves this body's relative links ([FR-WK-25]).
    /// One directory suffices because every consolidated category globs a single
    /// directory ([`DocCategory::source_glob`]).
    pub source_dir: String,
    /// `true` when the page is a **consolidated** page whose sections each carry a
    /// stable stem anchor ([`stem_anchor`]) — a link to one of its source documents
    /// lands on that section. `false` for a per-file User Guide page ([FR-WK-23]),
    /// which is one whole document, so a link to it targets the page route with no
    /// fragment.
    pub sectioned: bool,
}

/// The consolidated page for one Design/Specs `category`, or `None` when the
/// category's source glob matches no file — a category reported by absence, never
/// a fabricated empty page ([FR-WK-06], [FR-WK-20]).
pub(crate) fn consolidated_page(root: &Path, category: DocCategory) -> Option<PresentedPage> {
    assemble(
        category.slug(),
        category.title(),
        category.source_glob(),
        list_sources(root, category.source_glob()),
    )
}

/// The single-file Architecture page ([`super::PRESENTED_ARCHITECTURE_SLUG`] ←
/// [`super::ARCHITECTURE_DOC`]), or `None` when `docs/specs/architecture.md` is
/// absent. The Design-tier "Architecture" entry, presented rather than
/// synthesized ([CR-062]); the retired agent-authored `overview/architecture`
/// page's slug is reused so the menu/reader route is unchanged.
pub(crate) fn architecture_page(root: &Path) -> Option<PresentedPage> {
    assemble(
        super::PRESENTED_ARCHITECTURE_SLUG,
        super::PRESENTED_ARCHITECTURE_TITLE,
        super::ARCHITECTURE_DOC,
        list_sources(root, super::ARCHITECTURE_DOC),
    )
}

/// The single-file **SRS hub** page ([`super::PRESENTED_SRS_SLUG`] ←
/// [`super::SOFTWARE_SPEC_DOC`], [FR-WK-26], [FR-WK-11], [CR-064]), or `None` when
/// `docs/specs/software-spec.md` is absent. The Specs-tier hub, **presented**
/// rather than synthesized ([CR-062]) so `software-spec.md`/`§N` references from
/// other presented pages resolve onto it (via the [`Manifest`]). Because the
/// source is a single no-wildcard glob, [`list_sources`] reads exactly that one
/// file — **only** the SRS hub is presented, never the stray top-level
/// `docs/specs/*.md` analyst inputs (`analyst-frs.md`, `writer-nfr.md`, …), which
/// live beside it but are working intermediates, not the published hub.
pub(crate) fn srs_page(root: &Path) -> Option<PresentedPage> {
    assemble(
        super::PRESENTED_SRS_SLUG,
        super::PRESENTED_SRS_TITLE,
        super::SOFTWARE_SPEC_DOC,
        list_sources(root, super::SOFTWARE_SPEC_DOC),
    )
}

/// The **User Guide** tier's per-file pages ([FR-WK-23], [FR-WK-11]) — one
/// page per `docs/howto/*.md` file, sorted by source path so `README.md`
/// (ASCII-sorts before every lowercase guide name) leads and the set is
/// deterministic ([NFR-RA-06]). `README.md` presents at the tier landing
/// `guide/overview`; every other file presents at `guide/<name>` (`<name>` its
/// file stem) with a title humanized from that stem. Unlike
/// [`consolidated_page`], the body carries **no** added header — it is the
/// source file's bytes, rendered verbatim ([FR-WK-23]). Empty when
/// `docs/howto/` has no `*.md` file — reported by absence, never a fabricated
/// tier.
///
/// Two file names are **skipped** rather than presented, so one badly- or
/// confusingly-named guide can never break the whole presented tier ([FR-WK-20]
/// review finding):
/// - a non-`README.md` file whose stem case-insensitively collides with the
///   reserved `overview` landing name (e.g. a stray `overview.md`) — presenting
///   it would silently overwrite `README.md`'s `guide/overview` page under
///   `wiki.db`'s last-write-wins upsert, with no error or log;
/// - a file whose stem is not a valid slug segment ([`super::validate_slug`] —
///   lowercase ASCII letters/digits/`-`/`_` only, e.g. mixed case or spaces) —
///   presenting it would reject the **entire** `wiki materialize` write loop
///   ([`super::write`]) partway through, since guide pages share the write path
///   with the fixed-slug Design/Specs categories.
pub(crate) fn guide_pages(root: &Path) -> Vec<PresentedPage> {
    list_sources(root, super::HOWTO_GUIDE_GLOB)
        .into_iter()
        .filter_map(|doc| {
            let stem = stem_of(&doc.rel_path);
            let is_readme = stem.eq_ignore_ascii_case("README");
            if !is_readme && stem.eq_ignore_ascii_case("overview") {
                return None;
            }
            let (name, title) = if is_readme {
                ("overview".to_string(), "Overview".to_string())
            } else {
                (stem.to_string(), humanize_guide_title(stem))
            };
            let slug = format!("{}/{name}", super::GUIDE_SLUG_PREFIX);
            super::validate_slug(&slug).ok()?;
            Some(PresentedPage {
                slug,
                title,
                body: doc.body,
                anchors: vec![format!("file:{}", doc.rel_path)],
                source_dir: dir_of_glob(super::HOWTO_GUIDE_GLOB).to_string(),
                // A guide page is one whole document, not a sectioned consolidation.
                sectioned: false,
            })
        })
        .collect()
}

/// Humanize a `docs/howto` file stem (`error-handling`) into its page/menu
/// title (`Error Handling`) — split on `-`, capitalize each word, uppercasing
/// a small fixed set of technical acronyms (`ci`, `ui`, `api`, `cli`, `id`,
/// `url`) Title Case would otherwise mangle (`ci-integration` → `CI
/// Integration`, not `Ci Integration`).
fn humanize_guide_title(stem: &str) -> String {
    const ACRONYMS: &[&str] = &["ci", "ui", "api", "cli", "id", "url"];
    stem.split('-')
        .map(|word| {
            if ACRONYMS.contains(&word) {
                word.to_uppercase()
            } else {
                let mut chars = word.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Assemble a presented page from the sorted `docs`, or `None` when there is no
/// source document. The body is `# <title>`, a one-line provenance note naming
/// the `glob` it was presented from, then one [`section_for`] per source document
/// in sorted order, separated by a `---` rule — a pure function of the inputs, so
/// an unchanged source set yields a byte-identical body ([FR-WK-20]).
fn assemble(slug: &str, title: &str, glob: &str, docs: Vec<SourceDoc>) -> Option<PresentedPage> {
    if docs.is_empty() {
        return None;
    }
    let plural = if docs.len() == 1 { "" } else { "s" };
    let header = format!(
        "# {title}\n\n_Presented from `{glob}` — {n} source document{plural}, \
         assembled deterministically by Logos. Not model-generated._",
        n = docs.len(),
    );
    let anchors = docs.iter().map(|doc| format!("file:{}", doc.rel_path)).collect();
    // Call `section_for` through an explicit closure, not a bare `fn` reference: the
    // logos call-graph resolver records a call edge for a closure body but not for a
    // `.map(section_for)` function value, so the bare form leaves `section_for` (and
    // the helpers only reachable through it) read as dead by `logos check`. The
    // redundant_closure lint is suppressed for exactly this call-graph-visibility reason.
    #[allow(clippy::redundant_closure)]
    let sections =
        docs.iter().map(|doc| section_for(doc)).collect::<Vec<_>>().join("\n\n---\n\n");
    let body = format!("{header}\n\n{sections}\n");
    Some(PresentedPage {
        slug: slug.to_string(),
        title: title.to_string(),
        body,
        anchors,
        source_dir: dir_of_glob(glob).to_string(),
        sectioned: true,
    })
}

/// One consolidated-page section for `doc`, headed by the source document's own
/// title rather than its filename ([FR-WK-20], [FR-WK-25]) — the "duplicate title"
/// this story removes, where a synthetic filename heading previously sat directly
/// above the document's own, near-identical leading title.
///
/// The heading text is still literally the file **stem** (`NFR-RA-01`, not
/// `NFR-RA-01.md`), so the shared renderer ([FR-WK-15]) gives it the exact stable
/// `toc-<slug>` id [`stem_anchor`] computes — the assembler-controlled anchor the
/// reference transform ([`rewrite_refs`]) targets. That id is derived purely from the
/// heading's own rendered text, so it can only stay stable across a title edit if
/// the heading text itself never changes; the document's own title is therefore
/// shown as a **bolded subtitle** immediately under the heading, never as a second
/// competing heading, stripped of a redundant `<stem>: ` prefix when the title
/// repeats the stem ([`redundant_title_suffix`]) — a document whose title *is* its
/// stem (e.g. a Components page's `agent-core`) gets no subtitle at all, since there
/// is nothing left to say. The document's own leading heading line is removed from
/// the embedded remainder ([`split_leading_title`]) so it never renders a second time.
fn section_for(doc: &SourceDoc) -> String {
    let stem = stem_of(&doc.rel_path);
    let (title, rest) = split_leading_title(&doc.body);
    let subtitle = title.and_then(|t| redundant_title_suffix(t, stem));
    let mut section = format!("## {stem}");
    if let Some(subtitle) = subtitle {
        section.push_str(&format!("\n\n**{subtitle}**"));
    }
    // `rest` is verbatim (leading blank lines included) when `doc.body` had no
    // leading title for `split_leading_title` to trim; trim both ends here so
    // section spacing stays uniform regardless of whether a title was found.
    let rest = rest.trim_matches('\n');
    if !rest.is_empty() {
        section.push_str(&format!("\n\n{rest}"));
    }
    section
}

/// Split `body` into its leading ATX `# ` title text (if the first non-blank line
/// is one) and the remaining body with that heading line — and any blank lines
/// immediately following it — removed. `None` when the body does not open with a
/// level-1 heading, in which case `body` is returned unchanged (nothing to
/// suppress). Used by [`section_for`] so a source document's own title is shown
/// once, as the section's subtitle, rather than twice.
fn split_leading_title(body: &str) -> (Option<&str>, &str) {
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        if line.trim().is_empty() {
            offset += line.len();
            continue;
        }
        let Some(text) = line.trim().strip_prefix("# ") else { break };
        let mut rest = &body[offset + line.len()..];
        while let Some(nl) = rest.find('\n') {
            if rest[..nl].trim().is_empty() {
                rest = &rest[nl + 1..];
            } else {
                break;
            }
        }
        return (Some(text.trim()), rest);
    }
    (None, body)
}

/// The document's own `title`, stripped of a redundant `<stem>: ` prefix when its
/// leading heading duplicates the section's stem-derived heading — the literal
/// duplication [`section_for`] removes. `None` when nothing distinct remains (the
/// title *is* the stem, e.g. a Components/Integrations document titled after
/// itself) — no subtitle line is needed then. A title that does not follow the
/// `<stem>: text` convention used throughout `docs/specs/requirements/**` and
/// `docs/specs/architecture/decisions/**` is returned unstripped — best-effort, so
/// no title text is ever silently dropped.
fn redundant_title_suffix<'a>(title: &'a str, stem: &str) -> Option<&'a str> {
    let title = title.trim();
    if title == stem {
        return None;
    }
    match title.strip_prefix(stem).and_then(|rest| rest.strip_prefix(':')) {
        Some(rest) => {
            let rest = rest.trim();
            (!rest.is_empty()).then_some(rest)
        }
        None => Some(title),
    }
}

/// The existing source files a `docs/`-relative `glob` names, read verbatim and
/// returned in **sorted repo-relative-path order** so the assembled page is
/// deterministic regardless of the OS `read_dir` order ([FR-WK-20], [NFR-RA-06]).
///
/// The glob is one of the fixed [`DocCategory::source_glob`] forms (or a single
/// named file): a last segment with at most one `*` wildcard splits into a
/// `<prefix>*<suffix>` name filter over its parent directory (the same
/// prefix/suffix contract [`super::dir_has`] applies); a last segment with no
/// wildcard is a single named file. A pure local-FS read — no `wiki.db` touch, no
/// LLM, no network ([NFR-SE-01]).
fn list_sources(root: &Path, glob: &str) -> Vec<SourceDoc> {
    let mut docs: Vec<SourceDoc> = match glob.rsplit_once('/') {
        Some((dir, pattern)) if pattern.contains('*') => {
            // `<prefix>*<suffix>` over `dir` — e.g. `*.md` (prefix ``) or
            // `FR-*.md` (prefix `FR-`, which never matches an `NFR-`/`UAT-` file).
            let (prefix, suffix) = pattern.split_once('*').expect("pattern contains '*'");
            let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
                return Vec::new();
            };
            entries
                .flatten()
                .filter_map(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    (name.starts_with(prefix) && name.ends_with(suffix) && entry.path().is_file())
                        .then(|| read_doc(root, &format!("{dir}/{name}")))
                        .flatten()
                })
                .collect()
        }
        // A single named file (`docs/specs/architecture.md`, `frontend-design.md`).
        _ => read_doc(root, glob).into_iter().collect(),
    };
    docs.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    docs
}

/// Read one repo-relative source file verbatim into a [`SourceDoc`], or `None`
/// when it is absent or not valid UTF-8 (a document Logos cannot present, so it is
/// simply omitted rather than fabricated).
fn read_doc(root: &Path, rel_path: &str) -> Option<SourceDoc> {
    let body = std::fs::read_to_string(root.join(rel_path)).ok()?;
    Some(SourceDoc {
        rel_path: rel_path.to_string(),
        body,
    })
}

// ── FR-WK-25 / ADR-58: the presentation-layer reference transform ─────────────

/// The wiki prefix a rewritten in-body link resolves to — the SPA reader route
/// (`web/ui/src/views/wiki/wikiModel.ts::pageHref`, `/wiki/page/<slug>`). Kept in
/// one place so the rewrite and the reader agree on the route shape.
// The transform (its only consumer) is compiled out without `lang-markdown`.
#[cfg_attr(not(feature = "lang-markdown"), allow(dead_code))]
const WIKI_PAGE_ROUTE: &str = "/wiki/page/";

/// The same-origin **doc-asset route** prefix ([FR-WK-27], [ADR-58]) a rewritten
/// image `src` resolves to — the [web-surface] `/api/v1/wiki/asset/<repo-relative-path>`
/// endpoint that reads the doc-relative image file back, path-sandboxed to the doc
/// roots. Kept in one place so the rewrite here and the route in
/// `web/src/api_v1.rs::wiki_asset` agree on the shape: the value the transform emits
/// (`<prefix><resolved repo-relative path>`) is exactly the `*path` the route captures.
///
/// [web-surface]: ../../../docs/specs/architecture/components/web-surface.md
/// [FR-WK-27]: ../../../docs/specs/requirements/FR-WK-27.md
// The transform (its only consumer) is compiled out without `lang-markdown`.
#[cfg_attr(not(feature = "lang-markdown"), allow(dead_code))]
const WIKI_ASSET_ROUTE: &str = "/api/v1/wiki/asset/";

/// One resolved destination in the [`Manifest`]: the wiki page slug a source
/// document lands on, and its stable in-page section anchor (the `toc-<stem-slug>`
/// id the assembler's `## <stem>` heading renders to). The anchor is empty for a
/// per-file page — a User Guide page is one whole document, so a link to it targets
/// the page route with no fragment.
// The fields are read only by the transform, compiled out without `lang-markdown`.
#[cfg_attr(not(feature = "lang-markdown"), allow(dead_code))]
#[derive(Debug, Clone)]
struct Destination {
    slug: String,
    anchor: String,
}

/// The materialize-time **resolution manifest** ([FR-WK-25], [ADR-58]): every
/// presented source file's repo-relative path → its destination `(slug,
/// section-anchor)`. Built from the assembled page set, so it lists exactly what was
/// presented — a link whose resolved target is absent from this map points at no
/// presented page and is de-linked to plain text ([`rewrite_refs`]).
pub(crate) struct Manifest {
    // Read only by the transform, compiled out without `lang-markdown`.
    #[cfg_attr(not(feature = "lang-markdown"), allow(dead_code))]
    by_path: HashMap<String, Destination>,
}

impl Manifest {
    /// Build the manifest from the assembled `pages` — for each page, one entry per
    /// `file:<path>` anchor, mapping the source path to the page slug and (for a
    /// [`PresentedPage::sectioned`] page) that document's stable stem anchor.
    /// Deriving it from the pages themselves guarantees the manifest and the presented
    /// set never diverge.
    pub(crate) fn from_pages(pages: &[PresentedPage]) -> Manifest {
        let mut by_path = HashMap::new();
        for page in pages {
            for anchor in &page.anchors {
                let Some(path) = anchor.strip_prefix("file:") else {
                    continue;
                };
                let dest_anchor = if page.sectioned {
                    stem_anchor(stem_of(path))
                } else {
                    String::new()
                };
                by_path.insert(
                    path.to_string(),
                    Destination { slug: page.slug.clone(), anchor: dest_anchor },
                );
            }
        }
        Manifest { by_path }
    }

    /// The destination a resolved repo-relative source path presents to, or `None`
    /// when that path is not a presented document.
    #[cfg(feature = "lang-markdown")]
    fn resolve_dest(&self, path: &str) -> Option<&Destination> {
        self.by_path.get(path)
    }

    /// Whether `slug` names a **consolidated** (multi-document) page — a page at least
    /// one presented document lands on with a non-empty stem section anchor. Used to
    /// decide whether a bare `#frag` can safely target a heading id (single-document
    /// page) or must be left as authored (consolidated page — uniquify collision).
    #[cfg(feature = "lang-markdown")]
    fn page_is_sectioned(&self, slug: &str) -> bool {
        self.by_path.values().any(|d| d.slug == slug && !d.anchor.is_empty())
    }
}

/// The repo-relative file stem of a path: the basename minus a `.md`/`.markdown`
/// extension (`docs/specs/requirements/NFR-RA-01.md` → `NFR-RA-01`).
fn stem_of(path: &str) -> &str {
    let base = path.rsplit('/').next().unwrap_or(path);
    base.strip_suffix(".md").or_else(|| base.strip_suffix(".markdown")).unwrap_or(base)
}

/// The directory portion of a source `glob` — the single directory a consolidated
/// category's files live in, the base for its relative links (`docs/specs/requirements/FR-*.md`
/// → `docs/specs/requirements`; a no-wildcard `docs/specs/architecture.md` →
/// `docs/specs`).
fn dir_of_glob(glob: &str) -> &str {
    glob.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("")
}

/// The stable stem-derived section anchor for a source document — the `toc-<slug>`
/// heading id the shared renderer ([FR-WK-15]) emits for the assembler's `## <stem>`
/// section heading. Independent of the document's own title wording, so a cross- or
/// intra-document link targeting it survives a title edit ([FR-WK-25], [ADR-58]).
fn stem_anchor(stem: &str) -> String {
    format!("toc-{}", renderer_slug(stem))
}

/// A heading slug **byte-compatible with the shared Markdown renderer**
/// (`web/src/markdown.rs::slugify`, the `toc-<slug>` id source): lowercase ASCII
/// alphanumerics kept, every other run collapsed to a single `-`, no leading/trailing
/// `-`, `section` when empty. Kept in lockstep with the renderer so a rewritten anchor
/// targets the id the renderer actually emits — the [FR-WK-25] anchor-divergence risk.
fn renderer_slug(s: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "section".to_string()
    } else {
        out
    }
}

/// Resolve a relative link `path` against the source document's directory
/// `base_dir`, normalizing `.`/`..` segments
/// (`docs/specs/architecture/components` + `../decisions/ADR-01.md` →
/// `docs/specs/architecture/decisions/ADR-01.md`). `None` when a `..` escapes above
/// the repo root — an un-resolvable link the transform leaves untouched.
#[cfg(feature = "lang-markdown")]
fn resolve_rel(base_dir: &str, path: &str) -> Option<String> {
    let mut segments: Vec<&str> = if path.starts_with('/') {
        Vec::new()
    } else {
        base_dir.split('/').filter(|s| !s.is_empty()).collect()
    };
    for seg in path.trim_start_matches('/').split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            s => segments.push(s),
        }
    }
    Some(segments.join("/"))
}

/// `true` for a link destination pointing outside the repository — a `scheme://`
/// URL, a `mailto:`/`tel:` URI, or a protocol-relative `//host/…`. Mirrors the doc
/// extractor's external-link rule ([`crate::extract`]) so presentation and extraction
/// agree on what is in-repo.
#[cfg(feature = "lang-markdown")]
fn is_external(dest: &str) -> bool {
    dest.starts_with("//")
        || dest.contains("://")
        || dest.starts_with("mailto:")
        || dest.starts_with("tel:")
}

/// `true` when `p` is a repo-relative path under one of the **doc roots** the
/// asset route serves (`docs/specs/**`, `docs/howto/**`) — the same containment
/// posture the [web-surface] route enforces structurally at serve time
/// ([FR-WK-27], [NFR-SE-04]). A resolved image source outside these roots is left
/// authored (the route would refuse it anyway), so the transform never emits an
/// asset link the sandbox will reject.
///
/// [web-surface]: ../../../docs/specs/architecture/components/web-surface.md
#[cfg(feature = "lang-markdown")]
fn is_doc_asset_path(p: &str) -> bool {
    p.starts_with("docs/specs/") || p.starts_with("docs/howto/")
}

/// `true` when `p` names an **image file** by extension — the same allow-list the
/// [web-surface] asset route serves ([FR-WK-27]: "only image content-types are
/// served"). Kept in lockstep with `web/src/api_v1.rs::image_content_type`; a
/// mismatch is only a cosmetic broken image (the route is the security authority),
/// never a containment gap. A trailing `?query`/`#fragment` is ignored before the
/// extension test.
///
/// [web-surface]: ../../../docs/specs/architecture/components/web-surface.md
#[cfg(feature = "lang-markdown")]
fn is_image_path(p: &str) -> bool {
    let path = p.split(['?', '#']).next().unwrap_or(p);
    matches!(
        path.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()).as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "avif" | "bmp" | "ico")
    )
}

/// Decide how to rewrite one image `src` ([FR-WK-27], [ADR-58]). Returns the new
/// destination when `raw` is a **doc-relative image file** resolving under a doc
/// root — rewritten to the same-origin asset route ([`WIKI_ASSET_ROUTE`]) — and
/// `None` (leave authored) otherwise:
///
/// - an external (`scheme://`, protocol-relative) or `data:` source → left as
///   authored (the self-only CSP governs whether it loads — unchanged by this
///   transform);
/// - a non-image extension → left as authored (nothing to serve);
/// - a source resolving **outside** the doc roots (or escaping the repo via `..`)
///   → left as authored (the route would refuse it — no broken asset link emitted);
/// - a doc-relative image resolving under `docs/specs/**` / `docs/howto/**` →
///   `/api/v1/wiki/asset/<resolved repo-relative path>`.
#[cfg(feature = "lang-markdown")]
fn decide_image(raw: &str, base_dir: &str) -> Option<String> {
    let src = raw.trim();
    if src.is_empty() || is_external(src) || src.starts_with("data:") {
        return None;
    }
    if !is_image_path(src) {
        return None;
    }
    let resolved = resolve_rel(base_dir, src)?;
    is_doc_asset_path(&resolved).then(|| format!("{WIKI_ASSET_ROUTE}{resolved}"))
}

/// The rewrite verdict for one in-body Markdown link ([`decide_link`]).
#[cfg(feature = "lang-markdown")]
enum LinkRewrite {
    /// Leave the link untouched (external, empty, or un-resolvable).
    Keep,
    /// Replace only the destination with this new href.
    Dest(String),
    /// De-link to plain text — the target is not a presented document.
    Delink,
}

/// Decide how to rewrite one link whose raw destination is `raw` ([FR-WK-25]).
/// `current_sectioned` is whether the page the link lives on is a consolidated
/// (multi-document) page.
///
/// - external / empty → [`LinkRewrite::Keep`];
/// - a `path` / `path#frag` resolving to a presented document → its wiki route
///   (or an in-page `#anchor` when it lands on `current_slug`);
/// - a bare `#frag` → the current page's heading anchor, but only on a
///   single-document page (see the fragment note below);
/// - anything else (a resolved path absent from the manifest) → [`LinkRewrite::Delink`].
///
/// ## Why a fragment does not target the sub-heading on a consolidated page
///
/// The shared renderer assigns each heading a `toc-<slug>` id via `slugify` **and**
/// `uniquify` (`web/src/markdown.rs`), so the 2nd/3rd heading that slugifies the same
/// becomes `toc-<slug>-2`, `-3`, …. A consolidated page concatenates many documents
/// whose sub-headings repeat (`## Statement`, `## Consequences`), so a static
/// `toc-<frag>` would collide and land on the **first** document's heading — the wrong
/// document. Rather than replicate the renderer's cross-document `uniquify` (the
/// approach [ADR-58] explicitly rejects in favor of stable stem anchors), a fragment
/// into a consolidated section resolves to that document's **stable stem section
/// anchor** ([FR-WK-25] "mapped to the destination anchor"), and a bare `#frag` on a
/// consolidated page is left as authored. On a single-document (User Guide) page there
/// is no cross-document collision, so the `toc-<frag>` heading id is targeted directly.
#[cfg(feature = "lang-markdown")]
fn decide_link(
    raw: &str,
    base_dir: &str,
    current_slug: &str,
    current_sectioned: bool,
    manifest: &Manifest,
) -> LinkRewrite {
    let dest = raw.trim();
    if dest.is_empty() || is_external(dest) {
        return LinkRewrite::Keep;
    }
    // A bare intra-document fragment: safe to target the heading id on a
    // single-document page, left as authored on a consolidated page (uniquify
    // collision — see the doc comment above).
    if let Some(frag) = dest.strip_prefix('#') {
        if frag.is_empty() || current_sectioned {
            return LinkRewrite::Keep;
        }
        return LinkRewrite::Dest(format!("#toc-{}", renderer_slug(frag)));
    }
    let (path, frag) = match dest.split_once('#') {
        Some((p, f)) => (p, Some(f)),
        None => (dest, None),
    };
    let Some(resolved) = resolve_rel(base_dir, path) else {
        return LinkRewrite::Keep;
    };
    let Some(target) = manifest.resolve_dest(&resolved) else {
        return LinkRewrite::Delink;
    };
    // A sectioned destination (non-empty stem anchor): the fragment, if any, resolves
    // to that document's stable section anchor. A single-document (guide) destination
    // (empty stem anchor): a fragment can target the collision-free heading id.
    let anchor = if !target.anchor.is_empty() {
        target.anchor.clone()
    } else {
        match frag {
            Some(f) if !f.is_empty() => format!("toc-{}", renderer_slug(f)),
            _ => String::new(),
        }
    };
    let href = if anchor.is_empty() {
        format!("{WIKI_PAGE_ROUTE}{}", target.slug)
    } else if target.slug == current_slug {
        format!("#{anchor}")
    } else {
        format!("{WIKI_PAGE_ROUTE}{}#{anchor}", target.slug)
    };
    LinkRewrite::Dest(href)
}

/// The byte span (relative to the parsed inline text) of an `inline_link`'s
/// `link_destination` child, or `None` if it has none.
#[cfg(feature = "lang-markdown")]
fn link_destination_span(link: tree_sitter::Node<'_>) -> Option<(usize, usize)> {
    for i in 0..link.named_child_count() {
        if let Some(c) = link.named_child(i) {
            if c.kind() == "link_destination" {
                return Some((c.start_byte(), c.end_byte()));
            }
        }
    }
    None
}

/// The visible `[text]` of an `inline_link` in `itext`, given the link's start (`[`)
/// and its destination start — the substring between the `[` and the `]` that
/// precedes the `(`. Preserves any inline markup in the text (so `[**bold**](x)`
/// de-links to `**bold**`). `None` on a malformed link.
#[cfg(feature = "lang-markdown")]
fn link_visible_text(itext: &str, link_start: usize, dest_start: usize) -> Option<&str> {
    let paren = itext.get(..dest_start)?.rfind('(')?;
    let close = itext.get(..paren)?.rfind(']')?;
    let text_start = link_start + 1;
    (text_start <= close).then(|| itext.get(text_start..close)).flatten()
}

/// Rewrite one presented body's in-body Markdown **link targets** against the
/// resolution `manifest` ([FR-WK-25], [ADR-58]) — touching only link *destinations*,
/// never prose:
///
/// - a `path.md` link to a presented document → the destination wiki page route
///   (`/wiki/page/<slug>#<section-anchor>`), or an in-page `#<section-anchor>` when
///   the target lands on the **same** page as `current_slug`;
/// - a `path.md#frag` link → the destination document's stable stem section anchor on
///   a consolidated page, or its `toc-<frag>` heading id on a single-document page (see
///   [`decide_link`] for why a fragment does not target the sub-heading on a
///   consolidated page);
/// - a bare intra-document `#frag` link → the current page's heading anchor on a
///   single-document page, left as authored on a consolidated page;
/// - a link whose resolved target is **not** in the manifest → de-linked to plain
///   text (the visible text kept, the `href` removed), so nothing 404s.
///
/// A doc-relative image `src` ([`decide_image`]) is rewritten to the same-origin
/// asset route (`/api/v1/wiki/asset/<repo-relative-path>`, [FR-WK-27]/S-270) so the
/// presented diagrams resolve; an external, `data:`, or root-escaping image source is
/// left untouched. External links (`http(s)://`, `mailto:`, protocol-relative) are
/// left untouched. `base_dir` is the source directory relative links and images
/// resolve against. A pure function of the body and the manifest — no LLM, no network
/// ([NFR-SE-01]); an unchanged input yields byte-identical output ([NFR-RA-06]). Links
/// and images inside code spans and fenced code are untouched because they are not
/// `inline_link`/`image` nodes in the grammar.
#[cfg(feature = "lang-markdown")]
/// Resolve a single `inline_link` node to its edit, if any — the `decide_link`
/// dispatch lifted out of [`rewrite_refs`]'s walk so the loop body stays shallow
/// (keeps the transform under the architecture nesting ceiling).
#[cfg(feature = "lang-markdown")]
fn link_edit(
    n: tree_sitter::Node,
    itext: &str,
    istart: usize,
    base_dir: &str,
    current_slug: &str,
    current_sectioned: bool,
    manifest: &Manifest,
) -> Option<(usize, usize, String)> {
    let (ds, de) = link_destination_span(n)?;
    match decide_link(&itext[ds..de], base_dir, current_slug, current_sectioned, manifest) {
        LinkRewrite::Keep => None,
        LinkRewrite::Dest(new) => Some((istart + ds, istart + de, new)),
        LinkRewrite::Delink => {
            let text = link_visible_text(itext, n.start_byte(), ds)?;
            Some((istart + n.start_byte(), istart + n.end_byte(), text.to_string()))
        }
    }
}

pub(crate) fn rewrite_refs(
    body: &str,
    base_dir: &str,
    current_slug: &str,
    manifest: &Manifest,
) -> String {
    use tree_sitter::Parser;

    let mut block = Parser::new();
    if block.set_language(&tree_sitter_md::LANGUAGE.into()).is_err() {
        return body.to_string();
    }
    let Some(block_tree) = block.parse(body, None) else {
        return body.to_string();
    };
    let mut inline = Parser::new();
    if inline.set_language(&tree_sitter_md::INLINE_LANGUAGE.into()).is_err() {
        return body.to_string();
    }

    // Whether this page is consolidated (multi-document) — decides whether a bare
    // `#frag` can safely target a heading id or must be left as authored.
    let current_sectioned = manifest.page_is_sectioned(current_slug);

    // One edit per rewritten link, each `(start, end, replacement)` over `body`;
    // spans never overlap (one per link), so applying them in descending start order
    // keeps the offsets valid.
    let mut edits: Vec<(usize, usize, String)> = Vec::new();

    // The block grammar leaves inline content as opaque `inline` leaves; links live
    // only there. A `[x](y)` inside a fenced/indented code block is a separate block
    // kind, never an `inline`, so code is never touched.
    let mut inline_spans: Vec<(usize, usize)> = Vec::new();
    let mut stack = vec![block_tree.root_node()];
    while let Some(n) = stack.pop() {
        if n.kind() == "inline" {
            inline_spans.push((n.start_byte(), n.end_byte()));
            continue;
        }
        // A GFM pipe-table cell: tree-sitter-md keeps cell content out of `inline`
        // nodes (its body is raw `_word`/punctuation tokens, no `inline_link`), so a
        // link inside a table cell is never visited by the `inline` branch above.
        // Parse the cell's own span with the inline parser too — otherwise the
        // table-heavy Architecture/Component/Integration pages keep dead `.md` links
        // (the [FR-WK-25] in-body-link contract must hold in tables, not just prose).
        if n.kind() == "pipe_table_cell" {
            inline_spans.push((n.start_byte(), n.end_byte()));
            continue;
        }
        for i in 0..n.named_child_count() {
            if let Some(c) = n.named_child(i) {
                stack.push(c);
            }
        }
    }

    for (istart, iend) in inline_spans {
        let itext = &body[istart..iend];
        let Some(itree) = inline.parse(itext, None) else {
            continue;
        };
        let mut istack = vec![itree.root_node()];
        while let Some(n) = istack.pop() {
            if n.kind() == "inline_link" {
                if let Some(edit) =
                    link_edit(n, itext, istart, base_dir, current_slug, current_sectioned, manifest)
                {
                    edits.push(edit);
                }
                continue; // a link's own subtree holds no further links to rewrite
            }
            // An image's `src`: rewrite a doc-relative image to the same-origin
            // asset route ([FR-WK-27]/S-270), leaving external/`data:`/root-escaping
            // sources authored ([`decide_image`]). Like a link, an image node holds
            // no further links to rewrite, so descend no deeper.
            if n.kind() == "image" {
                if let Some((ds, de)) = link_destination_span(n) {
                    if let Some(new) = decide_image(&itext[ds..de], base_dir) {
                        edits.push((istart + ds, istart + de, new));
                    }
                }
                continue;
            }
            for i in 0..n.named_child_count() {
                if let Some(c) = n.named_child(i) {
                    istack.push(c);
                }
            }
        }
    }

    if edits.is_empty() {
        return body.to_string();
    }
    edits.sort_by_key(|(start, ..)| std::cmp::Reverse(*start));
    let mut out = body.to_string();
    for (start, end, replacement) in edits {
        out.replace_range(start..end, &replacement);
    }
    out
}

/// Without the markdown grammar there is no inline parser, so the transform is the
/// identity — the same graceful degradation the doc extractor takes ([`crate::extract`]).
#[cfg(not(feature = "lang-markdown"))]
pub(crate) fn rewrite_refs(
    body: &str,
    _base_dir: &str,
    _current_slug: &str,
    _manifest: &Manifest,
) -> String {
    body.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn consolidated_page_sorts_sections_and_anchors_one_file_per_document() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        // Deliberately write out of order to prove the sort.
        write(root, "docs/specs/requirements/FR-WK-02.md", "# FR-WK-02\n\nSecond.\n");
        write(root, "docs/specs/requirements/FR-WK-01.md", "# FR-WK-01\n\nFirst.\n");
        // A sibling NFR must NOT leak into the FR category (prefix partition).
        write(root, "docs/specs/requirements/NFR-PE-01.md", "# NFR-PE-01\n");

        let page = consolidated_page(root, DocCategory::FunctionalRequirements)
            .expect("FR sources are present");
        assert_eq!(page.slug, "specs/functional-requirements");
        assert_eq!(page.title, "Functional Requirements");
        // Anchors: one file:<path> per FR document, in sorted order, NFR excluded.
        assert_eq!(
            page.anchors,
            vec![
                "file:docs/specs/requirements/FR-WK-01.md".to_string(),
                "file:docs/specs/requirements/FR-WK-02.md".to_string(),
            ]
        );
        // Sections appear in sorted order — FR-01 before FR-02 regardless of the
        // write order above. The section heading is the file **stem** (no `.md`), the
        // stable-anchor source ([FR-WK-25]).
        let one = page.body.find("## FR-WK-01\n").expect("FR-01 section");
        let two = page.body.find("## FR-WK-02\n").expect("FR-02 section");
        assert!(one < two, "sections are sorted by source path");
        assert!(page.body.starts_with("# Functional Requirements\n"));
        assert!(page.body.contains("Not model-generated."));
        // The verbatim bodies survive.
        assert!(page.body.contains("First."));
        assert!(page.body.contains("Second."));
        // No NFR content leaked in.
        assert!(!page.body.contains("NFR-PE-01"));
    }

    #[test]
    fn a_category_with_no_source_files_yields_no_page() {
        let tmp = TempDir::new().unwrap();
        assert!(
            consolidated_page(tmp.path(), DocCategory::Adrs).is_none(),
            "an empty glob produces no page"
        );
        assert!(
            architecture_page(tmp.path()).is_none(),
            "an absent architecture.md produces no page"
        );
    }

    #[test]
    fn architecture_page_presents_the_single_authored_file() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, "docs/specs/architecture.md", "# Architecture\n\nThe design.\n");
        let page = architecture_page(root).expect("architecture.md present");
        assert_eq!(page.slug, super::super::PRESENTED_ARCHITECTURE_SLUG);
        assert_eq!(page.title, "Architecture");
        assert_eq!(page.anchors, vec!["file:docs/specs/architecture.md".to_string()]);
        assert!(page.body.contains("The design."));
    }

    #[test]
    fn the_three_requirements_categories_partition_one_shared_directory_by_prefix() {
        // FR, NFR and UAT all resolve into docs/specs/requirements/, partitioned
        // solely by the filename-prefix filter — assert each category selects only
        // its own files and excludes the other two (the shared-directory hot spot).
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        // Distinct body sentinels — "NFR-01" contains "FR-01" as a substring, so
        // cross-contamination is proven on unique markers, not on the IDs.
        write(root, "docs/specs/requirements/FR-01.md", "# FR-01\n\nSENTINEL_FUNCTIONAL\n");
        write(root, "docs/specs/requirements/NFR-01.md", "# NFR-01\n\nSENTINEL_NONFUNCTIONAL\n");
        write(root, "docs/specs/requirements/UAT-01.md", "# UAT-01\n\nSENTINEL_ACCEPTANCE\n");

        let cases = [
            (
                DocCategory::NonFunctionalRequirements,
                "specs/non-functional-requirements",
                "Non-Functional Requirements",
                "docs/specs/requirements/NFR-01.md",
                "SENTINEL_NONFUNCTIONAL",
                ["SENTINEL_FUNCTIONAL", "SENTINEL_ACCEPTANCE"],
            ),
            (
                DocCategory::UserAcceptanceTests,
                "specs/user-acceptance-tests",
                "User Acceptance Tests",
                "docs/specs/requirements/UAT-01.md",
                "SENTINEL_ACCEPTANCE",
                ["SENTINEL_FUNCTIONAL", "SENTINEL_NONFUNCTIONAL"],
            ),
        ];
        for (category, slug, title, only_anchor, own, excluded) in cases {
            let page = consolidated_page(root, category).expect("category is present");
            assert_eq!(page.slug, slug);
            assert_eq!(page.title, title);
            assert_eq!(page.anchors, vec![format!("file:{only_anchor}")]);
            assert!(page.body.contains(own), "{slug} must present its own document");
            for other in excluded {
                assert!(
                    !page.body.contains(other),
                    "{slug} must not leak {other} from the shared requirements/ directory"
                );
            }
        }
    }

    #[test]
    fn the_single_file_frontend_design_category_presents_via_consolidated_page() {
        // FrontendDesign is the only DocCategory with a no-wildcard glob — it takes
        // the single-file branch of list_sources, distinct from the wildcard path.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, "docs/specs/frontend-design.md", "# Frontend Design\n\nThe UI.\n");
        let page = consolidated_page(root, DocCategory::FrontendDesign)
            .expect("frontend-design.md is present");
        assert_eq!(page.slug, "specs/frontend-design");
        assert_eq!(page.title, "Frontend Design");
        assert_eq!(page.anchors, vec!["file:docs/specs/frontend-design.md".to_string()]);
        assert!(page.body.contains("The UI."));
    }

    #[test]
    fn srs_page_presents_only_the_hub_not_the_stray_analyst_inputs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(
            root,
            "docs/specs/software-spec.md",
            "# Software Requirements Specification\n\nThe SRS hub.\n",
        );
        // Stray top-level analyst intermediates share docs/specs/ but are working
        // files, not the published hub — they must NOT be presented ([FR-WK-26]).
        write(root, "docs/specs/analyst-frs.md", "# Analyst FRS\n\nAn intermediate.\n");
        write(root, "docs/specs/writer-nfr.md", "# Writer NFR\n\nAnother intermediate.\n");

        let page = srs_page(root).expect("software-spec.md present");
        assert_eq!(page.slug, super::super::PRESENTED_SRS_SLUG);
        assert_eq!(page.title, "Software Requirements Specification");
        assert_eq!(
            page.anchors,
            vec!["file:docs/specs/software-spec.md".to_string()],
            "exactly one file anchor — the hub, never the strays"
        );
        assert!(page.sectioned, "the hub is sectioned so a link resolves to its stem anchor");
        assert_eq!(page.source_dir, "docs/specs");
        assert!(page.body.contains("The SRS hub."));
        // No stray analyst intermediate leaked into the presented body.
        assert!(!page.body.contains("An intermediate."), "analyst-frs.md must not be presented");
        assert!(!page.body.contains("Another intermediate."), "writer-nfr.md must not be presented");
    }

    #[test]
    fn srs_page_is_none_when_the_hub_is_absent() {
        let tmp = TempDir::new().unwrap();
        assert!(srs_page(tmp.path()).is_none(), "no software-spec.md → no SRS page");
    }

    #[test]
    fn a_non_utf8_source_file_is_omitted_without_panicking() {
        // read_doc drops a file that is absent or not valid UTF-8 (omit rather than
        // fabricate or crash) — exercise that branch with an invalid-UTF-8 sibling.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, "docs/specs/architecture/decisions/ADR-01.md", "# ADR-01\n\nValid.\n");
        let bad = root.join("docs/specs/architecture/decisions/ADR-02.md");
        std::fs::write(&bad, [0xFF, 0xFE, 0x00, 0x9F]).unwrap();

        let page = consolidated_page(root, DocCategory::Adrs)
            .expect("the valid ADR still yields a page");
        assert_eq!(
            page.anchors,
            vec!["file:docs/specs/architecture/decisions/ADR-01.md".to_string()],
            "the non-UTF-8 file is omitted, leaving only the valid document"
        );
        assert!(page.body.contains("Valid."));
    }

    #[test]
    fn assembly_is_byte_identical_for_an_unchanged_source_set() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, "docs/specs/architecture/decisions/ADR-02.md", "# ADR-02\n\nB.\n");
        write(root, "docs/specs/architecture/decisions/ADR-01.md", "# ADR-01\n\nA.\n");
        let first = consolidated_page(root, DocCategory::Adrs).unwrap();
        let second = consolidated_page(root, DocCategory::Adrs).unwrap();
        assert_eq!(first.body, second.body, "re-assembly is byte-identical");
        assert_eq!(first.anchors, second.anchors);
    }

    #[test]
    fn assembly_with_bolded_subtitles_is_byte_identical_for_an_unchanged_source_set() {
        // The generic byte-identical check above uses stem-only titles, never
        // exercising `section_for`'s subtitle branch (S-268) — cover it here.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, "docs/specs/requirements/NFR-RA-01.md", "# NFR-RA-01: stdout-safety invariant\n\nDetail.\n");
        write(root, "docs/specs/requirements/NFR-RA-02.md", "# Some Unrelated Title\n\nMore.\n");
        let first = consolidated_page(root, DocCategory::NonFunctionalRequirements).unwrap();
        let second = consolidated_page(root, DocCategory::NonFunctionalRequirements).unwrap();
        assert_eq!(first.body, second.body, "re-assembly with subtitles is byte-identical");
        assert!(first.body.contains("**stdout-safety invariant**"), "{}", first.body);
        assert!(first.body.contains("**Some Unrelated Title**"), "{}", first.body);
    }

    // ── FR-WK-23: per-file User Guide mode ──────────────────────────────────

    #[test]
    fn guide_pages_yields_one_page_per_howto_file_with_readme_as_the_landing() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, "docs/howto/README.md", "# User Guide\n\nStart here.\n");
        write(root, "docs/howto/installation.md", "# Installation\n\nRun the installer.\n");
        write(root, "docs/howto/ci-integration.md", "# CI Integration\n\nWire up CI.\n");

        let pages = guide_pages(root);
        assert_eq!(pages.len(), 3, "one page per docs/howto/*.md file");

        let overview = pages.iter().find(|p| p.slug == "guide/overview").expect("README → landing");
        assert_eq!(overview.title, "Overview");
        assert_eq!(overview.anchors, vec!["file:docs/howto/README.md".to_string()]);
        // Verbatim body: no consolidated-mode "Presented verbatim from..." wrapper.
        assert_eq!(overview.body, "# User Guide\n\nStart here.\n");

        let installation =
            pages.iter().find(|p| p.slug == "guide/installation").expect("installation.md presented");
        assert_eq!(installation.title, "Installation");
        assert_eq!(installation.body, "# Installation\n\nRun the installer.\n");

        let ci = pages.iter().find(|p| p.slug == "guide/ci-integration").expect("ci-integration.md presented");
        assert_eq!(ci.title, "CI Integration", "the ci acronym is uppercased, not title-cased");
    }

    #[test]
    fn guide_pages_sorts_the_readme_landing_first() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        // Written out of alphabetical order to prove the sort, not the write order.
        write(root, "docs/howto/usage.md", "# Usage\n");
        write(root, "docs/howto/README.md", "# User Guide\n");
        write(root, "docs/howto/commands.md", "# Commands\n");

        let pages = guide_pages(root);
        assert_eq!(
            pages.iter().map(|p| p.slug.as_str()).collect::<Vec<_>>(),
            vec!["guide/overview", "guide/commands", "guide/usage"],
            "README.md ASCII-sorts before lowercase guide names, landing the tier"
        );
    }

    #[test]
    fn an_empty_howto_directory_yields_no_guide_pages() {
        let tmp = TempDir::new().unwrap();
        assert!(guide_pages(tmp.path()).is_empty(), "an absent docs/howto/ produces no pages");
    }

    #[test]
    fn guide_pages_assembly_is_byte_identical_for_an_unchanged_source_set() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, "docs/howto/README.md", "# User Guide\n\nStart here.\n");
        write(root, "docs/howto/usage.md", "# Usage\n\nHow to use it.\n");
        let first = guide_pages(root);
        let second = guide_pages(root);
        assert_eq!(first.len(), second.len());
        for (a, b) in first.iter().zip(second.iter()) {
            assert_eq!(a.slug, b.slug);
            assert_eq!(a.body, b.body, "re-assembly is byte-identical ([FR-WK-23])");
            assert_eq!(a.anchors, b.anchors);
        }
    }

    #[test]
    fn humanize_guide_title_title_cases_and_uppercases_known_acronyms() {
        assert_eq!(humanize_guide_title("usage"), "Usage");
        assert_eq!(humanize_guide_title("error-handling"), "Error Handling");
        assert_eq!(humanize_guide_title("ci-integration"), "CI Integration");
    }

    #[test]
    fn a_stray_overview_file_is_skipped_rather_than_clobbering_the_readme_landing() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, "docs/howto/README.md", "# User Guide\n\nThe real landing page.\n");
        // ASCII-sorts after README.md, so without the reserved-name skip this would
        // last-write-wins overwrite the README-derived guide/overview page.
        write(root, "docs/howto/overview.md", "# Overview\n\nAn impostor.\n");

        let pages = guide_pages(root);
        assert_eq!(pages.len(), 1, "the stray overview.md is skipped, not presented");
        let overview = &pages[0];
        assert_eq!(overview.slug, "guide/overview");
        assert_eq!(
            overview.body, "# User Guide\n\nThe real landing page.\n",
            "README.md keeps the landing slug; overview.md never overwrites it"
        );
    }

    #[test]
    fn a_file_whose_stem_is_not_a_valid_slug_segment_is_skipped_not_fatal() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, "docs/howto/README.md", "# User Guide\n\nStart here.\n");
        // Mixed case + a space: fails validate_slug's lowercase/digit/-/_ rule. A
        // per-page write() rejection here must not abort the whole presented tier
        // (guide pages share the write loop with the Design/Specs categories).
        write(root, "docs/howto/My Guide.md", "# My Guide\n\nNot slug-safe.\n");

        let pages = guide_pages(root);
        assert_eq!(pages.len(), 1, "the invalid-slug file is omitted, not fatal");
        assert_eq!(pages[0].slug, "guide/overview");
    }

    // ── FR-WK-25 / ADR-58: presentation-layer reference transform ───────────

    #[test]
    fn stem_and_anchor_helpers_match_the_renderer_slug_rule() {
        // The anchor is the file stem (no extension) run through the renderer's
        // `toc-<slug>` rule — the contract keeping a rewritten anchor in lockstep with
        // the id `web/src/markdown.rs` actually emits.
        assert_eq!(stem_of("docs/specs/requirements/NFR-RA-01.md"), "NFR-RA-01");
        assert_eq!(stem_of("agent-core.md"), "agent-core");
        assert_eq!(stem_of("bare"), "bare");
        assert_eq!(stem_anchor("NFR-RA-01"), "toc-nfr-ra-01");
        assert_eq!(stem_anchor("agent-core"), "toc-agent-core");
        assert_eq!(renderer_slug("FR-WK-25"), "fr-wk-25");
        assert_eq!(renderer_slug("3.24 Source Wiki"), "3-24-source-wiki");
        assert_eq!(renderer_slug("!!!"), "section", "an empty slug falls back");
    }

    #[test]
    fn dir_of_glob_takes_the_directory_portion() {
        assert_eq!(dir_of_glob("docs/specs/requirements/FR-*.md"), "docs/specs/requirements");
        assert_eq!(dir_of_glob("docs/specs/architecture.md"), "docs/specs");
        assert_eq!(dir_of_glob("docs/howto/*.md"), "docs/howto");
    }

    // A fixture project with the Design/Specs categories, the Architecture page, and
    // a User Guide — enough to exercise same-page, cross-page, fragment, guide, and
    // non-presented (de-link) resolution. chat-agent.md carries the links under test.
    #[cfg(feature = "lang-markdown")]
    fn fixture(root: &Path) {
        write(
            root,
            "docs/specs/architecture.md",
            "# Architecture\n\nSee [components](architecture/components/chat-agent.md).\n",
        );
        write(
            root,
            "docs/specs/architecture/components/chat-agent.md",
            "# Chat Agent\n\nUses [agent-core](agent-core.md) and the \
             [NFR](../../requirements/NFR-RA-01.md); see [CR](../../../requests/CR-064.md).\n",
        );
        write(root, "docs/specs/architecture/components/agent-core.md", "# Agent Core\n\nCore.\n");
        write(root, "docs/specs/requirements/FR-WK-20.md", "# FR-WK-20\n\nThe tier.\n");
        write(root, "docs/specs/requirements/FR-WK-25.md", "# FR-WK-25\n\nThe transform.\n");
        write(root, "docs/specs/requirements/NFR-RA-01.md", "# NFR-RA-01\n\nAvailability.\n");
        write(root, "docs/howto/README.md", "# User Guide\n\nRead [installation](installation.md).\n");
        write(root, "docs/howto/installation.md", "# Installation\n\nInstall.\n");
    }

    /// Build the resolution manifest the way [`super::super::materialize`] does — from
    /// the assembled page set.
    #[cfg(feature = "lang-markdown")]
    fn manifest_of(root: &Path) -> Manifest {
        let pages: Vec<PresentedPage> = architecture_page(root)
            .into_iter()
            .chain(DocCategory::ALL.into_iter().filter_map(|c| consolidated_page(root, c)))
            .chain(guide_pages(root))
            .collect();
        Manifest::from_pages(&pages)
    }

    #[test]
    #[cfg(feature = "lang-markdown")]
    fn resolve_rel_normalizes_dot_and_dotdot_segments() {
        let base = "docs/specs/architecture/components";
        assert_eq!(resolve_rel(base, "agent-core.md").as_deref(), Some("docs/specs/architecture/components/agent-core.md"));
        assert_eq!(resolve_rel(base, "../../requirements/NFR-RA-01.md").as_deref(), Some("docs/specs/requirements/NFR-RA-01.md"));
        assert_eq!(resolve_rel(base, "./agent-core.md").as_deref(), Some("docs/specs/architecture/components/agent-core.md"));
        assert_eq!(resolve_rel("docs", "../../etc/passwd"), None, "escaping the repo root is unresolvable");
    }

    #[test]
    #[cfg(feature = "lang-markdown")]
    fn a_cross_page_link_becomes_the_destination_route_and_section_anchor() {
        let tmp = TempDir::new().unwrap();
        fixture(tmp.path());
        let out = rewrite_refs(
            "Uses [NFR](../../requirements/NFR-RA-01.md) here.\n",
            "docs/specs/architecture/components",
            "architecture/components",
            &manifest_of(tmp.path()),
        );
        // Only the href changes; the surrounding prose and link text are untouched.
        assert_eq!(out, "Uses [NFR](/wiki/page/specs/non-functional-requirements#toc-nfr-ra-01) here.\n");
    }

    #[test]
    #[cfg(feature = "lang-markdown")]
    fn a_same_page_link_becomes_an_in_page_anchor() {
        let tmp = TempDir::new().unwrap();
        fixture(tmp.path());
        // A link between two components (both on architecture/components) is in-page.
        let out = rewrite_refs(
            "[core](agent-core.md)\n",
            "docs/specs/architecture/components",
            "architecture/components",
            &manifest_of(tmp.path()),
        );
        assert_eq!(out, "[core](#toc-agent-core)\n");
    }

    #[test]
    #[cfg(feature = "lang-markdown")]
    fn a_link_inside_a_table_cell_is_rewritten() {
        // The Architecture/Component/Integration pages are table-heavy; tree-sitter-md
        // keeps table-cell content out of `inline` nodes, so a cell link must be parsed
        // from the `pipe_table_cell` span or it stays a dead `.md` link.
        let tmp = TempDir::new().unwrap();
        fixture(tmp.path());
        let body = "| Component | Depends on |\n| --- | --- |\n| Foo | [core](agent-core.md) |\n";
        let out = rewrite_refs(
            body,
            "docs/specs/architecture/components",
            "architecture/components",
            &manifest_of(tmp.path()),
        );
        assert!(
            out.contains("[core](#toc-agent-core)"),
            "table-cell link should resolve to the same-page anchor, got:\n{out}"
        );
        // A non-table paragraph on the same page still rewrites (no regression).
        assert!(!out.contains("(agent-core.md)"), "the raw .md href must be gone");
    }

    #[test]
    #[cfg(feature = "lang-markdown")]
    fn a_fragment_into_a_consolidated_page_resolves_to_the_documents_stem_anchor() {
        // A `path.md#frag` into a consolidated (sectioned) page targets the destination
        // document's stable stem section anchor, NOT `toc-<frag>` — the sub-heading id
        // is not stable across the renderer's cross-document uniquify ([ADR-58]).
        let tmp = TempDir::new().unwrap();
        fixture(tmp.path());
        let out = rewrite_refs(
            "[s](../../requirements/FR-WK-20.md#statement)\n",
            "docs/specs/architecture/components",
            "architecture/components",
            &manifest_of(tmp.path()),
        );
        assert_eq!(out, "[s](/wiki/page/specs/functional-requirements#toc-fr-wk-20)\n");
    }

    #[test]
    #[cfg(feature = "lang-markdown")]
    fn a_fragment_never_mis_targets_a_repeated_sub_heading_across_documents() {
        // Regression for the uniquify collision: three ADRs each with `## Consequences`
        // land, on the consolidated ADRs page, ids toc-consequences / -2 / -3. A link to
        // the THIRD ADR's Consequences must reach ADR-03 (its stem anchor), never the
        // first document's `toc-consequences`.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        // architecture.md gates SRS mode; the ADRs are the collision fixture.
        write(root, "docs/specs/architecture.md", "# Architecture\n\nx\n");
        write(root, "docs/specs/architecture/decisions/ADR-01.md", "# ADR-01\n\n## Consequences\n\nA.\n");
        write(root, "docs/specs/architecture/decisions/ADR-02.md", "# ADR-02\n\n## Consequences\n\nB.\n");
        write(
            root,
            "docs/specs/architecture/decisions/ADR-03.md",
            "# ADR-03\n\nSee [its consequences](ADR-03.md#consequences) and [ADR-01](ADR-01.md#consequences).\n\n## Consequences\n\nC.\n",
        );
        let m = manifest_of(root);
        let out = rewrite_refs(
            "See [its consequences](ADR-03.md#consequences) and [ADR-01](ADR-01.md#consequences).\n",
            "docs/specs/architecture/decisions",
            "architecture/adrs",
            &m,
        );
        // Same-page fragment links land on each document's own stem anchor — ADR-03's
        // self-reference on toc-adr-03, the ADR-01 reference on toc-adr-01 — never on a
        // shared, collision-prone `#toc-consequences`.
        assert_eq!(
            out,
            "See [its consequences](#toc-adr-03) and [ADR-01](#toc-adr-01).\n",
        );
        assert!(!out.contains("toc-consequences"), "no unstable sub-heading id: {out}");
    }

    #[test]
    #[cfg(feature = "lang-markdown")]
    fn a_same_page_fragment_link_uses_the_targets_stem_anchor() {
        // Same-page + fragment (the 4th quadrant of decide_link): `agent-core.md#detail`
        // from within the Components page → the in-page stem anchor for agent-core.
        let tmp = TempDir::new().unwrap();
        fixture(tmp.path());
        let out = rewrite_refs(
            "[d](agent-core.md#detail)\n",
            "docs/specs/architecture/components",
            "architecture/components",
            &manifest_of(tmp.path()),
        );
        assert_eq!(out, "[d](#toc-agent-core)\n");
    }

    #[test]
    #[cfg(feature = "lang-markdown")]
    fn a_fragment_into_a_single_document_guide_page_targets_the_heading_id() {
        // A guide page is one document — no cross-document uniquify — so a fragment can
        // safely deep-link to the heading id.
        let tmp = TempDir::new().unwrap();
        fixture(tmp.path());
        let out = rewrite_refs(
            "[s](installation.md#step-2)\n",
            "docs/howto",
            "guide/overview",
            &manifest_of(tmp.path()),
        );
        assert_eq!(out, "[s](/wiki/page/guide/installation#toc-step-2)\n");
    }

    #[test]
    #[cfg(feature = "lang-markdown")]
    fn a_non_presented_target_is_de_linked_to_plain_text() {
        let tmp = TempDir::new().unwrap();
        fixture(tmp.path());
        // CR-064 is never presented → the link is dropped, its visible text kept.
        let out = rewrite_refs(
            "See [CR-064](../../../requests/CR-064.md) for context.\n",
            "docs/specs/architecture/components",
            "architecture/components",
            &manifest_of(tmp.path()),
        );
        assert_eq!(out, "See CR-064 for context.\n");
    }

    #[test]
    #[cfg(feature = "lang-markdown")]
    fn external_and_empty_links_are_never_rewritten() {
        let tmp = TempDir::new().unwrap();
        fixture(tmp.path());
        let m = manifest_of(tmp.path());
        let ext = "[home](https://example.com) [mail](mailto:a@b.com) [cdn](//cdn.x/y)\n";
        assert_eq!(rewrite_refs(ext, "docs/howto", "guide/overview", &m), ext);
    }

    #[test]
    #[cfg(feature = "lang-markdown")]
    fn a_bare_fragment_is_rewritten_on_a_single_document_page_and_left_on_a_consolidated_one() {
        let tmp = TempDir::new().unwrap();
        fixture(tmp.path());
        let m = manifest_of(tmp.path());
        // On a single-document (guide) page the heading id is collision-free.
        assert_eq!(
            rewrite_refs("[jump](#the-heading)\n", "docs/howto", "guide/overview", &m),
            "[jump](#toc-the-heading)\n",
        );
        // On a consolidated page a bare fragment could collide with another document's
        // sub-heading under the renderer's uniquify, so it is left as authored.
        assert_eq!(
            rewrite_refs("[jump](#consequences)\n", "docs/specs/architecture/components", "architecture/components", &m),
            "[jump](#consequences)\n",
        );
    }

    #[test]
    #[cfg(feature = "lang-markdown")]
    fn a_de_linked_target_preserves_inline_markup_in_the_visible_text() {
        // link_visible_text keeps the raw `[...]` bytes, so formatting survives.
        let tmp = TempDir::new().unwrap();
        fixture(tmp.path());
        let out = rewrite_refs(
            "See [**the CR**](../../../requests/CR-064.md).\n",
            "docs/specs/architecture/components",
            "architecture/components",
            &manifest_of(tmp.path()),
        );
        assert_eq!(out, "See **the CR**.\n");
    }

    #[test]
    #[cfg(feature = "lang-markdown")]
    fn a_root_escaping_relative_link_is_left_untouched() {
        // resolve_rel returns None → the link is KEPT verbatim (not de-linked, not
        // routed), so a pathological `..` chain never yields a bogus target.
        let tmp = TempDir::new().unwrap();
        fixture(tmp.path());
        let body = "[x](../../../../../../etc/passwd)\n";
        assert_eq!(
            rewrite_refs(body, "docs/specs/architecture/components", "architecture/components", &manifest_of(tmp.path())),
            body,
        );
    }

    #[test]
    #[cfg(feature = "lang-markdown")]
    fn a_guide_link_becomes_the_page_route_with_no_fragment() {
        let tmp = TempDir::new().unwrap();
        fixture(tmp.path());
        // A User Guide page is one whole document — a link to it carries no anchor.
        let out = rewrite_refs(
            "Read [install](installation.md).\n",
            "docs/howto",
            "guide/overview",
            &manifest_of(tmp.path()),
        );
        assert_eq!(out, "Read [install](/wiki/page/guide/installation).\n");
    }

    #[test]
    #[cfg(feature = "lang-markdown")]
    fn links_and_images_inside_code_are_never_rewritten() {
        let tmp = TempDir::new().unwrap();
        fixture(tmp.path());
        let m = manifest_of(tmp.path());
        // An inline code span and a fenced block keep their literal text — a link or
        // image `src` inside code is not an `inline_link`/`image` node, so neither the
        // link nor the image (S-270) rewrite touches it.
        let body = "Inline `[x](agent-core.md)` and `![d](images/x.png)` stay.\n\n```\n[y](agent-core.md)\n![z](images/y.png)\n```\n";
        assert_eq!(rewrite_refs(body, "docs/specs/architecture/components", "architecture/components", &m), body);
    }

    // ── FR-WK-27 / ADR-58 / S-270: image `<img src>` asset-route rewrite ────────

    #[test]
    #[cfg(feature = "lang-markdown")]
    fn a_doc_relative_image_src_is_rewritten_to_the_asset_route() {
        // The core S-270 behavior: a diagram authored as a doc-relative image path is
        // rewritten to the same-origin asset route (resolved to its repo-relative
        // path), so the presented page's `<img>` resolves. Only the `src` changes —
        // the alt text and surrounding prose are untouched.
        let tmp = TempDir::new().unwrap();
        fixture(tmp.path());
        let out = rewrite_refs(
            "See ![the diagram](images/overview.png) here.\n",
            "docs/specs/architecture",
            "specs/architecture",
            &manifest_of(tmp.path()),
        );
        assert_eq!(
            out,
            "See ![the diagram](/api/v1/wiki/asset/docs/specs/architecture/images/overview.png) here.\n",
        );
    }

    #[test]
    #[cfg(feature = "lang-markdown")]
    fn a_dotdot_image_src_resolving_under_a_doc_root_is_rewritten() {
        // A `..`-relative image that still resolves *within* the doc roots is served —
        // resolution is structural (normalized repo-relative path), not a textual `..`
        // ban. From a component page, `../images/x.png` lands under docs/specs.
        let tmp = TempDir::new().unwrap();
        fixture(tmp.path());
        let out = rewrite_refs(
            "![d](../images/flow.svg)\n",
            "docs/specs/architecture/components",
            "architecture/components",
            &manifest_of(tmp.path()),
        );
        assert_eq!(out, "![d](/api/v1/wiki/asset/docs/specs/architecture/images/flow.svg)\n");
    }

    #[test]
    #[cfg(feature = "lang-markdown")]
    fn an_image_src_outside_the_doc_roots_is_left_untouched() {
        // An image whose resolved path escapes the doc roots (here, up to a repo-root
        // sibling) is NOT rewritten — the transform never emits an asset link the
        // sandbox would refuse; the source is left exactly as authored.
        let tmp = TempDir::new().unwrap();
        fixture(tmp.path());
        let body = "![logo](../../../assets/logo.png)\n";
        assert_eq!(
            rewrite_refs(body, "docs/specs/architecture/components", "architecture/components", &manifest_of(tmp.path())),
            body,
        );
    }

    #[test]
    #[cfg(feature = "lang-markdown")]
    fn a_root_escaping_image_src_is_left_untouched() {
        // A pathological `..` chain that escapes the repo root is unresolvable
        // (`resolve_rel` → None) and left verbatim, never yielding a bogus asset link.
        let tmp = TempDir::new().unwrap();
        fixture(tmp.path());
        let body = "![x](../../../../../../etc/secret.png)\n";
        assert_eq!(
            rewrite_refs(body, "docs/specs/architecture/components", "architecture/components", &manifest_of(tmp.path())),
            body,
        );
    }

    #[test]
    #[cfg(feature = "lang-markdown")]
    fn external_and_data_image_srcs_are_left_untouched() {
        // External (`https://`, protocol-relative) and `data:` image sources are never
        // rewritten — the self-only CSP (unchanged by this transform) governs whether
        // they load; the transform only routes doc-relative files.
        let tmp = TempDir::new().unwrap();
        fixture(tmp.path());
        let m = manifest_of(tmp.path());
        for body in [
            "![cdn](https://cdn.example.com/x.png)\n",
            "![rel](//cdn.example.com/x.png)\n",
            "![inline](data:image/png;base64,iVBORw0KGgo=)\n",
        ] {
            assert_eq!(rewrite_refs(body, "docs/specs/architecture", "specs/architecture", &m), body);
        }
    }

    #[test]
    #[cfg(feature = "lang-markdown")]
    fn the_image_rewrite_is_idempotent() {
        // Re-running the transform on its own output is a no-op: the emitted
        // `/api/v1/wiki/asset/...` path resolves to `api/...`, not a doc root, so a
        // second pass leaves it byte-identical ([NFR-RA-06]).
        let tmp = TempDir::new().unwrap();
        fixture(tmp.path());
        let m = manifest_of(tmp.path());
        let base = "docs/specs/architecture";
        let once = rewrite_refs("![d](images/x.png)\n", base, "specs/architecture", &m);
        assert_eq!(once, "![d](/api/v1/wiki/asset/docs/specs/architecture/images/x.png)\n");
        assert_eq!(rewrite_refs(&once, base, "specs/architecture", &m), once, "second pass is a no-op");
    }

    #[test]
    #[cfg(feature = "lang-markdown")]
    fn image_helpers_gate_on_extension_and_doc_root() {
        assert!(is_image_path("images/x.png"));
        assert!(is_image_path("A.JPEG"), "the extension test is case-insensitive");
        assert!(is_image_path("d/e.svg?v=2"), "a trailing query is ignored");
        assert!(!is_image_path("notes.md"), "a non-image extension is not an image");
        assert!(!is_image_path("png"), "a bare word with no dot is not an image");
        assert!(is_doc_asset_path("docs/specs/architecture/images/x.png"));
        assert!(is_doc_asset_path("docs/howto/images/y.gif"));
        assert!(!is_doc_asset_path("docs/requests/x.png"), "only the two doc roots serve assets");
        assert!(!is_doc_asset_path("assets/x.png"));
        // decide_image ties the two gates together against a resolution base.
        assert_eq!(
            decide_image("images/x.png", "docs/howto").as_deref(),
            Some("/api/v1/wiki/asset/docs/howto/images/x.png"),
        );
        assert_eq!(decide_image("images/x.png", "src").as_deref(), None, "a non-doc base is not routed");
    }

    #[test]
    #[cfg(feature = "lang-markdown")]
    fn an_absolute_image_src_resolves_from_the_repo_root_not_the_base_dir() {
        // `resolve_rel` resets to the repo root on a leading `/`, so an absolute image
        // src is judged against the doc roots from the root down (not the page's base
        // dir): a `/docs/specs/**` path is routed, a `/other/**` path is left authored.
        let tmp = TempDir::new().unwrap();
        fixture(tmp.path());
        let m = manifest_of(tmp.path());
        // Under a doc root → rewritten (base_dir is irrelevant for an absolute src).
        assert_eq!(
            rewrite_refs("![d](/docs/specs/architecture/images/x.png)\n", "docs/howto", "guide/overview", &m),
            "![d](/api/v1/wiki/asset/docs/specs/architecture/images/x.png)\n",
        );
        // Outside the doc roots → left exactly as authored.
        let outside = "![d](/other/x.png)\n";
        assert_eq!(rewrite_refs(outside, "docs/specs/architecture", "specs/architecture", &m), outside);
    }

    #[test]
    #[cfg(feature = "lang-markdown")]
    fn the_stem_anchor_is_stable_across_a_destination_title_change() {
        // The rewrite target for a link to FR-WK-20 must not change when FR-WK-20's own
        // title heading is reworded — the anchor is stem-derived, not title-derived.
        let tmp1 = TempDir::new().unwrap();
        fixture(tmp1.path());
        write(tmp1.path(), "docs/specs/requirements/FR-WK-20.md", "# FR-WK-20: the presented tier\n\nx\n");
        let tmp2 = TempDir::new().unwrap();
        fixture(tmp2.path());
        write(tmp2.path(), "docs/specs/requirements/FR-WK-20.md", "# FR-WK-20: WORDED ENTIRELY DIFFERENTLY\n\ny\n");

        let link = "See [tier](../../requirements/FR-WK-20.md).\n";
        let base = "docs/specs/architecture/components";
        let a = rewrite_refs(link, base, "architecture/components", &manifest_of(tmp1.path()));
        let b = rewrite_refs(link, base, "architecture/components", &manifest_of(tmp2.path()));
        assert_eq!(a, b, "the stem anchor does not move when the destination title changes");
        assert!(a.contains("/wiki/page/specs/functional-requirements#toc-fr-wk-20"), "{a}");
    }

    #[test]
    #[cfg(feature = "lang-markdown")]
    fn the_rewrite_is_deterministic_and_leaves_a_link_free_body_identical() {
        let tmp = TempDir::new().unwrap();
        fixture(tmp.path());
        let m = manifest_of(tmp.path());
        // A body with no rewritable link is returned byte-for-byte ([NFR-RA-06]).
        let plain = "# Title\n\nPlain prose, `code`, no links.\n";
        assert_eq!(rewrite_refs(plain, "docs/specs", "specs/x", &m), plain);
        // Same input → same output (no ordering nondeterminism across edits).
        let linky = "[a](../../requirements/NFR-RA-01.md) and [b](agent-core.md)\n";
        let base = "docs/specs/architecture/components";
        assert_eq!(
            rewrite_refs(linky, base, "architecture/components", &m),
            rewrite_refs(linky, base, "architecture/components", &m),
        );
    }

    #[test]
    #[cfg(feature = "lang-markdown")]
    fn a_reference_to_the_srs_hub_resolves_to_the_specs_srs_page() {
        // S-269: with the SRS hub in the presented set, a `software-spec.md` link and a
        // `software-spec.md#§N` fragment from another presented page both resolve to the
        // `specs/srs` page's stable stem section anchor (the hub is a sectioned page).
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, "docs/specs/software-spec.md", "# SRS\n\nThe hub.\n");
        write(root, "docs/specs/architecture.md", "# Architecture\n\nx\n");
        let pages: Vec<PresentedPage> =
            architecture_page(root).into_iter().chain(srs_page(root)).collect();
        let m = Manifest::from_pages(&pages);
        let out = rewrite_refs(
            "See the [SRS](software-spec.md) and [§3.24](software-spec.md#3-24-source-wiki).\n",
            "docs/specs",
            "overview/architecture",
            &m,
        );
        assert_eq!(
            out,
            "See the [SRS](/wiki/page/specs/srs#toc-software-spec) \
             and [§3.24](/wiki/page/specs/srs#toc-software-spec).\n",
            "both the bare hub ref and the fragment resolve to the SRS page's stem anchor"
        );
    }

    #[test]
    #[cfg(feature = "lang-markdown")]
    fn an_assembled_consolidated_page_has_its_links_rewritten_and_anchor_headings() {
        let tmp = TempDir::new().unwrap();
        fixture(tmp.path());
        let m = manifest_of(tmp.path());
        let page = consolidated_page(tmp.path(), DocCategory::Components).expect("components present");
        let body = rewrite_refs(&page.body, &page.source_dir, &page.slug, &m);
        // chat-agent's same-category link → in-page anchor; cross-category → route.
        assert!(body.contains("[agent-core](#toc-agent-core)"), "{body}");
        assert!(body.contains("[NFR](/wiki/page/specs/non-functional-requirements#toc-nfr-ra-01)"), "{body}");
        // The non-presented CR link is de-linked to plain text.
        assert!(body.contains("see CR"), "the CR link is de-linked: {body}");
        assert!(!body.contains("CR-064.md"), "no broken href survives: {body}");
        // The stem section heading the anchor targets is present, and prose survives.
        assert!(body.contains("## agent-core\n"), "{body}");
        // Each document's own title is a bolded subtitle, never a second heading
        // duplicating the stem (S-268, [FR-WK-20]) — including when the title only
        // differs from the stem in case/spacing, with no `<stem>: ` convention
        // (agent-core.md's own title is "Agent Core", not "agent-core: ...").
        assert!(body.contains("**Agent Core**"), "{body}");
        assert!(body.contains("**Chat Agent**"), "{body}");
        assert!(!body.contains("# Chat Agent\n"), "no second, duplicating heading: {body}");
        // A separator rules off each entry.
        assert!(body.contains("\n\n---\n\n"), "{body}");
    }

    #[test]
    fn a_section_is_headed_by_the_documents_own_title_not_a_duplicated_stem_heading() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(
            root,
            "docs/specs/requirements/NFR-RA-01.md",
            "# NFR-RA-01: stdout-safety invariant\n\n> Detail.\n",
        );
        let page = consolidated_page(root, DocCategory::NonFunctionalRequirements)
            .expect("NFR source is present");
        // The stem heading carries the stable anchor ([FR-WK-25])...
        assert!(page.body.contains("## NFR-RA-01\n"), "{}", page.body);
        // ...immediately followed by the document's own title, stripped of the
        // redundant `<stem>: ` prefix so nothing repeats.
        assert!(page.body.contains("## NFR-RA-01\n\n**stdout-safety invariant**\n\n"), "{}", page.body);
        assert!(!page.body.contains("NFR-RA-01: stdout-safety invariant"), "{}", page.body);
        assert!(page.body.contains("> Detail."));
    }

    #[test]
    fn a_title_identical_to_the_stem_gets_no_redundant_subtitle() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, "docs/specs/architecture/components/agent-core.md", "# agent-core\n\nCore.\n");
        let page = consolidated_page(root, DocCategory::Components).expect("Components source is present");
        assert_eq!(page.body, "# Components\n\n_Presented from `docs/specs/architecture/components/*.md` — 1 source document, assembled deterministically by Logos. Not model-generated._\n\n## agent-core\n\nCore.\n");
    }

    #[test]
    fn multiple_entries_are_ruled_off_by_a_separator() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, "docs/specs/requirements/FR-WK-01.md", "# FR-WK-01\n\nFirst.\n");
        write(root, "docs/specs/requirements/FR-WK-02.md", "# FR-WK-02\n\nSecond.\n");
        let page = consolidated_page(root, DocCategory::FunctionalRequirements).expect("FR sources are present");
        let one = page.body.find("## FR-WK-01\n").expect("FR-01 section");
        let sep = page.body.find("\n\n---\n\n").expect("a separator between entries");
        let two = page.body.find("## FR-WK-02\n").expect("FR-02 section");
        assert!(one < sep && sep < two, "the separator sits between the two entries: {}", page.body);
    }
}
