//! Behavioural integration tests for the plugin substrate (S-004), driving it
//! through the public API exactly as the extraction engine (S-007) will.
//!
//! Covers the acceptance criteria that need a real parse:
//! - the Rust grammar loads, is listed, and parses a sample file
//!   ([UAT-PL-01](../../docs/specs/requirements/UAT-PL-01.md),
//!   [FR-PL-06](../../docs/specs/requirements/FR-PL-06.md));
//! - an on-disk query override takes effect without a rebuild
//!   ([UAT-PL-03](../../docs/specs/requirements/UAT-PL-03.md),
//!   [FR-PL-04](../../docs/specs/requirements/FR-PL-04.md),
//!   [FR-PL-05](../../docs/specs/requirements/FR-PL-05.md));
//! - a broken override fails fast naming the file
//!   ([FR-PL-02](../../docs/specs/requirements/FR-PL-02.md)).
//!
//! The whole file is gated on `lang-rust` (a logos-core feature, since
//! integration tests share the crate's feature set) so a `--no-default-features`
//! build that excludes the Rust grammar does not fail these Rust-specific
//! assertions.
#![cfg(feature = "lang-rust")]

use std::fs;

use logos_core::plugin::{LanguagePlugin, LanguageRegistry, PluginError};
use tree_sitter::{Parser, QueryCursor, StreamingIterator};

/// A small but structurally varied Rust sample: a struct, a function, an enum.
const SAMPLE: &str = r#"
pub struct Widget {
    size: u32,
}

pub fn build() -> Widget {
    Widget { size: 1 }
}

enum Color {
    Red,
    Green,
}
"#;

/// Parse `src` with `plugin`'s grammar, returning the parse tree.
fn parse(plugin: &dyn LanguagePlugin, src: &str) -> tree_sitter::Tree {
    let mut parser = Parser::new();
    parser
        .set_language(plugin.language())
        .expect("the loaded grammar's ABI is compatible (asserted at load)");
    parser.parse(src, None).expect("parser produced a tree")
}

/// Run `plugin`'s `capability` query over `src` and return the text of every
/// captured node — the symbols the query extracts.
fn extract(plugin: &dyn LanguagePlugin, capability: &str, src: &str) -> Vec<String> {
    let query = plugin
        .query(capability)
        .expect("capability has a compiled query");
    let tree = parse(plugin, src);
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), src.as_bytes());

    let mut out = Vec::new();
    while let Some(m) = matches.next() {
        for cap in m.captures {
            out.push(src[cap.node.byte_range()].to_string());
        }
    }
    out
}

#[test]
fn rust_grammar_loads_is_listed_and_parses_a_sample() {
    let root = tempfile::tempdir().unwrap(); // no overrides → embedded queries
    let registry = LanguageRegistry::load(root.path()).expect("embedded grammars load");

    // Listed (FR-PL-06).
    assert!(
        registry.iter().any(|p| p.name() == "rust"),
        "rust must be listed by the registry"
    );

    // Resolvable by extension.
    let rust = registry.for_extension("rs").expect("rust claims .rs");

    // Parses a sample file into a non-empty, error-free tree (UAT-PL-01).
    let tree = parse(rust, SAMPLE);
    assert!(
        tree.root_node().child_count() > 0,
        "parse tree is non-empty"
    );
    assert!(!tree.root_node().has_error(), "sample parses without error");

    // The symbols query actually extracts the declarations.
    let symbols = extract(rust, "symbols", SAMPLE);
    assert!(
        symbols.contains(&"Widget".to_string()),
        "found struct: {symbols:?}"
    );
    assert!(
        symbols.contains(&"build".to_string()),
        "found fn: {symbols:?}"
    );
    assert!(
        symbols.contains(&"Color".to_string()),
        "found enum: {symbols:?}"
    );
}

#[test]
fn on_disk_override_takes_effect_without_rebuild() {
    let root = tempfile::tempdir().unwrap();

    // Baseline: the embedded query extracts the struct, fn, and enum.
    let baseline = LanguageRegistry::load(root.path()).unwrap();
    let rust = baseline.for_extension("rs").unwrap();
    let embedded_symbols = extract(rust, "symbols", SAMPLE);
    assert!(embedded_symbols.contains(&"build".to_string()));
    assert!(rust.overridden_capabilities().is_empty());

    // Drop an override that captures ONLY struct names — no recompile, just a
    // file on disk under `.logos/plugins/rust/queries/`.
    let qdir = root.path().join(".logos/plugins/rust/queries");
    fs::create_dir_all(&qdir).unwrap();
    fs::write(
        qdir.join("symbols.scm"),
        "(struct_item name: (type_identifier) @only.struct)\n",
    )
    .unwrap();

    // Reload the SAME binary; the override now shadows the embedded query.
    let overridden = LanguageRegistry::load(root.path()).unwrap();
    let rust = overridden.for_extension("rs").unwrap();
    assert_eq!(
        rust.overridden_capabilities(),
        &["symbols".to_string()],
        "symbols capability must report as overridden"
    );

    let override_symbols = extract(rust, "symbols", SAMPLE);
    assert_eq!(
        override_symbols,
        vec!["Widget".to_string()],
        "override must extract only the struct, proving it took effect"
    );
    assert!(
        !override_symbols.contains(&"build".to_string()),
        "the overridden query must no longer capture the function"
    );

    // Removing the override restores embedded behaviour (no rebuild, FR-PL-05).
    fs::remove_file(qdir.join("symbols.scm")).unwrap();
    let restored = LanguageRegistry::load(root.path()).unwrap();
    let rust = restored.for_extension("rs").unwrap();
    assert!(rust.overridden_capabilities().is_empty());
    assert!(extract(rust, "symbols", SAMPLE).contains(&"build".to_string()));
}

#[test]
fn broken_on_disk_override_fails_fast_naming_the_file() {
    let root = tempfile::tempdir().unwrap();
    let qdir = root.path().join(".logos/plugins/rust/queries");
    fs::create_dir_all(&qdir).unwrap();
    // A query referencing a node type that does not exist in the grammar.
    fs::write(
        qdir.join("symbols.scm"),
        "(no_such_node name: (identifier) @x)\n",
    )
    .unwrap();

    let err = LanguageRegistry::load(root.path())
        .expect_err("a broken override query must fail the load (FR-PL-02)");

    match err {
        PluginError::QueryCompile { file, .. } => {
            assert!(
                file.contains("symbols.scm"),
                "error must name the offending file, got {file:?}"
            );
        }
        other => panic!("expected QueryCompile error, got {other:?}"),
    }
}
