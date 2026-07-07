//! Behavioural integration test for the markdown documentation layer (S-033),
//! driving the plugin substrate and extraction engine through their public API.
//!
//! Covers the S-033 acceptance criteria that need a real parse:
//! - the markdown grammar loads and is listed by `logos languages`
//!   ([FR-DG-01](../../docs/specs/requirements/FR-DG-01.md),
//!   [FR-PL-06](../../docs/specs/requirements/FR-PL-06.md));
//! - a sample doc parses into one `DocFile` with nested `DocSection` nodes whose
//!   ids follow `path#heading-slug`
//!   ([FR-DG-02](../../docs/specs/requirements/FR-DG-02.md));
//! - re-indexing the unchanged doc is byte-identical and an unrelated heading
//!   edit does not churn a sibling's id
//!   ([NFR-RA-06](../../docs/specs/requirements/NFR-RA-06.md)).
//!
//! Gated on `lang-markdown` so a `--no-default-features` build that excludes the
//! markdown grammar does not run these markdown-specific assertions.
#![cfg(feature = "lang-markdown")]

use std::fs;
use std::path::Path;

use logos_core::extract::doc::extract_doc;
use logos_core::extract::{FileInput, SymbolContext};
use logos_core::model::{EdgeKind, NodeKind};
use logos_core::plugin::LanguageRegistry;
use logos_core::{Engine, Runtime};

const SAMPLE: &str = "\
# Overview

intro prose

## Goals

### Non-goals

## Design
";

#[test]
fn markdown_grammar_loads_and_is_listed_by_languages() {
    // Listed through the same `languages` read-model `logos languages` prints
    // (FR-PL-06): a transient engine loads the registry on demand.
    let tmp = tempfile::tempdir().unwrap();
    let info = Engine::open(tmp.path()).languages();
    let md = info
        .languages
        .iter()
        .find(|d| d.name == "markdown")
        .expect("`logos languages` must list the markdown grammar (FR-DG-01)");
    assert!(
        md.extensions.iter().any(|e| e == "md") && md.extensions.iter().any(|e| e == "markdown"),
        "markdown claims .md/.markdown: {:?}",
        md.extensions
    );
    assert_eq!(md.abi_version, 15, "the markdown block grammar is ABI 15");

    // The registry exposes it as a documentation plugin with the .md extension.
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
    let plugin = reg.for_extension("md").expect("markdown claims .md");
    assert_eq!(plugin.name(), "markdown");
    assert!(
        plugin.is_documentation(),
        "markdown is registered as a documentation plugin (ADR-19)"
    );
}

#[test]
fn a_sample_doc_parses_into_a_docfile_and_nested_docsections() {
    let tmp = tempfile::tempdir().unwrap();
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
    let plugin = reg.for_extension("md").expect("markdown grammar");
    let ctx = SymbolContext::default();

    let facts = extract_doc(&FileInput::new("docs/spec.md", SAMPLE), plugin, &ctx);

    // One DocFile, four DocSections (Overview, Goals, Non-goals, Design).
    assert_eq!(
        facts
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::DocFile)
            .count(),
        1
    );
    let names: Vec<&str> = facts
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::DocSection)
        .map(|n| n.name.as_str())
        .collect();
    assert_eq!(names.len(), 4, "one DocSection per heading: {names:?}");

    // The heading hierarchy is materialised as Contains edges: Overview → Goals
    // → Non-goals, Overview → Design.
    let sym = |name: &str| {
        facts
            .nodes
            .iter()
            .find(|n| n.name == name)
            .map(|n| n.symbol.as_str())
            .unwrap()
    };
    let edge = |parent: &str, child: &str| {
        facts.edges.iter().any(|e| {
            e.kind == EdgeKind::Contains
                && e.source.as_str() == sym(parent)
                && e.target.as_str() == sym(child)
        })
    };
    assert!(
        edge("spec.md", "Overview"),
        "DocFile contains the top section"
    );
    assert!(edge("Overview", "Goals"));
    assert!(edge("Goals", "Non-goals"));
    assert!(edge("Overview", "Design"));

    // Ids follow the nested `path#heading-slug` anchor scheme (FR-DG-02).
    assert!(sym("Overview").ends_with("overview#"));
    assert!(sym("Non-goals").ends_with("overview#goals#non-goals#"));
}

#[test]
fn re_index_is_byte_identical_and_unrelated_edits_do_not_churn_ids() {
    let tmp = tempfile::tempdir().unwrap();
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
    let plugin = reg.for_extension("md").expect("markdown grammar");
    let ctx = SymbolContext::default();
    let run = |src: &str| extract_doc(&FileInput::new("docs/spec.md", src), plugin, &ctx);

    // Byte-identical re-index (NFR-RA-06).
    assert_eq!(run(SAMPLE), run(SAMPLE));

    // Renaming `## Goals` must not churn the unrelated `## Design` id.
    let before = run(SAMPLE);
    let after = run(&SAMPLE.replace("## Goals", "## Objectives"));
    let id = |facts: &logos_core::extract::Facts, name: &str| {
        facts
            .nodes
            .iter()
            .find(|n| n.name == name)
            .map(|n| n.symbol.to_scip_string())
    };
    assert_eq!(
        id(&before, "Design"),
        id(&after, "Design"),
        "an unrelated heading rename must not churn the Design section id"
    );
}

// ── S-034: documentation discovery and ingestion through the pipeline ─────────
//
// These drive the S-034 acceptance criteria end-to-end through the public
// `Engine` façade — markdown rides the same discover → extract → persist
// pipeline and blake3 sync as code (FR-DG-01, FR-IX-02, FR-CF-01).

/// Write `contents` to `<root>/<rel>`, creating parent directories.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    fs::write(path, contents).expect("write fixture");
}

/// The human-facing names of every node of `kind` currently in the graph.
fn node_names(rt: &Runtime, kind: NodeKind) -> Vec<String> {
    rt.submit_read(move |store| {
        Ok(store
            .all_nodes()?
            .into_iter()
            .filter(|n| n.kind == kind)
            .map(|n| n.name)
            .collect())
    })
    .expect("read runs")
}

/// The project-relative paths of every indexed file.
fn indexed_paths(rt: &Runtime) -> Vec<String> {
    rt.submit_read(|store| Ok(store.indexed_files()?.into_iter().map(|f| f.path).collect()))
        .expect("read runs")
}

#[test]
fn index_admits_only_markdown_under_the_default_doc_globs() {
    // AC1: a repo with docs/**/*.md and a top-level README.md produces doc nodes;
    // an `.md` outside the default globs is not indexed (FR-DG-01, FR-IX-02).
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "docs/guide.md", "# Guide\n\n## Setup\n");
    write(root, "README.md", "# Readme\n\n## Install\n");
    write(root, "notes/scratch.md", "# Scratch\n\n## Secret\n");

    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let rt = engine.runtime().expect("runtime present");

    let files = indexed_paths(rt);
    assert!(
        files.contains(&"docs/guide.md".to_string()),
        "got: {files:?}"
    );
    assert!(files.contains(&"README.md".to_string()), "got: {files:?}");
    assert!(
        !files.contains(&"notes/scratch.md".to_string()),
        "an out-of-glob .md must not be indexed: {files:?}"
    );

    // The admitted files produced DocFile + DocSection nodes; the excluded one
    // contributed nothing (its `Secret` heading never enters the graph).
    let doc_files = node_names(rt, NodeKind::DocFile);
    assert!(
        doc_files.contains(&"guide.md".to_string()),
        "got: {doc_files:?}"
    );
    assert!(
        doc_files.contains(&"README.md".to_string()),
        "got: {doc_files:?}"
    );
    let sections = node_names(rt, NodeKind::DocSection);
    assert!(sections.contains(&"Setup".to_string()), "got: {sections:?}");
    assert!(
        sections.contains(&"Install".to_string()),
        "got: {sections:?}"
    );
    assert!(
        !sections.contains(&"Secret".to_string()),
        "out-of-glob doc sections must not be indexed: {sections:?}"
    );
}

#[test]
fn editing_one_doc_and_syncing_updates_only_that_files_nodes() {
    // AC2: editing one doc and re-running sync updates only that file's doc nodes
    // — documentation rides blake3 dirty-detection like code (FR-DG-01).
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "docs/guide.md", "# Guide\n\n## Setup\n");
    write(root, "README.md", "# Readme\n\n## Install\n");

    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let rt = engine.runtime().expect("runtime present");

    // Edit only docs/guide.md (rename its section); README.md is untouched.
    write(root, "docs/guide.md", "# Guide\n\n## Configuration\n");
    let result = engine.sync(&[std::path::PathBuf::from("docs/guide.md")]);
    assert_eq!(
        result.files_modified, 1,
        "exactly the edited doc re-extracted"
    );

    let sections = node_names(rt, NodeKind::DocSection);
    assert!(
        sections.contains(&"Configuration".to_string()),
        "the edited doc's new section is present: {sections:?}"
    );
    assert!(
        !sections.contains(&"Setup".to_string()),
        "the edited doc's old section is gone: {sections:?}"
    );
    assert!(
        sections.contains(&"Install".to_string()),
        "the untouched doc's section is unchanged: {sections:?}"
    );

    // Re-sync BOTH docs with no further edits: blake3 dirty-detection skips both
    // (the unchanged README is submitted explicitly, not merely omitted), proving
    // docs ride the same hash-compare skip path as code — no re-extraction.
    let resync = engine.sync(&[
        std::path::PathBuf::from("docs/guide.md"),
        std::path::PathBuf::from("README.md"),
    ]);
    assert_eq!(
        resync.files_modified, 0,
        "unchanged docs are not re-extracted"
    );
    assert_eq!(resync.files_added, 0);
}

#[test]
fn disabling_documentation_in_config_produces_no_doc_nodes() {
    // AC3: disabling documentation in config.toml produces no doc nodes; code is
    // unaffected (FR-CF-01).
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        root,
        ".logos/config.toml",
        "[documentation]\nenabled = false\n",
    );
    write(root, "docs/guide.md", "# Guide\n\n## Setup\n");
    write(root, "README.md", "# Readme\n");
    // A code file alongside the docs: disabling documentation must leave code
    // indexing untouched.
    write(root, "src/lib.rs", "pub fn helper() {}\n");

    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let rt = engine.runtime().expect("runtime present");

    assert!(
        node_names(rt, NodeKind::DocFile).is_empty(),
        "no DocFile nodes when documentation is disabled"
    );
    assert!(
        node_names(rt, NodeKind::DocSection).is_empty(),
        "no DocSection nodes when documentation is disabled"
    );
    let files = indexed_paths(rt);
    assert!(
        !files.iter().any(|p| p.ends_with(".md")),
        "no markdown is indexed when documentation is disabled: {files:?}"
    );
    // Code indexing is independent of the documentation toggle.
    #[cfg(feature = "lang-rust")]
    assert!(
        files.contains(&"src/lib.rs".to_string()),
        "code is still indexed with documentation disabled: {files:?}"
    );
}

#[test]
fn deleting_a_doc_removes_its_nodes_even_when_no_longer_admitted() {
    // A previously-indexed doc that is deleted from disk must be reconciled out
    // of the graph even if the config no longer admits it (e.g. documentation was
    // disabled after the index). Without the removal check preceding the
    // admission gate, the deleted doc's nodes would orphan until a full re-index.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "docs/guide.md", "# Guide\n\n## Setup\n");

    // Index with documentation on (default) — the doc nodes are created.
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let rt = engine.runtime().expect("runtime present");
    assert!(
        indexed_paths(rt).contains(&"docs/guide.md".to_string()),
        "the doc is indexed up front"
    );

    // Disable documentation, then delete the doc and sync its path.
    write(
        root,
        ".logos/config.toml",
        "[documentation]\nenabled = false\n",
    );
    fs::remove_file(root.join("docs/guide.md")).expect("remove the doc");
    let result = engine.sync(&[std::path::PathBuf::from("docs/guide.md")]);

    assert_eq!(result.files_removed, 1, "the deleted doc is reconciled out");
    assert!(
        !indexed_paths(rt).contains(&"docs/guide.md".to_string()),
        "the deleted doc's file row is gone"
    );
    assert!(
        node_names(rt, NodeKind::DocSection).is_empty(),
        "the deleted doc's sections are gone, not orphaned"
    );
}

#[test]
fn overriding_the_doc_globs_is_honoured() {
    // AC3: the globs are overridable. Scope docs to guide/** and assert the new
    // set is honoured — the default docs/ + README scoping no longer applies.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        root,
        ".logos/config.toml",
        "[documentation]\ninclude = [\"guide/**/*.md\"]\n",
    );
    write(root, "guide/intro.md", "# Intro\n\n## First Steps\n");
    write(root, "docs/spec.md", "# Spec\n\n## Details\n");
    write(root, "README.md", "# Readme\n");

    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let rt = engine.runtime().expect("runtime present");

    let files = indexed_paths(rt);
    assert_eq!(
        files,
        vec!["guide/intro.md".to_string()],
        "only the overridden glob is honoured: {files:?}"
    );
    let sections = node_names(rt, NodeKind::DocSection);
    assert!(
        sections.contains(&"First Steps".to_string()),
        "got: {sections:?}"
    );
    assert!(
        !sections.contains(&"Details".to_string()),
        "docs/ is no longer in scope after the override: {sections:?}"
    );
}
