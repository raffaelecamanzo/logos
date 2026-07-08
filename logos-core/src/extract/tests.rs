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
                framework_methods: std::collections::BTreeMap::new(),
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

/// Extract one in-memory source string at `path`, with a fixed coordinate.
fn extract_src(path: &str, source: &str) -> Facts {
    let reg = registry();
    let plugin = reg.for_extension("rs").expect("rust grammar");
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
    // Two `hello` nodes exist (the trait default and the impl method); at least
    // one is a Method (the impl one) and at least one is a Function (the default).
    let hellos: Vec<NodeKind> = facts
        .nodes
        .iter()
        .filter(|n| n.name == "hello")
        .map(|n| n.kind)
        .collect();
    assert!(
        hellos.contains(&NodeKind::Method),
        "the trait-impl `hello` must be a Method, got {hellos:?}"
    );
    assert!(
        hellos.contains(&NodeKind::Function),
        "the trait *default* `hello` must stay a Function, got {hellos:?}"
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
