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
use crate::extract::broker::{Callee, Forwarded, ForwardingOutcome};
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
                import_specifier: crate::plugin::ImportSpecifier::Name,
                specifier_extensions: Vec::new(),
                package_modules: None,
                complexity_keywords: Vec::new(),
                nesting_block_kinds: Vec::new(),
                body_node_kinds: Vec::new(),
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
                properties: None,
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

/// The symbol of the node named `name` with `kind`.
fn symbol_of_kind<'a>(facts: &'a Facts, name: &str, kind: NodeKind) -> &'a str {
    facts
        .nodes
        .iter()
        .find(|n| n.name == name && n.kind == kind)
        .map(|n| n.symbol.as_str())
        .unwrap_or_else(|| panic!("no {kind:?} named {name}: {:?}", facts.nodes))
}

#[test]
fn same_name_siblings_of_one_family_are_numbered_together_across_kinds() {
    // S-512: a Go method and free function of one name both render the method
    // descriptor, so they are numbered as one family in canonical (start-byte)
    // order — the method first — rather than each taking ordinal 0.
    let go = extract_lang(
        "go",
        "p/run.go",
        "package p\n\ntype T struct{}\n\nfunc (T) F() {}\n\nfunc F() {}\n",
    );
    assert!(symbol_of_kind(&go, "F", NodeKind::Method).ends_with("`run.go`/F()."));
    assert!(symbol_of_kind(&go, "F", NodeKind::Function).ends_with("`run.go`/F(1)."));

    // A TS interface and class of one name share the type descriptor; the class
    // takes the trailing meta, and its member's chain carries it.
    let ts = extract_lang(
        "ts",
        "src/x.ts",
        "export interface X { a: number }\nexport class X { b = 1; }\n",
    );
    assert!(symbol_of_kind(&ts, "X", NodeKind::Interface).ends_with("`x.ts`/X#"));
    let class = symbol_of_kind(&ts, "X", NodeKind::Class);
    assert!(class.ends_with("`x.ts`/X#1:"), "{class}");
    assert!(
        ts.nodes.iter().any(|n| n.symbol.as_str() == format!("{class}b.")),
        "the merged class's member nests under its disambiguated symbol: {:?}",
        ts.nodes
    );
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "emitted one symbol for two declarations")]
fn one_symbol_emitted_twice_in_a_file_trips_the_debug_assertion() {
    // The test-build guard behind every extraction in the suite (S-512).
    let mut facts = extract_src("src/lib.rs", "fn a() {}\n");
    let twin = facts
        .nodes
        .iter()
        .find(|n| n.name == "a")
        .expect("fn a")
        .clone();
    facts.nodes.push(twin);
    debug_assert_unique_symbols(&facts);
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

/// The arm's **reference** targets — every captured row that named a route
/// (S-374).
///
/// [`http_client_call_targets`] is deliberately left unfiltered, the way
/// `extract::broker`'s `targets` is: a keyless refusal row is a real ledger row
/// and a helper that hid it would let the refusal path regress unnoticed. So
/// tests about *references* read this, tests about *refusals* read
/// [`http_client_call_refusals`], and the two are asserted side by side.
fn http_client_call_references(facts: &Facts) -> Vec<String> {
    http_client_call_targets(facts)
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect()
}

/// The declarations the arm recorded a **keyless refusal** for, in ledger order
/// (S-374, [FR-WS-08] AC2).
///
/// A recorded refusal is an `HttpClientCall` row whose target is empty: no
/// fabricated template, so it promotes no node, keys nothing and binds nothing,
/// and the [FR-WS-05] tier reports it `base-url-runtime`.
fn http_client_call_refusals(facts: &Facts) -> Vec<String> {
    facts
        .refs
        .iter()
        .filter(|r| {
            r.relation == Some(crate::model::ArtifactRelation::HttpClientCall)
                && r.target.is_empty()
        })
        .map(|r| r.source.as_str().to_string())
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
/// arm's normalizer returns `None`, so **no reference** (base-url-runtime, never
/// approximately matched) and, since S-374, **one keyless refusal row** naming
/// the calling declaration.
///
/// The two halves are asserted together on purpose. "Emits no reference" and
/// "leaves no trace" used to be the same statement, and conflating them is what
/// left [FR-WS-08] AC2's second half unmet for four sprints: the site vanished,
/// so an estate whose paths are all composed at runtime was indistinguishable
/// from one with no outbound calls at all ([NFR-CC-04]).
#[test]
fn a_runtime_composed_client_call_is_refused_and_recorded() {
    let bare = extract_src(
        "src/c.rs",
        r#"use reqwest::Client;
async fn f(client: Client, url: String) { client.get(url).await; }"#,
    );
    assert!(
        http_client_call_references(&bare).is_empty(),
        "a bare-variable path is base-url-runtime: {:?}",
        http_client_call_targets(&bare)
    );
    assert_eq!(
        http_client_call_refusals(&bare).len(),
        1,
        "the declined site leaves one keyless row: {:?}",
        bare.refs
    );

    let composed = extract_src(
        "src/c.rs",
        r#"use reqwest::Client;
async fn f(client: Client, base: String) { client.get(format!("{base}/users")).await; }"#,
    );
    assert!(
        http_client_call_references(&composed).is_empty(),
        "a format!-composed path is base-url-runtime: {:?}",
        http_client_call_targets(&composed)
    );
    assert_eq!(http_client_call_refusals(&composed).len(), 1);
}

/// A catch-all/non-normalizable absolute literal, and a relative (base-URL) one,
/// are refused — path-not-composed / base-url-runtime, never approximated.
///
/// **This is also where the two refusals part company (S-374).** Both emit no
/// reference, but only the base-url-runtime one is *recorded*: the coverage tier
/// tells the two apart by whether the row's target is empty, so a keyless row
/// means "no static path was present" by construction and a `path-not-composed`
/// row would need a non-keyless target — a mechanism this story does not build,
/// for the reasons stated on `capture_http_client_call_arm`. The asymmetry is
/// pinned here rather than left to be inferred from an absence.
#[test]
fn a_non_composable_client_call_literal_is_not_captured() {
    let catch_all = extract_src(
        "src/c.rs",
        r#"use reqwest::Client;
async fn f(client: Client) { client.get("/files/{*rest}").await; }"#,
    );
    assert!(
        http_client_call_targets(&catch_all).is_empty(),
        "a catch-all path is path-not-composed — no reference AND, \
         deliberately, no recorded refusal: {:?}",
        http_client_call_targets(&catch_all)
    );

    let relative = extract_src(
        "src/c.rs",
        r#"use reqwest::Client;
async fn f(client: Client) { client.get("users/{id}").await; }"#,
    );
    assert!(
        http_client_call_references(&relative).is_empty(),
        "a relative path has no absolute route prefix: {:?}",
        http_client_call_targets(&relative)
    );
    assert_eq!(
        http_client_call_refusals(&relative).len(),
        1,
        "a relative literal is base-url-runtime, so it IS recorded: {:?}",
        relative.refs
    );
}

/// **S-374 acceptance: a declined call site leaves one keyless row, and the row
/// is inert.** ([FR-WS-08] AC2, [CR-120].)
///
/// The arm's refusal grain, asserted end-to-end through `extract` rather than
/// through the recorder: three refused shapes in three functions are three rows,
/// two refused calls in ONE function are one row, and a function that bound a
/// literal contributes a reference and no refusal beside it.
///
/// The two grains are both real and neither is a bug:
///
/// - the recorder dedups per `(relation, declaration, line)`, so one site never
///   records twice;
/// - `dedup_sort_refs` then keys on `(source, target, form, kind, relation)` and
///   ignores `line`, so two refused sites in one declaration reach the ledger as
///   **one** row.
///
/// That second collapse is why the reference-workspace figure is reconciled at a
/// declaration grain and not a call grain: a method with four composed calls
/// contributes one row, not four. Pinned here so the measurement's denominator
/// is not mistaken for a site count.
///
/// Inertness is asserted on the row's own shape — an empty target on the `Path`
/// form — because that is what makes it inert everywhere downstream: `route_key`
/// refuses an empty `"METHOD /template"`, so the binder resolves nothing, the
/// bridge keys nothing, and no promotion pass has a name to promote.
#[test]
fn a_refused_client_call_records_one_keyless_row_per_declaration() {
    let facts = extract_src(
        "src/c.rs",
        r#"use reqwest::Client;
async fn bound(client: Client) { client.get("/health").await; }
async fn bare(client: Client, url: String) { client.get(url).await; }
async fn composed(client: Client, base: String) { client.get(format!("{base}/x")).await; }
async fn relative(client: Client) { client.get("users/{id}").await; }
async fn twice(client: Client, a: String, b: String) {
    client.get(a).await;
    client.post(b).await;
}"#,
    );

    assert_eq!(
        http_client_call_references(&facts),
        vec!["GET /health".to_string()],
        "the one static absolute literal is the only reference: {:?}",
        facts.refs
    );

    // Asserted as an exact sorted set, not as a count plus a membership test: a
    // count of four with an `all(|s| s.contains(a) || s.contains(b) || …)`
    // membership test passes on four rows that all name `twice`, which is
    // precisely the mis-attribution and the failed collapse this is about.
    let mut refusals = http_client_call_refusals(&facts);
    refusals.sort();
    assert_eq!(
        refusals,
        vec![
            "logos cargo logos-core 0.1.0 src/`c.rs`/bare().".to_string(),
            "logos cargo logos-core 0.1.0 src/`c.rs`/composed().".to_string(),
            "logos cargo logos-core 0.1.0 src/`c.rs`/relative().".to_string(),
            "logos cargo logos-core 0.1.0 src/`c.rs`/twice().".to_string(),
        ],
        "one row per declining declaration — three refusing functions plus \
         `twice`, whose two calls collapse to one; each attributed to its own \
         calling function and never to the file module, and `bound` absent \
         because a site that bound records no refusal beside its reference: \
         {:?}",
        facts.refs
    );

    // The row's shape — this is the whole of its inertness.
    let keyless: Vec<&crate::extract::RefFact> = facts
        .refs
        .iter()
        .filter(|r| {
            r.relation == Some(crate::model::ArtifactRelation::HttpClientCall)
                && r.target.is_empty()
        })
        .collect();
    for row in &keyless {
        assert_eq!(row.form, crate::model::RefForm::Path);
        assert_eq!(row.kind, EdgeKind::ArtifactBinding);
        assert!(row.alias.is_none());
        assert!(row.line > 0, "the row names the call's line: {row:?}");
        // No fabricated template, not even the operand's source text — which is
        // exactly why nothing downstream can promote or bind it.
        assert!(row.target.is_empty());
        assert!(
            crate::resolve::route_template::route_key(&row.target).is_none(),
            "an empty target never keys, so it never resolves a route"
        );
    }
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

/// [`ts_client_call_targets`] restricted to the arm's **references** — every row
/// that named a route, excluding the keyless refusal rows S-374 records.
///
/// The ts/tsx parity assertion above covers both populations, so a query that
/// refused in one dialect and captured in the other still fails there.
#[cfg(feature = "lang-typescript")]
fn ts_client_call_references(source: &str) -> Vec<String> {
    ts_client_call_targets(source)
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect()
}

/// The number of keyless refusal rows the TypeScript/TSX arms record for
/// `source` — asserted equal across the two dialects for the same reason the
/// targets are (S-374).
#[cfg(feature = "lang-typescript")]
fn ts_client_call_refusals(source: &str) -> usize {
    let ts = http_client_call_refusals(&extract_lang("ts", "src/client.ts", source)).len();
    let tsx = http_client_call_refusals(&extract_lang("tsx", "src/client.tsx", source)).len();
    assert_eq!(
        ts, tsx,
        "the typescript and tsx queries must REFUSE identically too — \
         ts={ts} tsx={tsx}, source:\n{source}"
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
///
/// Since S-374 each of these sites also leaves **one keyless refusal row**, so
/// the reason reaches the [FR-WS-05] tier instead of the site vanishing. That
/// half is asserted here too: the reference contract and the refusal contract
/// are one behaviour and testing only the first is how the second went
/// unimplemented.
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
            ts_client_call_references(source).is_empty(),
            "a runtime-composed path is base-url-runtime, never approximated: {source}"
        );
        assert_eq!(
            ts_client_call_refusals(source),
            1,
            "the declined site is recorded, not swallowed: {source}"
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
/// absolute route prefix, each emit **no reference**. The classification itself
/// is the shared interpreter's and is fixture-pinned there; this proves the
/// TypeScript query feeds it the slots it expects.
///
/// The two cases differ in what they *record* (S-374), and the pair is kept in
/// one test so the difference is visible: the catch-all is `path-not-composed`
/// and leaves nothing, the relative literal is `base-url-runtime` and leaves one
/// keyless row. Same reference outcome, different coverage outcome.
#[test]
#[cfg(feature = "lang-typescript")]
fn a_non_composable_typescript_path_literal_is_not_captured() {
    for (source, refusals) in [
        (
            r#"import axios from "axios";
export async function raw() { return axios.get("/files/{*rest}"); }"#,
            0,
        ),
        (
            r#"export async function raw() { return fetch("users/{id}"); }"#,
            1,
        ),
    ] {
        assert!(
            ts_client_call_references(source).is_empty(),
            "a non-normalizing or relative literal is refused: {source}"
        );
        assert_eq!(
            ts_client_call_refusals(source),
            refusals,
            "only the base-url-runtime half is recorded: {source}"
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
#[cfg(any(
    feature = "lang-typescript",
    feature = "lang-go",
    feature = "lang-kotlin",
    feature = "lang-c-sharp",
    feature = "lang-rust",
    feature = "lang-java",
    feature = "lang-python",
    feature = "lang-ruby",
    feature = "lang-php"
))]
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
    // `collect_invocation_sites` also returns each call's path-operand byte range
    // (the S-374 refusal-site grain); these fixtures assert on slots and lines,
    // so the range is dropped here rather than threaded through every caller.
    collect_invocation_sites(
        query,
        tree.root_node(),
        source.as_bytes(),
        &[],
        &[],
        Some(&module),
        // The plugin's OWN table, not an empty one (S-346). For a language that
        // declares no rows this is the pass-through these fixtures always had;
        // for C# it is the difference between reaching the interpreter and
        // producing zero sites, so an empty map here would make the C# twins
        // below vacuous in the opposite direction.
        &plugin.semantics().invocation_methods,
        // No configuration-binding view: these fixtures are single files, and an
        // accessor's owning class is declared in another one by construction
        // (S-397). The fixtures that DO exercise the accessor hop drive
        // `extract_files`, which is where the member's index is built.
        None,
    )
    .into_iter()
    .map(|call| call.site)
    .collect()
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
            // Single-file fixture; see `invocation_sites` for why no
            // configuration-binding view is supplied here.
            None,
        )
        .into_iter()
        .map(|call| call.site)
        .collect()
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


// ── S-342 / FR-WS-08 / CR-108: Kotlin slot-level refusal reasons ────────────
//
// The rest of the Kotlin arm's suite lives in `tests/kotlin_http_client_call.rs`.
// These two stay here for the same reason the Go pair above does — they need the
// crate-private `collect_invocation_sites` and `classify_client_call`.
//
// Kotlin needs them more than its siblings do. `tree-sitter-kotlin-ng` does not
// model a bare `$name` interpolation: it splits the fragment at the `$` into
// plain `string_content` children, which `static_string_literal` happily
// concatenates back into what looks like a static literal. The query's answer is
// a `(#not-match? … "[$]")` guard plus a companion pattern that re-captures the
// ARGUMENT node so the site is still handed over as dynamic. Every refs-level
// assertion for that is `.is_empty()`, which is equally satisfied by "the query
// never matched" — so without these tests the companion pattern could be deleted
// and Kotlin would lose its coverage reasons silently.

/// FR-WS-08 shared negative case 2 for **Kotlin**, pinned at slot level.
///
/// Both interpolation spellings must reach the interpreter carrying the
/// dynamic-path marker: the braced `${…}` form (which the grammar models as an
/// `interpolation` node) and the bare `$name` form (which it does not model at
/// all). The third fixture is the one that motivates the guard — an interpolated
/// path that reads back **absolute**, and so would otherwise be bound as a static
/// template rather than refused.
#[test]
#[cfg(feature = "lang-kotlin")]
fn a_kotlin_string_template_reaches_the_interpreter_as_a_dynamic_path() {
    use crate::resolve::http_client_call::{DYNAMIC_PATH_SLOT, METHOD_SLOT, PATH_SLOT};

    for literal in ["\"$base/users\"", "\"${base}/users\"", "\"/users/$id/roles\"", "path"] {
        let source = format!(
            r#"package client

import org.springframework.web.client.RestClient

class Calls(private val restClient: RestClient, private val base: String) {{
    fun fetch(id: String, path: String): String =
        restClient.get().uri({literal}).retrieve().body(String::class.java)
}}
"#
        );
        let sites = invocation_sites("kt", &source);
        assert_eq!(sites.len(), 1, "exactly one invocation site for {literal}");
        let slots = &sites[0].slots;
        assert_eq!(
            slots.get(METHOD_SLOT).map(String::as_str),
            Some("get"),
            "the verb still reaches the interpreter — only the path is dynamic"
        );
        assert!(
            slots.contains_key(DYNAMIC_PATH_SLOT),
            "a runtime-composed Kotlin path must carry the dynamic-path marker \
             (that marker is what makes the refusal `base-url-runtime` rather \
             than a silent non-match) for {literal}: {slots:?}"
        );
        assert!(
            !slots.contains_key(PATH_SLOT),
            "no static path is guessed from a composed one: {slots:?}"
        );
        assert_eq!(
            crate::resolve::http_client_call::classify_client_call(slots),
            Err(crate::resolve::http_client_call::ClientCallRefusal::BaseUrlRuntime),
            "and the interpreter's reason is base-url-runtime for {literal}"
        );
    }
}

/// FR-WS-08 shared negative case 3 for **Kotlin**, pinned at slot level — and
/// the relative-literal case beside it, which refuses for a *different* reason.
///
/// The refs-level tests prove only that both emit nothing, so they cannot show
/// that the two carry distinct coverage reasons. Case 3's whole point is the
/// distinct reason, so it is asserted where the reason exists: the query hands
/// over a **static** `path` slot, and the refusal comes from classifying it, not
/// from failing to match.
#[test]
#[cfg(feature = "lang-kotlin")]
fn a_non_normalizing_kotlin_path_reaches_the_interpreter_as_a_static_path() {
    use crate::resolve::http_client_call::{
        classify_client_call, ClientCallRefusal, DYNAMIC_PATH_SLOT, PATH_SLOT,
    };

    for (literal, refusal) in [
        ("/files/**", ClientCallRefusal::PathNotComposed),
        ("users/me", ClientCallRefusal::BaseUrlRuntime),
    ] {
        let source = format!(
            r#"package client

import org.springframework.web.client.RestClient

class Calls(private val restClient: RestClient) {{
    fun fetch(): String =
        restClient.get().uri("{literal}").retrieve().body(String::class.java)
}}
"#
        );
        let sites = invocation_sites("kt", &source);
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

// ── S-346 / FR-WS-08 / CR-108: C# slot-level refusal reasons ────────────────
//
// The rest of the C# arm's suite lives in `tests/c_sharp_invocations.rs`. These
// stay here for the same reason the Go pair above does — `collect_invocation_sites`
// and `classify_client_call` are crate-private (see [`invocation_sites`]) — and
// they are the third arm to adopt the obligation sprint-63's review ruled on:
// FR-WS-08's negative cases 2 and 3 require "emits no reference AND surfaces the
// reason", which a refs-level `is_empty()` cannot distinguish from "the query
// never matched".

/// FR-WS-08 shared negative case 2 for **C#**, pinned at slot level, across both
/// of the language's anchor shapes.
///
/// `tests/c_sharp_invocations.rs::a_runtime_composed_path_is_not_captured` proves
/// only that an interpolated path emits nothing — equally true if the query never
/// matched. The C# query header claims the site IS handed over ("still yields a
/// SITE, so the arm classifies it base-url-runtime"); without this test that claim
/// is unpinned, and narrowing `(argument (_) @invoke.http.arg)` to a literal
/// alternation would delete C#'s coverage reasons with every other test green.
#[test]
#[cfg(feature = "lang-c-sharp")]
fn a_runtime_composed_c_sharp_path_reaches_the_interpreter_as_a_dynamic_path() {
    use crate::resolve::http_client_call::{DYNAMIC_PATH_SLOT, METHOD_SLOT, PATH_SLOT};

    // Verb-as-method-name, and verb-as-constructor-argument (the named constant
    // this story's normalizer table resolves).
    for (call, verb) in [
        (r#"client.GetAsync($"{baseUrl}/users");"#, "GET"),
        (r#"client.GetAsync(baseUrl);"#, "GET"),
        (
            r#"client.SendAsync(new HttpRequestMessage(HttpMethod.Delete, $"{baseUrl}/users"));"#,
            "DELETE",
        ),
    ] {
        let source = format!(
            "using System.Net.Http;\n\npublic class C\n{{\n    HttpClient client;\n\
             \x20   string baseUrl;\n    public void M() {{ {call} }}\n}}\n"
        );
        let sites = invocation_sites("cs", &source);
        assert_eq!(sites.len(), 1, "exactly one invocation site for {call:?}");
        let slots = &sites[0].slots;
        assert_eq!(
            slots.get(METHOD_SLOT).map(String::as_str),
            Some(verb),
            "the verb still reaches the interpreter, normalized through \
             [invocation_methods] — only the path is dynamic: {slots:?}"
        );
        assert!(
            slots.contains_key(DYNAMIC_PATH_SLOT),
            "a runtime-composed path must carry the dynamic-path marker (that \
             marker is what makes the refusal `base-url-runtime` rather than a \
             silent non-match) for {call:?}: {slots:?}"
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

/// FR-WS-08 shared negative case 3 for **C#**, pinned at slot level — and the
/// relative-literal case beside it, which refuses for a *different* reason.
/// Case 3's whole point is the distinct reason, so it is asserted where the
/// reason exists.
#[test]
#[cfg(feature = "lang-c-sharp")]
fn a_non_normalizing_c_sharp_path_reaches_the_interpreter_as_a_static_path() {
    use crate::resolve::http_client_call::{
        classify_client_call, ClientCallRefusal, DYNAMIC_PATH_SLOT, PATH_SLOT,
    };

    for (literal, refusal) in [
        ("/v{version}/users", ClientCallRefusal::PathNotComposed),
        ("users/{id}", ClientCallRefusal::BaseUrlRuntime),
    ] {
        let source = format!(
            "using System.Net.Http;\n\npublic class C\n{{\n    HttpClient client;\n\
             \x20   public void M() {{ client.GetAsync(\"{literal}\"); }}\n}}\n"
        );
        let sites = invocation_sites("cs", &source);
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

/// The `[invocation_methods]` **value** half, which no descriptor can exercise:
/// a mistyped verb (`"GTE"`) captures nothing rather than inventing a method.
/// Both `plugin.toml` and `PluginManifest::invocation_methods` state this;
/// nothing else asserts it, because a real descriptor's values are all valid.
#[test]
#[cfg(feature = "lang-c-sharp")]
fn a_table_value_that_is_not_an_http_verb_captures_nothing() {
    use std::collections::BTreeMap;

    let reg = registry();
    let plugin = reg.for_extension("cs").expect("c-sharp grammar");
    let query = plugin.query("invocations").expect("ships an invocations query");
    let source = "using System.Net.Http;\n\npublic class C\n{\n    HttpClient client;\n\
                  \x20   public void M() { client.GetAsync(\"/users\"); }\n}\n";
    let ctx = SymbolContext::cargo("logos-core", "0.1.0");
    let facts = extract(&FileInput::new("src/client.cs", source), plugin, &ctx);
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

    let sites = |table: BTreeMap<String, String>| {
        collect_invocation_sites(
            query,
            tree.root_node(),
            source.as_bytes(),
            &[],
            &[],
            Some(&module),
            &table,
            // Single-file fixture; see `invocation_sites` for why no
            // configuration-binding view is supplied here.
            None,
        )
    };

    let good = sites(BTreeMap::from([("GetAsync".into(), "GET".into())]));
    assert_eq!(good.len(), 1, "a well-formed row yields the site");

    let typo = sites(BTreeMap::from([("GetAsync".into(), "GTE".into())]));
    assert!(
        typo.is_empty(),
        "a mistyped verb is dropped by is_http_method, never invented; got {} sites",
        typo.len()
    );
}

// ── The FR-WS-08 case-2 obligation, closed over EVERY landed arm ────────────
//
// Cases 2 and 3 of the shared negative-case fixture contract require more than
// "emits no reference": they require the query to hand the site OVER, so the
// refusal carries a coverage reason (`base-url-runtime` / `path-not-composed`)
// rather than being a silent non-match. A refs-level `is_empty()` cannot tell
// those two apart, and every arm's query header claims the stronger version.
//
// Four arms adopted a slot-level pin for it under task review — typescript
// (S-343), go (S-INT), kotlin (S-342) and c-sharp (S-346), each with its own
// richly-documented test above. Four did not: java, python, ruby and php still
// claim the reason in their headers with nothing asserting it. The obligation
// was carried in an ENUMERATED `#[cfg(any(…))]` list, and a list like that
// cannot notice the arm that never joined it.
//
// So the roster below is closed against the plugin set instead: every plugin
// declaring `invocations` must appear here, and the assertion fails naming the
// language if a tenth arm lands without a row. The per-language tests above stay
// as they are — this is the guard that no arm is missing one, not a replacement
// for what they document.

/// One landed arm's runtime-composed-path fixture: the language's own spelling
/// of "a client call whose path is not a static literal".
#[cfg(all(
    feature = "lang-rust",
    feature = "lang-java",
    feature = "lang-kotlin",
    feature = "lang-go",
    feature = "lang-python",
    feature = "lang-typescript",
    feature = "lang-c-sharp",
    feature = "lang-ruby",
    feature = "lang-php"
))]
struct DynamicPathCase {
    /// The plugin name, matched against the registry's own roster.
    plugin: &'static str,
    /// The extension routing the fixture to that plugin's grammar.
    ext: &'static str,
    /// The verb the query must still hand over — only the PATH is dynamic.
    verb: &'static str,
    source: &'static str,
}

#[cfg(all(
    feature = "lang-rust",
    feature = "lang-java",
    feature = "lang-kotlin",
    feature = "lang-go",
    feature = "lang-python",
    feature = "lang-typescript",
    feature = "lang-c-sharp",
    feature = "lang-ruby",
    feature = "lang-php"
))]
const DYNAMIC_PATH_CASES: &[DynamicPathCase] = &[
    DynamicPathCase {
        plugin: "rust",
        ext: "rs",
        verb: "get",
        source: "fn call(client: &Client, url: &str) { let _ = client.get(url); }\n",
    },
    DynamicPathCase {
        plugin: "java",
        ext: "java",
        verb: "get",
        source: "public class Calls {\n    private RestClient restClient;\n\
                 \x20   String a(String path) { return restClient.get().uri(path).retrieve().body(String.class); }\n}\n",
    },
    DynamicPathCase {
        plugin: "kotlin",
        ext: "kt",
        verb: "get",
        source: "class Calls(private val restClient: RestClient) {\n\
                 \x20   fun a(path: String): String = restClient.get().uri(path).retrieve().body(String::class.java)\n}\n",
    },
    DynamicPathCase {
        plugin: "go",
        ext: "go",
        verb: "Get",
        source: "package client\n\nfunc Call(url string) { http.Get(url) }\n",
    },
    DynamicPathCase {
        plugin: "python",
        ext: "py",
        verb: "get",
        source: "def call(session, url):\n    return session.get(url)\n",
    },
    DynamicPathCase {
        plugin: "typescript",
        ext: "ts",
        verb: "get",
        source: "export function call(url: string) { return axios.get(url); }\n",
    },
    DynamicPathCase {
        plugin: "tsx",
        ext: "tsx",
        verb: "get",
        source: "export function call(url: string) { return axios.get(url); }\n",
    },
    DynamicPathCase {
        plugin: "c-sharp",
        ext: "cs",
        verb: "GET",
        source: "public class C\n{\n    HttpClient client;\n    string baseUrl;\n\
                 \x20   public void M() { client.GetAsync(baseUrl); }\n}\n",
    },
    DynamicPathCase {
        plugin: "ruby",
        ext: "rb",
        verb: "get",
        source: "def call(conn, path)\n  conn.get(path)\nend\n",
    },
    DynamicPathCase {
        plugin: "php",
        ext: "php",
        verb: "get",
        source: "<?php\n$client = new Client();\n$client->get($url);\n",
    },
];

/// **Every landed `invocations` arm hands a runtime-composed path OVER to the
/// interpreter** — FR-WS-08 shared negative case 2, pinned for all of them at
/// once ([NFR-RA-05]).
///
/// The roster is closed against the plugin set, not written out: a plugin
/// declaring `invocations` with no row here fails, so a tenth language cannot
/// land an arm and skip the obligation the way java, python, ruby and php did
/// when it lived in an enumerated `#[cfg(any(…))]` list. (Found by the sprint-63
/// review: four arms claimed `base-url-runtime` in their query headers with
/// nothing asserting the site ever reached the interpreter to be refused.)
///
/// What each row proves, in the language's own spelling:
/// - exactly one site — the query matched, so the refusal below is the
///   interpreter's and not a silent non-match;
/// - the VERB still reaches the interpreter (normalized through
///   `[invocation_methods]` where the language declares one) — only the path is
///   dynamic;
/// - the dynamic-path marker is set and no static path is guessed, which is what
///   makes the reason `base-url-runtime` rather than nothing at all.
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[test]
#[cfg(all(
    feature = "lang-rust",
    feature = "lang-java",
    feature = "lang-kotlin",
    feature = "lang-go",
    feature = "lang-python",
    feature = "lang-typescript",
    feature = "lang-c-sharp",
    feature = "lang-ruby",
    feature = "lang-php"
))]
fn every_landed_invocations_arm_hands_a_dynamic_path_to_the_interpreter() {
    use crate::resolve::http_client_call::{
        classify_client_call, ClientCallRefusal, DYNAMIC_PATH_SLOT, METHOD_SLOT, PATH_SLOT,
    };

    // The roster, closed against the plugin set: no arm may be absent from it.
    let reg = registry();
    let mut declaring: Vec<&str> = reg
        .iter()
        .filter(|p| p.capabilities().iter().any(|c| c == "invocations"))
        .map(|p| p.name())
        .collect();
    declaring.sort_unstable();
    let mut covered: Vec<&str> = DYNAMIC_PATH_CASES.iter().map(|c| c.plugin).collect();
    covered.sort_unstable();
    assert_eq!(
        covered, declaring,
        "every plugin declaring `invocations` needs a DYNAMIC_PATH_CASES row — \
         FR-WS-08 case 2 requires the query to hand the site OVER so the refusal \
         carries a coverage reason, and a refs-level `is_empty()` in the arm's \
         own suite cannot show that"
    );

    for case in DYNAMIC_PATH_CASES {
        let sites = invocation_sites(case.ext, case.source);
        assert_eq!(
            sites.len(),
            1,
            "{}: exactly one invocation site must reach the interpreter",
            case.plugin
        );
        let slots = &sites[0].slots;
        assert_eq!(
            slots.get(METHOD_SLOT).map(String::as_str),
            Some(case.verb),
            "{}: the verb still reaches the interpreter — only the path is \
             dynamic: {slots:?}",
            case.plugin
        );
        assert!(
            slots.contains_key(DYNAMIC_PATH_SLOT),
            "{}: a runtime-composed path must carry the dynamic-path marker — \
             that marker is what makes the refusal `base-url-runtime` rather \
             than a silent non-match: {slots:?}",
            case.plugin
        );
        assert!(
            !slots.contains_key(PATH_SLOT),
            "{}: no static path is guessed from a composed one: {slots:?}",
            case.plugin
        );
        assert_eq!(
            classify_client_call(slots),
            Err(ClientCallRefusal::BaseUrlRuntime),
            "{}: and the interpreter's own reason is base-url-runtime",
            case.plugin
        );
    }
}

/// One landed arm's **static, non-normalizing** path fixture — FR-WS-08 shared
/// negative case 3's counterpart to [`DynamicPathCase`].
#[cfg(all(
    feature = "lang-rust",
    feature = "lang-java",
    feature = "lang-kotlin",
    feature = "lang-go",
    feature = "lang-python",
    feature = "lang-typescript",
    feature = "lang-c-sharp",
    feature = "lang-ruby",
    feature = "lang-php"
))]
struct StaticPathCase {
    plugin: &'static str,
    ext: &'static str,
    verb: &'static str,
    source: &'static str,
}

#[cfg(all(
    feature = "lang-rust",
    feature = "lang-java",
    feature = "lang-kotlin",
    feature = "lang-go",
    feature = "lang-python",
    feature = "lang-typescript",
    feature = "lang-c-sharp",
    feature = "lang-ruby",
    feature = "lang-php"
))]
const STATIC_PATH_CASES: &[StaticPathCase] = &[
    StaticPathCase {
        plugin: "rust",
        ext: "rs",
        verb: "get",
        source: "fn call(client: &Client) { let _ = client.get(\"/files/**\"); }\n",
    },
    StaticPathCase {
        plugin: "java",
        ext: "java",
        verb: "get",
        source: "public class Calls {\n    private RestClient restClient;\n\
                 \x20   String a() { return restClient.get().uri(\"/files/**\").retrieve().body(String.class); }\n}\n",
    },
    StaticPathCase {
        plugin: "kotlin",
        ext: "kt",
        verb: "get",
        source: "class Calls(private val restClient: RestClient) {\n\
                 \x20   fun a(): String = restClient.get().uri(\"/files/**\").retrieve().body(String::class.java)\n}\n",
    },
    StaticPathCase {
        plugin: "go",
        ext: "go",
        verb: "Get",
        source: "package client\n\nfunc Call() { http.Get(\"/files/**\") }\n",
    },
    StaticPathCase {
        plugin: "python",
        ext: "py",
        verb: "get",
        source: "def call(session):\n    return session.get(\"/files/**\")\n",
    },
    StaticPathCase {
        plugin: "typescript",
        ext: "ts",
        verb: "get",
        source: "export function call() { return axios.get(\"/files/**\"); }\n",
    },
    StaticPathCase {
        plugin: "tsx",
        ext: "tsx",
        verb: "get",
        source: "export function call() { return axios.get(\"/files/**\"); }\n",
    },
    StaticPathCase {
        plugin: "c-sharp",
        ext: "cs",
        verb: "GET",
        source: "public class C\n{\n    HttpClient client;\n\
                 \x20   public void M() { client.GetAsync(\"/files/**\"); }\n}\n",
    },
    StaticPathCase {
        plugin: "ruby",
        ext: "rb",
        verb: "get",
        source: "def call(conn)\n  conn.get(\"/files/**\")\nend\n",
    },
    StaticPathCase {
        plugin: "php",
        ext: "php",
        verb: "get",
        source: "<?php\n$client = new Client();\n$client->get('/files/**');\n",
    },
];

/// **Every landed `invocations` arm hands a non-normalizing STATIC path over as a
/// static path** — FR-WS-08 shared negative case 3, the twin of
/// [`every_landed_invocations_arm_hands_a_dynamic_path_to_the_interpreter`]
/// ([NFR-RA-05]).
///
/// Case 3's whole point is that the refusal reason is *different* from case 2's:
/// the query hands over a **static** `path` slot and the refusal comes from
/// classifying it (`path-not-composed`), not from failing to match. A refs-level
/// `is_empty()` shows neither, which is why it is asserted here — and closed
/// against the same plugin-set roster, so no arm can land without it.
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[test]
#[cfg(all(
    feature = "lang-rust",
    feature = "lang-java",
    feature = "lang-kotlin",
    feature = "lang-go",
    feature = "lang-python",
    feature = "lang-typescript",
    feature = "lang-c-sharp",
    feature = "lang-ruby",
    feature = "lang-php"
))]
fn every_landed_invocations_arm_hands_a_non_normalizing_path_to_the_interpreter() {
    use crate::resolve::http_client_call::{
        classify_client_call, ClientCallRefusal, DYNAMIC_PATH_SLOT, METHOD_SLOT, PATH_SLOT,
    };

    let reg = registry();
    let mut declaring: Vec<&str> = reg
        .iter()
        .filter(|p| p.capabilities().iter().any(|c| c == "invocations"))
        .map(|p| p.name())
        .collect();
    declaring.sort_unstable();
    let mut covered: Vec<&str> = STATIC_PATH_CASES.iter().map(|c| c.plugin).collect();
    covered.sort_unstable();
    assert_eq!(
        covered, declaring,
        "every plugin declaring `invocations` needs a STATIC_PATH_CASES row — \
         FR-WS-08 case 3's distinguishing claim is the REASON, and a refs-level \
         `is_empty()` in the arm's own suite cannot show which one fired"
    );

    for case in STATIC_PATH_CASES {
        let sites = invocation_sites(case.ext, case.source);
        assert_eq!(
            sites.len(),
            1,
            "{}: one site must reach the interpreter",
            case.plugin
        );
        let slots = &sites[0].slots;
        assert_eq!(
            slots.get(METHOD_SLOT).map(String::as_str),
            Some(case.verb),
            "{}: the verb reaches the interpreter: {slots:?}",
            case.plugin
        );
        assert_eq!(
            slots.get(PATH_SLOT).map(String::as_str),
            Some("/files/**"),
            "{}: the static literal is handed over verbatim, not pre-judged: {slots:?}",
            case.plugin
        );
        assert!(
            !slots.contains_key(DYNAMIC_PATH_SLOT),
            "{}: a static literal never carries the dynamic marker: {slots:?}",
            case.plugin
        );
        assert_eq!(
            classify_client_call(slots),
            Err(ClientCallRefusal::PathNotComposed),
            "{}: and the interpreter's own reason is path-not-composed, which is \
             what makes case 3 distinct from case 2",
            case.plugin
        );
    }
}

// ── S-397: the accessor capture hop reaches the invocation arm ──────────────

/// The Spring shape the reference estate is built out of: a
/// `@ConfigurationProperties` class in one file, and a `WebClient` call in
/// another whose request path is an accessor on an injected instance of it.
///
/// Two files, because that is the whole point — an accessor's owning class is
/// never in the file that reads it, which is why the index is built once for the
/// member and not per file.
#[cfg(feature = "lang-java")]
const ACCESSOR_PROPS_FILE: &str = "src/main/java/MailServerConfigurationApi.java";
#[cfg(feature = "lang-java")]
const ACCESSOR_PROPS_SOURCE: &str = "package a;\n\
    @ConfigurationProperties(prefix = \"mailserver.api\")\n\
    public class MailServerConfigurationApi {\n\
    \x20   private String uriGetArchive;\n\
    }\n";
#[cfg(feature = "lang-java")]
const ACCESSOR_CALLER_FILE: &str = "src/main/java/ArchiveClient.java";
#[cfg(feature = "lang-java")]
const ACCESSOR_CALLER_SOURCE: &str = "package a;\n\
    import org.springframework.web.client.RestClient;\n\
    public class ArchiveClient {\n\
    \x20   private RestClient restClient;\n\
    \x20   private final MailServerConfigurationApi api;\n\
    \x20   ArchiveClient(MailServerConfigurationApi api) { this.api = api; }\n\
    \x20   String a() { return restClient.get().uri(api.getUriGetArchive()).retrieve().body(String.class); }\n\
    }\n";

/// Every **HTTP client-call** reference `extract_files` emitted for `path`, by
/// target.
///
/// Filtered on the relation, not on `RefForm::Path` alone: a Java import lands
/// as a `Path`-form ref too, and reading the arm's output through the wider
/// filter made this fixture assert on `RestClient` beside the target it is about.
#[cfg(feature = "lang-java")]
fn client_call_targets(facts: &[Facts], path: &str) -> Vec<String> {
    broker_targets(facts, path, ArtifactRelation::HttpClientCall)
}

/// **S-397 AC1.** The extract pass builds the member's properties index, and the
/// invocation arm records the canonical key the accessor resolves to — as a
/// `${…}` placeholder, so the site reaches S-382's resolution by the same path a
/// source-written placeholder already takes ([FR-WS-19]).
///
/// Asserted through `extract_files`, the driver the pipeline actually calls,
/// because the whole defect this closes was that the substrate had no production
/// caller: a unit test of the index would have passed on the shipped 1.4.9
/// binary too.
///
/// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
#[test]
#[cfg(feature = "lang-java")]
fn an_accessor_operand_reaches_the_ledger_as_its_canonical_configuration_key() {
    let reg = registry();
    let ctx = SymbolContext::cargo("logos-core", "0.1.0");
    let inputs = vec![
        FileInput::new(ACCESSOR_PROPS_FILE, ACCESSOR_PROPS_SOURCE),
        FileInput::new(ACCESSOR_CALLER_FILE, ACCESSOR_CALLER_SOURCE),
    ];
    let facts = extract_files(&inputs, &reg, &ctx);

    assert_eq!(
        client_call_targets(&facts, ACCESSOR_CALLER_FILE),
        vec!["GET ${mailserver.api.urigetarchive}".to_string()],
        "the accessor's canonical key is the operand's key, spelled as the \
         placeholder S-382's resolution already consumes",
    );
    // …and the EMITTED target — not a copy of it written out here — classifies
    // as config-bound rather than refused, which is the difference the story
    // exists to make. Read back through the arm's own classifier, so the
    // fixture cannot drift from `classify_client_call`.
    use crate::resolve::http_client_call::{METHOD_SLOT, PATH_SLOT};
    let emitted = client_call_targets(&facts, ACCESSOR_CALLER_FILE);
    let (method, path) = emitted[0].split_once(' ').expect("a METHOD /template target");
    let slots: std::collections::BTreeMap<String, String> = [
        (METHOD_SLOT.to_string(), method.to_string()),
        (PATH_SLOT.to_string(), path.to_string()),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        crate::resolve::http_client_call::classify_client_call(&slots),
        Ok(crate::resolve::http_client_call::ClientCallPath::ConfigBound(
            emitted[0].clone()
        )),
        "the recorded target takes the ConfigBound admission — the same one a \
         source-written `${{…}}` takes, with no second rule",
    );
}

/// The negative half, on the **same** two-file fixture: with the properties
/// class removed, the identical call site records no key and stays a
/// runtime-composed path.
///
/// This is what makes the positive assertion above about the *hop* rather than
/// about the fixture: only one input changes between the two.
#[test]
#[cfg(feature = "lang-java")]
fn without_the_properties_class_the_same_accessor_records_no_key() {
    let reg = registry();
    let ctx = SymbolContext::cargo("logos-core", "0.1.0");
    let inputs = vec![FileInput::new(ACCESSOR_CALLER_FILE, ACCESSOR_CALLER_SOURCE)];
    let facts = extract_files(&inputs, &reg, &ctx);

    // `assert_eq!` against the exact one-row expectation, NOT `all(is_empty)`:
    // `all` over an empty vector is true, so that spelling also passed when the
    // whole client-call arm was suppressed for this member — which a mutation
    // demonstrated. The row's PRESENCE is half of what this test is for.
    assert_eq!(
        client_call_targets(&facts, ACCESSOR_CALLER_FILE),
        vec![String::new()],
        "an unresolvable accessor contributes no bind target — and still leaves \
         the keyless refusal row the arm always recorded",
    );
}

/// **S-398 AC1, end to end.** The shape the reference estate actually writes —
/// the accessor behind a field qualifier — reaches the ledger as the same
/// `"METHOD /template"` reference the unqualified spelling does.
///
/// Asserted through `extract_files` rather than through the unit, for the reason
/// [`an_accessor_operand_reaches_the_ledger_as_its_canonical_configuration_key`]
/// records: the defect S-397 closed was a substrate with no production caller,
/// and a unit test of the operand predicate would have passed on a binary where
/// the qualified receiver never reached the arm at all.
///
/// The caller source differs from [`ACCESSOR_CALLER_SOURCE`] in the qualifier
/// alone, so what this pins is the qualifier.
#[test]
#[cfg(feature = "lang-java")]
fn a_self_qualified_accessor_reaches_the_ledger_as_the_same_reference() {
    let reg = registry();
    let ctx = SymbolContext::cargo("logos-core", "0.1.0");
    let qualified = ACCESSOR_CALLER_SOURCE.replace("uri(api.get", "uri(this.api.get");
    assert_ne!(qualified, ACCESSOR_CALLER_SOURCE, "the fixture really is qualified");

    let facts = extract_files(
        &[
            FileInput::new(ACCESSOR_PROPS_FILE, ACCESSOR_PROPS_SOURCE),
            FileInput::new(ACCESSOR_CALLER_FILE, &qualified),
        ],
        &reg,
        &ctx,
    );
    assert_eq!(
        client_call_targets(&facts, ACCESSOR_CALLER_FILE),
        vec!["GET ${mailserver.api.urigetarchive}".to_string()],
        "`this.api.getUriGetArchive()` names the same canonical key the bare \
         spelling does, by the same path",
    );

    // The negative control, on the SAME qualified fixture: with the properties
    // class removed the identical site records no key and stays the keyless
    // refusal row. Without it the assertion above would also be produced by a
    // qualifier that resolved something else entirely.
    let without = extract_files(
        &[FileInput::new(ACCESSOR_CALLER_FILE, &qualified)],
        &reg,
        &ctx,
    );
    assert_eq!(
        client_call_targets(&without, ACCESSOR_CALLER_FILE),
        vec![String::new()],
        "…and with no properties class the qualified site binds nothing, so the \
         hop is what resolved it",
    );
}

/// **S-399 AC1, end to end.** The estate's second egress shape — the SAME
/// qualified accessor, nested one lambda deep inside `.uri(builder ->
/// builder.path(…).build(…))` — reaches the ledger as the reference the direct
/// spelling reaches it as.
///
/// The caller source differs from the S-398 fixture in the lambda alone, and
/// the direct spelling is run first as the control the nested runs are compared
/// against — so a build in which BOTH spellings broke the same way fails on the
/// control rather than passing silently. The control is itself anchored on the
/// expected key, which is what makes the comparison mean something; an earlier
/// draft of this comment claimed the expectation was never transcribed, which
/// the line below it contradicts.
///
/// Asserted through `extract_files` for the reason
/// [`an_accessor_operand_reaches_the_ledger_as_its_canonical_configuration_key`]
/// records: the query-layer fixtures in `tests/java_http_client_call.rs` prove
/// the pattern captures, and only the whole pipeline proves the captured
/// operand still reaches `BindingView` and resolves.
#[test]
#[cfg(feature = "lang-java")]
fn a_lambda_nested_accessor_reaches_the_ledger_as_the_same_reference() {
    let reg = registry();
    let ctx = SymbolContext::cargo("logos-core", "0.1.0");
    let direct = ACCESSOR_CALLER_SOURCE.replace("uri(api.get", "uri(this.api.get");
    let nested = ACCESSOR_CALLER_SOURCE.replace(
        "uri(api.getUriGetArchive())",
        "uri(builder -> builder.path(this.api.getUriGetArchive()).build(1))",
    );
    assert_ne!(nested, ACCESSOR_CALLER_SOURCE, "the fixture really is nested");

    let expected = {
        let facts = extract_files(
            &[
                FileInput::new(ACCESSOR_PROPS_FILE, ACCESSOR_PROPS_SOURCE),
                FileInput::new(ACCESSOR_CALLER_FILE, &direct),
            ],
            &reg,
            &ctx,
        );
        client_call_targets(&facts, ACCESSOR_CALLER_FILE)
    };
    assert_eq!(
        expected,
        vec!["GET ${mailserver.api.urigetarchive}".to_string()],
        "the direct spelling is the control, and it still binds",
    );

    let facts = extract_files(
        &[
            FileInput::new(ACCESSOR_PROPS_FILE, ACCESSOR_PROPS_SOURCE),
            FileInput::new(ACCESSOR_CALLER_FILE, &nested),
        ],
        &reg,
        &ctx,
    );
    assert_eq!(
        client_call_targets(&facts, ACCESSOR_CALLER_FILE),
        expected,
        "the accessor one lambda deep names the same canonical key, by the same \
         path — and leaves no refusal row beside it, which the exact-equality \
         comparison is what catches (an uncancelled candidate reads as an extra \
         empty string)",
    );

    // The BARE-receiver spelling too. Pattern 5 binds the operand as `(_)`, so
    // the qualified and unqualified accessors are structurally identical at the
    // query layer — but which `DeclaredTypes` arm answers for them is not
    // (S-398 split `get` from `field` precisely there), so both spellings are
    // asserted rather than one standing for the other.
    let bare = ACCESSOR_CALLER_SOURCE.replace(
        "uri(api.getUriGetArchive())",
        "uri(builder -> builder.path(api.getUriGetArchive()).build(1))",
    );
    assert_ne!(bare, nested, "the two spellings really do differ");
    let facts = extract_files(
        &[
            FileInput::new(ACCESSOR_PROPS_FILE, ACCESSOR_PROPS_SOURCE),
            FileInput::new(ACCESSOR_CALLER_FILE, &bare),
        ],
        &reg,
        &ctx,
    );
    assert_eq!(
        client_call_targets(&facts, ACCESSOR_CALLER_FILE),
        expected,
        "an unqualified receiver inside the lambda reaches the same key through \
         `DeclaredTypes::get`, as it does outside one",
    );

    // The negative control, on the SAME nested fixture: with the properties
    // class removed the identical site records no key and stays the keyless
    // refusal row. Without it the assertion above would also be produced by a
    // lambda that resolved something else entirely.
    let without = extract_files(&[FileInput::new(ACCESSOR_CALLER_FILE, &nested)], &reg, &ctx);
    assert_eq!(
        client_call_targets(&without, ACCESSOR_CALLER_FILE),
        vec![String::new()],
        "…and with no properties class the nested site binds nothing, so the \
         hop is what resolved it",
    );
}

/// **S-405 AC2, end to end on the estate's DOMINANT lambda ([CR-129]).** The
/// same nested accessor, in a lambda that also chains a `queryParam`, resolves
/// to the same key — the query parameter names the query component and cannot
/// reach the path template, so it does not make the path "composed from a
/// non-resolvable operand" ([FR-WS-08] AC2).
///
/// This is the criterion's real value, and it is asserted on the shape that
/// carries it: most of the reference workspace's `.uri(<lambda>)` sites chain at
/// least one `queryParam` — the measured figure is stated once, in the
/// composition rule in `plugins/java/queries/invocations.scm` — so this fixture,
/// not the bare `path(…)` one above, is what most of the estate looks like.
///
/// **S-399 asserted the opposite here and was right at its date.** It refused
/// every link beyond `build()`, which refused this shape too; [CR-129] read
/// that back against the requirement and found the product stricter than its
/// own AC2. The negative control below is what did NOT move: a link the
/// contract test cannot prove path-neutral still refuses the whole chain, with
/// the properties class present and the accessor resolvable, so what this test
/// pins is the contract test and not "chains now bind".
///
/// [CR-129]: ../../docs/requests/CR-129-path-neutral-composer-link-in-a-uribuilder-lambda.md
/// [FR-WS-08]: ../../docs/specs/requirements/FR-WS-08.md
#[test]
#[cfg(feature = "lang-java")]
fn a_lambda_that_chains_a_path_neutral_link_binds_through_it() {
    let reg = registry();
    let ctx = SymbolContext::cargo("logos-core", "0.1.0");
    let chained = ACCESSOR_CALLER_SOURCE.replace(
        "uri(api.getUriGetArchive())",
        "uri(builder -> builder.path(this.api.getUriGetArchive()).queryParam(\"page\", 1).build(1))",
    );
    assert_ne!(chained, ACCESSOR_CALLER_SOURCE, "the fixture really is chained");

    let facts = extract_files(
        &[
            FileInput::new(ACCESSOR_PROPS_FILE, ACCESSOR_PROPS_SOURCE),
            FileInput::new(ACCESSOR_CALLER_FILE, &chained),
        ],
        &reg,
        &ctx,
    );
    assert_eq!(
        client_call_targets(&facts, ACCESSOR_CALLER_FILE),
        vec!["GET ${mailserver.api.urigetarchive}".to_string()],
        "the properties class is present, the accessor is resolvable, and the \
         one other link in the chain provably cannot alter the path — so the \
         site binds the key the direct spelling binds",
    );

    // The negative control, on the SAME fixture and the same class: one link the
    // contract test cannot prove path-neutral, and the whole chain refuses.
    // `pathSegment` is chosen because it is the near miss — a real `UriBuilder`
    // method one prefix away from the link that supplies the path.
    let reaching = ACCESSOR_CALLER_SOURCE.replace(
        "uri(api.getUriGetArchive())",
        "uri(builder -> builder.path(this.api.getUriGetArchive()).pathSegment(\"page\").build(1))",
    );
    let refused = extract_files(
        &[
            FileInput::new(ACCESSOR_PROPS_FILE, ACCESSOR_PROPS_SOURCE),
            FileInput::new(ACCESSOR_CALLER_FILE, &reaching),
        ],
        &reg,
        &ctx,
    );
    assert_eq!(
        client_call_targets(&refused, ACCESSOR_CALLER_FILE),
        vec![String::new()],
        "a link that reaches the path template refuses the chain whole — the \
         keyless runtime-composed row, unchanged (NFR-RA-05)",
    );
}

/// **S-398 AC3, at the surface the criterion speaks about.** A generic wrapper
/// whose URI is a method **parameter** emits no `"METHOD /template"` reference,
/// and that is a correct refusal rather than a miss.
///
/// The fixture is the shape that would make a call-shape-gated admission
/// fabricate: the class injects a bound properties class, forwards to a genuine
/// client receiver inside a file the ledger gate admits, and the only thing
/// wrong with it is that the path is a value its own **caller** supplies, which
/// no committed source defines.
///
/// What it emits instead is the keyless refusal row the arm has always recorded
/// — asserted as that exact row rather than as `is_empty`, because an empty
/// vector would also be produced by the whole arm being suppressed, which is the
/// failure [`without_the_properties_class_the_same_accessor_records_no_key`]
/// records having been demonstrated by a mutation.
#[test]
#[cfg(feature = "lang-java")]
fn a_wrapper_whose_uri_is_a_method_parameter_emits_no_reference() {
    let reg = registry();
    let ctx = SymbolContext::cargo("logos-core", "0.1.0");
    let wrapper = "package a;\n\
        import org.springframework.web.client.RestClient;\n\
        public class ArchiveClient {\n\
        \x20   private RestClient restClient;\n\
        \x20   private final MailServerConfigurationApi api;\n\
        \x20   String get(String uri, Object... args) {\n\
        \x20     return restClient.get().uri(uri).retrieve().body(String.class);\n\
        \x20   }\n\
        }\n";
    let facts = extract_files(
        &[
            FileInput::new(ACCESSOR_PROPS_FILE, ACCESSOR_PROPS_SOURCE),
            FileInput::new(ACCESSOR_CALLER_FILE, wrapper),
        ],
        &reg,
        &ctx,
    );
    assert_eq!(
        client_call_targets(&facts, ACCESSOR_CALLER_FILE),
        vec![String::new()],
        "the path is a method parameter, so the site stays the keyless \
         runtime-composed row — no target is guessed from the bound class the \
         same file happens to inject",
    );
}

/// **The hop changes the client-call row and nothing else**: extract the
/// identical caller file in a member that declares the properties class and in
/// one that does not, and every other fact is equal.
///
/// Scoped deliberately, because an earlier docstring here claimed more than the
/// test proves. This is a differential between **two arms of the same build**,
/// so it constrains only what the `!properties.is_empty()` branch does; a change
/// on the path both arms share cancels out of it, and a mutation that pushed a
/// warning unconditionally survived. [FR-WS-19] AC7's "a member with no
/// configuration corpus is byte-for-byte unaffected" is pinned by its sibling
/// [`without_the_properties_class_the_same_accessor_records_no_key`], which
/// asserts the exact row such a member emits.
///
/// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
#[test]
#[cfg(feature = "lang-java")]
fn the_hop_changes_the_client_call_row_and_nothing_else() {
    let reg = registry();
    let ctx = SymbolContext::cargo("logos-core", "0.1.0");
    let caller = FileInput::new(ACCESSOR_CALLER_FILE, ACCESSOR_CALLER_SOURCE);

    let with_class = extract_files(
        &[
            FileInput::new(ACCESSOR_PROPS_FILE, ACCESSOR_PROPS_SOURCE),
            caller.clone(),
        ],
        &reg,
        &ctx,
    );
    let without = extract_files(std::slice::from_ref(&caller), &reg, &ctx);

    let bound = with_class
        .iter()
        .find(|f| f.path == ACCESSOR_CALLER_FILE)
        .expect("the caller was extracted in both arms");
    let plain = &without[0];

    assert_eq!(bound.nodes, plain.nodes, "the hop emits no node");
    assert_eq!(bound.edges, plain.edges, "…no edge");
    assert_eq!(bound.warnings, plain.warnings, "…no warning");
    assert_eq!(bound.config_source, plain.config_source, "…and no corpus fact");
    assert_eq!(bound.partial, plain.partial);

    // Every reference EXCEPT the client-call row is byte-identical; the two arms
    // differ in that row alone, and in its target alone.
    let others = |f: &Facts| -> Vec<RefFact> {
        f.refs
            .iter()
            .filter(|r| r.relation != Some(ArtifactRelation::HttpClientCall))
            .cloned()
            .collect()
    };
    assert_eq!(others(bound), others(plain), "no other reference moves");
    assert_eq!(
        client_call_targets(&with_class, ACCESSOR_CALLER_FILE),
        vec!["GET ${mailserver.api.urigetarchive}".to_string()],
    );
    assert_eq!(
        client_call_targets(&without, ACCESSOR_CALLER_FILE),
        vec![String::new()],
        "without the class, the same site is the keyless refusal row it always was",
    );
}

/// **The fabrication review reproduced, pinned end to end.** A cast anywhere in
/// the caller must not give an undeclared receiver a type.
///
/// This is the harm behind `accessor_tests`'
/// `a_call_expression_binds_no_name_however_its_result_is_cast`, and it is
/// asserted here as well as there because the unit test alone would not have
/// shown what was at stake: `api` is inherited and declared nowhere in this
/// file, so before the fix one unrelated cast statement was the whole evidence
/// behind a `config-bound` target.
#[test]
#[cfg(feature = "lang-java")]
fn a_cast_elsewhere_in_the_file_gives_an_undeclared_receiver_no_key() {
    let reg = registry();
    let ctx = SymbolContext::cargo("logos-core", "0.1.0");
    let caller = "package a;\n\
        import org.springframework.web.client.RestClient;\n\
        public class ArchiveClient extends BaseClient {\n\
        \x20   private RestClient restClient;\n\
        \x20   void warm(Registry reg) { Object o = (MailServerConfigurationApi) reg.api(); }\n\
        \x20   String a() { return restClient.get().uri(api.getUriGetArchive()).retrieve().body(String.class); }\n\
        }\n";
    let facts = extract_files(
        &[
            FileInput::new(ACCESSOR_PROPS_FILE, ACCESSOR_PROPS_SOURCE),
            FileInput::new(ACCESSOR_CALLER_FILE, caller),
        ],
        &reg,
        &ctx,
    );

    assert_eq!(
        client_call_targets(&facts, ACCESSOR_CALLER_FILE),
        vec![String::new()],
        "`api` is declared nowhere in this file; a cast of an unrelated call's \
         result is not a declaration of it, and the site must stay the keyless \
         refusal row",
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// S-409 — the accessor hop reaches the BROKER arm ([FR-WS-19], [CR-131] §3.2 A2)
// ─────────────────────────────────────────────────────────────────────────────

/// The topic-bearing `@ConfigurationProperties` class the broker fixtures below
/// read, and the member that reads it.
///
/// A second properties class rather than a reuse of [`ACCESSOR_PROPS_SOURCE`]:
/// the HTTP fixtures' class carries a *URI* property, and a topic fixture
/// keyed on `mailserver.api.uri-get-archive` would read as though the two arms
/// shared a key space. They share a **mechanism**, which is what the fixtures
/// are about; two prefixes keep that distinction visible in the assertions.
#[cfg(feature = "lang-java")]
const TOPIC_PROPS_FILE: &str = "src/main/java/KafkaTopics.java";
#[cfg(feature = "lang-java")]
const TOPIC_PROPS_SOURCE: &str = "package a;\n\
    @ConfigurationProperties(prefix = \"kafka.topics\")\n\
    public class KafkaTopics {\n\
    \x20   private String archiveEvents;\n\
    \x20   private String archiveReporting;\n\
    }\n";
#[cfg(feature = "lang-java")]
const TOPIC_CALLER_FILE: &str = "src/main/java/TopologyService.java";

/// The canonical placeholder `kafkaTopics.getArchiveEvents()` resolves to —
/// relaxed binding applied, exactly as the HTTP arm stores it.
#[cfg(feature = "lang-java")]
const ARCHIVE_EVENTS_KEY: &str = "${kafka.topics.archiveevents}";

/// Every reference `extract_files` emitted for `path` under one artifact
/// relation, by target.
///
/// Filtered on the **relation**, never on `RefForm` alone: a Java import lands as
/// a `Path`-form reference too, and reading an arm's output through the wider
/// filter once made a fixture assert on `RestClient` beside the target it was
/// about. [`client_call_targets`] is this function with the HTTP relation applied,
/// rather than a second copy of the same filter chain.
#[cfg(feature = "lang-java")]
fn broker_targets(facts: &[Facts], path: &str, relation: ArtifactRelation) -> Vec<String> {
    facts
        .iter()
        .filter(|f| f.path == path)
        .flat_map(|f| f.refs.iter())
        .filter(|r| r.relation == Some(relation))
        .map(|r| r.target.clone())
        .collect()
}

/// Every reference `extract_files` emitted for `path` under one broker relation,
/// as `(target, line)`.
///
/// The line is projected because the resolved path sets it from a DIFFERENT node
/// than the literal path does (the refusal slot's operand, not the topic node),
/// and a row's line is ledger-visible. Without it a constant offset in the
/// resolved arm passes every assertion — demonstrated by mutation during the
/// S-409 review, where `+ 1` became `+ 77` with the whole suite green.
#[cfg(feature = "lang-java")]
fn broker_rows(facts: &[Facts], path: &str, relation: ArtifactRelation) -> Vec<(String, u32)> {
    facts
        .iter()
        .filter(|f| f.path == path)
        .flat_map(|f| f.refs.iter())
        .filter(|r| r.relation == Some(relation))
        .map(|r| (r.target.clone(), r.line))
        .collect()
}

/// **S-409 AC1.** Each of the three broker forms the arm captures — the
/// annotation subscribe, the `setHeader(KafkaHeaders.TOPIC, …)` publish, and
/// S-408's Kafka Streams topology link — stores a `@ConfigurationProperties`
/// accessor operand as the canonical `${prefix.key}` placeholder.
///
/// All three forms are asserted rather than one standing for the others,
/// because the three reach the interpreter by **different capture shapes**: an
/// `element_value_pair` site, an `argument_list` site behind two positional
/// anchors, and a receiver-gated `argument_list` site. Only the last is reached
/// at all on a build where S-408's gate rejects the chain, so a single-form
/// fixture would pass while two thirds of the estate's sites stayed refused.
///
/// Both receiver spellings are exercised across the table — the bare
/// `kafkaTopics.…` and the self-qualified `this.kafkaTopics.…` — because S-398
/// answers them from two different `DeclaredTypes` arms.
///
/// Each row carries its own negative control: with the properties class removed
/// and nothing else changed, the same site records the keyless
/// `topic-not-literal` row it recorded before this story. That is what makes the
/// positive assertion about the **hop** rather than about the fixture.
///
/// [CR-131]: ../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
/// [FR-WS-19]: ../../docs/specs/requirements/FR-WS-19.md
#[test]
#[cfg(feature = "lang-java")]
fn an_accessor_topic_reaches_the_ledger_in_every_captured_broker_form() {
    let reg = registry();
    let ctx = SymbolContext::cargo("logos-core", "0.1.0");

    // (form, source, expected subscribe targets, expected publish targets)
    let annotation = "package a;\n\
        public class TopologyService {\n\
        \x20   private final KafkaTopics kafkaTopics;\n\
        \x20   @KafkaListener(topics = kafkaTopics.getArchiveEvents())\n\
        \x20   void onEvent(String m) {}\n\
        }\n";
    let header = "package a;\n\
        public class TopologyService {\n\
        \x20   private final KafkaTopics kafkaTopics;\n\
        \x20   void publish(String payload) {\n\
        \x20     MessageBuilder.withPayload(payload)\n\
        \x20       .setHeader(KafkaHeaders.TOPIC, this.kafkaTopics.getArchiveEvents())\n\
        \x20       .build();\n\
        \x20   }\n\
        }\n";
    let streams = "package a;\n\
        public class TopologyService {\n\
        \x20   private final KafkaTopics kafkaTopics;\n\
        \x20   public void kStream(StreamsBuilder streamsBuilder) {\n\
        \x20     streamsBuilder\n\
        \x20       .stream(kafkaTopics.getArchiveEvents())\n\
        \x20       .to(this.kafkaTopics.getArchiveReporting());\n\
        \x20   }\n\
        }\n";

    let cases: [(&str, &str, Vec<String>, Vec<String>); 3] = [
        (
            "annotation form",
            annotation,
            vec![ARCHIVE_EVENTS_KEY.to_string()],
            Vec::new(),
        ),
        (
            "header form",
            header,
            Vec::new(),
            vec![ARCHIVE_EVENTS_KEY.to_string()],
        ),
        (
            "Streams topology form",
            streams,
            vec![ARCHIVE_EVENTS_KEY.to_string()],
            vec!["${kafka.topics.archivereporting}".to_string()],
        ),
    ];

    for (form, src, want_sub, want_pub) in cases {
        let facts = extract_files(
            &[
                FileInput::new(TOPIC_PROPS_FILE, TOPIC_PROPS_SOURCE),
                FileInput::new(TOPIC_CALLER_FILE, src),
            ],
            &reg,
            &ctx,
        );
        assert_eq!(
            broker_targets(&facts, TOPIC_CALLER_FILE, ArtifactRelation::BrokerSubscribe),
            want_sub,
            "{form}: the subscribe side stores the canonical placeholder, and \
             leaves no refusal row beside it — an uncancelled candidate reads as \
             an extra empty string, which exact equality is what catches",
        );
        assert_eq!(
            broker_targets(&facts, TOPIC_CALLER_FILE, ArtifactRelation::BrokerPublish),
            want_pub,
            "{form}: the publish side stores the canonical placeholder",
        );

        // Each resolved row reports at its OWN operand's line. Asserted against the
        // line the fixture literally writes the operand on — found by searching the
        // source, so the expectation cannot drift when a line is added above it —
        // because the resolved path takes its line from the refusal slot's operand
        // while the literal path takes it from the topic node, and nothing else
        // here would notice a constant offset. The Streams case carries two rows on
        // two different lines, which is what makes a uniform offset unable to pass.
        let line_of = |needle: &str| -> u32 {
            src.lines()
                .position(|l| l.contains(needle))
                .map(|i| i as u32 + 1)
                .unwrap_or_else(|| panic!("{form}: the fixture writes {needle:?}"))
        };
        for (relation, want, needle) in [
            (ArtifactRelation::BrokerSubscribe, &want_sub, ".stream(kafkaTopics"),
            (ArtifactRelation::BrokerPublish, &want_pub, ".to(this.kafkaTopics"),
        ] {
            if form != "Streams topology form" || want.is_empty() {
                continue;
            }
            assert_eq!(
                broker_rows(&facts, TOPIC_CALLER_FILE, relation),
                vec![(want[0].clone(), line_of(needle))],
                "{form}: the resolved row reports at its own operand's line",
            );
        }

        // The negative control, on the SAME fixture: remove the properties class
        // and the identical site is the keyless `topic-not-literal` row again.
        let without = extract_files(&[FileInput::new(TOPIC_CALLER_FILE, src)], &reg, &ctx);
        assert_eq!(
            broker_targets(&without, TOPIC_CALLER_FILE, ArtifactRelation::BrokerSubscribe),
            vec![String::new(); want_sub.len()],
            "{form}: with no properties class the subscribe site refuses, so the \
             hop is what resolved it",
        );
        assert_eq!(
            broker_targets(&without, TOPIC_CALLER_FILE, ArtifactRelation::BrokerPublish),
            vec![String::new(); want_pub.len()],
            "{form}: …and so does the publish site",
        );
    }
}

/// **S-409 AC2.** Every one of [FR-WS-19]'s nine named accessor faults leaves
/// the broker arm's own keyless refusal row — the **same** row, on the same
/// build and the same operand, that the HTTP arm leaves.
///
/// Read what this asserts and what it does not. The nine are a **census**
/// vocabulary ([`crate::resolve::binding::Refusal`]), produced by the
/// operand-resolvability harness; no invocation arm has ever carried them to the
/// ledger. What each arm carries is one keyless row, which the [FR-WS-05]
/// coverage tier labels from the row's **relation** —
/// `base-url-runtime` for the HTTP arm, `topic-not-literal` for this one. So
/// "the same wire tokens as the HTTP arm" is the claim that the broker arm
/// collapses the nine exactly as the HTTP arm does, into the arm's own existing
/// word, and that is what the side-by-side assertion below pins: both arms read
/// the identical operand in the identical file, and both record exactly one
/// keyless row for it. A story that gave one arm a finer reason than the other
/// fails here.
///
/// The `methodParameter` row is the one the story is required to leave refused:
/// the operand's value arrives one call frame away, and the two-frame wrapper
/// hop is S-417's, out of scope here.
///
/// **What the row LABELS are, and are not.** They name the census vocabulary so
/// the criterion is auditable against it, but four of the nine — `methodParameter`,
/// `unboundName`, `ambiguousBinding`, `unrecognisedAccessor` — are operands that
/// are not member calls at all, so they are refused at `placeholder_for`'s first
/// line and never enter the accessor chain. That is not a weakness of the fixture:
/// this arm has no concept of the nine, and the claim under test is exactly the one
/// asserted — each shape leaves the arm's own keyless row, on both arms. Read the
/// labels as naming the SHAPES the census counts, not as evidence that nine
/// distinct code paths ran.
///
/// [FR-WS-05]: ../../docs/specs/requirements/FR-WS-05.md
/// [FR-WS-19]: ../../docs/specs/requirements/FR-WS-19.md
#[test]
#[cfg(feature = "lang-java")]
fn each_named_accessor_fault_leaves_the_brokers_own_refusal_row() {
    let reg = registry();
    let ctx = SymbolContext::cargo("logos-core", "0.1.0");

    // One operand per fault, spelled so the fault is the ONLY thing wrong with
    // it — the properties class is present in every run, so a row that refuses
    // for the wrong reason would have to refuse a resolvable neighbour too.
    let cases: [(&str, &str); 9] = [
        ("nestedAccessor", "kafkaTopics.getNested().getArchiveEvents()"),
        ("methodParameter", "topic"),
        ("unboundName", "undeclaredTopic"),
        ("ambiguousBinding", "ambiguousTopic"),
        ("notAGetter", "kafkaTopics.computeArchiveEvents()"),
        ("receiverTypeUnknown", "unknownTopics.getArchiveEvents()"),
        ("noPropertiesClass", "otherTopics.getArchiveEvents()"),
        ("propertyNotDeclared", "kafkaTopics.getMissingTopic()"),
        ("unrecognisedAccessor", "Topics.ARCHIVE_EVENTS"),
    ];

    for (fault, operand) in cases {
        // Both arms read the SAME operand in the SAME file: the header-form
        // publish and a `RestClient` call one method apart. The import is what
        // admits the file to the HTTP arm's ledger gate.
        let src = format!(
            "package a;\n\
             import org.springframework.web.client.RestClient;\n\
             public class TopologyService {{\n\
             \x20   private RestClient restClient;\n\
             \x20   private final KafkaTopics kafkaTopics;\n\
             \x20   private OtherTopics otherTopics;\n\
             \x20   private String ambiguousTopic = kafkaTopics.getArchiveEvents();\n\
             \x20   void rebind() {{ ambiguousTopic = kafkaTopics.getArchiveReporting(); }}\n\
             \x20   void publish(String payload, String topic) {{\n\
             \x20     MessageBuilder.withPayload(payload)\n\
             \x20       .setHeader(KafkaHeaders.TOPIC, {operand})\n\
             \x20       .build();\n\
             \x20   }}\n\
             \x20   String read(String topic) {{\n\
             \x20     return restClient.get().uri({operand}).retrieve().body(String.class);\n\
             \x20   }}\n\
             }}\n"
        );
        let facts = extract_files(
            &[
                FileInput::new(TOPIC_PROPS_FILE, TOPIC_PROPS_SOURCE),
                FileInput::new(TOPIC_CALLER_FILE, &src),
            ],
            &reg,
            &ctx,
        );
        assert_eq!(
            broker_targets(&facts, TOPIC_CALLER_FILE, ArtifactRelation::BrokerPublish),
            vec![String::new()],
            "{fault}: the broker arm records exactly its one keyless row — the \
             row the FR-WS-05 tier reports `topic-not-literal`, no key \
             fabricated from an operand the source does not prove",
        );
        assert_eq!(
            client_call_targets(&facts, TOPIC_CALLER_FILE),
            vec![String::new()],
            "{fault}: …and the HTTP arm records exactly its one keyless row for \
             the identical operand, so neither arm resolves what the other \
             refuses",
        );
    }

    // The discriminating control: the identical file, with an operand that IS a
    // resolvable accessor, binds on BOTH arms. Without it every assertion above
    // would also be produced by a build in which the hop never ran.
    let resolvable = format!(
        "package a;\n\
         import org.springframework.web.client.RestClient;\n\
         public class TopologyService {{\n\
         \x20   private RestClient restClient;\n\
         \x20   private final KafkaTopics kafkaTopics;\n\
         \x20   void publish(String payload) {{\n\
         \x20     MessageBuilder.withPayload(payload)\n\
         \x20       .setHeader(KafkaHeaders.TOPIC, {operand})\n\
         \x20       .build();\n\
         \x20   }}\n\
         \x20   String read() {{\n\
         \x20     return restClient.get().uri({operand}).retrieve().body(String.class);\n\
         \x20   }}\n\
         }}\n",
        operand = "kafkaTopics.getArchiveEvents()"
    );
    let facts = extract_files(
        &[
            FileInput::new(TOPIC_PROPS_FILE, TOPIC_PROPS_SOURCE),
            FileInput::new(TOPIC_CALLER_FILE, &resolvable),
        ],
        &reg,
        &ctx,
    );
    assert_eq!(
        broker_targets(&facts, TOPIC_CALLER_FILE, ArtifactRelation::BrokerPublish),
        vec![ARCHIVE_EVENTS_KEY.to_string()],
        "the control binds on the broker arm",
    );
    assert_eq!(
        client_call_targets(&facts, TOPIC_CALLER_FILE),
        vec![format!("GET {ARCHIVE_EVENTS_KEY}")],
        "…and on the HTTP arm, from the same operand in the same file",
    );
}

/// **S-409 review finding.** A getter-NAMED call invoked **with an argument** is
/// not the property getter it resembles, and must not bind that property's key.
///
/// A generated `@ConfigurationProperties` getter is zero-arity, so
/// `kafkaTopics.getArchiveEvents(suffix)` computes something the source does not
/// prove is the `archiveEvents` property — binding it would fabricate a key
/// ([NFR-RA-05]). The refusal-slot enumeration admits `(method_invocation)`
/// whatever its arity, so nothing upstream filtered this out.
///
/// Asserted on **both arms**, because the guard lives in the shared
/// `BindingView::placeholder_for` and tightening it must tighten both identically.
/// The zero-argument spelling of the same call is the control, so what this pins
/// is the ARITY and not the fixture: the estate writes 0 sites of this shape, so a
/// test is the only evidence the guard can have.
///
/// [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
#[test]
#[cfg(feature = "lang-java")]
fn a_getter_named_call_that_takes_an_argument_binds_nothing_on_either_arm() {
    let reg = registry();
    let ctx = SymbolContext::cargo("logos-core", "0.1.0");
    let caller = |operand: &str| {
        format!(
            "package a;\n             import org.springframework.web.client.RestClient;\n             public class TopologyService {{\n             \x20   private RestClient restClient;\n             \x20   private final KafkaTopics kafkaTopics;\n             \x20   void publish(String payload, String suffix) {{\n             \x20     MessageBuilder.withPayload(payload)\n             \x20       .setHeader(KafkaHeaders.TOPIC, {operand})\n             \x20       .build();\n             \x20   }}\n             \x20   String read(String suffix) {{\n             \x20     return restClient.get().uri({operand}).retrieve().body(String.class);\n             \x20   }}\n             }}\n"
        )
    };
    let run = |operand: &str| {
        extract_files(
            &[
                FileInput::new(TOPIC_PROPS_FILE, TOPIC_PROPS_SOURCE),
                FileInput::new(TOPIC_CALLER_FILE, caller(operand)),
            ],
            &reg,
            &ctx,
        )
    };

    // The control: the same call, zero-arity, binds on both arms. Without it this
    // test would also pass on a build where the hop never ran at all.
    let zero_arity = run("kafkaTopics.getArchiveEvents()");
    assert_eq!(
        broker_targets(&zero_arity, TOPIC_CALLER_FILE, ArtifactRelation::BrokerPublish),
        vec![ARCHIVE_EVENTS_KEY.to_string()],
        "the control binds on the broker arm",
    );
    assert_eq!(
        client_call_targets(&zero_arity, TOPIC_CALLER_FILE),
        vec![format!("GET {ARCHIVE_EVENTS_KEY}")],
        "…and on the HTTP arm",
    );

    // One argument away, and both arms refuse.
    let with_argument = run("kafkaTopics.getArchiveEvents(suffix)");
    assert_eq!(
        broker_targets(&with_argument, TOPIC_CALLER_FILE, ArtifactRelation::BrokerPublish),
        vec![String::new()],
        "a getter-named call with an argument is not that getter — the broker arm \
         records its keyless row rather than the property's key",
    );
    assert_eq!(
        client_call_targets(&with_argument, TOPIC_CALLER_FILE),
        vec![String::new()],
        "…and the HTTP arm refuses the identical operand, so the guard tightened \
         both arms and not one",
    );
}

/// **S-409 AC4, the byte-for-byte half.** The hop changes the broker row and
/// nothing else: extract the identical caller file in a member that declares the
/// properties class and in one that does not, and every other fact is equal.
///
/// Scoped exactly as its HTTP sibling
/// [`the_hop_changes_the_client_call_row_and_nothing_else`] is, and for the same
/// stated reason: this is a differential between two arms of ONE build, so it
/// constrains only what the `!properties.is_empty()` branch does. "A member with
/// no `@ConfigurationProperties` class is byte-for-byte unaffected" is pinned by
/// the exact rows the `without` arm emits, asserted here and in every negative
/// control of
/// [`an_accessor_topic_reaches_the_ledger_in_every_captured_broker_form`].
#[test]
#[cfg(feature = "lang-java")]
fn the_broker_hop_changes_the_broker_row_and_nothing_else() {
    let reg = registry();
    let ctx = SymbolContext::cargo("logos-core", "0.1.0");
    let src = "package a;\n\
        public class TopologyService {\n\
        \x20   private final KafkaTopics kafkaTopics;\n\
        \x20   void publish(String payload) {\n\
        \x20     MessageBuilder.withPayload(payload)\n\
        \x20       .setHeader(KafkaHeaders.TOPIC, this.kafkaTopics.getArchiveEvents())\n\
        \x20       .build();\n\
        \x20   }\n\
        }\n";
    let caller = FileInput::new(TOPIC_CALLER_FILE, src);

    let with_class = extract_files(
        &[
            FileInput::new(TOPIC_PROPS_FILE, TOPIC_PROPS_SOURCE),
            caller.clone(),
        ],
        &reg,
        &ctx,
    );
    let without = extract_files(std::slice::from_ref(&caller), &reg, &ctx);

    let bound = with_class
        .iter()
        .find(|f| f.path == TOPIC_CALLER_FILE)
        .expect("the caller was extracted in both arms");
    let plain = &without[0];

    assert_eq!(bound.nodes, plain.nodes, "the hop emits no node");
    assert_eq!(bound.edges, plain.edges, "…no edge");
    assert_eq!(bound.warnings, plain.warnings, "…no warning");
    assert_eq!(bound.config_source, plain.config_source, "…and no corpus fact");
    assert_eq!(bound.partial, plain.partial);

    let others = |f: &Facts| -> Vec<RefFact> {
        f.refs
            .iter()
            .filter(|r| r.relation != Some(ArtifactRelation::BrokerPublish))
            .cloned()
            .collect()
    };
    assert_eq!(others(bound), others(plain), "no other reference moves");
    assert_eq!(
        broker_targets(&with_class, TOPIC_CALLER_FILE, ArtifactRelation::BrokerPublish),
        vec![ARCHIVE_EVENTS_KEY.to_string()],
    );
    assert_eq!(
        broker_targets(&without, TOPIC_CALLER_FILE, ArtifactRelation::BrokerPublish),
        vec![String::new()],
        "without the class, the same site is the keyless refusal row it always was",
    );
}

// ── S-417: the two-frame wrapper hop ([FR-WS-26], [CR-131] §3.2 C2) ──────────
//
// The population is the one S-392 measured and FALSIFIED at one frame and S-416
// re-measured and carried at two: a header-form publish whose topic operand is a
// bare parameter of the method enclosing it. Every fixture below is that shape,
// and every one carries its own negative control — the same tree with the thing
// under test removed — because a positive assertion about a hop is only about
// the hop if the same fixture refuses without it.
//
// [CR-131]: ../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
// [FR-WS-26]: ../../docs/specs/requirements/FR-WS-26.md

/// The wrapper: a publish whose topic is its own third parameter. Nothing in
/// this file proves the topic, and nothing ever will — that is the point.
#[cfg(feature = "lang-java")]
const WRAPPER_FILE: &str = "producer/src/main/java/KafkaProducer.java";
#[cfg(feature = "lang-java")]
const WRAPPER_SOURCE: &str = "package a;\n\
    public class KafkaProducer {\n\
    \x20   void sendMessage(String key, String payload, String topic) {\n\
    \x20     MessageBuilder.withPayload(payload)\n\
    \x20       .setHeader(KafkaHeaders.TOPIC, topic)\n\
    \x20       .build();\n\
    \x20   }\n\
    }\n";
#[cfg(feature = "lang-java")]
const WRAPPER_MODULE_POM: &str = "producer/pom.xml";
#[cfg(feature = "lang-java")]
const WRAPPER_PROPS_FILE: &str = "producer/src/main/java/KafkaTopics.java";

/// One in-module `src/main` caller that supplies `accessor` at the topic slot,
/// through a field declared with type `receiver`.
///
/// The receiver's declared type is a parameter and not a constant because it is
/// what decides WHICH declaration of `sendMessage/3` the call site is a caller
/// of — the rule `declares_a_different_receiver` applies. A fixture that always
/// held the base type could not express the two-frame case at all.
#[cfg(feature = "lang-java")]
fn wrapper_caller_via(class: &str, receiver: &str, accessor: &str) -> String {
    format!(
        "package a;\n\
         public class {class} {{\n\
         \x20   private final KafkaTopics kafkaTopics;\n\
         \x20   private final {receiver} producer;\n\
         \x20   void archive(String key, String payload) {{\n\
         \x20     producer.sendMessage(key, payload, {accessor});\n\
         \x20   }}\n\
         }}\n"
    )
}

/// The common case: the caller holds the wrapper's own declaring type.
#[cfg(feature = "lang-java")]
fn wrapper_caller(class: &str, accessor: &str) -> String {
    wrapper_caller_via(class, "KafkaProducer", accessor)
}

/// The base inputs every fixture starts from: the module descriptor, the
/// properties class, and the wrapper.
#[cfg(feature = "lang-java")]
fn wrapper_estate(extra: &[(&str, String)]) -> Vec<FileInput> {
    let mut inputs = vec![
        FileInput::new(WRAPPER_MODULE_POM, "<project/>\n"),
        FileInput::new(WRAPPER_PROPS_FILE, TOPIC_PROPS_SOURCE),
        FileInput::new(WRAPPER_FILE, WRAPPER_SOURCE),
    ];
    inputs.extend(extra.iter().map(|(path, src)| FileInput::new(*path, src.clone())));
    inputs
}

/// The forwarding outcomes recorded for `path`, in ledger order.
#[cfg(feature = "lang-java")]
fn forwarding_outcomes(facts: &[Facts], path: &str) -> Vec<ForwardingOutcome> {
    facts
        .iter()
        .filter(|f| f.path == path)
        .flat_map(|f| f.forwarding.iter())
        .map(|c| {
            c.outcome
                .clone()
                .expect("extract_files always decides every candidate")
        })
        .collect()
}

/// The refusal reasons recorded for `path`, by their stable token.
#[cfg(feature = "lang-java")]
fn forwarding_refusals(facts: &[Facts], path: &str) -> Vec<&'static str> {
    forwarding_outcomes(facts, path)
        .iter()
        .map(|o| match o {
            ForwardingOutcome::Refused(r) => r.as_str(),
            ForwardingOutcome::Resolved(_) => "resolved",
        })
        .collect()
}

/// **[FR-WS-26] AC1, one frame.** A wrapper whose only `src/main` callers pass
/// one `@ConfigurationProperties` accessor resolves to that accessor's canonical
/// key, and the keyless `topic-not-literal` row it used to leave is **retracted**
/// — asserted by exact equality, because an uncancelled refusal reads as an extra
/// empty target.
///
/// The provenance names the one frame that was taken. Its negative control is
/// the same tree with the caller removed: the wrapper alone still refuses, which
/// is what makes this assertion about the hop rather than about the fixture.
///
/// [FR-WS-26]: ../../docs/specs/requirements/FR-WS-26.md
#[test]
#[cfg(feature = "lang-java")]
fn a_wrapper_whose_main_callers_pass_one_accessor_resolves_at_one_frame() {
    let (reg, ctx) = (registry(), SymbolContext::cargo("logos-core", "0.1.0"));
    let caller = (
        "producer/src/main/java/ArchiveService.java",
        wrapper_caller("ArchiveService", "kafkaTopics.getArchiveEvents()"),
    );

    let facts = extract_files(&wrapper_estate(&[caller]), &reg, &ctx);
    assert_eq!(
        broker_targets(&facts, WRAPPER_FILE, ArtifactRelation::BrokerPublish),
        vec![ARCHIVE_EVENTS_KEY.to_string()],
        "the wrapper's site is keyed on what its callers pass, and the keyless \
         refusal row beside it is retracted",
    );
    assert_eq!(
        forwarding_outcomes(&facts, WRAPPER_FILE),
        vec![ForwardingOutcome::Resolved(Forwarded {
            topic: ARCHIVE_EVENTS_KEY.to_string(),
            chain: vec!["KafkaProducer.sendMessage/3".to_string()],
        })],
        "one frame, and the provenance names it",
    );

    let alone = extract_files(&wrapper_estate(&[]), &reg, &ctx);
    assert_eq!(
        broker_targets(&alone, WRAPPER_FILE, ArtifactRelation::BrokerPublish),
        vec![String::new()],
        "with no caller the identical site is the keyless refusal row it always was",
    );
    assert_eq!(forwarding_refusals(&alone, WRAPPER_FILE), vec!["no-call-site"]);
}

/// **[FR-WS-26] AC1, two frames.** The same wrapper reached through a
/// `super.sendMessage(…)` override resolves at the second frame, with provenance
/// naming **both**.
///
/// The override is the estate's own shape, and it is why the receiver rule
/// exists: the `super` call inside the override is a caller of the BASE, and it
/// must not also count as a caller of the override when the second frame looks
/// that override up — following it there reads the override's own parameter and
/// reports three frames.
///
/// [FR-WS-26]: ../../docs/specs/requirements/FR-WS-26.md
#[test]
#[cfg(feature = "lang-java")]
fn a_super_override_resolves_the_wrapper_at_two_frames() {
    let (reg, ctx) = (registry(), SymbolContext::cargo("logos-core", "0.1.0"));
    let override_source = "package a;\n\
        public class ArchiveEventKafkaProducer extends KafkaProducer {\n\
        \x20   @Override\n\
        \x20   void sendMessage(String key, String payload, String topic) {\n\
        \x20     super.sendMessage(key, payload, topic);\n\
        \x20   }\n\
        }\n";
    let files = [
        (
            "producer/src/main/java/ArchiveEventKafkaProducer.java",
            override_source.to_string(),
        ),
        (
            "producer/src/main/java/ArchiveService.java",
            // The service holds the CONCRETE producer — Spring's own injection
            // shape, and the only one that makes this a genuine two-frame case.
            // A service holding the base type calls the BASE's declaration
            // directly, which frame one resolves without ever needing a second;
            // it is the override's parameter that has no caller then, and the
            // hop says so. Pinned below.
            wrapper_caller_via(
                "ArchiveService",
                "ArchiveEventKafkaProducer",
                "kafkaTopics.getArchiveEvents()",
            ),
        ),
    ];

    let facts = extract_files(&wrapper_estate(&files), &reg, &ctx);
    assert_eq!(
        forwarding_outcomes(&facts, WRAPPER_FILE),
        vec![ForwardingOutcome::Resolved(Forwarded {
            topic: ARCHIVE_EVENTS_KEY.to_string(),
            chain: vec![
                "KafkaProducer.sendMessage/3".to_string(),
                "ArchiveEventKafkaProducer.sendMessage/3".to_string(),
            ],
        })],
        "the provenance names both frames, and names them apart — the two share a \
         signature, so a chain keyed on the signature alone would print one frame \
         twice",
    );
    assert_eq!(
        broker_targets(&facts, WRAPPER_FILE, ArtifactRelation::BrokerPublish),
        vec![ARCHIVE_EVENTS_KEY.to_string()],
    );

    // The negative control that isolates the SECOND frame: drop the service and
    // the only caller left is the `super` call, whose operand is the override's
    // own parameter with nothing behind it.
    let without = extract_files(&wrapper_estate(&files[..1]), &reg, &ctx);
    assert_eq!(
        forwarding_refusals(&without, WRAPPER_FILE),
        vec!["no-call-site"],
        "with nothing calling the override, the second frame has no call site and \
         the wrapper refuses",
    );
}

/// **[FR-WS-26] AC3, the receiver rule's second half.** A homonymous
/// `(name, arity)` on an **unrelated** type is not a caller, and must not supply
/// the operand.
///
/// This is the shape review reproduced, and it is the reason the rule exists.
/// [`Callee`] is `(name, arity)` — all the `Calls` ledger's target text can
/// express — so without the rule every same-signature call in the module is
/// admitted. `sendMessage`, `send` and `publish` are exactly the names the
/// population is built on, so the collision is the common case rather than a
/// contrived one.
///
/// Both directions are pinned on one estate, because the hazard is symmetric and
/// a fixture for either alone would leave the other free to regress:
///
/// * **fabrication** — the wrapper has NO caller of its own, and the homonym's
///   topic must not become its topic. Before the rule this emitted a
///   `BrokerPublish` on `"audit-log"` and retracted the keyless row, so the
///   invented key was the only thing left ([NFR-RA-05]).
/// * **destruction** — the wrapper HAS a genuine accessor caller, and the
///   homonym beside it must not turn that resolution into a disagreement.
///
/// [FR-WS-26]: ../../docs/specs/requirements/FR-WS-26.md
/// [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
#[test]
#[cfg(feature = "lang-java")]
fn a_homonym_on_an_unrelated_type_is_not_a_caller() {
    let (reg, ctx) = (registry(), SymbolContext::cargo("logos-core", "0.1.0"));
    // A different class, the same (name, arity), and a caller that holds IT.
    let homonym = (
        "producer/src/main/java/AuditProducer.java",
        "package a;\n\
         public class AuditProducer {\n\
         \x20   void sendMessage(String key, String payload, String topic) {}\n\
         }\n"
            .to_string(),
    );
    let audit_caller = (
        "producer/src/main/java/AuditService.java",
        wrapper_caller_via("AuditService", "AuditProducer", "\"audit-log\""),
    );

    let fabrication = extract_files(
        &wrapper_estate(&[homonym.clone(), audit_caller.clone()]),
        &reg,
        &ctx,
    );
    assert_eq!(
        forwarding_refusals(&fabrication, WRAPPER_FILE),
        vec!["no-call-site"],
        "the wrapper has no caller of its own, so it refuses — it does not borrow \
         the topic the homonym's caller passed",
    );
    assert_eq!(
        broker_targets(&fabrication, WRAPPER_FILE, ArtifactRelation::BrokerPublish),
        vec![String::new()],
        "and the keyless refusal row stands: no topic is fabricated",
    );

    let beside_a_real_caller = extract_files(
        &wrapper_estate(&[
            homonym,
            audit_caller,
            (
                "producer/src/main/java/ArchiveService.java",
                wrapper_caller("ArchiveService", "kafkaTopics.getArchiveEvents()"),
            ),
        ]),
        &reg,
        &ctx,
    );
    assert_eq!(
        broker_targets(&beside_a_real_caller, WRAPPER_FILE, ArtifactRelation::BrokerPublish),
        vec![ARCHIVE_EVENTS_KEY.to_string()],
        "and the homonym does not destroy a genuine resolution either — it is not \
         a disagreeing caller, it is not a caller",
    );
}

/// **[FR-WS-26] AC3.** A caller holding the **base** type calls the base's own
/// declaration, so the wrapper resolves at ONE frame — and the override's
/// parameter, which nothing then calls, is what refuses.
///
/// The counterpart to [`a_super_override_resolves_the_wrapper_at_two_frames`],
/// on a tree identical but for the receiver's declared type. Together they pin
/// that the receiver rule decides WHICH frame carries the answer, rather than
/// merely removing call sites: the same two files resolve through one frame or
/// through two depending on a single type name.
///
/// [FR-WS-26]: ../../docs/specs/requirements/FR-WS-26.md
#[test]
#[cfg(feature = "lang-java")]
fn a_base_typed_caller_resolves_the_base_at_one_frame() {
    let (reg, ctx) = (registry(), SymbolContext::cargo("logos-core", "0.1.0"));
    let override_source = "package a;\n\
        public class ArchiveEventKafkaProducer extends KafkaProducer {\n\
        \x20   @Override\n\
        \x20   void sendMessage(String key, String payload, String topic) {\n\
        \x20     super.sendMessage(key, payload, topic);\n\
        \x20   }\n\
        }\n";
    let facts = extract_files(
        &wrapper_estate(&[
            (
                "producer/src/main/java/ArchiveEventKafkaProducer.java",
                override_source.to_string(),
            ),
            (
                "producer/src/main/java/ArchiveService.java",
                wrapper_caller_via(
                    "ArchiveService",
                    "KafkaProducer",
                    "kafkaTopics.getArchiveEvents()",
                ),
            ),
        ]),
        &reg,
        &ctx,
    );
    assert_eq!(
        forwarding_refusals(&facts, WRAPPER_FILE),
        vec!["no-call-site"],
        "the base-typed call site resolves frame one, but the override's OWN \
         super-call still forwards a parameter nothing supplies, and the \
         agreement refuses on the frame-two cause rather than averaging it away",
    );
    assert_eq!(
        broker_targets(&facts, WRAPPER_FILE, ArtifactRelation::BrokerPublish),
        vec![String::new()],
        "so no topic is admitted, and the keyless row stands",
    );
}

/// **[FR-WS-26] AC2.** The five refusals, each pinned, and each on a tree whose
/// only difference from the resolving one is the thing being refused.
///
/// They are one test because they are one table: the same wrapper, the same
/// module, one caller shape per row. Splitting them would repeat the estate
/// three hundred lines over and let a row drift from the shape it is contrasted
/// with.
///
/// [FR-WS-26]: ../../docs/specs/requirements/FR-WS-26.md
#[test]
#[cfg(feature = "lang-java")]
fn each_named_forwarding_refusal_is_pinned_on_its_own_tree() {
    let (reg, ctx) = (registry(), SymbolContext::cargo("logos-core", "0.1.0"));

    // A third frame: the wrapper's caller forwards a parameter of its own, and
    // *that* method's caller forwards again.
    let relay = "package a;\n\
        public class Relay {\n\
        \x20   private final KafkaProducer producer;\n\
        \x20   void relay(String topic) {\n\
        \x20     producer.sendMessage(\"k\", \"p\", topic);\n\
        \x20   }\n\
        \x20   void outer(String topic) {\n\
        \x20     relay(topic);\n\
        \x20   }\n\
        }\n";

    // (case, the caller files it adds to the base estate, the refusal it must
    // report). A named alias so the table's shape reads as one thing.
    type RefusalCase<'a> = (&'a str, Vec<(&'a str, String)>, &'a str);
    let cases: [RefusalCase<'_>; 5] = [
        (
            "a third frame",
            vec![("producer/src/main/java/Relay.java", relay.to_string())],
            "three-or-more-frames",
        ),
        (
            "a caller outside the build module",
            vec![
                ("consumer/pom.xml", "<project/>\n".to_string()),
                (
                    "consumer/src/main/java/ArchiveService.java",
                    wrapper_caller("ArchiveService", "kafkaTopics.getArchiveEvents()"),
                ),
            ],
            "out-of-module",
        ),
        (
            "two callers passing different keys",
            vec![
                (
                    "producer/src/main/java/ArchiveService.java",
                    wrapper_caller("ArchiveService", "kafkaTopics.getArchiveEvents()"),
                ),
                (
                    "producer/src/main/java/ReportService.java",
                    wrapper_caller("ReportService", "kafkaTopics.getArchiveReporting()"),
                ),
            ],
            "disagree",
        ),
        (
            "a caller passing a bare variable",
            vec![(
                "producer/src/main/java/ArchiveService.java",
                wrapper_caller("ArchiveService", "ARCHIVE_TOPIC"),
            )],
            "unresolvable-operand",
        ),
        (
            "a caller reaching the wrapper by method reference only",
            vec![(
                "producer/src/main/java/ArchiveService.java",
                "package a;\n\
                 public class ArchiveService {\n\
                 \x20   private final KafkaProducer producer;\n\
                 \x20   void archive(java.util.List<String> all) {\n\
                 \x20     all.forEach(producer::sendMessage);\n\
                 \x20   }\n\
                 }\n"
                    .to_string(),
            )],
            "unresolvable-operand",
        ),
    ];

    for (case, files, want) in cases {
        let facts = extract_files(&wrapper_estate(&files), &reg, &ctx);
        assert_eq!(
            forwarding_refusals(&facts, WRAPPER_FILE),
            vec![want],
            "{case}: refuses, and reports the cause it actually had",
        );
        assert_eq!(
            broker_targets(&facts, WRAPPER_FILE, ArtifactRelation::BrokerPublish),
            vec![String::new()],
            "{case}: the keyless refusal row stands, and no topic is fabricated",
        );
    }
}

/// **[FR-WS-26] AC2, the signature refusals.** A varargs or explicit-receiver
/// wrapper is refused **before** any slot arithmetic, because the positional
/// correspondence the hop rests on does not hold for either — a varargs slot
/// absorbs any number of arguments, and a receiver parameter is supplied by
/// none.
///
/// Both are contrasted with the ordinary signature on an otherwise identical
/// tree, so the row is about the signature and not about the caller.
///
/// [FR-WS-26]: ../../docs/specs/requirements/FR-WS-26.md
#[test]
#[cfg(feature = "lang-java")]
fn a_varargs_or_explicit_receiver_signature_is_refused_before_the_slot_arithmetic() {
    let (reg, ctx) = (registry(), SymbolContext::cargo("logos-core", "0.1.0"));
    let wrapper = |params: &str, call: &str| {
        format!(
            "package a;\n\
             public class KafkaProducer {{\n\
             \x20   void sendMessage({params}) {{\n\
             \x20     MessageBuilder.withPayload(\"p\")\n\
             \x20       .setHeader(KafkaHeaders.TOPIC, topic)\n\
             \x20       .build();\n\
             \x20   }}\n\
             \x20   void call(KafkaTopics kafkaTopics) {{\n\
             \x20     {call};\n\
             \x20   }}\n\
             }}\n"
        )
    };

    for (case, params, call, want) in [
        (
            "the ordinary signature, which must resolve",
            "String topic",
            "sendMessage(kafkaTopics.getArchiveEvents())",
            "resolved",
        ),
        (
            "varargs",
            "String topic, String... headers",
            "sendMessage(kafkaTopics.getArchiveEvents())",
            "unsupported-signature",
        ),
        (
            "an explicit receiver",
            "KafkaProducer this, String topic",
            "sendMessage(kafkaTopics.getArchiveEvents())",
            "unsupported-signature",
        ),
    ] {
        let inputs = vec![
            FileInput::new(WRAPPER_MODULE_POM, "<project/>\n"),
            FileInput::new(WRAPPER_PROPS_FILE, TOPIC_PROPS_SOURCE),
            FileInput::new(WRAPPER_FILE, wrapper(params, call)),
        ];
        let facts = extract_files(&inputs, &reg, &ctx);
        assert_eq!(
            forwarding_refusals(&facts, WRAPPER_FILE),
            vec![want],
            "{case}",
        );
    }
}

/// **[FR-WS-26] AC2, the test tree.** A `src/test` call site neither admits nor
/// vetoes: the `Mockito.any()` stub beside a real `src/main` caller does not stop
/// the wrapper resolving, and on its own it is an absence rather than a refusal
/// with a cause it did not have.
///
/// The control is the same file moved into `src/main`, where the identical
/// `any()` operand DOES veto. That is what proves the exclusion is the tree rule
/// and not the operand shape.
///
/// [FR-WS-26]: ../../docs/specs/requirements/FR-WS-26.md
#[test]
#[cfg(feature = "lang-java")]
fn a_test_tree_call_site_neither_admits_nor_vetoes() {
    let (reg, ctx) = (registry(), SymbolContext::cargo("logos-core", "0.1.0"));
    let stub = "package a;\n\
        public class ArchiveServiceTest {\n\
        \x20   private final KafkaProducer producer;\n\
        \x20   void sends() {\n\
        \x20     producer.sendMessage(any(), any(), any());\n\
        \x20   }\n\
        }\n";
    let real = (
        "producer/src/main/java/ArchiveService.java",
        wrapper_caller("ArchiveService", "kafkaTopics.getArchiveEvents()"),
    );

    let beside = extract_files(
        &wrapper_estate(&[
            real.clone(),
            (
                "producer/src/test/java/ArchiveServiceTest.java",
                stub.to_string(),
            ),
        ]),
        &reg,
        &ctx,
    );
    assert_eq!(
        forwarding_refusals(&beside, WRAPPER_FILE),
        vec!["resolved"],
        "a test call site beside a real one does not veto",
    );

    let alone = extract_files(
        &wrapper_estate(&[(
            "producer/src/test/java/ArchiveServiceTest.java",
            stub.to_string(),
        )]),
        &reg,
        &ctx,
    );
    assert_eq!(
        forwarding_refusals(&alone, WRAPPER_FILE),
        vec!["no-call-site"],
        "and on its own it does not admit either — the main-tree reading is left \
         with no call site, which is what it reports",
    );

    let promoted = extract_files(
        &wrapper_estate(&[(
            "producer/src/main/java/ArchiveServiceStub.java",
            stub.to_string(),
        )]),
        &reg,
        &ctx,
    );
    assert_eq!(
        forwarding_refusals(&promoted, WRAPPER_FILE),
        vec!["unresolvable-operand"],
        "the identical operand in src/main DOES veto, so the exclusion above is \
         the tree rule and not the operand's shape",
    );
}

/// **[FR-WS-26], the population boundary.** A topic that is a **lambda**
/// parameter is not a forwarding candidate at all — a lambda is not a positional
/// method call site, so there is nothing to look up.
///
/// Both spellings are pinned. The parenthesised one puts a `formal_parameters`
/// list in the lambda's `parameters` field; the bare one puts a single
/// `identifier` there, and without recognising that shape the walk climbs past
/// the lambda and does the slot arithmetic against the ENCLOSING method's
/// parameter list — a different list, a wrong arity, and a slot index that means
/// nothing.
///
/// The enclosing method therefore declares a parameter of the **same name**,
/// deliberately: that is the only shape in which climbing past the lambda finds
/// anything at all, so a fixture without it would pass with the lambda rule
/// removed and pin nothing.
///
/// [FR-WS-26]: ../../docs/specs/requirements/FR-WS-26.md
#[test]
#[cfg(feature = "lang-java")]
fn a_lambda_parameter_topic_is_not_a_forwarding_candidate() {
    let (reg, ctx) = (registry(), SymbolContext::cargo("logos-core", "0.1.0"));
    for (case, lambda) in [
        ("parenthesised", "(String topic) ->"),
        ("bare", "topic ->"),
    ] {
        let src = format!(
            "package a;\n\
             public class Registrar {{\n\
             \x20   void register(String topic, java.util.function.Consumer<Object> sink) {{\n\
             \x20     sink.accept({lambda} MessageBuilder.withPayload(topic)\n\
             \x20       .setHeader(KafkaHeaders.TOPIC, topic)\n\
             \x20       .build());\n\
             \x20   }}\n\
             }}\n"
        );
        let path = "producer/src/main/java/Registrar.java";
        let facts = extract_files(
            &[
                FileInput::new(WRAPPER_MODULE_POM, "<project/>\n"),
                FileInput::new(WRAPPER_PROPS_FILE, TOPIC_PROPS_SOURCE),
                FileInput::new(path, &src),
            ],
            &reg,
            &ctx,
        );
        assert!(
            forwarding_outcomes(&facts, path).is_empty(),
            "{case}: a lambda parameter is outside the population, so no candidate \
             is recorded and none is decided",
        );
        assert_eq!(
            broker_targets(&facts, path, ArtifactRelation::BrokerPublish),
            vec![String::new()],
            "{case}: the site is still reported refused, exactly as before the hop",
        );
    }
}

/// **[FR-WS-26], the ledger contract.** The hop retracts a keyless row only when
/// **every** refusal that row stands for has resolved.
///
/// The ledger dedups on `(source, target, form, kind, relation)` and ignores the
/// line, so two refused sites in one declaration reach it as ONE row. Retracting
/// it on the first resolution would silence the second site, which is still
/// refused — the invisible loss [NFR-CC-04] forbids, in the one shape this hop
/// could create.
///
/// [FR-WS-26]: ../../docs/specs/requirements/FR-WS-26.md
/// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
#[test]
#[cfg(feature = "lang-java")]
fn a_keyless_row_standing_for_two_refusals_survives_one_resolution() {
    let (reg, ctx) = (registry(), SymbolContext::cargo("logos-core", "0.1.0"));
    // One declaration, two publish sites: one forwards its own parameter (which
    // the hop resolves), one forwards a parameter of a wrapper nothing calls.
    let two_sites = "package a;\n\
        public class KafkaProducer {\n\
        \x20   void sendMessage(String key, String payload, String topic) {\n\
        \x20     MessageBuilder.withPayload(payload)\n\
        \x20       .setHeader(KafkaHeaders.TOPIC, topic)\n\
        \x20       .build();\n\
        \x20     MessageBuilder.withPayload(payload)\n\
        \x20       .setHeader(KafkaHeaders.TOPIC, key)\n\
        \x20       .build();\n\
        \x20   }\n\
        }\n";
    let inputs = vec![
        FileInput::new(WRAPPER_MODULE_POM, "<project/>\n"),
        FileInput::new(WRAPPER_PROPS_FILE, TOPIC_PROPS_SOURCE),
        FileInput::new(WRAPPER_FILE, two_sites),
        FileInput::new(
            "producer/src/main/java/ArchiveService.java",
            format!(
                "package a;\n\
                 public class ArchiveService {{\n\
                 \x20   private final KafkaTopics kafkaTopics;\n\
                 \x20   private final KafkaProducer producer;\n\
                 \x20   void archive(String payload) {{\n\
                 \x20     producer.sendMessage(UNPROVABLE, payload, {});\n\
                 \x20   }}\n\
                 }}\n",
                "kafkaTopics.getArchiveEvents()"
            ),
        ),
    ];

    let facts = extract_files(&inputs, &reg, &ctx);
    assert_eq!(
        forwarding_refusals(&facts, WRAPPER_FILE),
        vec!["resolved", "unresolvable-operand"],
        "the topic slot resolves; the key slot does not",
    );
    let mut targets = broker_targets(&facts, WRAPPER_FILE, ArtifactRelation::BrokerPublish);
    targets.sort();
    assert_eq!(
        targets,
        vec![String::new(), ARCHIVE_EVENTS_KEY.to_string()],
        "the resolved site is admitted AND the keyless row stays, because the row \
         still stands for a site that is refused",
    );
}

/// **[FR-WS-26] AC6, at the grain that actually broke it.** Two refused sites on
/// **one physical line** are two sites behind one row, and resolving one of them
/// must not retract it.
///
/// The sibling test above covers two sites on two lines, which the first version
/// of this hop got right. This one covers the same declaration with both
/// publishes on ONE line, which it got wrong: the keyless row is deduped per
/// `(relation, declaration, line)`, so counting ROWS instead of SITES made the
/// two look like one — the hop resolved the first, retracted the row, and the
/// second site left the output carrying neither a topic nor a refusal. That is
/// strictly less evidence than before the hop existed, which is the one thing it
/// is never allowed to produce ([NFR-CC-04]).
///
/// Java permits the shape and the reference estate does not write it; it is
/// pinned because the invariant is stated in the ledger contract, not because
/// the input is likely.
///
/// [FR-WS-26]: ../../docs/specs/requirements/FR-WS-26.md
/// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
#[test]
#[cfg(feature = "lang-java")]
fn two_refused_sites_on_one_line_are_two_sites_behind_one_row() {
    let (reg, ctx) = (registry(), SymbolContext::cargo("logos-core", "0.1.0"));
    // Both publishes on one physical line, in one declaration.
    let two_sites = "package a;\n\
        public class KafkaProducer {\n\
        \x20   void sendBoth(String t1, String t2) {\n\
        \x20     MessageBuilder.withPayload(\"p\").setHeader(KafkaHeaders.TOPIC, t1).build(); MessageBuilder.withPayload(\"p\").setHeader(KafkaHeaders.TOPIC, t2).build();\n\
        \x20   }\n\
        }\n";
    let inputs = vec![
        FileInput::new(WRAPPER_MODULE_POM, "<project/>\n"),
        FileInput::new(WRAPPER_PROPS_FILE, TOPIC_PROPS_SOURCE),
        FileInput::new(WRAPPER_FILE, two_sites),
        FileInput::new(
            "producer/src/main/java/Caller2.java",
            "package a;\n\
             public class Caller2 {\n\
             \x20   private final KafkaTopics kafkaTopics;\n\
             \x20   private final KafkaProducer two;\n\
             \x20   void go() {\n\
             \x20     two.sendBoth(kafkaTopics.getArchiveEvents(), UNPROVABLE);\n\
             \x20   }\n\
             }\n",
        ),
    ];

    let facts = extract_files(&inputs, &reg, &ctx);
    assert_eq!(
        forwarding_refusals(&facts, WRAPPER_FILE),
        vec!["resolved", "unresolvable-operand"],
        "BOTH sites are looked up — the second is not dropped by a line dedup",
    );
    let mut targets = broker_targets(&facts, WRAPPER_FILE, ArtifactRelation::BrokerPublish);
    targets.sort();
    assert_eq!(
        targets,
        vec![String::new(), ARCHIVE_EVENTS_KEY.to_string()],
        "the resolved site is admitted AND the keyless row survives for the one \
         that is still refused — never fewer rows than before the hop existed",
    );
}

/// **[FR-WS-26] and [NFR-PE-02].** A literal topic operand is outside the
/// population, and a tree whose broker site carries one is untouched by the hop.
///
/// Two assertions, and the second is what makes this more than a statement about
/// emptiness: the wrapper file's whole [`Facts`] is compared against the SAME
/// tree with the accessor-bearing caller removed, so a hop that retracted, moved
/// or re-lined a literal row in passing fails here even though it recorded no
/// candidate.
///
/// [FR-WS-26]: ../../docs/specs/requirements/FR-WS-26.md
/// [NFR-PE-02]: ../../docs/specs/requirements/NFR-PE-02.md
#[test]
#[cfg(feature = "lang-java")]
fn a_tree_with_no_parameter_operand_is_untouched_by_the_hop() {
    let (reg, ctx) = (registry(), SymbolContext::cargo("logos-core", "0.1.0"));
    let literal = "package a;\n\
        public class KafkaProducer {\n\
        \x20   void sendMessage(String key, String payload) {\n\
        \x20     MessageBuilder.withPayload(payload)\n\
        \x20       .setHeader(KafkaHeaders.TOPIC, \"archive-events\")\n\
        \x20       .build();\n\
        \x20   }\n\
        }\n";
    let inputs = vec![
        FileInput::new(WRAPPER_MODULE_POM, "<project/>\n"),
        FileInput::new(WRAPPER_PROPS_FILE, TOPIC_PROPS_SOURCE),
        FileInput::new(WRAPPER_FILE, literal),
        FileInput::new(
            "producer/src/main/java/ArchiveService.java",
            wrapper_caller("ArchiveService", "kafkaTopics.getArchiveEvents()"),
        ),
    ];
    let facts = extract_files(&inputs, &reg, &ctx);
    let wrapper = facts
        .iter()
        .find(|f| f.path == WRAPPER_FILE)
        .expect("the wrapper was extracted");
    assert!(
        wrapper.forwarding.is_empty(),
        "a literal topic is not a forwarding candidate",
    );
    assert_eq!(
        broker_targets(&facts, WRAPPER_FILE, ArtifactRelation::BrokerPublish),
        vec!["archive-events".to_string()],
        "and the literal keys exactly as it did before the hop existed",
    );

    let without_caller = extract_files(&inputs[..3], &reg, &ctx);
    assert_eq!(
        without_caller.iter().find(|f| f.path == WRAPPER_FILE),
        Some(wrapper),
        "every field of the wrapper's facts is identical with the caller removed:          the hop moved no node, no edge, no warning and no reference in passing",
    );
}

/// **[FR-WS-26].** The single-file [`extract`] entry point records the candidate
/// and decides **nothing**.
///
/// One file cannot answer a cross-file question, and the honest output is an
/// undecided candidate rather than a refusal that was never tested. The same
/// distinction [`extract`] already documents for the accessor chain's properties
/// index.
///
/// [FR-WS-26]: ../../docs/specs/requirements/FR-WS-26.md
#[test]
#[cfg(feature = "lang-java")]
fn the_single_file_entry_point_records_the_candidate_and_decides_nothing() {
    let reg = registry();
    let plugin = reg.for_extension("java").expect("java plugin present");
    let facts = extract(
        &FileInput::new(WRAPPER_FILE, WRAPPER_SOURCE),
        plugin,
        &SymbolContext::cargo("logos-core", "0.1.0"),
    );
    assert_eq!(facts.forwarding.len(), 1, "the site is in the population");
    assert_eq!(
        facts.forwarding[0].outcome, None,
        "and is left undecided, not refused",
    );
    assert_eq!(
        facts.forwarding[0].wrapper,
        Callee { name: "sendMessage".to_string(), arity: 3 },
    );
    assert_eq!(facts.forwarding[0].slot, 2);
}


// ── S-439 / CR-142 D1: a module specifier is a path, not a member expression ──

/// Every `Imports` ledger target the file records, sorted, with its alias.
fn import_targets(facts: &Facts) -> Vec<(String, Option<String>)> {
    let mut out: Vec<(String, Option<String>)> = facts
        .refs
        .iter()
        .filter(|r| r.kind == EdgeKind::Imports)
        .map(|r| (r.target.clone(), r.alias.clone()))
        .collect();
    out.sort();
    out
}

fn targets_only(facts: &Facts) -> Vec<String> {
    import_targets(facts).into_iter().map(|(t, _)| t).collect()
}

#[cfg(feature = "lang-typescript")]
#[test]
fn a_ts_import_with_an_explicit_extension_is_recorded_as_a_relative_path() {
    // The two CR-142 §3.1 evidence rows, verbatim: before S-439 the ledger held
    // `nav::ts` and `shell::Header::tsx`. Pinned separately from the
    // extension-less spelling below — that distinction is what produced 0 bound
    // imports in one corpus and 9 in another.
    let src = "import { navItemsFor } from \"./nav.ts\";\nimport Header from \"./shell/Header.tsx\";\n";
    for (ext, path) in [("tsx", "web/ui/src/App.tsx"), ("ts", "web/ui/src/app.ts")] {
        let facts = extract_lang(ext, path, src);
        assert_eq!(
            import_targets(&facts),
            [
                (".::nav".to_string(), Some("nav".to_string())),
                (".::shell::Header".to_string(), Some("Header".to_string())),
            ],
            "{ext}: the extension is the file's own and is stripped; `.` marks it relative"
        );
    }
}

#[cfg(feature = "lang-typescript")]
#[test]
fn a_ts_import_without_an_extension_is_recorded_as_the_same_relative_path() {
    // desk-picker's spelling, pinned on its own fixture.
    let src = "import { useAuth } from './auth/AuthContext';\nimport api from '../api';\n";
    for (ext, path) in [
        ("tsx", "frontend/src/pages/Home.tsx"),
        ("ts", "frontend/src/pages/home.ts"),
    ] {
        let facts = extract_lang(ext, path, src);
        assert_eq!(
            targets_only(&facts),
            ["..::api", ".::auth::AuthContext"],
            "{ext}: an extension-less relative specifier keeps its relative head"
        );
    }
}

#[cfg(feature = "lang-typescript")]
#[test]
fn a_js_require_is_a_path_specifier_too() {
    // JavaScript rides the typescript grammar (`.js`/`.mjs`/`.cjs`).
    let src = "const util = require('./lib/util.js');\nconst express = require('express');\n";
    let facts = extract_lang("js", "server/app.js", src);
    assert_eq!(targets_only(&facts), [".::lib::util", "express"]);
}

#[cfg(feature = "lang-typescript")]
#[test]
fn a_ts_package_import_is_unchanged_by_the_path_grammar() {
    // A bare specifier names a package: no relative head, no stripping, and the
    // `::`-joined form the framework candidacy gate matches (`next`, `react`).
    let src = "import React from 'react';\nimport Link from 'next/link';\nimport '@tanstack/react-query';\nimport c from 'chart.js';\n";
    let facts = extract_lang("tsx", "web/ui/src/Page.tsx", src);
    assert_eq!(
        targets_only(&facts),
        ["@tanstack::react-query", "chart.js", "next::link", "react"]
    );
}

#[cfg(feature = "lang-go")]
#[test]
fn a_go_import_path_keeps_its_dotted_host_name_whole() {
    // The Go evidence row: before S-439 the ledger held
    // `github::com::sourcesense::desk-picker::internal::admin`.
    let src = "package main\n\nimport (\n\t\"context\"\n\t\"net/http\"\n\n\t\"github.com/lib/pq\"\n\t\"github.com/sourcesense/desk-picker/internal/admin\"\n)\n";
    let facts = extract_lang("go", "cmd/server/main.go", src);
    assert_eq!(
        import_targets(&facts),
        [
            ("context".to_string(), Some("context".to_string())),
            ("github.com::lib::pq".to_string(), Some("pq".to_string())),
            (
                "github.com::sourcesense::desk-picker::internal::admin".to_string(),
                Some("admin".to_string())
            ),
            ("net::http".to_string(), Some("http".to_string())),
        ]
    );
}

#[test]
fn a_name_grammar_import_and_a_member_path_still_split_on_every_dot() {
    // The member-path grammar is unchanged in every language (S-439 AC5): a
    // Python dotted import and a Java scoped import stay name-shaped, and a TS
    // `a.b.c()` still records only its member name.
    #[cfg(feature = "lang-python")]
    {
        let facts = extract_lang(
            "py",
            "app/views.py",
            "import a.b.c\nfrom django.urls import path\n",
        );
        assert_eq!(targets_only(&facts), ["a::b::c", "django::urls"]);
    }
    #[cfg(feature = "lang-java")]
    {
        let facts = extract_lang(
            "java",
            "src/main/java/x/A.java",
            "import org.springframework.web.bind.annotation.GetMapping;\nclass A {}\n",
        );
        assert_eq!(
            targets_only(&facts),
            ["org::springframework::web::bind::annotation::GetMapping"]
        );
    }
    #[cfg(feature = "lang-typescript")]
    {
        let facts = extract_lang("ts", "src/x.ts", "function f() { a.b.c(); }\n");
        assert!(facts
            .refs
            .iter()
            .any(|r| r.kind == EdgeKind::Calls && r.form == RefForm::Method && r.target == "c"));
    }
}

// ── S-440 / CR-142 D2: a call through an import is recorded through it ──

/// Every `Calls` ledger row the file records, as `(target, form)`, sorted.
fn call_targets(facts: &Facts) -> Vec<(String, RefForm)> {
    let mut out: Vec<(String, RefForm)> = facts
        .refs
        .iter()
        .filter(|r| r.kind == EdgeKind::Calls)
        .map(|r| (r.target.clone(), r.form))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[cfg(feature = "lang-typescript")]
#[test]
fn a_ts_call_through_a_named_import_is_recorded_through_its_module() {
    // The dominant shape in both CR-142 corpora (320 of 454, 345 of 429
    // relative import statements are named imports), a rename, and the
    // namespace form — in both TypeScript grammars.
    let src = "import { navItemsFor, isAppLevelPath } from './nav.ts';\n\
import { slugify as slug } from '../util';\n\
import * as api from './api';\n\
export function f(p: string) { navItemsFor(p); slug(p); api.get(p); return isAppLevelPath(p); }\n";
    for (ext, path) in [("ts", "src/a/f.ts"), ("tsx", "src/a/f.tsx")] {
        let facts = extract_lang(ext, path, src);
        assert_eq!(
            call_targets(&facts),
            [
                ("..::util::slugify".to_string(), RefForm::Path),
                (".::api::get".to_string(), RefForm::Path),
                (".::nav::isAppLevelPath".to_string(), RefForm::Path),
                (".::nav::navItemsFor".to_string(), RefForm::Path),
            ],
            "{ext}: a rename reads the exported name; a namespace member is qualified"
        );
    }
}

#[cfg(feature = "lang-typescript")]
#[test]
fn a_ts_call_not_through_a_relative_import_keeps_its_bare_form() {
    // A package import (`react`), a default import, a global, a method on a
    // value, and a local declaration shadowing an imported name all record
    // exactly what they recorded before S-440.
    let src = "import { useState } from 'react';\n\
import Header from './Header';\n\
import { local, obj } from './m';\n\
function local() { return 1; }\n\
export function f() { useState(); Header(); fetch('/x'); obj.run(); local(); }\n";
    let facts = extract_lang("ts", "src/f.ts", src);
    assert_eq!(
        call_targets(&facts),
        [
            ("Header".to_string(), RefForm::Path),
            ("fetch".to_string(), RefForm::Path),
            ("local".to_string(), RefForm::Path),
            ("run".to_string(), RefForm::Method),
            ("useState".to_string(), RefForm::Path),
        ]
    );
}

#[cfg(feature = "lang-typescript")]
#[test]
fn a_jsx_component_element_is_recorded_as_a_call() {
    // `<RuleFindingsCard />` calls the component; `<div>` is an intrinsic
    // element; `<Nav.Item>` is not a plain name. An imported component is
    // recorded through its module like any named-import call.
    let src = "import { Badge } from './Badge';\n\
export function View() { return <div><RuleFindingsCard /><Badge tone=\"x\">ok</Badge><Nav.Item /></div>; }\n\
function RuleFindingsCard() { return <p />; }\n";
    let facts = extract_lang("tsx", "src/View.tsx", src);
    assert_eq!(
        call_targets(&facts),
        [
            (".::Badge::Badge".to_string(), RefForm::Path),
            ("RuleFindingsCard".to_string(), RefForm::Path),
        ]
    );
}

#[cfg(feature = "lang-go")]
#[test]
fn a_go_package_qualified_call_is_recorded_through_its_import_path() {
    // `admin.Register()` names the package; `adm` is an explicit alias; `fmt`
    // is external and is recorded through its path all the same (binding it is
    // the resolver's decision); `s.Start()` is a method on a value and stays a
    // bare method name (FR-RS-06); a dot import binds no qualifier.
    let src = "package main\n\nimport (\n\t\"fmt\"\n\tadm \"example.com/shop/internal/audit\"\n\t\"example.com/shop/internal/admin\"\n\t. \"example.com/shop/internal/dot\"\n)\n\n\
func main() {\n\tadmin.Register()\n\tadm.Log()\n\tfmt.Println()\n\ts := admin.Server{}\n\ts.Start()\n\tHelper()\n}\n";
    let facts = extract_lang("go", "cmd/main.go", src);
    assert_eq!(
        call_targets(&facts),
        [
            ("Helper".to_string(), RefForm::Path),
            ("Start".to_string(), RefForm::Method),
            ("example.com::shop::internal::admin::Register".to_string(), RefForm::Path),
            ("example.com::shop::internal::audit::Log".to_string(), RefForm::Path),
            ("fmt::Println".to_string(), RefForm::Path),
        ]
    );
}

#[cfg(feature = "lang-rust")]
#[test]
fn a_rust_call_is_recorded_exactly_as_before_s440() {
    // Rust declares name-grammar imports: no import is read for qualification,
    // so `use a::helper; helper()` and `m.run()` record what they always did.
    let facts = extract_lang(
        "rs",
        "src/lib.rs",
        "use crate::a::helper;\nfn f(m: M) { helper(); m.run(); util::go(); }\n",
    );
    assert_eq!(
        call_targets(&facts),
        [
            ("helper".to_string(), RefForm::Path),
            ("run".to_string(), RefForm::Method),
            ("util::go".to_string(), RefForm::Path),
        ]
    );
}

// ── S-466 / CR-149 §3.2 B: Java type relations are captured ─────────────────

/// Every type-relation row as `(source name, kind, target)`, sorted — the source
/// named by its node, so a field's row reads as the field's.
#[cfg(feature = "lang-java")]
fn type_relation_rows(facts: &Facts) -> Vec<(String, EdgeKind, String)> {
    let name_of: HashMap<&str, &str> = facts
        .nodes
        .iter()
        .map(|n| (n.symbol.as_str(), n.name.as_str()))
        .collect();
    let mut out: Vec<(String, EdgeKind, String)> = facts
        .refs
        .iter()
        .filter(|r| {
            matches!(
                r.kind,
                EdgeKind::Extends | EdgeKind::Implements | EdgeKind::Instantiates | EdgeKind::TypeUses
            )
        })
        .map(|r| {
            assert_eq!(r.form, RefForm::Path, "a type relation is Path form: {r:?}");
            (
                name_of.get(r.source.as_str()).copied().unwrap_or("?").to_string(),
                r.kind,
                r.target.clone(),
            )
        })
        .collect();
    out.sort_by(|a, b| (&a.0, a.1.as_i32(), &a.2).cmp(&(&b.0, b.1.as_i32(), &b.2)));
    out
}

#[test]
#[cfg(feature = "lang-java")]
fn java_type_relations_record_each_shape_as_a_path_row_of_its_declaration() {
    let src = "package com.x;\n\
\n\
public class Svc extends com.x.base.Base<Dto> implements Port, Wide<Req> {\n\
    private Dto a, b;\n\
    private Map<String, List<? extends Dto>>[] nested;\n\
    public Out.In make(int n, Req... reqs) {\n\
        var local = new Dto();\n\
        for (Item item : items) {}\n\
        try (Res res = open()) {} catch (Bad | Worse e) {}\n\
        return null;\n\
    }\n\
}\n";
    let facts = extract_lang("java", "src/main/java/com/x/Svc.java", src);
    let row = |source: &str, kind: EdgeKind, target: &str| {
        (source.to_string(), kind, target.to_string())
    };
    assert_eq!(
        type_relation_rows(&facts),
        [
            row("Svc", EdgeKind::Implements, "Port"),
            row("Svc", EdgeKind::Implements, "Wide"),
            row("Svc", EdgeKind::Extends, "com::x::base::Base"),
            // The superclass's and super-interface's type arguments.
            row("Svc", EdgeKind::TypeUses, "Dto"),
            row("Svc", EdgeKind::TypeUses, "Req"),
            // Each declarator of `private Dto a, b;` owns the field's type.
            row("a", EdgeKind::TypeUses, "Dto"),
            row("b", EdgeKind::TypeUses, "Dto"),
            row("make", EdgeKind::Instantiates, "Dto"),
            // `var` is not a type name; primitives name no type.
            row("make", EdgeKind::TypeUses, "Bad"),
            row("make", EdgeKind::TypeUses, "Item"),
            row("make", EdgeKind::TypeUses, "Out::In"),
            row("make", EdgeKind::TypeUses, "Req"),
            row("make", EdgeKind::TypeUses, "Res"),
            row("make", EdgeKind::TypeUses, "Worse"),
            // An array of a generic, a wildcard's bound, every argument.
            row("nested", EdgeKind::TypeUses, "Dto"),
            row("nested", EdgeKind::TypeUses, "List"),
            row("nested", EdgeKind::TypeUses, "Map"),
            row("nested", EdgeKind::TypeUses, "String"),
        ]
    );
}

#[test]
#[cfg(feature = "lang-java")]
fn declared_superclass_reads_the_one_extends_row_a_class_records() {
    let src = "package com.x;\n\
\n\
public class Sub extends Base<Dto> {}\n\
class Plain {}\n\
interface Both extends A, B {}\n\
interface One extends A {}\n";
    let facts = extract_lang("java", "src/main/java/com/x/Sub.java", src);
    let symbol = |name: &str| {
        facts
            .nodes
            .iter()
            .find(|n| n.name == name && n.kind != NodeKind::Module)
            .unwrap_or_else(|| panic!("node {name}"))
            .symbol
            .clone()
    };
    assert_eq!(declared_superclass(&facts.refs, &symbol("Sub")), Some("Base"));
    assert_eq!(declared_superclass(&facts.refs, &symbol("Plain")), None);
    // Two super-interfaces name no single `super` type.
    assert_eq!(declared_superclass(&facts.refs, &symbol("Both")), None);
    assert_eq!(declared_superclass(&facts.refs, &symbol("One")), Some("A"));
}

#[test]
fn a_rust_file_records_no_extends_instantiates_or_type_use_rows() {
    // The captures are Java's alone: Rust's only type relation stays the S-281
    // `Implements` row, byte-identical.
    let src = "pub trait T { fn f(&self); }\npub struct S { x: Vec<S> }\nimpl T for S { fn f(&self) { let _ = S { x: Vec::new() }; } }\n";
    let facts = extract_src("src/lib.rs", src);
    let kinds: Vec<EdgeKind> = facts.refs.iter().map(|r| r.kind).collect();
    assert!(!kinds.contains(&EdgeKind::Extends));
    assert!(!kinds.contains(&EdgeKind::Instantiates));
    assert!(!kinds.contains(&EdgeKind::TypeUses));
    assert_eq!(implements_refs(&facts).len(), 1);
}

// ── S-500 / FR-EX-11: the callable has-body fact ──────────────────────────────

/// Fixtures for the four languages that declare no body node kind (Rust, Go,
/// Python, C). Each carries functions, methods and — where the language has one
/// — a bodyless signature that is **not** extracted as a node (a Rust trait
/// `function_signature_item`, a Go interface method, a C prototype).
const NO_BODY_KIND_FIXTURES: [(&str, &str, &str); 4] = [
    (
        "rs",
        "src/shapes.rs",
        "pub trait Area { fn area(&self) -> f64; fn unit(&self) -> &str { \"m2\" } }\n\
pub struct Sq { side: f64 }\n\
impl Area for Sq {\n    fn area(&self) -> f64 {\n        if self.side > 0.0 { self.side * self.side } else { 0.0 }\n    }\n}\n\
impl Sq { pub fn new(side: f64) -> Self { Sq { side } } }\n\
fn helper(xs: &[u32]) -> u32 {\n    let mut t = 0;\n    for x in xs { if *x > 1 { t += x; } }\n    t\n}\n\
#[cfg(test)]\nmod tests { #[test] fn t() { assert_eq!(super::helper(&[2]), 2); } }\n",
    ),
    (
        "go",
        "shapes/shapes.go",
        "package shapes\n\n\
type Area interface {\n\tArea() float64\n}\n\n\
type Sq struct{ side float64 }\n\n\
func (s Sq) Area() float64 {\n\tif s.side > 0 {\n\t\treturn s.side * s.side\n\t}\n\treturn 0\n}\n\n\
func New(side float64) Sq { return Sq{side: side} }\n\n\
func helper(xs []int) int {\n\tt := 0\n\tfor _, x := range xs {\n\t\tt += x\n\t}\n\treturn t\n}\n",
    ),
    (
        "py",
        "shapes/area.py",
        "import abc\n\n\
class Area(abc.ABC):\n    @abc.abstractmethod\n    def area(self):\n        ...\n\n\
class Sq(Area):\n    def __init__(self, side):\n        self.side = side\n\n    def area(self):\n        if self.side > 0:\n            return self.side * self.side\n        return 0\n\n\
def helper(xs):\n    t = 0\n    for x in xs:\n        t += x\n    return t\n\n\
def test_helper():\n    assert helper([2]) == 2\n",
    ),
    (
        "c",
        "src/shapes.c",
        "#include <stdio.h>\n\n\
int helper(const int *xs, int n);\n\n\
struct sq { double side; };\n\n\
static double area(struct sq *s) {\n    if (s->side > 0) { return s->side * s->side; }\n    return 0;\n}\n\n\
int helper(const int *xs, int n) {\n    int t = 0;\n    for (int i = 0; i < n; i++) { t += xs[i]; }\n    return t;\n}\n",
    ),
];

/// A canonical text rendering of every fact extraction produced **before**
/// S-500 — every `NodeFact` field that existed then, plus edges, refs, the
/// partial flag and the warnings. The has-body fact and its token count are
/// deliberately left out: this is the "before" half of the byte-identity
/// comparison, so it must render only what the old graph carried.
fn pre_s500_rendering(facts: &Facts) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    writeln!(out, "{} {} partial={}", facts.path, facts.language, facts.partial).unwrap();
    for n in &facts.nodes {
        writeln!(
            out,
            "N {} {} {} {}-{} m={:?} x={} fp={:?} te={} b={:?} nd={:?} sh={:?}",
            n.symbol.as_str(),
            n.kind.as_str(),
            n.name,
            n.start_line,
            n.end_line,
            n.metrics.map(|m| (m.cyclomatic_complexity, m.line_count)),
            n.exported,
            n.fingerprint,
            n.test_evidence,
            n.body,
            n.max_nesting_depth,
            n.shingles,
        )
        .unwrap();
    }
    for e in &facts.edges {
        writeln!(out, "E {e:?}").unwrap();
    }
    for r in &facts.refs {
        writeln!(out, "R {r:?}").unwrap();
    }
    for w in &facts.warnings {
        writeln!(out, "W {w}").unwrap();
    }
    out
}

/// The blake3 digest of [`pre_s500_rendering`] over [`NO_BODY_KIND_FIXTURES`],
/// computed on the base commit (`e80309fd`, before S-500) and pinned here.
const PRE_S500_DIGEST: &str = "25987e59b78f8651e8d3ffdf7996a40f978a08291440ddcacc2031a8c40f8717";

#[test]
fn languages_declaring_no_body_kind_extract_byte_identically_to_before() {
    let mut hasher = blake3::Hasher::new();
    for (ext, path, src) in NO_BODY_KIND_FIXTURES {
        let facts = extract_lang(ext, path, src);
        assert!(!facts.partial, "{path}: fixture parses cleanly");
        hasher.update(pre_s500_rendering(&facts).as_bytes());
    }
    let digest = hasher.finalize().to_hex().to_string();
    assert_eq!(
        digest, PRE_S500_DIGEST,
        "the Rust/Go/Python/C graphs moved: every pre-S-500 fact must be byte-identical"
    );
}

/// `(name, has_body, body_tokens)` for every `Function`/`Method` node, in node
/// order — the S-500 fact a fixture asserts on.
fn callable_bodies(facts: &Facts) -> Vec<(String, bool, u32)> {
    facts
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Function | NodeKind::Method))
        .map(|n| {
            let m = n.metrics.expect("a callable carries metrics");
            (n.name.clone(), m.has_body, m.body_tokens)
        })
        .collect()
}

/// The has-body fact of the one callable named `name`.
fn has_body(facts: &Facts, name: &str) -> bool {
    let hits: Vec<bool> = callable_bodies(facts)
        .into_iter()
        .filter(|(n, _, _)| n == name)
        .map(|(_, b, _)| b)
        .collect();
    assert_eq!(hits.len(), 1, "exactly one callable named {name}: {:?}", callable_bodies(facts));
    hits[0]
}

#[test]
#[cfg(feature = "lang-java")]
fn java_abstract_and_interface_methods_record_no_body_and_implemented_ones_do() {
    let src = "package com.x;\n\
public abstract class Shape {\n    public abstract double area();\n    public String label() { return \"shape\"; }\n}\n\
interface Port {\n    void send(String m);\n    default void ping() { send(\"ping\"); }\n}\n\
class Raw { native int peek(); }\n";
    let facts = extract_lang("java", "src/main/java/com/x/Shape.java", src);
    assert!(!has_body(&facts, "area"), "an abstract method has no body");
    assert!(!has_body(&facts, "send"), "an interface method with no default has no body");
    assert!(!has_body(&facts, "peek"), "a native method has no body");
    assert!(has_body(&facts, "label"), "an implemented method has a body");
    assert!(has_body(&facts, "ping"), "a default interface method has a body");
}

#[test]
#[cfg(feature = "lang-kotlin")]
fn kotlin_abstract_members_record_no_body_and_implemented_ones_do() {
    let src = "package com.x\n\n\
abstract class Shape {\n    abstract fun area(): Double\n    fun label(): String { return \"shape\" }\n    fun short() = \"s\"\n}\n\n\
interface Port {\n    fun send(m: String)\n    fun ping() { send(\"ping\") }\n}\n";
    let facts = extract_lang("kt", "src/main/kotlin/com/x/Shape.kt", src);
    assert!(!has_body(&facts, "area"), "an abstract fun has no body");
    assert!(!has_body(&facts, "send"), "an interface fun with no default has no body");
    assert!(has_body(&facts, "label"), "a block-bodied fun has a body");
    assert!(has_body(&facts, "short"), "an expression-bodied fun has a body");
    assert!(has_body(&facts, "ping"), "an interface fun with a default has a body");
    // No `body` field: the count is the matched `function_body` (`= "s"` → 4),
    // never the whole declaration.
    let short = callable_bodies(&facts).into_iter().find(|(n, _, _)| n == "short").unwrap();
    assert_eq!(short.2, 4);
}

#[test]
#[cfg(feature = "lang-c-sharp")]
fn csharp_abstract_members_record_no_body_and_implemented_ones_do() {
    let src = "namespace X {\n\
public abstract class Shape {\n    public abstract double Area();\n    public string Label() { return \"shape\"; }\n    public string Short() => \"s\";\n}\n\
public interface IPort { void Send(string m); }\n}\n";
    let facts = extract_lang("cs", "src/Shape.cs", src);
    assert!(!has_body(&facts, "Area"), "an abstract method has no body");
    assert!(!has_body(&facts, "Send"), "an interface method with no default has no body");
    assert!(has_body(&facts, "Label"), "a block-bodied method has a body");
    assert!(has_body(&facts, "Short"), "an expression-bodied method has a body");
}

#[test]
#[cfg(feature = "lang-cpp")]
fn cpp_pure_virtuals_and_prototypes_record_no_body_and_definitions_do() {
    let src = "class Shape {\npublic:\n    virtual double area() const = 0;\n    virtual const char* label() const { return \"shape\"; }\n    void declared();\n};\n\
int proto(int x);\n\
int helper(int x) { return x + 1; }\n\
int guarded(int x) try { return x; } catch (...) { return 0; }\n";
    let facts = extract_lang("cpp", "src/shape.cpp", src);
    assert!(!has_body(&facts, "area"), "a pure-virtual member has no body");
    assert!(!has_body(&facts, "declared"), "an in-class prototype has no body");
    assert!(!has_body(&facts, "proto"), "a free prototype has no body");
    assert!(has_body(&facts, "label"), "an in-class definition has a body");
    assert!(has_body(&facts, "helper"), "a free definition has a body");
    assert!(has_body(&facts, "guarded"), "a function-try-block is a body");
}

/// TypeScript's bodyless callables are separate node kinds — an overload is a
/// `function_signature`, an abstract or interface member a
/// `(abstract_)method_signature` — that the symbols query does not capture, so
/// none reaches the graph as a callable at all. What does reach it — the
/// overload set's implementation, a method, an arrow or function-expression
/// binding — records a body.
#[test]
#[cfg(feature = "lang-typescript")]
fn typescript_overload_signatures_record_no_bodied_callable_and_implementations_do() {
    let src = "export function parse(input: string): number;\n\
export function parse(input: number): number;\n\
export function parse(input: string | number): number {\n  return Number(input);\n}\n\
export const twice = (n: number) => n * 2;\n\
export const named = function (n: number) { return n; };\n\
export abstract class Shape {\n  abstract area(): number;\n  label(): string { return \"shape\"; }\n}\n\
export interface Port { send(m: string): void; }\n";
    for (ext, path) in [("ts", "src/shape.ts"), ("tsx", "src/shape.tsx")] {
        let facts = extract_lang(ext, path, src);
        assert!(has_body(&facts, "parse"), "{ext}: the one `parse` callable is the implementation");
        assert!(has_body(&facts, "twice"), "{ext}: an expression-bodied arrow has a body");
        assert!(has_body(&facts, "named"), "{ext}: a function expression has a body");
        assert!(has_body(&facts, "label"), "{ext}: an implemented method has a body");
        let names: Vec<String> = callable_bodies(&facts).into_iter().map(|(n, _, _)| n).collect();
        assert!(
            !names.iter().any(|n| n == "area" || n == "send"),
            "{ext}: abstract and interface signatures are not callables: {names:?}"
        );
    }
}

#[test]
#[cfg(feature = "lang-scala")]
fn scala_abstract_defs_record_no_body_and_defined_ones_do() {
    let src = "package x\n\n\
abstract class Shape {\n  def area: Double\n  def label: String = \"shape\"\n  def block(): Int = { 1 + 2 }\n}\n\n\
trait Port { def send(m: String): Unit }\n";
    let facts = extract_lang("scala", "src/main/scala/x/Shape.scala", src);
    assert!(!has_body(&facts, "area"), "an abstract def has no body");
    assert!(!has_body(&facts, "send"), "a trait def with no default has no body");
    assert!(has_body(&facts, "label"), "an expression-bodied def has a body");
    assert!(has_body(&facts, "block"), "a block-bodied def has a body");
    // The declaration's own kind matched, so the count is its `body` field
    // alone, never the `def` signature: `"shape"` → 1, `{ 1 + 2 }` → 5.
    let tokens = |name: &str| callable_bodies(&facts).into_iter().find(|(n, _, _)| n == name).unwrap().2;
    assert_eq!((tokens("label"), tokens("block")), (1, 5));
}

#[test]
#[cfg(feature = "lang-php")]
fn php_abstract_and_interface_methods_record_no_body_and_implemented_ones_do() {
    let src = "<?php\n\
abstract class Shape {\n    abstract public function area(): float;\n    public function label(): string { return \"shape\"; }\n}\n\
interface Port { public function send(string $m): void; }\n\
function helper(int $x): int { return $x + 1; }\n";
    let facts = extract_lang("php", "src/Shape.php", src);
    assert!(!has_body(&facts, "area"), "an abstract method has no body");
    assert!(!has_body(&facts, "send"), "an interface method has no body");
    assert!(has_body(&facts, "label"), "an implemented method has a body");
    assert!(has_body(&facts, "helper"), "a function has a body");
}

/// A language declaring no body node kind treats every callable as bodied —
/// a Python `@abstractmethod` stub included.
#[test]
fn every_callable_is_bodied_in_a_language_declaring_no_body_kind() {
    for (ext, path, src) in NO_BODY_KIND_FIXTURES {
        let facts = extract_lang(ext, path, src);
        let bodies = callable_bodies(&facts);
        assert!(bodies.len() >= 2, "{path}: the fixture yields callables: {bodies:?}");
        assert!(
            bodies.iter().all(|(_, has_body, _)| *has_body),
            "{path}: every callable is bodied: {bodies:?}"
        );
    }
}

/// The token count is the body's normalized stream — the one the near-clone
/// shingles k-gram — and `0` for a declaration with no body.
#[test]
fn the_body_token_count_is_the_normalized_body_stream_and_zero_without_one() {
    // `{ 1 + 2 }` → `{`, literal, `+`, literal, `}`: five tokens, the signature
    // never counted.
    let facts = extract_src("src/lib.rs", "fn small() { 1 + 2 }\n");
    assert_eq!(callable_bodies(&facts), vec![("small".to_string(), true, 5)]);
    // A renamed twin with different literals normalizes to the same count.
    let twin = extract_src("src/lib.rs", "pub fn other_name(x: u32) -> u32 { 7 + 9 }\n");
    assert_eq!(callable_bodies(&twin)[0].2, 5);
    #[cfg(feature = "lang-java")]
    {
        let java = extract_lang(
            "java",
            "src/main/java/com/x/A.java",
            "package com.x;\nabstract class A {\n    abstract int f();\n    int g() { return 1 + 2; }\n}\n",
        );
        let bodies = callable_bodies(&java);
        assert_eq!(bodies[0], ("f".to_string(), false, 0), "no body, no tokens");
        // `{ return 1 + 2 ; }` → 7 tokens.
        assert_eq!(bodies[1], ("g".to_string(), true, 7));
    }
}

/// A TypeScript arrow or function-expression binding counts its **body**, never
/// its parameters, type annotations or `=>`: four callables with one body and
/// one signature record one count, whatever their declaration form.
#[test]
#[cfg(feature = "lang-typescript")]
fn a_typescript_callable_counts_its_body_tokens_whatever_its_declaration_form() {
    let src = "export function g(a: number, b: number) { return a; }\n\
export const f = (a: number, b: number) => { return a; };\n\
export const h = function (a: number, b: number) { return a; };\n\
export class C { m(a: number, b: number) { return a; } }\n";
    let bodies = callable_bodies(&extract_lang("ts", "src/forms.ts", src));
    // `{ return a ; }` → five tokens for every form.
    let counts: Vec<(&str, u32)> = bodies.iter().map(|(n, _, t)| (n.as_str(), *t)).collect();
    assert_eq!(counts, [("g", 5), ("f", 5), ("h", 5), ("m", 5)]);
    // An expression-bodied arrow counts its expression alone: `n * 2` → 3.
    let twice = callable_bodies(&extract_lang("ts", "src/twice.ts", "export const twice = (n: number) => n * 2;\n"));
    assert_eq!(twice, [("twice".to_string(), true, 3)]);
}

/// NFR-RA-06: extracting an unchanged file again yields the identical fact.
#[test]
#[cfg(feature = "lang-java")]
fn re_extracting_an_unchanged_file_yields_an_identical_has_body_fact() {
    let src = "package com.x;\npublic abstract class Shape {\n    public abstract double area();\n    public String label() { return \"shape\"; }\n}\n";
    let first = callable_bodies(&extract_lang("java", "src/main/java/com/x/Shape.java", src));
    let second = callable_bodies(&extract_lang("java", "src/main/java/com/x/Shape.java", src));
    assert_eq!(first, second);
    assert_eq!(first.len(), 2);
}

/// FR-EX-30 fixture, shaped on ccache's `util/string.cpp` (written fresh, not
/// copied): a `TRY_ASSIGN(auto x, …)` macro statement and a later `a < b ? … : …`
/// make tree-sitter-cpp recover the whole file into one ERROR node, with
/// `parse_umask`'s `function_declarator` (line 18) a direct child of it and two
/// whole function definitions beside it; `trim`'s declarator sits in a nested
/// ERROR. Before S-578 the declarator lift
/// climbed into that ERROR, so `parse_umask` spanned the file and its complexity
/// counted every branch in it (ccache: lines 1–627, CC 117).
#[cfg(feature = "lang-cpp")]
const STRANDED_DECLARATOR_CPP: &str = "\
int
clamp_level(int level, int low, int high)
{
  if (level < low || level > high) {
    return low;
  } else {
    return level;
  }
}

int
count_digits(const std::string& text)
{
  return text.size();
}

tl::expected<mode_t, std::string>
parse_umask(std::string_view value)
{
  TRY_ASSIGN(auto mode, parse_unsigned(value, 0, 0777, \"umask\", 8));
  return static_cast<mode_t>(mode);
}

std::string_view
trim(const std::string_view text)
{
  const auto first = std::find_if_not(text.begin(), text.end(), is_space);
  const auto last = std::find_if_not(text.rbegin(), text.rend(), is_space).base();
  return first < last ? text.substr(first - text.begin(), last - first)
                      : std::string_view{};
}
";

/// FR-EX-30 fixture, shaped on nlohmann/json's `detail/exceptions.hpp` (written
/// fresh): the namespace-opening macro turns the namespace into a function body,
/// and the `LIB_NON_NULL(3)` macro before the constructor makes recovery tear
/// `class exception` apart — `class`, its name and its base clause sit loose in
/// an ERROR node (lines 10–21), and no `class_specifier` exists to capture.
#[cfg(feature = "lang-cpp")]
const STRANDED_CLASS_HEAD_HPP: &str = "\
#pragma once

#include <exception>
#include <string>

LIB_NAMESPACE_BEGIN
namespace detail
{

class exception : public std::exception
{
  public:
    const char* what() const noexcept override
    {
        return m.what();
    }

    const int id;

  protected:
    LIB_NON_NULL(3)
    exception(int id_, const char* what_arg) : id(id_), m(what_arg) {}

  private:
    std::runtime_error m;
};

struct position : base_position
{
    int line;
};

}  // namespace detail
LIB_NAMESPACE_END
";

/// The one node named `name` of `kind`.
fn node_of_kind<'a>(facts: &'a Facts, name: &str, kind: NodeKind) -> &'a NodeFact {
    let hits: Vec<&NodeFact> =
        facts.nodes.iter().filter(|n| n.name == name && n.kind == kind).collect();
    assert_eq!(hits.len(), 1, "exactly one {kind:?} named {name}: {:?}", facts.nodes);
    hits[0]
}

/// The file's partial-extraction warning (FR-IX-04).
fn partial_warning(facts: &Facts) -> &str {
    facts
        .warnings
        .iter()
        .find(|w| w.contains("partial extraction"))
        .unwrap_or_else(|| panic!("a partial-extraction warning: {:?}", facts.warnings))
}

#[test]
#[cfg(feature = "lang-cpp")]
fn cpp_a_declarator_stranded_in_a_parse_error_keeps_its_own_span() {
    // FR-EX-30 / S-578: the lift stops below the ERROR, so `parse_umask` is its
    // own one-line declarator — not the whole file at the file's complexity.
    let facts = extract_lang("cpp", "src/util/string.cpp", STRANDED_DECLARATOR_CPP);
    assert!(facts.partial, "the fixture parses with an error");
    let umask = node_of_kind(&facts, "parse_umask", NodeKind::Function);
    assert_eq!(
        (umask.start_line, umask.end_line),
        (18, 18),
        "parse_umask spans its own declarator, not the ERROR region around it"
    );
    let m = umask.metrics.expect("a callable carries metrics");
    assert_eq!(m.cyclomatic_complexity, 1, "no branch of the region is counted");
    assert!(!m.has_body, "the body error recovery tore off is not claimed");

    // `trim`'s declarator sits in a nested ERROR (lines 20–27) that used to be
    // its span; now it is the declarator's own lines.
    let trim = node_of_kind(&facts, "trim", NodeKind::Function);
    assert_eq!((trim.start_line, trim.end_line), (25, 27));

    // The whole definitions beside it inside the ERROR are untouched.
    let clamp = node_of_kind(&facts, "clamp_level", NodeKind::Function);
    assert_eq!((clamp.start_line, clamp.end_line), (1, 9));
    assert_eq!(clamp.metrics.expect("metrics").cyclomatic_complexity, 4);
    assert!(clamp.metrics.expect("metrics").has_body);
}

#[test]
#[cfg(feature = "lang-cpp")]
fn cpp_a_class_head_stranded_in_a_parse_error_is_a_class_on_its_own_line() {
    // FR-EX-30 / S-578: json's `class exception` vanished because recovery left
    // no `class_specifier`; its head is captured, and the node is the name itself.
    let facts = extract_lang("cpp", "include/detail/exceptions.hpp", STRANDED_CLASS_HEAD_HPP);
    assert!(facts.partial, "the fixture parses with an error");
    let class = node_of_kind(&facts, "exception", NodeKind::Class);
    assert_eq!((class.start_line, class.end_line), (10, 10), "the head line only");

    // The class claims no body: the member recovery placed inside the ERROR is
    // not nested under it, because where the class ends is unknown.
    let what = node_of_kind(&facts, "what", NodeKind::Method);
    assert!(
        !what.symbol.as_str().starts_with(class.symbol.as_str()),
        "{} is not a member of {}",
        what.symbol.as_str(),
        class.symbol.as_str()
    );
    // A well-formed struct after the damage is extracted as before.
    let position = node_of_kind(&facts, "position", NodeKind::Struct);
    assert_eq!((position.start_line, position.end_line), (28, 31));

    // `final` sits between the name and the base clause and still reads as a head.
    let fin = STRANDED_CLASS_HEAD_HPP.replace("class exception :", "class exception final :");
    let facts = extract_lang("cpp", "include/detail/exceptions.hpp", &fin);
    assert_eq!(node_of_kind(&facts, "exception", NodeKind::Class).start_line, 10);

    // A `struct` head stranded the same way is a Struct on its own line.
    let st = STRANDED_CLASS_HEAD_HPP.replace("class exception :", "struct exception :");
    let facts = extract_lang("cpp", "include/detail/exceptions.hpp", &st);
    let head = node_of_kind(&facts, "exception", NodeKind::Struct);
    assert_eq!((head.start_line, head.end_line), (10, 10));
}

#[test]
#[cfg(all(feature = "lang-cpp", feature = "lang-c"))]
fn parse_damage_is_counted_into_the_partial_extraction_warning() {
    // FR-EX-30 + CR-168 §4.1(2): each declaration a parse error cost the file is
    // counted in its one partial-extraction warning — truncated to its own node
    // (S-578), or skipped for naming nothing (S-512).
    // `parse_umask` and `trim` (whose declarator sits in a nested ERROR).
    let umask = extract_lang("cpp", "src/util/string.cpp", STRANDED_DECLARATOR_CPP);
    assert_eq!(
        partial_warning(&umask),
        "syntax error(s) present; partial extraction; \
         2 declaration(s) truncated and 0 skipped at a parse error"
    );
    let head = extract_lang("cpp", "include/detail/exceptions.hpp", STRANDED_CLASS_HEAD_HPP);
    assert_eq!(
        partial_warning(&head),
        "syntax error(s) present; partial extraction; \
         1 declaration(s) truncated and 0 skipped at a parse error"
    );
    let nameless = extract_lang(
        "c",
        "src/anon.c",
        "typedef struct { int a; } ;\nint after(void) { return 0; }\n",
    );
    assert_eq!(
        partial_warning(&nameless),
        "syntax error(s) present; partial extraction; \
         0 declaration(s) truncated and 1 skipped at a parse error"
    );
    assert_eq!(
        umask.warnings.iter().filter(|w| w.contains("partial extraction")).count(),
        1,
        "the count joins the one warning, never a second one"
    );
}

#[test]
#[cfg(feature = "lang-java")]
fn a_damaged_rust_or_java_file_that_loses_no_declaration_keeps_its_warning() {
    // S-578 byte-identity: neither grammar climbs declarators nor captures in an
    // ERROR region, so a syntax error costs no declaration and the warning text
    // is the pre-S-578 one, exactly.
    let rust = extract_src("src/lib.rs", "fn ok() {}\nfn broken( {\n");
    assert_eq!(rust.warnings, vec!["syntax error(s) present; partial extraction"]);
    assert!(rust.nodes.iter().any(|n| n.name == "ok"));
    let java = extract_lang(
        "java",
        "src/main/java/com/x/A.java",
        "package com.x;\nclass A { void ok() {} void broken( { }\n",
    );
    assert_eq!(java.warnings, vec!["syntax error(s) present; partial extraction"]);
    assert!(java.nodes.iter().any(|n| n.name == "ok"));
}

#[test]
#[cfg(feature = "lang-cpp")]
fn cpp_a_cut_climb_keeps_the_names_own_declarator_not_an_outer_wrapper() {
    // FR-EX-30 / S-578 review: two `DECORATE(…)` macro statements make recovery
    // nest `classify`'s declarator (line 6) inside wrappers whose ERROR children
    // hold the torn `{ if … else if` body (6–8, 6–10), all under an ERROR. The
    // outermost wrapper reached before the ERROR is not the declaration's own
    // node: it ends mid-body and its complexity counts the body's branches.
    let src = "namespace detail {\n\
\n\
DECORATE(test_suite, const char*, \"\");\n\
DECORATE(description, const char*, \"\");\n\
\n\
int classify(int x)\n\
{\n\
    if (x > 10) {\n\
        return 2;\n\
    } else if (x > 5) {\n\
        return 1;\n\
    }\n\
    for (int i = 0; i < x; ++i) {\n\
        if (i == 3 && x == 4) {\n\
            return 3;\n\
        }\n\
    }\n\
    return 0;\n\
}\n\
\n\
} // namespace detail\n";
    let facts = extract_lang("cpp", "include/detail/decorators.hpp", src);
    assert!(facts.partial, "the fixture parses with an error");
    let classify = node_of_kind(&facts, "classify", NodeKind::Function);
    assert_eq!((classify.start_line, classify.end_line), (6, 6), "its own declarator line");
    assert_eq!(
        classify.metrics.expect("metrics").cyclomatic_complexity,
        1,
        "no branch of the torn body is counted"
    );
}
