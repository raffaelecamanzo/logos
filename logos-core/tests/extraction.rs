//! Integration tests for the extraction engine (S-007), driving the public API
//! exactly as the pipeline-orchestrator (S-010) will.
//!
//! Covers the two acceptance criteria that need real files end-to-end:
//! - a file with a syntax error is partially extracted, never aborting the run
//!   ([FR-IX-04](../../docs/specs/requirements/FR-IX-04.md),
//!   [UAT-IX-02](../../docs/specs/requirements/UAT-IX-02.md));
//! - the engine dogfoods on Logos's own source tree, producing stable, unique
//!   symbol IDs and per-function metrics
//!   ([UAT-EX-02](../../docs/specs/requirements/UAT-EX-02.md),
//!   [UAT-EX-03](../../docs/specs/requirements/UAT-EX-03.md)).
//!
//! Gated on `lang-rust`: integration tests share the crate's feature set, and a
//! `--no-default-features` build excludes the Rust grammar these tests need.
#![cfg(feature = "lang-rust")]

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::extract::{extract, extract_files, FileInput, SymbolContext};
use logos_core::plugin::LanguageRegistry;

/// Load the registry with the embedded Rust grammar, no on-disk overrides.
fn registry() -> LanguageRegistry {
    let tmp = tempfile::tempdir().expect("tempdir");
    LanguageRegistry::load(tmp.path()).expect("embedded grammars load")
}

#[test]
fn a_syntax_error_yields_a_partial_extraction_not_an_abort() {
    // The middle function is malformed; tree-sitter recovers and the well-formed
    // declarations on BOTH sides of the break are still extracted (FR-IX-04,
    // UAT-IX-02).
    let broken = "\
fn good_one() {}

fn broken() -> { }

fn good_two() {}
";
    let reg = registry();
    let plugin = reg.for_extension("rs").unwrap();
    let facts = extract(
        &FileInput::new("src/broken.rs", broken),
        plugin,
        &SymbolContext::cargo("logos-core", "0.1.0"),
    );

    assert!(facts.partial, "a syntax error must mark the file partial");
    assert!(
        !facts.warnings.is_empty(),
        "a partial parse records a warning"
    );
    let names: Vec<&str> = facts.nodes.iter().map(|n| n.name.as_str()).collect();
    assert!(
        names.contains(&"good_one"),
        "the declaration before the break is still extracted: {names:?}"
    );
    assert!(
        names.contains(&"good_two"),
        "the declaration after the break is still extracted: {names:?}"
    );
}

#[test]
fn a_broken_file_does_not_abort_a_multi_file_run() {
    let reg = registry();
    let ctx = SymbolContext::cargo("logos-core", "0.1.0");
    let inputs = vec![
        FileInput::new("src/a.rs", "fn a() {}\n"),
        FileInput::new("src/broken.rs", "fn broken( {\n"),
        FileInput::new("src/b.rs", "fn b() {}\n"),
    ];

    let out = extract_files(&inputs, &reg, &ctx);
    assert_eq!(out.len(), 3, "all three files produce facts");

    let by_path = |p: &str| out.iter().find(|f| f.path == p).unwrap();
    assert!(!by_path("src/a.rs").partial, "clean file is not partial");
    assert!(by_path("src/broken.rs").partial, "broken file is partial");
    assert!(!by_path("src/b.rs").partial, "clean file is not partial");
    // The clean files around the broken one extracted normally.
    assert!(by_path("src/a.rs").nodes.iter().any(|n| n.name == "a"));
    assert!(by_path("src/b.rs").nodes.iter().any(|n| n.name == "b"));
}

/// Recursively collect every `.rs` file under `dir`, returned as `FileInput`s
/// with a path relative to `root`.
fn collect_rust_sources(root: &Path, dir: &Path, out: &mut Vec<FileInput>) {
    for entry in fs::read_dir(dir).expect("readable dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect_rust_sources(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let source = fs::read_to_string(&path).expect("readable source");
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push(FileInput::new(rel, source));
        }
    }
}

#[test]
fn dogfoods_on_logos_own_source_tree() {
    // The first dogfood milestone: extract logos-core's own `src/` tree and
    // assert the output is well-formed (UAT-EX-02, UAT-EX-03).
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src = crate_root.join("src");

    let mut inputs = Vec::new();
    collect_rust_sources(&crate_root, &src, &mut inputs);
    assert!(
        inputs.len() >= 10,
        "expected to discover logos-core's source files, found {}",
        inputs.len()
    );

    let reg = registry();
    let ctx = SymbolContext::cargo("logos-core", "0.1.0");
    let results = extract_files(&inputs, &reg, &ctx);
    assert_eq!(results.len(), inputs.len(), "every .rs file is extracted");

    let total_nodes: usize = results.iter().map(|f| f.nodes.len()).sum();
    let total_edges: usize = results.iter().map(|f| f.edges.len()).sum();
    assert!(
        total_nodes > 100,
        "the source tree should yield many nodes, got {total_nodes}"
    );
    assert!(
        total_edges > 0,
        "nested declarations should yield Contains edges"
    );

    // Every extracted function/method carries complexity + line counts, and
    // every symbol is globally unique across the tree (FR-EX-02..04).
    let mut symbols: HashSet<String> = HashSet::new();
    for facts in &results {
        for node in &facts.nodes {
            assert!(
                symbols.insert(node.symbol.as_str().to_string()),
                "duplicate symbol id across the tree: {}",
                node.symbol.as_str()
            );
            if matches!(
                node.kind,
                logos_core::model::NodeKind::Function | logos_core::model::NodeKind::Method
            ) {
                let m = node
                    .metrics
                    .unwrap_or_else(|| panic!("function {} is missing metrics", node.name));
                assert!(m.cyclomatic_complexity >= 1, "CC is at least the base path");
                assert!(m.line_count >= 1, "a function spans at least one line");
            }
        }
    }
}

#[test]
fn dogfood_symbol_ids_are_stable_across_a_re_extract() {
    // Re-extracting the same tree produces identical symbol IDs (NFR-RA-06):
    // the canonical-ordinal scheme is a pure function of the source.
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src = crate_root.join("src");
    let mut inputs = Vec::new();
    collect_rust_sources(&crate_root, &src, &mut inputs);

    let reg = registry();
    let ctx = SymbolContext::cargo("logos-core", "0.1.0");
    let first = extract_files(&inputs, &reg, &ctx);
    let second = extract_files(&inputs, &reg, &ctx);
    assert_eq!(first, second, "the dogfood extraction must be reproducible");
}
