//! Behavioural unit tests for the extraction engine (S-007), exercised against
//! the real Rust grammar. Gated on `lang-rust` (see the `cfg` at the `mod tests`
//! declaration in `mod.rs`).
//!
//! These cover the story's acceptance criteria:
//! - stable symbol IDs across an unrelated edit ([FR-EX-02], [NFR-RA-03]);
//! - cyclomatic complexity + line counts per function ([FR-EX-03], [FR-EX-04]);
//! - deterministic output across rayon thread counts ([FR-IX-03], [NFR-RA-06]).
//!
//! Syntax-error tolerance and the dogfood pass live in the integration test
//! (`tests/extraction.rs`).

use super::*;
use crate::model::{EdgeKind, NodeKind};
use crate::plugin::{LanguagePlugin, LanguageRegistry, Semantics};
use tree_sitter::{Language, Query};

/// Load the registry with the embedded Rust grammar and no on-disk overrides.
fn registry() -> LanguageRegistry {
    let tmp = tempfile::tempdir().expect("tempdir");
    LanguageRegistry::load(tmp.path()).expect("embedded grammars load")
}

/// A minimal [`LanguagePlugin`] that binds a real grammar (so `set_language`
/// and `parse` succeed) but exposes **no** `symbols` query — used to exercise
/// the "no symbols capability → clean empty facts" branch of `extract_one`.
struct NoSymbolsPlugin {
    language: Language,
    semantics: Semantics,
}

impl NoSymbolsPlugin {
    fn new() -> Self {
        Self {
            language: tree_sitter_rust::LANGUAGE.into(),
            semantics: Semantics {
                module_separator: "::".to_string(),
                complexity_keywords: Vec::new(),
                nesting_block_kinds: Vec::new(),
                abi_version: 15,
                framework_detectors: Vec::new(),
                http_client_detectors: Vec::new(),
                framework_methods: std::collections::BTreeMap::new(),
                invocation_methods: std::collections::BTreeMap::new(),
                export_convention: crate::plugin::ExportConvention::All,
                test_convention: crate::plugin::TestConvention::None,
                reachability: false,
                documentation: false,
                artifact: false,
                filenames: Vec::new(),
                config: None,
            },
        }
    }
}

impl LanguagePlugin for NoSymbolsPlugin {
    fn name(&self) -> &str {
        "mock"
    }
    fn extensions(&self) -> &[String] {
        &[]
    }
    fn language(&self) -> &Language {
        &self.language
    }
    fn semantics(&self) -> &Semantics {
        &self.semantics
    }
    fn capabilities(&self) -> &[String] {
        &[]
    }
    fn query(&self, _capability: &str) -> Option<&Query> {
        None
    }
}

/// Extract one in-memory Rust source string at `path`, with a fixed coordinate.
fn extract_src(path: &str, source: &str) -> Facts {
    extract_lang("rs", path, source)
}

/// Extract one in-memory source string with the plugin owning `ext` — the
/// multi-language twin of [`extract_src`], for the arms whose behaviour is
/// carried by a per-language `.scm` rather than by the Rust grammar.
fn extract_lang(ext: &str, path: &str, source: &str) -> Facts {
    let reg = registry();
    let plugin = reg
        .for_extension(ext)
        .unwrap_or_else(|| panic!("{ext} grammar"));
    let ctx = SymbolContext::cargo("logos-core", "0.1.0");
    extract(&FileInput::new(path, source), plugin, &ctx)
}

/// The symbol string of the (first) node named `name`, if present.
fn symbol_of<'a>(facts: &'a Facts, name: &str) -> Option<&'a str> {
    facts
        .nodes
        .iter()
        .find(|n| n.name == name)
        .map(|n| n.symbol.as_str())
}

#[test]
fn extracts_top_level_declarations_with_kinds() {
    let src = "\
pub struct Widget { size: u32 }
pub fn build() -> Widget { Widget { size: 1 } }
enum Color { Red, Green }
const MAX: u32 = 9;
";
    let facts = extract_src("src/lib.rs", src);
    assert_eq!(facts.language, "rust");
    assert!(!facts.partial, "clean source is not partial");

    let kind_of = |name: &str| facts.nodes.iter().find(|n| n.name == name).map(|n| n.kind);
    assert_eq!(kind_of("Widget"), Some(NodeKind::Struct));
    assert_eq!(kind_of("build"), Some(NodeKind::Function));
    assert_eq!(kind_of("Color"), Some(NodeKind::Enum));
    assert_eq!(kind_of("MAX"), Some(NodeKind::Constant));
}

#[test]
fn nested_module_yields_a_contains_edge_and_nested_symbol() {
    let src = "\
mod outer {
    fn inner() {}
}
";
    let facts = extract_src("src/lib.rs", src);

    let outer = symbol_of(&facts, "outer").expect("module node");
    let inner = symbol_of(&facts, "inner").expect("nested fn node");
    // The nested symbol carries the module as a namespace descriptor.
    assert!(outer.ends_with("outer/"), "module symbol: {outer}");
    assert!(
        inner.contains("outer/inner()."),
        "nested fn symbol: {inner}"
    );

    // A Contains edge links the module scope to the function it encloses.
    assert!(
        facts.edges.iter().any(|e| e.kind == EdgeKind::Contains
            && e.source.as_str() == outer
            && e.target.as_str() == inner),
        "expected outer -Contains-> inner, got {:?}",
        facts.edges
    );
}

#[test]
fn symbol_id_is_stable_across_an_unrelated_edit() {
    // FR-EX-02 / NFR-RA-03 / UAT-EX-02: editing an unrelated symbol (and adding
    // a line above) must NOT change the target symbol's ID.
    let v1 = "\
fn alpha() {}

fn target() -> u32 { 1 }
";
    let v2 = "\
// a brand-new unrelated comment
fn alpha() { let _x = 1 + 2; }

fn target() -> u32 { 1 }
";
    let before = extract_src("src/lib.rs", v1);
    let after = extract_src("src/lib.rs", v2);

    let target_before = symbol_of(&before, "target").expect("target v1");
    let target_after = symbol_of(&after, "target").expect("target v2");
    assert_eq!(
        target_before, target_after,
        "target's symbol churned on an unrelated edit (NFR-RA-03)"
    );
}

#[test]
fn renaming_a_sibling_keeps_the_target_stable_but_changes_the_renamed_one() {
    let v1 = "\
fn alpha() {}
fn target() {}
";
    let v2 = "\
fn beta() {}
fn target() {}
";
    let before = extract_src("src/lib.rs", v1);
    let after = extract_src("src/lib.rs", v2);

    // The untouched sibling is stable...
    assert_eq!(
        symbol_of(&before, "target"),
        symbol_of(&after, "target"),
        "an unrelated rename must not move target's ID"
    );
    // ...and the renamed symbol is genuinely a different identity.
    assert!(symbol_of(&before, "alpha").is_some());
    assert!(symbol_of(&after, "beta").is_some());
    assert_ne!(symbol_of(&before, "alpha"), symbol_of(&after, "beta"));
}

#[test]
fn same_name_siblings_get_ordinal_disambiguated_symbols() {
    // Two methods named `run` in two impl blocks of the same type land in the
    // same parent scope (impl is not a captured declaration), so the canonical
    // ordinal disambiguates them (ADR-07).
    let src = "\
struct Foo;
impl Foo { fn run(&self) {} }
impl Foo { fn run(&self, _x: u32) {} }
";
    let facts = extract_src("src/lib.rs", src);
    let runs: Vec<&str> = facts
        .nodes
        .iter()
        .filter(|n| n.name == "run")
        .map(|n| n.symbol.as_str())
        .collect();
    assert_eq!(runs.len(), 2, "both run methods extracted: {runs:?}");
    assert_ne!(runs[0], runs[1], "the two run symbols must be distinct");
    assert!(
        runs.iter().any(|s| s.ends_with("run().")),
        "first ordinal is the bare method symbol: {runs:?}"
    );
    assert!(
        runs.iter().any(|s| s.contains("run(1).")),
        "second ordinal rides the SCIP disambiguator: {runs:?}"
    );
}

#[test]
fn functions_carry_complexity_and_line_counts_but_types_do_not() {
    // FR-EX-03 / FR-EX-04 / UAT-EX-03.
    let decided =
        "fn decided(a: bool) -> u32 {\n    if a {\n        1\n    } else {\n        2\n    }\n}\n";
    let facts = extract_src("src/lib.rs", decided);
    let f = facts
        .nodes
        .iter()
        .find(|n| n.name == "decided")
        .expect("function node");
    let m = f.metrics.expect("function carries metrics");
    assert_eq!(m.cyclomatic_complexity, 3, "base 1 + if + else");
    assert_eq!(m.line_count, 7, "the function spans 7 physical lines");

    // A type declaration has no function metrics.
    let with_struct = extract_src("src/lib.rs", "struct S { x: u32 }\n");
    let s = with_struct
        .nodes
        .iter()
        .find(|n| n.name == "S")
        .expect("struct node");
    assert!(s.metrics.is_none(), "structs carry no function metrics");
}

#[test]
fn single_line_function_has_line_count_one() {
    let facts = extract_src("src/lib.rs", "fn one() { let _x = 1; }\n");
    let m = facts
        .nodes
        .iter()
        .find(|n| n.name == "one")
        .unwrap()
        .metrics;
    assert_eq!(m.unwrap().line_count, 1);
    assert_eq!(m.unwrap().cyclomatic_complexity, 1);
}

#[test]
fn extraction_is_deterministic_across_repeated_runs() {
    let src = "\
mod m {
    fn a() { if true {} }
    struct B;
}
fn c() {}
";
    let first = extract_src("src/lib.rs", src);
    let second = extract_src("src/lib.rs", src);
    assert_eq!(first, second, "repeated extraction must be byte-identical");
}

#[test]
fn parallel_extraction_is_independent_of_thread_count() {
    // FR-IX-03 / NFR-PE-08 / NFR-RA-06: the rayon driver's output must not
    // depend on how many worker threads run.
    let reg = registry();
    let ctx = SymbolContext::cargo("logos-core", "0.1.0");
    let inputs: Vec<FileInput> = (0..32)
        .map(|i| {
            FileInput::new(
                format!("src/f{i}.rs"),
                format!("mod m{i} {{ fn run{i}() {{ if true {{}} }} }}\nfn top{i}() {{}}\n"),
            )
        })
        .collect();

    let run_with = |threads: usize| -> Vec<Facts> {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .expect("thread pool")
            .install(|| extract_files(&inputs, &reg, &ctx))
    };

    let single = run_with(1);
    let many = run_with(8);
    assert_eq!(single.len(), inputs.len(), "every file is extracted");
    // Output preserves input order regardless of thread count — the explicit
    // determinism contract the rayon driver relies on (NFR-RA-06).
    for (result, input) in single.iter().zip(inputs.iter()) {
        assert_eq!(result.path, input.path, "output order matches input order");
    }
    assert_eq!(
        single, many,
        "thread count changed the extraction output (NFR-RA-06)"
    );
}

#[test]
fn unsupported_extensions_are_skipped_by_the_driver() {
    let reg = registry();
    let ctx = SymbolContext::default();
    // `.txt` is claimed by no grammar (markdown claims .md/.markdown, S-033), so
    // it is skipped while the .rs file is extracted.
    let inputs = vec![
        FileInput::new("notes.txt", "not source\n"),
        FileInput::new("src/lib.rs", "fn k() {}\n"),
    ];
    let out = extract_files(&inputs, &reg, &ctx);
    assert_eq!(out.len(), 1, "only the .rs file is extracted");
    assert_eq!(out[0].path, "src/lib.rs");
}

#[test]
fn plugin_without_a_symbols_query_yields_clean_empty_facts() {
    // A grammar that binds and parses but exposes no `symbols` query is not an
    // error — extraction returns empty, clean facts (the documented branch).
    let plugin = NoSymbolsPlugin::new();
    let facts = extract(
        &FileInput::new("src/x.mock", "fn k() { if true {} }\n"),
        &plugin,
        &SymbolContext::default(),
    );
    assert_eq!(facts.language, "mock");
    assert!(facts.nodes.is_empty(), "no symbols query → no nodes");
    assert!(facts.edges.is_empty(), "no symbols query → no edges");
    assert!(!facts.partial, "valid source parses cleanly");
    assert!(
        facts.warnings.is_empty(),
        "a missing symbols capability is not a warning-worthy error"
    );
}

// ── S-011: the file-module node ──────────────────────────────────────────────

#[test]
fn every_file_carries_a_module_node_that_contains_top_level_decls() {
    let src = "\
pub fn build() {}
pub struct Widget;
";
    let facts = extract_src("src/widget.rs", src);

    let module = facts
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::Module && n.name == "widget")
        .expect("the file-module node exists, named after the file stem");
    assert_eq!(module.start_line, 1);
    assert!(module.metrics.is_none(), "a module carries no fn metrics");

    // Top-level declarations are contained by the file module.
    for name in ["build", "Widget"] {
        let target = symbol_of(&facts, name).unwrap();
        assert!(
            facts.edges.iter().any(|e| e.kind == EdgeKind::Contains
                && e.source.as_str() == module.symbol.as_str()
                && e.target.as_str() == target),
            "{name} must be contained by the file module"
        );
    }
}

#[test]
fn file_module_name_resolves_mod_lib_main_stems_to_the_enclosing_dir() {
    let module_name = |path: &str| {
        let facts = extract_src(path, "pub fn f() {}\n");
        facts
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Module)
            .map(|n| n.name.clone())
            .expect("file module present")
    };
    assert_eq!(module_name("src/extract/mod.rs"), "extract");
    assert_eq!(module_name("src/lib.rs"), "crate");
    assert_eq!(module_name("src/main.rs"), "crate");
    assert_eq!(module_name("logos-core/src/lib.rs"), "logos-core");
    assert_eq!(module_name("src/engine.rs"), "engine");
}

#[test]
fn file_module_does_not_perturb_existing_declaration_symbols() {
    // ADR-07 regression guard: the synthesized module node must never join a
    // scope chain — a declaration's symbol is byte-for-byte what it was
    // before S-011 (no extra namespace segment, no ordinal churn).
    let facts = extract_src("src/lib.rs", "pub fn build() {}\n");
    assert_eq!(
        symbol_of(&facts, "build").unwrap(),
        "logos cargo logos-core 0.1.0 src/`lib.rs`/build().",
        "decl symbols keep their pre-S-011 shape"
    );
}

// ── S-011: outgoing reference collection ─────────────────────────────────────

/// The refs of `facts` filtered to one form, as (source-suffix, target) pairs.
fn refs_of(facts: &Facts, form: RefForm) -> Vec<(&str, &str)> {
    facts
        .refs
        .iter()
        .filter(|r| r.form == form)
        .map(|r| (r.source.as_str(), r.target.as_str()))
        .collect()
}

#[test]
fn call_paths_are_attributed_to_their_enclosing_function() {
    let src = "\
fn alpha() {
    helper();
    crate::extract::run();
}
fn helper() {}
";
    let facts = extract_src("src/lib.rs", src);
    let alpha = symbol_of(&facts, "alpha").unwrap();

    let calls = refs_of(&facts, RefForm::Path);
    assert!(calls.contains(&(alpha, "helper")), "bare call recorded");
    assert!(
        calls.contains(&(alpha, "crate::extract::run")),
        "scoped call path recorded verbatim"
    );
    // Every call ref carries EdgeKind::Calls.
    assert!(
        facts
            .refs
            .iter()
            .filter(|r| r.form == RefForm::Path && r.kind == EdgeKind::Calls)
            .count()
            >= 2
    );
}

#[test]
fn method_calls_record_only_the_name_as_method_form() {
    let src = "\
fn alpha(v: Vec<u32>) {
    v.clear();
}
";
    let facts = extract_src("src/lib.rs", src);
    let alpha = symbol_of(&facts, "alpha").unwrap();
    assert_eq!(refs_of(&facts, RefForm::Method), vec![(alpha, "clear")]);
}

#[test]
fn turbofish_calls_normalise_to_the_plain_path() {
    let src = "\
fn alpha() {
    parse::<u32>();
    Vec::<u8>::new();
}
";
    let facts = extract_src("src/lib.rs", src);
    let targets: Vec<&str> = facts
        .refs
        .iter()
        .filter(|r| r.form == RefForm::Path)
        .map(|r| r.target.as_str())
        .collect();
    assert!(targets.contains(&"parse"), "generic_function unwraps");
    assert!(targets.contains(&"Vec::new"), "turbofish strips from paths");
}

#[test]
fn use_imports_are_attributed_to_the_file_module_with_aliases() {
    let src = "\
use crate::extract::run;
use crate::model::{NodeKind as NK, EdgeKind};
use crate::plugin::*;

fn alpha() {}
";
    let facts = extract_src("src/lib.rs", src);
    let module_symbol = facts
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::Module)
        .map(|n| n.symbol.as_str().to_string())
        .unwrap();

    let import = |target: &str| {
        facts
            .refs
            .iter()
            .find(|r| r.kind == EdgeKind::Imports && r.target == target)
            .unwrap_or_else(|| panic!("import ref for {target}"))
    };

    let run = import("crate::extract::run");
    assert_eq!(
        run.source.as_str(),
        module_symbol,
        "file-scope use → file module"
    );
    assert_eq!(run.alias.as_deref(), Some("run"));
    assert_eq!(run.form, RefForm::Path);

    let nk = import("crate::model::NodeKind");
    assert_eq!(nk.alias.as_deref(), Some("NK"), "as-rename keeps the alias");

    let glob = import("crate::plugin");
    assert_eq!(glob.form, RefForm::Glob);
    assert_eq!(glob.alias, None);
}

#[test]
fn duplicate_refs_collapse_to_one_row() {
    let src = "\
fn alpha() {
    helper();
    helper();
}
";
    let facts = extract_src("src/lib.rs", src);
    let count = facts
        .refs
        .iter()
        .filter(|r| r.target == "helper" && r.form == RefForm::Path)
        .count();
    assert_eq!(count, 1, "the same (source,target,form,kind) dedups");
    let line = facts
        .refs
        .iter()
        .find(|r| r.target == "helper")
        .map(|r| r.line)
        .unwrap();
    assert_eq!(line, 2, "the first occurrence's line is kept");
}

#[test]
fn calls_inside_macro_bodies_are_extracted_as_refs() {
    // tree-sitter parses macro arguments as token trees, not expressions, so the
    // `references` query cannot match calls inside `format!(...)`. S-162
    // (CR-043) lifts that v1 limitation by walking the token tree in code: a
    // path call inside a macro argument now produces a `Calls` Path ref,
    // attributed to the enclosing function — so a callee whose only call site is
    // a macro argument is no longer mis-bound dead.
    let src = "\
fn alpha() {
    println!(\"{}\", helper());
    let _ = format!(\"{x}\", x = self.thing.method_call());
}
";
    let facts = extract_src("src/lib.rs", src);
    // The free call `helper()` is a Path ref.
    assert!(
        facts
            .refs
            .iter()
            .any(|r| r.target == "helper" && r.form == RefForm::Path && r.kind == EdgeKind::Calls),
        "a path call inside a macro arg is extracted as a Calls Path ref"
    );
    // The receiver-method call `.method_call()` is a Method ref (bare name).
    assert!(
        facts.refs.iter().any(
            |r| r.target == "method_call" && r.form == RefForm::Method && r.kind == EdgeKind::Calls
        ),
        "a receiver-method call inside a macro arg is extracted as a Calls Method ref"
    );
}

// ── S-014 / FR-AN-01: declaration visibility → the exported flag ─────────────

#[test]
fn pub_and_private_visibility_land_on_exported() {
    let facts = extract_src(
        "src/vis.rs",
        r#"
pub fn open_api() {}
fn internal() {}
pub(crate) fn crate_wide() {}
pub struct Widget { pub field: u32, hidden: u32 }
"#,
    );
    let exported_of = |name: &str| {
        facts
            .nodes
            .iter()
            .find(|n| n.name == name)
            .unwrap_or_else(|| panic!("node {name} extracted"))
            .exported
    };
    assert!(exported_of("open_api"), "pub fn is exported");
    assert!(!exported_of("internal"), "private fn is not exported");
    // Any visibility modifier counts as exported — the conservative
    // exported-is-live reading (a false 'live' beats a false 'dead', FR-AN-01).
    assert!(exported_of("crate_wide"), "pub(crate) counts as exported");
    assert!(exported_of("Widget"), "pub struct is exported");
}

// ── S-014 / FR-AN-02: the normalised AST-shape fingerprint ───────────────────

#[test]
fn fingerprint_matches_renamed_identifier_twins() {
    // The UAT-AN-02 shape: identical structure, every identifier renamed.
    let facts = extract_src(
        "src/dup.rs",
        r#"
fn first(input: u32) -> u32 {
    let doubled = input * 2;
    if doubled > 10 {
        return doubled;
    }
    doubled + 1
}

fn second(value: u32) -> u32 {
    let scaled = value * 2;
    if scaled > 10 {
        return scaled;
    }
    scaled + 1
}
"#,
    );
    let fp = |name: &str| {
        facts
            .nodes
            .iter()
            .find(|n| n.name == name)
            .unwrap_or_else(|| panic!("node {name} extracted"))
            .fingerprint
            .clone()
            .unwrap_or_else(|| panic!("function {name} carries a fingerprint"))
    };
    assert_eq!(
        fp("first"),
        fp("second"),
        "identical structure with renamed identifiers must collide (FR-AN-02)"
    );
}

#[test]
fn fingerprint_distinguishes_structurally_different_functions() {
    let facts = extract_src(
        "src/distinct.rs",
        r#"
fn loops(input: u32) -> u32 {
    let mut acc = 0;
    for i in 0..input {
        acc += i;
    }
    acc
}

fn branches(value: u32) -> u32 {
    if value > 10 {
        return value;
    }
    value + 1
}
"#,
    );
    let fp = |name: &str| {
        facts
            .nodes
            .iter()
            .find(|n| n.name == name)
            .unwrap()
            .fingerprint
            .clone()
            .unwrap()
    };
    assert_ne!(
        fp("loops"),
        fp("branches"),
        "structurally distinct functions must not collide"
    );
}

#[test]
fn fingerprint_ignores_comments_and_whitespace() {
    let bare = extract_src("src/a.rs", "fn f(x: u32) -> u32 { x + 1 }\n");
    let commented = extract_src(
        "src/a.rs",
        r#"
// a leading comment
fn f(y: u32) -> u32 {
    // an inner comment

    y    +     1
}
"#,
    );
    let fp = |facts: &Facts| {
        facts
            .nodes
            .iter()
            .find(|n| n.name == "f")
            .unwrap()
            .fingerprint
            .clone()
            .unwrap()
    };
    assert_eq!(
        fp(&bare),
        fp(&commented),
        "comments and whitespace are stripped from the shape (FR-AN-02)"
    );
}

#[test]
fn fingerprint_is_carried_by_functions_and_methods_only() {
    let facts = extract_src(
        "src/kinds.rs",
        r#"
pub struct S;
impl S {
    fn method(&self) {}
}
fn free() {}
"#,
    );
    for node in &facts.nodes {
        match node.kind {
            NodeKind::Function | NodeKind::Method => assert!(
                node.fingerprint.is_some(),
                "{} ({:?}) carries a fingerprint",
                node.name,
                node.kind
            ),
            _ => assert!(
                node.fingerprint.is_none(),
                "{} ({:?}) must not carry a fingerprint",
                node.name,
                node.kind
            ),
        }
    }
}

#[test]
fn operators_count_in_the_shape() {
    // Identifier names are stripped but anonymous tokens (operators) are not:
    // `a + b` and `a - b` differ structurally.
    let facts = extract_src(
        "src/ops.rs",
        r#"
fn add(a: u32, b: u32) -> u32 { a + b }
fn sub(a: u32, b: u32) -> u32 { a - b }
"#,
    );
    let fp = |name: &str| {
        facts
            .nodes
            .iter()
            .find(|n| n.name == name)
            .unwrap()
            .fingerprint
            .clone()
            .unwrap()
    };
    assert_ne!(
        fp("add"),
        fp("sub"),
        "operator tokens are part of the shape"
    );
}

// ── S-027 / FR-EX-06: extraction-time test-marker evidence ───────────────────

/// The `test_evidence` flag of the (first) node named `name`.
fn evidence_of(facts: &Facts, name: &str) -> bool {
    facts
        .nodes
        .iter()
        .find(|n| n.name == name)
        .unwrap_or_else(|| panic!("node {name} extracted"))
        .test_evidence
}

#[test]
fn rust_test_markers_yield_evidence_only_on_the_marked_functions() {
    // FR-EX-06 / UAT-EX-05: each Rust idiom marks exactly its functions; a
    // production function adjacent to the tests carries no evidence (no
    // proximity false positives, ADR-18).
    let facts = extract_src(
        "src/widget.rs",
        r#"
pub fn prod() {}

#[test]
fn marked() {}

#[tokio::test]
async fn marked_async() {}

#[cfg(test)]
mod tests {
    fn helper() {}
}
"#,
    );
    assert!(evidence_of(&facts, "marked"), "#[test] fn is evidence");
    assert!(
        evidence_of(&facts, "marked_async"),
        "#[tokio::test] fn is evidence"
    );
    assert!(
        evidence_of(&facts, "helper"),
        "fn inside #[cfg(test)] mod is evidence"
    );
    assert!(
        !evidence_of(&facts, "prod"),
        "an adjacent production fn carries no evidence (no proximity false positive)"
    );
    // The synthetic file-module node and the `tests` module node are never
    // test evidence — evidence is per function only.
    assert!(
        !evidence_of(&facts, "widget"),
        "the file module is not evidence"
    );
    assert!(
        !evidence_of(&facts, "tests"),
        "a module node is not evidence"
    );
}

#[test]
fn test_evidence_does_not_perturb_symbol_ids_or_determinism() {
    // FR-EX-06 acceptance: capturing evidence leaves symbol IDs and the
    // byte-identical extraction output unaffected (NFR-RA-03, NFR-RA-06). The
    // same source, with and without a test attribute on an *unrelated* sibling,
    // keeps the production symbol's ID identical; and a re-extract is identical.
    let without = extract_src("src/lib.rs", "fn target() {}\nfn other() {}\n");
    let with = extract_src("src/lib.rs", "fn target() {}\n#[test]\nfn other() {}\n");
    assert_eq!(
        symbol_of(&without, "target"),
        symbol_of(&with, "target"),
        "marking a sibling as a test must not move target's symbol ID"
    );
    // `other` is the same identity either way — only its evidence flag flips.
    assert_eq!(symbol_of(&without, "other"), symbol_of(&with, "other"));
    assert!(!evidence_of(&without, "other"));
    assert!(evidence_of(&with, "other"));

    let again = extract_src("src/lib.rs", "fn target() {}\n#[test]\nfn other() {}\n");
    assert_eq!(with, again, "extraction (incl. evidence) is byte-identical");
}

// ── CR-068 Part B: associated-function Method kinding (FR-EX-05) ──────────────

/// The kind of the (first) node named `name`, if present.
fn kind_of(facts: &Facts, name: &str) -> Option<NodeKind> {
    facts.nodes.iter().find(|n| n.name == name).map(|n| n.kind)
}

#[test]
fn impl_associated_functions_are_methods_free_functions_stay_functions() {
    // FR-EX-05 (CR-068 Part B): a `function_item` directly inside an `impl`
    // block is kinded `Method`; a free function stays `Function`.
    let src = "\
pub fn free_fn() {}

pub struct Widget;

impl Widget {
    pub fn new() -> Self { Widget }
    fn helper(&self) {}
}
";
    let facts = extract_src("src/lib.rs", src);
    assert_eq!(kind_of(&facts, "free_fn"), Some(NodeKind::Function));
    assert_eq!(kind_of(&facts, "Widget"), Some(NodeKind::Struct));
    assert_eq!(
        kind_of(&facts, "new"),
        Some(NodeKind::Method),
        "an inherent associated fn is a Method"
    );
    assert_eq!(
        kind_of(&facts, "helper"),
        Some(NodeKind::Method),
        "an inherent method is a Method"
    );
}

#[test]
fn trait_impl_methods_are_methods_but_trait_defaults_and_local_fns_are_not() {
    // Only `impl`-nested functions are re-kinded. A trait *default* method lives
    // in a `trait_item` (not an `impl_item`) and stays `Function`; a local `fn`
    // nested in a method body (parent is a `block`) stays `Function` too.
    let src = "\
trait Greet {
    fn hello(&self) {}          // trait default — NOT an impl method
}

struct S;

impl Greet for S {
    fn hello(&self) {           // trait-impl method — a Method
        fn local() {}           // nested local fn — stays a Function
        local();
    }
}
";
    let facts = extract_src("src/lib.rs", src);
    // Two `hello` nodes exist; assert the mapping by *identity*, not set
    // membership — the earlier-declared node is the trait default (line 2) and
    // must stay `Function`; the later one is the trait-impl method and must be
    // `Method`. (A set-membership check would pass even if the two were swapped.)
    let mut hellos: Vec<&NodeFact> = facts.nodes.iter().filter(|n| n.name == "hello").collect();
    hellos.sort_by_key(|n| n.start_line);
    assert_eq!(hellos.len(), 2, "one trait-default + one trait-impl `hello`");
    assert_eq!(
        hellos[0].kind,
        NodeKind::Function,
        "the trait *default* `hello` (declared first) must stay a Function"
    );
    assert_eq!(
        hellos[1].kind,
        NodeKind::Method,
        "the trait-impl `hello` (declared later) must be a Method"
    );
    assert_eq!(
        kind_of(&facts, "local"),
        Some(NodeKind::Function),
        "a local fn nested in a method body is not an associated method"
    );
}

#[test]
fn method_kinding_preserves_symbol_ids_and_joint_ordinals() {
    // NFR-RA-06 byte-identity: re-kinding is emission-only, so a free `fn` and a
    // same-named associated fn in one module still share the SCIP method slot and
    // the joint `(kind, name)` ordinal grouping — one gets `insert().`, the other
    // `insert(1).`. Were the kind flipped *before* ordinal assignment, both would
    // render `insert().` and collide.
    let src = "\
pub fn insert() {}

pub struct Store;

impl Store {
    pub fn insert(&self) {}
}
";
    let facts = extract_src("src/lib.rs", src);
    let mut insert_syms: Vec<&str> = facts
        .nodes
        .iter()
        .filter(|n| n.name == "insert")
        .map(|n| n.symbol.as_str())
        .collect();
    insert_syms.sort();
    assert_eq!(insert_syms.len(), 2, "one free fn + one associated fn");
    assert_ne!(
        insert_syms[0], insert_syms[1],
        "the two `insert` symbols must be ordinal-disambiguated, not collide"
    );
    // Both ride the SCIP method descriptor slot (`insert().` / `insert(1).`) —
    // the Function/Method-identical encoding that keeps IDs byte-identical.
    assert!(
        insert_syms.iter().any(|s| s.ends_with("insert().")),
        "one `insert` keeps the ordinal-0 method descriptor: {insert_syms:?}"
    );
    assert!(
        insert_syms.iter().any(|s| s.ends_with("insert(1).")),
        "the other `insert` takes the ordinal-1 method disambiguator: {insert_syms:?}"
    );
    // Re-extraction is byte-identical (determinism holds with the re-kinding).
    let again = extract_src("src/lib.rs", src);
    assert_eq!(facts, again, "extraction is byte-identical across runs");
}

#[test]
fn impl_method_carries_function_metrics_like_a_free_function() {
    // A re-kinded `Method` is still a callable: it must carry per-function
    // metrics (complexity/line count), exactly as it did as a `Function`.
    let src = "\
pub struct S;
impl S {
    pub fn m(&self, x: u32) -> u32 { if x > 0 { x } else { 0 } }
}
";
    let facts = extract_src("src/lib.rs", src);
    let m = facts.nodes.iter().find(|n| n.name == "m").expect("method m");
    assert_eq!(m.kind, NodeKind::Method);
    assert!(
        m.metrics.is_some(),
        "an impl method carries FunctionMetrics like a free fn"
    );
}

// ── Trait-object dyn-dispatch refs (S-281, CR-073, FR-RS-08) ─────────────────

/// The `(source, target)` pairs of every `Implements` reference (impl → trait).
fn implements_refs(facts: &Facts) -> Vec<(&str, &str)> {
    facts
        .refs
        .iter()
        .filter(|r| r.kind == EdgeKind::Implements)
        .map(|r| (r.source.as_str(), r.target.as_str()))
        .collect()
}

/// The `Method`/`Calls` reference targets (the receiver-method calls).
fn method_call_targets(facts: &Facts) -> Vec<&str> {
    facts
        .refs
        .iter()
        .filter(|r| r.form == RefForm::Method && r.kind == EdgeKind::Calls)
        .map(|r| r.target.as_str())
        .collect()
}

#[test]
fn dyn_param_receiver_method_call_is_trait_qualified() {
    // A receiver typed `&dyn LanguagePlugin` by an explicit parameter qualifies
    // the call as `LanguagePlugin::is_documentation` so the binder fans out.
    let src = "\
fn extract_one(plugin: &dyn LanguagePlugin) {
    if plugin.is_documentation() {}
}
";
    let facts = extract_src("src/lib.rs", src);
    assert!(
        method_call_targets(&facts).contains(&"LanguagePlugin::is_documentation"),
        "expected a trait-qualified target, got {:?}",
        method_call_targets(&facts)
    );
}

#[test]
fn box_and_mut_dyn_receivers_are_trait_qualified() {
    let boxed = extract_src("src/lib.rs", "fn f(p: Box<dyn Plug>) { p.run(); }");
    assert!(
        method_call_targets(&boxed).contains(&"Plug::run"),
        "Box<dyn Plug> receiver → Plug::run, got {:?}",
        method_call_targets(&boxed)
    );
    let mutref = extract_src("src/lib.rs", "fn f(p: &mut dyn Plug) { p.run(); }");
    assert!(
        method_call_targets(&mutref).contains(&"Plug::run"),
        "&mut dyn Plug receiver → Plug::run, got {:?}",
        method_call_targets(&mutref)
    );
}

#[test]
fn let_bound_dyn_receiver_is_trait_qualified() {
    let src = "fn f() { let p: &dyn Foo = pick(); p.bar(); }";
    let facts = extract_src("src/lib.rs", src);
    assert!(
        method_call_targets(&facts).contains(&"Foo::bar"),
        "let-bound &dyn Foo receiver → Foo::bar, got {:?}",
        method_call_targets(&facts)
    );
}

#[test]
fn scoped_dyn_trait_takes_its_last_segment() {
    // `&dyn a::b::Multi` → the trait's simple name `Multi`.
    let src = "fn f(p: &dyn a::b::Multi) { p.go(); }";
    let facts = extract_src("src/lib.rs", src);
    assert!(
        method_call_targets(&facts).contains(&"Multi::go"),
        "scoped dyn trait → Multi::go, got {:?}",
        method_call_targets(&facts)
    );
}

#[test]
fn non_dyn_receiver_method_call_stays_a_bare_name() {
    // A concrete-typed receiver is NOT a provable trait object: the call stays a
    // bare `run` and never fans out (the CR-066 guard, FR-RS-06). An inferred
    // closure/unknown receiver likewise stays bare.
    let concrete = extract_src("src/lib.rs", "fn f(p: &CompiledPlugin) { p.run(); }");
    assert!(
        method_call_targets(&concrete).contains(&"run")
            && !method_call_targets(&concrete)
                .iter()
                .any(|t| t.contains("::")),
        "a concrete receiver stays bare, got {:?}",
        method_call_targets(&concrete)
    );
    let inferred = extract_src("src/lib.rs", "fn f(xs: X) { xs.iter().map(|p| p.run()); }");
    assert!(
        !method_call_targets(&inferred)
            .iter()
            .any(|t| *t == "X::run" || t.ends_with("::run")),
        "an inferred closure receiver is not provable, got {:?}",
        method_call_targets(&inferred)
    );
}

#[test]
fn trait_impl_method_emits_an_implements_ref() {
    // `impl Plug for Compiled { fn run() }` emits `Compiled::run --Implements--> Plug`.
    let src = "\
struct Compiled;
impl Plug for Compiled {
    fn run(&self) {}
}
";
    let facts = extract_src("src/lib.rs", src);
    let impls = implements_refs(&facts);
    assert_eq!(impls.len(), 1, "one Implements ref, got {impls:?}");
    let (source, target) = impls[0];
    assert_eq!(target, "Plug", "the impl method points at its trait");
    assert!(
        source.contains("run"),
        "the Implements source is the impl method `run`: {source}"
    );
}

#[test]
fn inherent_impl_method_emits_no_implements_ref() {
    // `impl X { fn helper() }` carries no trait — no Implements ref.
    let src = "\
struct X;
impl X {
    fn helper(&self) {}
}
";
    let facts = extract_src("src/lib.rs", src);
    assert!(
        implements_refs(&facts).is_empty(),
        "an inherent impl method emits no Implements ref, got {:?}",
        implements_refs(&facts)
    );
}

#[test]
fn generic_trait_impl_emits_an_implements_ref_to_the_base_trait() {
    // `impl<S> Handler<S> for T { fn on(&self) }` → Implements to the base `Handler`.
    let src = "\
struct T;
impl<S> Handler<S> for T {
    fn on(&self, _s: S) {}
}
";
    let facts = extract_src("src/lib.rs", src);
    let impls = implements_refs(&facts);
    assert_eq!(impls.len(), 1, "one Implements ref, got {impls:?}");
    assert_eq!(impls[0].1, "Handler", "generic trait impl → base trait name");
}

#[test]
fn shadowed_receiver_name_is_not_provable_and_stays_bare() {
    // A receiver name bound more than once (a concrete parameter shadowed by a
    // later `&dyn` let) is NOT a per-file-provable single type. The scope-blind
    // walk must bail rather than guess which binding is live at a given call site,
    // else the FIRST `c.draw()` (on the concrete `c`) would be fabricated as a
    // `Draw::draw` dispatch edge (NFR-RA-05). Both calls stay bare.
    let src = "\
fn render(c: &Canvas) {
    c.draw();
    let c: &dyn Draw = pick();
    c.draw();
}
";
    let facts = extract_src("src/lib.rs", src);
    assert!(
        !method_call_targets(&facts).iter().any(|t| t.contains("::")),
        "a shadowed receiver name must not qualify to a trait, got {:?}",
        method_call_targets(&facts)
    );
}

#[test]
fn smart_pointer_dyn_with_trait_bounds_is_trait_qualified() {
    // `Arc<dyn Plug + Send>` peels the `bounded_type` inside the generic argument,
    // symmetric with the reference path that already handles `&dyn Plug + Send`.
    let facts = extract_src("src/lib.rs", "fn f(p: Arc<dyn Plug + Send>) { p.run(); }");
    assert!(
        method_call_targets(&facts).contains(&"Plug::run"),
        "Arc<dyn Plug + Send> receiver → Plug::run, got {:?}",
        method_call_targets(&facts)
    );
}

// ── S-252 / FR-WS-08: HTTP client-call arm capture ───────────────────────────

/// The `HttpClientCall` reference targets captured from a source.
fn http_client_call_targets(facts: &Facts) -> Vec<String> {
    facts
        .refs
        .iter()
        .filter(|r| r.relation == Some(crate::model::ArtifactRelation::HttpClientCall))
        .map(|r| r.target.clone())
        .collect()
}

/// A static, absolute client call is captured as a `"METHOD /template"` ref under
/// the `HttpClientCall` relation — an artifact→code binding on the `Path` form,
/// so it rides the same route binder the OpenAPI arm does. The `use reqwest`
/// import makes the file a client-call candidate (the FR-FW-04-style gate).
#[test]
fn a_static_client_call_is_captured_as_a_method_template_ref() {
    let facts = extract_src(
        "src/client.rs",
        r#"use reqwest::Client;
async fn fetch(client: Client) { client.get("/users/{id}").await; }"#,
    );
    assert_eq!(
        http_client_call_targets(&facts),
        vec!["GET /users/{id}".to_string()]
    );
    let r = facts
        .refs
        .iter()
        .find(|r| r.relation == Some(crate::model::ArtifactRelation::HttpClientCall))
        .expect("the client-call ref is present");
    assert_eq!(r.form, crate::model::RefForm::Path);
    assert_eq!(r.kind, EdgeKind::ArtifactBinding);
}

/// A runtime-composed path — a bare variable or a `format!` — is refused: the
/// arm's normalizer returns `None`, so no reference and no ledger entry
/// (base-url-runtime, never approximately matched).
#[test]
fn a_runtime_composed_client_call_is_not_captured() {
    let bare = extract_src(
        "src/c.rs",
        r#"use reqwest::Client;
async fn f(client: Client, url: String) { client.get(url).await; }"#,
    );
    assert!(
        http_client_call_targets(&bare).is_empty(),
        "a bare-variable path is base-url-runtime: {:?}",
        http_client_call_targets(&bare)
    );

    let composed = extract_src(
        "src/c.rs",
        r#"use reqwest::Client;
async fn f(client: Client, base: String) { client.get(format!("{base}/users")).await; }"#,
    );
    assert!(
        http_client_call_targets(&composed).is_empty(),
        "a format!-composed path is base-url-runtime: {:?}",
        http_client_call_targets(&composed)
    );
}

/// A catch-all/non-normalizable absolute literal, and a relative (base-URL) one,
/// are refused — path-not-composed / base-url-runtime, never approximated.
#[test]
fn a_non_composable_client_call_literal_is_not_captured() {
    let catch_all = extract_src(
        "src/c.rs",
        r#"use reqwest::Client;
async fn f(client: Client) { client.get("/files/{*rest}").await; }"#,
    );
    assert!(
        http_client_call_targets(&catch_all).is_empty(),
        "a catch-all path is path-not-composed: {:?}",
        http_client_call_targets(&catch_all)
    );

    let relative = extract_src(
        "src/c.rs",
        r#"use reqwest::Client;
async fn f(client: Client) { client.get("users/{id}").await; }"#,
    );
    assert!(
        http_client_call_targets(&relative).is_empty(),
        "a relative path has no absolute route prefix: {:?}",
        http_client_call_targets(&relative)
    );
}

/// Within a client file, a method call whose name is not an HTTP verb (a
/// collection `insert`) is never captured, even with a `/`-shaped literal — the
/// HTTP-verb filter keeps the broad anchor from over-capturing.
#[test]
fn a_non_http_method_call_is_not_captured() {
    let facts = extract_src(
        "src/c.rs",
        r#"use reqwest::Client;
async fn f(client: Client, map: std::collections::HashMap<String, i32>) {
    let _ = client.get("/health").await;
    map.insert("/users/{id}".to_string(), 1);
}"#,
    );
    // Only the genuine `client.get` is captured; `map.insert` is not an HTTP verb.
    assert_eq!(
        http_client_call_targets(&facts),
        vec!["GET /health".to_string()],
        "only the HTTP-verb call is captured, not `insert`"
    );
}

/// Never-fabricate gate (S-252 review-fix): a `/`-shaped-key collection `.get`
/// in a file that does NOT reference any HTTP-client crate is NOT captured — so
/// an incidental `perms.get("/admin/users")` can never fabricate a cross-service
/// edge to a same-shaped route in another member ([NFR-RA-05]).
#[test]
fn a_route_shaped_get_in_a_non_client_file_is_not_captured() {
    let facts = extract_src(
        "src/authz.rs",
        r#"use std::collections::HashMap;
fn authorize(perms: HashMap<String, i32>) { let _ = perms.get("/admin/users"); }"#,
    );
    assert!(
        http_client_call_targets(&facts).is_empty(),
        "a non-client file never emits an outbound-call ref: {:?}",
        http_client_call_targets(&facts)
    );
}

// ── S-343 / FR-WS-08 / CR-108: TypeScript + TSX HTTP client-call capture ─────
//
// Every assertion below runs against **both** TypeScript grammars. `.ts`/`.js`
// and `.tsx`/`.jsx` are one language split across two tree-sitter `Language`s
// (ADR-09) shipping two copies of the same query, so a rule proved in only one
// leaves half the surface unproven.
#[cfg(feature = "lang-typescript")]
const TS_EXTENSIONS: [&str; 2] = ["ts", "tsx"];

/// Extract a TypeScript fixture under both grammars and assert they agree,
/// returning the (shared) captured client-call targets. A failure names which
/// grammar produced which capture — the two are not interchangeable when one
/// of them regresses.
#[cfg(feature = "lang-typescript")]
fn ts_client_call_targets(source: &str) -> Vec<String> {
    let ts = http_client_call_targets(&extract_lang("ts", "src/client.ts", source));
    let tsx = http_client_call_targets(&extract_lang("tsx", "src/client.tsx", source));
    assert_eq!(
        ts, tsx,
        "the typescript and tsx queries must capture identically (they are one \
         language, ADR-09) — ts={ts:?} tsx={tsx:?}, source:\n{source}"
    );
    ts
}

/// FR-WS-08 AC (axios, receiver-verb form): `axios.get("/users")` yields exactly
/// one `"GET /users"` reference on the `Path`/`ArtifactBinding` shape the route
/// binder consumes — the same ledger row the Rust arm files, proving the
/// per-language query is the whole of the language-specific surface.
#[test]
#[cfg(feature = "lang-typescript")]
fn an_axios_receiver_call_is_captured_as_a_method_template_ref() {
    const AXIOS_GET_USERS: &str = r#"import axios from "axios";
export async function listUsers() { return axios.get("/users"); }"#;

    assert_eq!(
        ts_client_call_targets(AXIOS_GET_USERS),
        vec!["GET /users".to_string()]
    );

    // The shape stays a `Path`-form artifact binding, not a plain code ref —
    // asserted under BOTH grammars, since `ts_client_call_targets` compares only
    // the rendered target and would not notice a form/kind divergence.
    for ext in TS_EXTENSIONS {
        let facts = extract_lang(ext, &format!("src/client.{ext}"), AXIOS_GET_USERS);
        let r = facts
            .refs
            .iter()
            .find(|r| r.relation == Some(crate::model::ArtifactRelation::HttpClientCall))
            .unwrap_or_else(|| panic!("the client-call ref is present in {ext}"));
        assert_eq!(r.form, crate::model::RefForm::Path, "{ext}");
        assert_eq!(r.kind, EdgeKind::ArtifactBinding, "{ext}");
    }

    // A non-`get` verb and a member-qualified, conventionally-named instance
    // both resolve — the receiver rule is a name rule, not an identity rule.
    assert_eq!(
        ts_client_call_targets(
            r#"import axios from "axios";
class Api { async create(body: unknown) { return this.axiosClient.post("/users", body); } }"#
        ),
        vec!["POST /users".to_string()]
    );
}

/// FR-WS-08 AC (axios, object-argument form): `axios({url, method})` yields one
/// reference, in **either** key order — an object literal has no canonical key
/// order, and tree-sitter matches siblings in source order, so both spellings
/// are pinned rather than assumed.
#[test]
#[cfg(feature = "lang-typescript")]
fn an_axios_object_argument_call_is_captured_in_either_key_order() {
    assert_eq!(
        ts_client_call_targets(
            r#"import axios from "axios";
export async function listUsers() { return axios({url: "/users", method: "get"}); }"#
        ),
        vec!["GET /users".to_string()],
        "url-then-method order"
    );
    assert_eq!(
        ts_client_call_targets(
            r#"import axios from "axios";
export async function listUsers() { return axios({method: "get", url: "/users"}); }"#
        ),
        vec!["GET /users".to_string()],
        "method-then-url order"
    );
    // The `axios.request({…})` spelling of the same shape.
    assert_eq!(
        ts_client_call_targets(
            r#"import axios from "axios";
export async function put(body: unknown) { return axios.request({url: "/users", method: "put", data: body}); }"#
        ),
        vec!["PUT /users".to_string()],
        "the axios.request spelling captures once, not twice"
    );

    // A member-qualified callee resolves in the object-argument form exactly as
    // it does in the receiver-verb form. Anchoring the callee only at `^` made
    // one file disagree with itself: `this.axiosClient.post("/u")` captured while
    // `this.axios({…})` silently did not (review-fix).
    for source in [
        r#"import axios from "axios";
class Api { list() { return this.axios({url: "/users", method: "get"}); } }"#,
        r#"import axios from "axios";
class Api { list() { return this.axiosClient.request({method: "get", url: "/users"}); } }"#,
    ] {
        assert_eq!(
            ts_client_call_targets(source),
            vec!["GET /users".to_string()],
            "a member-qualified axios object-argument call is captured: {source}"
        );
    }
}

/// FR-WS-08 AC (the free-function anchor, S-343's own question): `fetch` takes
/// no receiver at all, and the generic dispatch carries it — a bare
/// `fetch("/users")` resolves to `GET`, the WHATWG Fetch default declared by the
/// `@invoke.http.method.get` capture name. The method-bearing form is the next
/// test's subject, not this one's.
#[test]
#[cfg(feature = "lang-typescript")]
fn a_fetch_free_function_call_is_captured_with_its_verb() {
    assert_eq!(
        ts_client_call_targets(
            r#"export async function listUsers() { return fetch("/users"); }"#
        ),
        vec!["GET /users".to_string()],
        "a verb-less fetch is a GET by the Fetch standard"
    );

    // `window.fetch` / `globalThis.fetch` / `self.fetch` are the same global
    // spelled explicitly — captured rather than left as an unstated ceiling.
    for receiver in ["window", "globalThis", "self"] {
        assert_eq!(
            ts_client_call_targets(&format!(
                "export async function listUsers() {{ return {receiver}.fetch(\"/users\"); }}"
            )),
            vec!["GET /users".to_string()],
            "{receiver}.fetch is the same global"
        );
    }
}

/// NFR-RA-05 (the AC's sharpest edge): a method-bearing `fetch` resolves to its
/// stated verb and emits **no** silent `GET` alongside it. The two fetch
/// patterns are disjoint by construction (the verb-less one anchors on a call
/// with exactly one argument), and the dispatch ranks a source-read verb above
/// a name-declared one — this asserts the outcome of both, since a single
/// spurious `GET /users` here would fabricate a second cross-service edge.
#[test]
#[cfg(feature = "lang-typescript")]
fn a_method_bearing_fetch_resolves_to_its_verb_and_never_also_to_get() {
    let targets = ts_client_call_targets(
        r#"export async function createUser(body: unknown) {
    return fetch("/users", {method: "POST", body: JSON.stringify(body)});
}"#,
    );
    assert_eq!(
        targets,
        vec!["POST /users".to_string()],
        "exactly one reference, and it is the POST — never a defaulted GET"
    );
}

/// FR-WS-08 shared negative-case contract, case 2 (base-url-runtime): a
/// template literal composes its path at runtime, so it emits no reference —
/// the interpolation makes the literal dynamic and the arm refuses it rather
/// than guessing a target. Asserted on both the axios and the fetch anchor, so
/// neither idiom can regress into approximate matching independently.
#[test]
#[cfg(feature = "lang-typescript")]
fn a_template_literal_path_emits_no_reference() {
    for source in [
        r#"import axios from "axios";
const base = "https://api.example.com";
export async function listUsers() { return axios.get(`${base}/users`); }"#,
        r#"const base = "https://api.example.com";
export async function listUsers() { return fetch(`${base}/users`); }"#,
        // A bare variable is the same refusal for a different reason.
        r#"import axios from "axios";
export async function listUsers(url: string) { return axios.get(url); }"#,
    ] {
        assert!(
            ts_client_call_targets(source).is_empty(),
            "a runtime-composed path is base-url-runtime, never approximated: {source}"
        );
    }

    // A template literal with no substitution is static text and still binds —
    // the refusal is about interpolation, not about the backtick.
    assert_eq!(
        ts_client_call_targets(
            r#"export async function listUsers() { return fetch(`/users`); }"#
        ),
        vec!["GET /users".to_string()],
    );
}

/// FR-WS-08 shared negative-case contract, case 1 (a same-shaped non-HTTP
/// receiver call) — and the CR-110 false-positive class this arm must not
/// reintroduce on the consumer side.
///
/// S-350 retracted the unscoped `<receiver>.get("string")` anchor on the
/// **provider** side after an Angular `formGroup.get("year")` and a
/// `cache.get("/cache/key")` each promoted a route. Both consumer-side defences
/// are asserted here: the query refuses an unrecognised receiver even inside a
/// genuine axios file (first case), and the arm's ledger gate refuses a file
/// that references no client (second case, with a positive control isolating
/// the gate as the only difference).
///
/// The two are independent for `axios`, which is a real import specifier. They
/// are **not** for `fetch`: it is a global, so the reference that opens the gate
/// is the call itself and only the query's exact-name guard stands — see the
/// non-independent-gate rule on [`capture_http_client_call_arm`], which is where
/// that rule is stated once. That is why every pattern in this language's query
/// is scoped to a named client rather than relying on the gate.
#[test]
#[cfg(feature = "lang-typescript")]
fn a_non_client_receiver_call_is_never_captured() {
    // In a real axios file — the ledger gate is open, so only the query's
    // receiver scoping stands between `cache.get` and a fabricated edge.
    assert_eq!(
        ts_client_call_targets(
            r#"import axios from "axios";
export async function listUsers() {
    const cached = cache.get("/cache/key");
    const year = formGroup.get("year");
    return cached ?? year ?? axios.get("/users");
}"#
        ),
        vec!["GET /users".to_string()],
        "only the axios call is captured — `cache.get` and `formGroup.get` are \
         the exact CR-110 shapes, and must not reappear on the consumer side"
    );

    // The ledger gate, isolated. The receiver here is one the QUERY accepts
    // (`axiosCache` begins with `axios`), so the only thing that can produce the
    // empty result is `capture_http_client_call_arm`'s `is_http_client_file`
    // check. The previous fixture used `perms.get(…)`, which no pattern matches
    // — it would have passed with the gate deleted (review-fix).
    const GATED: &str = r#"export function lookup() { return axiosCache.get("/users"); }"#;
    assert!(
        ts_client_call_targets(GATED).is_empty(),
        "no reference to axios or fetch anywhere ⇒ the file is never scanned"
    );
    // Positive control: the identical source, with the import that opens the
    // gate, does capture — so the emptiness above is attributable to the gate
    // and to nothing else.
    assert_eq!(
        ts_client_call_targets(&format!("import axios from \"axios\";\n{GATED}")),
        vec!["GET /users".to_string()],
        "the same source with the client import captures — the gate is the only \
         difference between these two cases"
    );

    // The CR-110 shape the boundary rule exists for: a receiver that merely
    // CONTAINS `axios` is not an axios instance. A substring test admitted
    // `notaxiosCache.get("/cache/key")` and fabricated a cross-service call from
    // a cache lookup (review-fix).
    assert!(
        ts_client_call_targets(
            r#"import axios from "axios";
export function lookup() { return notaxiosCache.get("/cache/key"); }"#
        )
        .is_empty(),
        "the receiver name rule is a boundary rule, never a substring test"
    );

    // A plain-identifier call that is not `fetch` is refused by the anchored
    // `#match?` name guard — otherwise the verb-less pattern would make every
    // one-argument free function in a client file a GET.
    assert!(
        ts_client_call_targets(
            r#"import axios from "axios";
export function load() { return readConfig("/etc/app/config"); }"#
        )
        .is_empty(),
        "only `fetch` carries the verb-less free-function anchor"
    );

    // The MEMBER-EXPRESSION half of the same guard, which the ledger gate
    // provably cannot backstop: `fetch` is itself one of this descriptor's
    // `http_client_detectors` rows and `references.scm` emits a name-only
    // `@ref.method` for `repo.fetch(…)`, so such a file self-satisfies the gate.
    // Only the anchored `^((window|globalThis|self)\.)?fetch$` regex stands, and
    // relaxing it to the `(^|\.)` form the axios patterns use — the natural
    // "make the two consistent" edit — would turn every repository/queue refresh
    // into a fabricated cross-service call (NFR-RA-05, the CR-110 class the
    // query header names in so many words).
    for source in [
        r#"import axios from "axios";
export function refresh(repo: Repo) { return repo.fetch("/users"); }"#,
        r#"import axios from "axios";
export function drain(queue: Queue) { return queue.fetch("/jobs"); }"#,
        r#"import axios from "axios";
class Api { load() { return this.fetch("/users"); } }"#,
    ] {
        assert!(
            ts_client_call_targets(source).is_empty(),
            "a bare `x.fetch(…)` on an arbitrary receiver is a refresh far more \
             often than an HTTP call — only the explicit global spellings are \
             admitted: {source}"
        );
    }
}

/// FR-WS-08 shared negative-case contract, case 3 (path-not-composed) and the
/// relative-path refusal, reached through the TypeScript anchors: a static
/// absolute literal that does not positionally normalize, and a literal with no
/// absolute route prefix, each emit nothing. The classification itself is the
/// shared interpreter's and is fixture-pinned there; this proves the TypeScript
/// query feeds it the slots it expects.
#[test]
#[cfg(feature = "lang-typescript")]
fn a_non_composable_typescript_path_literal_is_not_captured() {
    for source in [
        r#"import axios from "axios";
export async function raw() { return axios.get("/files/{*rest}"); }"#,
        r#"export async function raw() { return fetch("users/{id}"); }"#,
    ] {
        assert!(
            ts_client_call_targets(source).is_empty(),
            "a non-normalizing or relative literal is refused: {source}"
        );
    }
}

/// Drive `collect_invocation_sites` directly for a fixture in the language owning
/// `ext`, returning the raw slots that language's query hands the shared
/// interpreter.
///
/// The refs-level helpers can only see what survived classification, so they
/// cannot distinguish "the query never matched" from "the interpreter refused
/// it" — and only the second surfaces a coverage reason. Every negative case
/// whose contract names a specific reason is pinned through this.
///
/// This is why a language arm's slot-level tests live in this module rather than
/// beside the rest of its suite in `tests/`: both
/// [`collect_invocation_sites`] and
/// [`classify_client_call`](crate::resolve::http_client_call::classify_client_call)
/// are crate-private, and widening them to `pub` for a test would be a worse
/// trade than the module gate documented on `extract::tests`.
#[cfg(any(feature = "lang-typescript", feature = "lang-go"))]
fn invocation_sites(ext: &str, source: &str) -> Vec<crate::extract::config::InvocationSite> {
    let reg = registry();
    let plugin = reg
        .for_extension(ext)
        .unwrap_or_else(|| panic!("{ext} grammar"));
    let query = plugin
        .query("invocations")
        .unwrap_or_else(|| panic!("the {ext} plugin ships an invocations query"));
    let ctx = SymbolContext::cargo("logos-core", "0.1.0");
    let facts = extract(&FileInput::new(format!("src/client.{ext}"), source), plugin, &ctx);
    // Borrow a real file-module symbol so the site has an attributable scope.
    let module = facts
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::Module)
        .expect("the file module node")
        .symbol
        .clone();

    let mut parser = tree_sitter::Parser::new();
    parser.set_language(plugin.language()).expect("set language");
    let tree = parser.parse(source, None).expect("parses");
    collect_invocation_sites(
        query,
        tree.root_node(),
        source.as_bytes(),
        &[],
        &[],
        Some(&module),
        // The pass-through default: these fixtures assert the dispatch itself,
        // not a descriptor's `[invocation_methods]` normalization (S-346).
        &std::collections::BTreeMap::new(),
    )
}

/// [`invocation_sites`] for a TypeScript fixture (S-343's original spelling).
#[cfg(feature = "lang-typescript")]
fn ts_invocation_sites(ext: &str, source: &str) -> Vec<crate::extract::config::InvocationSite> {
    invocation_sites(ext, source)
}

/// FR-WS-08 shared negative-case contract, case 2 — pinned at the level the
/// contract asks a language story to pin it: **the slots**, not the
/// classification.
///
/// "Emits no reference" is satisfied by two very different outcomes — the query
/// never matched at all, or the query matched and the interpreter refused it —
/// and only the second one reports `base-url-runtime`. This asserts the second:
/// the TypeScript anchors do hand an interpolated template literal to the
/// interpreter, carrying the `path_dynamic` marker rather than a `path`, which
/// is exactly what makes `classify_client_call` return
/// `ClientCallRefusal::BaseUrlRuntime` (fixture-pinned in `http_client_call.rs`,
/// never re-derived here).
///
/// Both idioms run under **both** grammars, like every other fixture in this
/// section: the two plugins compile the same query text against different
/// `Language`s ([ADR-09]), so proving axios under `ts` alone and fetch under
/// `tsx` alone would leave a per-grammar regression invisible at exactly the
/// level where the coverage reason lives.
///
/// [ADR-09]: ../../../docs/specs/architecture/decisions/ADR-09.md
#[test]
#[cfg(feature = "lang-typescript")]
fn a_template_literal_reaches_the_interpreter_as_a_dynamic_path() {
    use crate::resolve::http_client_call::{DYNAMIC_PATH_SLOT, METHOD_SLOT, PATH_SLOT};

    for (source, verb) in [
        (
            r#"import axios from "axios";
const base = "https://api.example.com";
export async function listUsers() { return axios.get(`${base}/users`); }"#,
            "get",
        ),
        (
            r#"const base = "https://api.example.com";
export async function listUsers() { return fetch(`${base}/users`, {method: "POST"}); }"#,
            "POST",
        ),
    ] {
        for ext in TS_EXTENSIONS {
            let sites = ts_invocation_sites(ext, source);
            assert_eq!(sites.len(), 1, "exactly one invocation site in {ext}");
            let slots = &sites[0].slots;
            assert_eq!(
                slots.get(METHOD_SLOT).map(String::as_str),
                Some(verb),
                "the verb still reaches the interpreter — only the path is \
                 dynamic ({ext})"
            );
            assert!(
                slots.contains_key(DYNAMIC_PATH_SLOT),
                "an interpolated template literal must carry the dynamic-path \
                 marker (that marker is what makes the refusal \
                 `base-url-runtime` rather than a silent non-match) in {ext}: \
                 {slots:?}"
            );
            assert!(
                !slots.contains_key(PATH_SLOT),
                "no static path is guessed from a runtime-composed literal in \
                 {ext}: {slots:?}"
            );
            assert!(
                crate::resolve::http_client_call::render_client_call_target(slots).is_none(),
                "and the interpreter therefore renders no target ({ext})"
            );
        }
    }
}

/// FR-WS-08 shared negative-case contract, case 3 (`path-not-composed`) — pinned
/// at slot level for the same reason case 2 is.
///
/// The refs-level test above proves only that these emit nothing, and the two
/// fixtures there actually exercise **different** refusals: a catch-all segment
/// is `PathNotComposed`, a relative literal is `BaseUrlRuntime`. Since case 3's
/// whole point is the distinct coverage reason, it has to be asserted where the
/// reason exists — the query must hand over a static `path`, not the dynamic
/// marker, so the refusal comes from classifying a composed path rather than
/// from the query failing to match at all.
#[test]
#[cfg(feature = "lang-typescript")]
fn a_non_normalizing_absolute_path_reaches_the_interpreter_as_a_static_path() {
    use crate::resolve::http_client_call::{
        classify_client_call, ClientCallRefusal, DYNAMIC_PATH_SLOT, PATH_SLOT,
    };

    for (source, literal, refusal) in [
        (
            r#"import axios from "axios";
export async function raw() { return axios.get("/files/{*rest}"); }"#,
            "/files/{*rest}",
            ClientCallRefusal::PathNotComposed,
        ),
        (
            r#"export async function raw() { return fetch("users/{id}"); }"#,
            "users/{id}",
            ClientCallRefusal::BaseUrlRuntime,
        ),
    ] {
        for ext in TS_EXTENSIONS {
            let sites = ts_invocation_sites(ext, source);
            assert_eq!(sites.len(), 1, "one site reaches the interpreter in {ext}");
            let slots = &sites[0].slots;
            assert_eq!(
                slots.get(PATH_SLOT).map(String::as_str),
                Some(literal),
                "the static literal is handed over verbatim, not pre-judged: {slots:?}"
            );
            assert!(
                !slots.contains_key(DYNAMIC_PATH_SLOT),
                "a static literal never carries the dynamic marker: {slots:?}"
            );
            assert_eq!(
                classify_client_call(slots),
                Err(refusal),
                "the interpreter's own refusal reason for {literal} in {ext}"
            );
        }
    }
}

/// The `fetch("/users", {headers: …})` ceiling the query records — asserted
/// **empty on purpose**, so widening a pattern re-litigates the intent instead
/// of silently changing it.
///
/// The risk here is not the under-capture. It is that a method-less init object
/// could fall through to the verb-less pattern and emit a phantom `GET`, or that
/// a `method` key nested inside another object could be mistaken for the init's
/// own — which is the plausible regression, since the init pattern matches a
/// `pair` among the object's direct children.
#[test]
#[cfg(feature = "lang-typescript")]
fn a_method_less_fetch_init_object_is_the_stated_ceiling() {
    for source in [
        r#"export async function f() { return fetch("/users", {headers: {}}); }"#,
        // The nested-`method` trap: `{method: "POST"}` here belongs to `headers`,
        // not to the init object, and must not be read as the request's verb.
        r#"export async function f() { return fetch("/users", {headers: {method: "POST"}}); }"#,
    ] {
        assert!(
            ts_client_call_targets(source).is_empty(),
            "a method-less init object is the stated ceiling — it must emit \
             nothing, never a phantom GET: {source}"
        );
    }

    // ...and when the init object DOES carry its own `method`, an unrelated
    // nested one never displaces it.
    assert_eq!(
        ts_client_call_targets(
            r#"export async function f() {
    return fetch("/users", {headers: {method: "POST"}, method: "PUT"});
}"#
        ),
        vec!["PUT /users".to_string()],
        "the init object's own method wins over a nested lookalike"
    );
}

/// The three guarantees `DECLARED_METHOD_PREFIX` states in its rustdoc, pinned
/// directly rather than through whatever the shipped `.scm` files happen to
/// contain — no shipped query exercises the failure or precedence branches, so
/// they are otherwise dead to the suite.
///
/// These are core-level claims against [NFR-RA-05]: a name-declared verb must
/// pass the same `is_http_method` gate as a source-read one (so a typo captures
/// nothing rather than inventing a method); a verb spelled in the source must
/// always outrank one declared by a capture name (so a query binding both can
/// never downgrade a real `POST` to a shape's default); and when a query binds
/// two conflicting declared verbs to one match, the **first declaration wins**
/// — chosen so the resolved verb never depends on node position, which a
/// droppable on-disk query ([FR-PL-04]) makes a reachable case.
///
/// Runs against the `ts` grammar alone on purpose, unlike every other test in
/// this section: its subject is the core dispatch driven by ad-hoc queries, not
/// plugin data, so the TSX `Language` would re-prove the same core code.
///
/// [FR-PL-04]: ../../../docs/specs/requirements/FR-PL-04.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[test]
#[cfg(feature = "lang-typescript")]
fn a_name_declared_verb_is_gated_and_outranked_by_a_source_read_one() {
    use crate::resolve::http_client_call::METHOD_SLOT;

    let reg = registry();
    let plugin = reg.for_extension("ts").expect("typescript grammar");
    let ctx = SymbolContext::cargo("logos-core", "0.1.0");
    let source = r#"export async function f() { return frob("/users", {method: "POST"}); }"#;
    let facts = extract(&FileInput::new("src/c.ts", source), plugin, &ctx);
    let module = facts
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::Module)
        .expect("the file module node")
        .symbol
        .clone();
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(plugin.language()).expect("set language");
    let tree = parser.parse(source, None).expect("parses");

    let sites = |query_src: &str| -> Vec<crate::extract::config::InvocationSite> {
        let query = Query::new(plugin.language(), query_src).expect("query compiles");
        collect_invocation_sites(
            &query,
            tree.root_node(),
            source.as_bytes(),
            &[],
            &[],
            Some(&module),
            &std::collections::BTreeMap::new(),
        )
    };

    // A capture name whose suffix is not an HTTP verb captures nothing — the
    // name-declared path is gated exactly like a source-read one.
    assert!(
        sites(
            r#"(call_expression
                 function: (identifier) @invoke.http.method.frobnicate
                 arguments: (arguments . (_) @invoke.http.arg))"#
        )
        .is_empty(),
        "`frobnicate` is not an HTTP verb, so no site is emitted"
    );

    // Sanity: the same shape with a real verb in the capture name does emit,
    // so the assertion above fails for the verb and not for the query shape.
    assert_eq!(
        sites(
            r#"(call_expression
                 function: (identifier) @invoke.http.method.head
                 arguments: (arguments . (_) @invoke.http.arg))"#
        )
        .len(),
        1,
        "a real verb in the capture name does emit a site"
    );

    // A verb spelled in the source outranks one declared by the capture name.
    let ranked = sites(
        r#"(call_expression
             function: (identifier) @invoke.http.method.get
             arguments: (arguments
               . (_) @invoke.http.arg
               . (object (pair value: (string (string_fragment) @invoke.http.method)))))"#,
    );
    assert_eq!(ranked.len(), 1, "one site");
    assert_eq!(
        ranked[0].slots.get(METHOD_SLOT).map(String::as_str),
        Some("POST"),
        "the source-read POST outranks the name-declared GET — a query binding \
         both must never downgrade a spelled-out verb"
    );

    // Two conflicting DECLARED verbs on one match: the first capture declaration
    // wins, so the resolved verb never depends on which node tree-sitter reports
    // first. Both captures are real HTTP verbs, so `is_http_method` cannot be
    // what decides it.
    let first_wins = sites(
        r#"(call_expression
             function: (identifier) @invoke.http.method.head
             arguments: (arguments
               . (_) @invoke.http.arg
               . (object) @invoke.http.method.put))"#,
    );
    assert_eq!(first_wins.len(), 1, "one site");
    assert_eq!(
        first_wins[0].slots.get(METHOD_SLOT).map(String::as_str),
        Some("head"),
        "the FIRST name-declared verb wins — resolving by capture order would \
         make the verb depend on node position (FR-PL-04 droppable queries)"
    );
}

/// A verb-less shape reports the **call's** line, not its path argument's
/// ([FR-WS-08], [NFR-RA-05]).
///
/// The name-declared branch has no `@invoke.http.method` node to attribute to,
/// so `collect_invocation_sites` anchors on the node the declaring capture bound
/// — for the shipped `fetch` pattern, the callee. Anchoring on the path argument
/// instead put a wrapped call's reference one line below the call, disagreeing
/// with every method-bearing shape and with the other four language arms, and
/// sending anyone navigating from the reference to the wrong line.
///
/// Also pins the site's owning symbol, which the same anchor decides: the two
/// travel together, so a regression in one is a regression in both.
///
/// [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[test]
#[cfg(feature = "lang-typescript")]
fn a_verb_less_call_is_attributed_to_the_call_not_to_its_argument() {
    // The call opens on line 2; its sole argument is on line 3.
    const WRAPPED: &str = r#"export async function listUsers() {
    return fetch(
        "/users"
    );
}"#;

    // Driven through the real `extract` path, not `ts_invocation_sites`: that
    // helper passes empty `decls`/`symbols`, so every site there falls back to
    // the file module and the enclosing-symbol half would be unfalsifiable.
    for ext in TS_EXTENSIONS {
        let facts = extract_lang(ext, &format!("src/client.{ext}"), WRAPPED);
        let refs: Vec<_> = facts
            .refs
            .iter()
            .filter(|r| r.relation == Some(crate::model::ArtifactRelation::HttpClientCall))
            .collect();
        assert_eq!(refs.len(), 1, "one client-call reference in {ext}");
        assert_eq!(refs[0].target, "GET /users", "{ext}");
        assert_eq!(
            refs[0].line, 2,
            "the reference reports the `fetch` line, not the wrapped \
             argument's line 3 ({ext})"
        );
        assert!(
            refs[0].source.to_string().contains("listUsers"),
            "the reference is attributed to its enclosing declaration, not the \
             file module ({ext}): {:?}",
            refs[0].source
        );
    }
}


// ── S-345 / FR-WS-08 / CR-108: Go slot-level refusal reasons ────────────────
//
// The rest of the Go arm's suite lives in `tests/go_invocations.rs`. These two
// stay here because they need the crate-private `collect_invocation_sites` and
// `classify_client_call` — see [`invocation_sites`]. They are the Go twins of
// `a_template_literal_reaches_the_interpreter_as_a_dynamic_path` and
// `a_non_normalizing_absolute_path_reaches_the_interpreter_as_a_static_path`.

/// FR-WS-08 shared negative case 2 for **Go**, pinned at slot level.
///
/// `tests/go_invocations.rs::a_runtime_composed_path_is_not_captured` proves
/// only that a runtime-composed path emits nothing — which is equally true if
/// the query never matched it. The contract's actual obligation is that the
/// query DOES hand the site over, carrying the dynamic-path marker, so the
/// refusal is `base-url-runtime` rather than a silent non-match. The Go query
/// header states exactly this ("A composed path still yields a SITE, so the arm
/// classifies it base-url-runtime instead of never seeing it"); without this
/// test that claim is unpinned, and narrowing the query's `(_) @invoke.http.arg`
/// to a string-literal alternation — a plausible "tighten the anchor" edit —
/// would delete Go's coverage reasons with every existing test still green.
#[test]
#[cfg(feature = "lang-go")]
fn a_runtime_composed_go_path_reaches_the_interpreter_as_a_dynamic_path() {
    use crate::resolve::http_client_call::{DYNAMIC_PATH_SLOT, METHOD_SLOT, PATH_SLOT};

    // Both Go anchor shapes: verb-as-method-name and verb-as-constructor-arg.
    for (body, verb) in [
        ("http.Get(url)", "Get"),
        (r#"req, _ := http.NewRequest("GET", url, nil); c.Do(req)"#, "GET"),
    ] {
        let source = format!(
            r#"package client

import "net/http"

func Fetch(c *http.Client, url string) {{
	{body}
}}
"#
        );
        let sites = invocation_sites("go", &source);
        assert_eq!(sites.len(), 1, "exactly one invocation site for {body:?}");
        let slots = &sites[0].slots;
        assert_eq!(
            slots.get(METHOD_SLOT).map(String::as_str),
            Some(verb),
            "the verb still reaches the interpreter — only the path is dynamic"
        );
        assert!(
            slots.contains_key(DYNAMIC_PATH_SLOT),
            "a runtime-composed path must carry the dynamic-path marker (that \
             marker is what makes the refusal `base-url-runtime` rather than a \
             silent non-match) for {body:?}: {slots:?}"
        );
        assert!(
            !slots.contains_key(PATH_SLOT),
            "no static path is guessed from a composed one: {slots:?}"
        );
        assert!(
            crate::resolve::http_client_call::render_client_call_target(slots).is_none(),
            "and the interpreter therefore renders no target"
        );
    }
}

/// FR-WS-08 shared negative case 3 for **Go**, pinned at slot level — and the
/// relative-literal case beside it, which refuses for a *different* reason.
///
/// The refs-level Go tests prove only that both emit nothing, so they cannot
/// show that the two carry distinct coverage reasons. Case 3's whole point is
/// the distinct reason, so it is asserted where the reason exists: the query
/// hands over a **static** `path` slot, and the refusal comes from classifying
/// it, not from failing to match.
#[test]
#[cfg(feature = "lang-go")]
fn a_non_normalizing_go_path_reaches_the_interpreter_as_a_static_path() {
    use crate::resolve::http_client_call::{
        classify_client_call, ClientCallRefusal, DYNAMIC_PATH_SLOT, PATH_SLOT,
    };

    for (literal, refusal) in [
        ("/v{version}/users", ClientCallRefusal::PathNotComposed),
        ("users/{id}", ClientCallRefusal::BaseUrlRuntime),
    ] {
        let source = format!(
            r#"package client

import "net/http"

func ListUsers() {{ http.Get("{literal}") }}
"#
        );
        let sites = invocation_sites("go", &source);
        assert_eq!(sites.len(), 1, "one site reaches the interpreter for {literal}");
        let slots = &sites[0].slots;
        assert_eq!(
            slots.get(PATH_SLOT).map(String::as_str),
            Some(literal),
            "the static literal is handed over verbatim, not pre-judged: {slots:?}"
        );
        assert!(
            !slots.contains_key(DYNAMIC_PATH_SLOT),
            "a static literal never carries the dynamic marker: {slots:?}"
        );
        assert_eq!(
            classify_client_call(slots),
            Err(refusal),
            "the interpreter's own refusal reason for {literal}"
        );
    }
}
