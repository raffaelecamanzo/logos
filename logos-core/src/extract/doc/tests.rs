//! Behavioural unit tests for structural documentation extraction (S-033),
//! exercised against the real `tree-sitter-md` grammar. Gated on `lang-markdown`
//! (see the `cfg` at the `mod tests` declaration in `doc.rs`).
//!
//! These cover the story's acceptance criteria:
//! - a doc parses into one `DocFile` with a nested `DocSection` tree whose ids
//!   follow `path#heading-slug` ([FR-DG-01], [FR-DG-02]);
//! - re-extracting the unchanged doc is byte-identical ([NFR-RA-06]);
//! - an unrelated heading edit does not churn another section's id ([FR-DG-02]).

use super::*;
use crate::extract::SymbolContext;
use crate::model::{EdgeKind, NodeKind, RefForm};
use crate::plugin::LanguageRegistry;

/// Load the registry with the embedded grammars (markdown included by default).
fn registry() -> LanguageRegistry {
    let tmp = tempfile::tempdir().expect("tempdir");
    LanguageRegistry::load(tmp.path()).expect("embedded grammars load")
}

/// Extract one in-memory markdown source at `path`.
fn extract_md(path: &str, source: &str) -> Facts {
    let reg = registry();
    let plugin = reg.for_extension("md").expect("markdown grammar is loaded");
    assert!(
        plugin.is_documentation(),
        "the markdown plugin must be a documentation plugin"
    );
    let ctx = SymbolContext::default();
    extract_doc(&FileInput::new(path, source), plugin, &ctx)
}

/// The symbol string of the (first) node named `name`, if present.
fn symbol_of<'a>(facts: &'a Facts, name: &str) -> Option<&'a str> {
    facts
        .nodes
        .iter()
        .find(|n| n.name == name)
        .map(|n| n.symbol.as_str())
}

/// `true` if `facts` carries a Contains edge from the node named `parent` to the
/// node named `child`.
fn contains(facts: &Facts, parent: &str, child: &str) -> bool {
    let p = symbol_of(facts, parent);
    let c = symbol_of(facts, child);
    match (p, c) {
        (Some(p), Some(c)) => facts.edges.iter().any(|e| {
            e.kind == EdgeKind::Contains && e.source.as_str() == p && e.target.as_str() == c
        }),
        _ => false,
    }
}

const GUIDE: &str = "\
# Title

intro prose

## Setup

setup prose

## Usage

### Details

deep prose
";

#[test]
fn parses_into_a_docfile_and_nested_docsections() {
    let facts = extract_md("docs/guide.md", GUIDE);

    // Exactly one DocFile, and a DocSection per heading (Title, Setup, Usage,
    // Details) — FR-DG-02.
    let doc_files: Vec<_> = facts
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::DocFile)
        .collect();
    assert_eq!(doc_files.len(), 1, "exactly one DocFile per file");
    assert_eq!(doc_files[0].name, "guide.md");

    let sections: Vec<_> = facts
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::DocSection)
        .map(|n| n.name.as_str())
        .collect();
    assert_eq!(sections.len(), 4, "one DocSection per heading");
    for h in ["Title", "Setup", "Usage", "Details"] {
        assert!(sections.contains(&h), "missing DocSection for '{h}'");
    }

    // The heading hierarchy is the Contains tree: DocFile → Title → {Setup,
    // Usage}; Usage → Details.
    assert!(contains(&facts, "guide.md", "Title"));
    assert!(contains(&facts, "Title", "Setup"));
    assert!(contains(&facts, "Title", "Usage"));
    assert!(contains(&facts, "Usage", "Details"));
}

/// FR-DG-05 / S-037: a `DocSection`'s `body` holds its own prose beneath the
/// heading, and **excludes** nested sub-section prose (that is body-indexed
/// under the sub-section). A section that is pure heading scaffolding carries
/// no body (`None`, not an empty string).
#[test]
fn docsection_body_captures_own_prose_excluding_subsections() {
    let facts = extract_md("docs/guide.md", GUIDE);

    let body_of = |name: &str| -> Option<String> {
        facts
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::DocSection && n.name == name)
            .and_then(|n| n.body.clone())
    };

    // Title has its OWN prose ("intro prose") and child sections (Setup, Usage):
    // its body is only its own prose, never the children's — the hardest
    // "exclude sub-section" case (FR-DG-05).
    assert_eq!(
        body_of("Title").as_deref(),
        Some("intro prose"),
        "a parent with own prose AND children indexes only its own prose"
    );

    // Setup is a leaf section: its body is exactly its own prose.
    assert_eq!(body_of("Setup").as_deref(), Some("setup prose"));

    // Usage owns "Details" as a sub-section; Details' "deep prose" must NOT leak
    // into Usage's body. Usage has no prose of its own → no body.
    assert_eq!(
        body_of("Usage"),
        None,
        "a heading-only section carries no body; sub-section prose stays out"
    );
    assert_eq!(body_of("Details").as_deref(), Some("deep prose"));

    // The DocFile root and code/module nodes never carry a body.
    assert!(facts
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::DocFile)
        .all(|n| n.body.is_none()));
}

#[test]
fn section_ids_follow_path_heading_slug() {
    let facts = extract_md("docs/guide.md", GUIDE);

    // The DocFile id is the path rendered as a namespace (the `path` half).
    let doc_file = symbol_of(&facts, "guide.md").unwrap();
    assert!(
        doc_file.contains("docs/") && doc_file.contains("guide.md"),
        "DocFile id carries the file path: {doc_file}"
    );

    // A top-level section appends its heading slug as the `#`-suffixed anchor
    // descriptor: `…/guide.md/title#`.
    let title = symbol_of(&facts, "Title").unwrap();
    assert!(title.ends_with("title#"), "Title id is path#slug: {title}");

    // A nested section chains the slugs, so Details renders the full
    // `title#usage#details#` heading path — the deterministic nested anchor.
    let details = symbol_of(&facts, "Details").unwrap();
    assert!(
        details.ends_with("title#usage#details#"),
        "Details id chains the heading-slug path: {details}"
    );
}

#[test]
fn duplicate_sibling_slugs_disambiguate_by_ordinal() {
    // Two sibling `## Notes` headings slugify identically; ADR-07 ordinal-within-
    // parent disambiguation gives them distinct ids (the second gets a trailing
    // ordinal meta), so both DocSection nodes exist with unique symbols.
    let src = "# Top\n\n## Notes\n\nfirst\n\n## Notes\n\nsecond\n";
    let facts = extract_md("docs/dup.md", src);

    let notes: Vec<&str> = facts
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::DocSection && n.name == "Notes")
        .map(|n| n.symbol.as_str())
        .collect();
    assert_eq!(
        notes.len(),
        2,
        "both duplicate-slug headings become sections"
    );
    assert_ne!(
        notes[0], notes[1],
        "duplicate sibling slugs get distinct ids"
    );
    assert!(
        notes.iter().any(|s| s.ends_with("notes#")) && notes.iter().any(|s| s.contains("notes#1:")),
        "the second sibling carries the ordinal meta: {notes:?}"
    );
}

#[test]
fn re_extracting_an_unchanged_doc_is_byte_identical() {
    // Determinism (NFR-RA-06): the same input yields byte-identical facts.
    let a = extract_md("docs/guide.md", GUIDE);
    let b = extract_md("docs/guide.md", GUIDE);
    assert_eq!(a, b);
}

#[test]
fn an_unrelated_heading_edit_does_not_churn_a_sibling_id() {
    // FR-DG-02 / NFR-RA-03: editing one heading's text changes only that
    // section's id (and its descendants'), never an unrelated section's.
    let before = extract_md("docs/guide.md", GUIDE);

    // Rename `## Setup` → `## Installation`; Usage and its child Details are
    // unrelated and must keep byte-identical ids.
    let edited = GUIDE.replace("## Setup", "## Installation");
    let after = extract_md("docs/guide.md", &edited);

    for unaffected in ["Usage", "Details", "Title"] {
        assert_eq!(
            symbol_of(&before, unaffected),
            symbol_of(&after, unaffected),
            "'{unaffected}' id churned on an unrelated heading edit"
        );
    }
    // The edited heading itself naturally has a new id and name.
    assert!(symbol_of(&after, "Setup").is_none());
    assert!(symbol_of(&after, "Installation").is_some());
}

#[test]
fn a_doc_with_no_headings_yields_only_a_docfile() {
    // Prose-only README: no headings → no DocSections, just the DocFile
    // (FR-DG-02 "each heading is a DocSection" — none here).
    let facts = extract_md("README.md", "just some prose\n\nand more prose\n");
    assert_eq!(
        facts
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::DocFile)
            .count(),
        1
    );
    assert_eq!(
        facts
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::DocSection)
            .count(),
        0
    );
    assert!(facts.edges.is_empty(), "no Contains edges without headings");
}

#[test]
fn punctuation_only_heading_uses_the_empty_slug_fallback() {
    // A heading that slugifies to empty (only punctuation) must still get a
    // valid, stable identity via the EMPTY_SLUG substitution (doc.rs), not an
    // empty/invalid descriptor — exercised here end-to-end through the parse.
    let facts = extract_md("docs/x.md", "# ***\n\ntext\n");
    let sections: Vec<&str> = facts
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::DocSection)
        .map(|n| n.symbol.as_str())
        .collect();
    assert_eq!(
        sections.len(),
        1,
        "the punctuation-only heading is a section"
    );
    assert!(
        sections[0].ends_with("section#"),
        "punctuation-only heading falls back to EMPTY_SLUG: {}",
        sections[0]
    );
}

#[test]
fn slugify_is_github_style_and_deterministic() {
    assert_eq!(slugify("Hello World"), "hello-world");
    assert_eq!(slugify("Introduction & Setup"), "introduction-setup");
    assert_eq!(slugify("API (v2)"), "api-v2");
    assert_eq!(slugify("  Trailing spaces  "), "trailing-spaces");
    assert_eq!(slugify("Already-Hyphenated"), "already-hyphenated");
    // Punctuation-only slugifies to empty (the caller substitutes EMPTY_SLUG).
    assert_eq!(slugify("***"), "");
}

#[test]
fn heading_slug_applies_the_empty_slug_fallback() {
    assert_eq!(heading_slug("Setup"), "setup");
    assert_eq!(heading_slug("***"), EMPTY_SLUG);
}

// ── Reference extraction (S-035, FR-DG-03/FR-DG-04) ───────────────────────────

/// The refs of `facts` as `(target, form)` pairs — the ledger-shaping fields a
/// `DocReference` carries.
fn doc_refs(facts: &Facts) -> Vec<(&str, RefForm)> {
    facts
        .refs
        .iter()
        .filter(|r| r.kind == EdgeKind::DocReference)
        .map(|r| (r.target.as_str(), r.form))
        .collect()
}

#[test]
fn markdown_links_become_path_form_doc_references() {
    let src = "\
# Guide

See [the requirement](../specs/FR-DG-03.md#statement) and the
[overview](overview.md) for details.
";
    let facts = extract_md("docs/guide.md", src);
    let refs = doc_refs(&facts);
    assert!(
        refs.contains(&("../specs/FR-DG-03.md#statement", RefForm::Path)),
        "a link with an anchor is a path-form doc reference: {refs:?}"
    );
    assert!(
        refs.contains(&("overview.md", RefForm::Path)),
        "a plain doc link is a path-form doc reference: {refs:?}"
    );
    // Every doc reference is attributed to the enclosing section, never fabricated.
    assert!(facts.refs.iter().all(|r| r.kind == EdgeKind::DocReference));
}

#[test]
fn external_links_are_not_references() {
    let src = "\
# Links

[home](https://example.com), [mail](mailto:a@b.com), and
[scheme-relative](//cdn.example.com/x) are not graph references.
";
    let facts = extract_md("docs/links.md", src);
    assert!(
        doc_refs(&facts).is_empty(),
        "external/URL links must not become references: {:?}",
        doc_refs(&facts)
    );
}

#[test]
fn inline_code_identifier_is_a_method_form_doc_reference() {
    let src = "# API\n\nCall `extract_files` to walk the tree, then `Vec::new`.\n";
    let facts = extract_md("docs/api.md", src);
    let refs = doc_refs(&facts);
    // A bare identifier resolves by name; a `::` path binds by its last segment.
    assert!(
        refs.contains(&("extract_files", RefForm::Method)),
        "inline-code identifier is a name reference: {refs:?}"
    );
    assert!(
        refs.contains(&("new", RefForm::Method)),
        "the last `::` segment is the bound name: {refs:?}"
    );
}

#[test]
fn inline_code_repo_path_is_a_path_form_doc_reference() {
    let src = "# Files\n\nThe walk lives in `logos-core/src/extract/doc.rs`.\n";
    let facts = extract_md("docs/files.md", src);
    assert!(
        doc_refs(&facts).contains(&("logos-core/src/extract/doc.rs", RefForm::Path)),
        "an explicit repo path is a path-form reference: {:?}",
        doc_refs(&facts)
    );
}

#[test]
fn code_span_inside_link_text_is_not_a_separate_reference() {
    // A code span that is a link's display text is styling, not a freestanding
    // code mention: only the link destination is a reference (review-fix CQ-1).
    let src = "# L\n\nSee [`extract_files`](api.md) for the walker.\n";
    let facts = extract_md("docs/l.md", src);
    let refs = doc_refs(&facts);
    assert!(
        refs.contains(&("api.md", RefForm::Path)),
        "the link destination is a reference: {refs:?}"
    );
    assert!(
        !refs.iter().any(|(t, _)| *t == "extract_files"),
        "the link's code-styled anchor text is NOT a code reference: {refs:?}"
    );
}

#[test]
fn bare_filename_token_resolves_as_a_path_not_an_extension_name() {
    // A slash-less filename with a known extension is a path reference, never a
    // bogus last-segment name like `md` (review-fix CQ-2).
    let facts = extract_md("docs/f.md", "# F\n\nSee `README.md` at the root.\n");
    let refs = doc_refs(&facts);
    assert!(
        refs.contains(&("README.md", RefForm::Path)),
        "a bare filename is a path-form reference: {refs:?}"
    );
    assert!(
        !refs.iter().any(|(t, _)| *t == "md"),
        "the extension must not become a name reference: {refs:?}"
    );
}

#[test]
fn prose_and_multiword_code_spans_are_never_references() {
    // A multi-word inline-code span is a code *sample*, not a token; an operator
    // or flag is not a plausible identifier. None may become a candidate.
    let src = "# Prose\n\nRun `cargo build --release` or pass `--flag`; see `a b c`.\n";
    let facts = extract_md("docs/prose.md", src);
    assert!(
        doc_refs(&facts).is_empty(),
        "multi-word / non-identifier code spans are not references: {:?}",
        doc_refs(&facts)
    );
}

#[test]
fn single_token_fenced_block_is_a_doc_reference() {
    let src = "# Fenced\n\n```\nextract_files\n```\n";
    let facts = extract_md("docs/fenced.md", src);
    assert!(
        doc_refs(&facts).contains(&("extract_files", RefForm::Method)),
        "a single-token fenced block names a symbol: {:?}",
        doc_refs(&facts)
    );

    // A multi-line code sample is not a single reference.
    let sample = "# Sample\n\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n";
    assert!(
        doc_refs(&extract_md("docs/sample.md", sample)).is_empty(),
        "a multi-line fenced sample is not a reference"
    );
}

#[test]
fn references_are_attributed_to_the_enclosing_section() {
    let src = "\
# Top

links to [a](a.md).

## Child

links to [b](b.md).
";
    let facts = extract_md("docs/attr.md", src);
    let top = symbol_of(&facts, "Top").unwrap();
    let child = symbol_of(&facts, "Child").unwrap();
    let a = facts.refs.iter().find(|r| r.target == "a.md").unwrap();
    let b = facts.refs.iter().find(|r| r.target == "b.md").unwrap();
    assert_eq!(a.source.as_str(), top, "the `a.md` link belongs to Top");
    assert_eq!(b.source.as_str(), child, "the `b.md` link belongs to Child");
}

#[test]
fn preamble_links_are_attributed_to_the_doc_file() {
    // Content before the first heading is the file preamble; its links belong to
    // the DocFile, not to any section.
    let src = "see [readme](README.md)\n\n# First\n\nbody\n";
    let facts = extract_md("docs/pre.md", src);
    let doc_file = symbol_of(&facts, "pre.md").unwrap();
    let r = facts.refs.iter().find(|r| r.target == "README.md").unwrap();
    assert_eq!(
        r.source.as_str(),
        doc_file,
        "a preamble link belongs to the DocFile"
    );
}

#[test]
fn reference_extraction_is_byte_identical_across_runs() {
    // NFR-RA-06: refs are deterministic and canonically ordered.
    let src = "# T\n\n[a](a.md) `sym` [b](b.md#h) `mod::thing`\n";
    assert_eq!(extract_md("docs/d.md", src), extract_md("docs/d.md", src));
}
