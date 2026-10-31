//! Kotlin declarations are kinded and complete, and a misparse is never silent
//! (S-559, CR-178, [FR-EX-24]): a member `fun` is a Method, an `enum class` an
//! Enum, a `typealias` a TypeAlias, a `companion object` a node that holds its
//! members, and a primary-constructor `val`/`var` parameter a Field — and a
//! file the grammar mis-parses carries a partial-extraction warning.
//!
//! Capture-level checks run one in-memory file through the embedded Kotlin
//! plugin; the misparse and Focus checks go through the public [`Engine`].
//! The identity of a re-kinded `fun` is pinned by its exact symbol: Function
//! and Method render one descriptor, so no `scip_id` moves ([NFR-RA-06]).
//!
//! [FR-EX-24]: ../../docs/specs/requirements/FR-EX-24.md
//! [NFR-RA-06]: ../../docs/specs/requirements/NFR-RA-06.md
#![cfg(feature = "lang-kotlin")]

use std::fs;
use std::path::Path;

use logos_core::extract::{extract, Facts, FileInput, SymbolContext};
use logos_core::model::NodeKind;
use logos_core::plugin::LanguageRegistry;
use logos_core::Engine;

#[path = "support/graph_fingerprint.rs"]
mod graph_fingerprint;
use graph_fingerprint::graph_fingerprint;

#[path = "support/coordinate.rs"]
mod coordinate;

fn extract_kt(source: &str) -> Facts {
    extract_as("a.kt", source)
}

fn extract_as(path: &str, source: &str) -> Facts {
    let tmp = tempfile::tempdir().expect("tempdir");
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
    let plugin = reg.for_extension(path.rsplit('.').next().unwrap()).expect("kotlin grammar");
    extract(&FileInput::new(path, source), plugin, &SymbolContext::default())
}

/// `name:kind` of every node but the file's module, in source order.
fn declared(facts: &Facts) -> Vec<String> {
    facts
        .nodes
        .iter()
        .filter(|n| n.kind != NodeKind::Module)
        .map(|n| format!("{}:{}", n.name, n.kind.as_str()))
        .collect()
}

/// The symbol of the one node named `name`, minus the `logos . . .` prefix.
fn symbol_of(facts: &Facts, name: &str) -> String {
    let mut found = facts.nodes.iter().filter(|n| n.name == name);
    let node = found.next().unwrap_or_else(|| panic!("no node {name}: {:?}", declared(facts)));
    assert!(found.next().is_none(), "two nodes named {name}");
    node.symbol.as_str().trim_start_matches("logos . . . ").to_string()
}

fn strings(of: &[&str]) -> Vec<String> {
    of.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_member_fun_is_a_method_and_a_top_level_or_local_fun_is_a_function() {
    let facts = extract_kt(
        "class C {\n    fun m() {\n        fun local() {}\n    }\n}\n\
         interface I {\n    fun a()\n    fun b() = 1\n}\n\
         object O {\n    fun o() {}\n}\n\
         fun top() {}\n\
         fun String.ext() = 1\n",
    );
    assert_eq!(
        declared(&facts),
        strings(&[
            "C:class",
            "m:method",
            "local:function",
            "I:interface",
            "a:method",
            "b:method",
            "O:class",
            "o:method",
            "top:function",
            "ext:function",
        ])
    );
}

#[test]
fn a_fun_of_an_object_expression_or_an_enum_entry_stays_a_function() {
    // Neither body belongs to a declared type, so neither is a member that a
    // container counts: they were Functions and stay Functions.
    let facts = extract_kt(
        "interface I {\n    fun f()\n}\n\
         val top = object : I {\n    override fun f() {}\n}\n\
         class C {\n    val cb = object : I {\n        override fun f() {}\n    }\n    fun m() {\n        val o = object : I { override fun f() {} }\n    }\n}\n\
         enum class E {\n    A {\n        override fun g() {}\n    },\n    B;\n    open fun g() {}\n}\n",
    );
    let kinds: Vec<(String, String)> = facts
        .nodes
        .iter()
        .filter(|n| matches!(n.name.as_str(), "f" | "g" | "m"))
        .map(|n| (n.symbol.as_str().trim_start_matches("logos . . . ").to_string(), n.kind.as_str().to_string()))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("`a.kt`/I#f().".to_string(), "method".to_string()),
            ("`a.kt`/f().".to_string(), "function".to_string()),
            ("`a.kt`/C#f().".to_string(), "function".to_string()),
            ("`a.kt`/C#m().".to_string(), "method".to_string()),
            ("`a.kt`/C#m().f().".to_string(), "function".to_string()),
            ("`a.kt`/E#g().".to_string(), "function".to_string()),
            ("`a.kt`/E#g(1).".to_string(), "method".to_string()),
        ]
    );
}

#[test]
fn a_re_kinded_funs_symbol_is_the_one_it_had_as_a_function() {
    // `Svc#run().` and `Svc#run(1).` — the method descriptor and its ordinal —
    // are what a member `fun` rendered while it was a Function.
    let facts = extract_kt("class Svc {\n    fun run() {}\n    fun run(a: Int) {}\n}\nfun top() {}\n");
    let runs: Vec<String> = facts
        .nodes
        .iter()
        .filter(|n| n.name == "run")
        .map(|n| n.symbol.as_str().trim_start_matches("logos . . . ").to_string())
        .collect();
    assert_eq!(runs, strings(&["`a.kt`/Svc#run().", "`a.kt`/Svc#run(1)."]));
    assert_eq!(symbol_of(&facts, "top"), "`a.kt`/top().");
}

#[test]
fn an_enum_class_is_an_enum_with_or_without_a_body_and_a_class_naming_enum_is_not() {
    let facts = extract_kt(
        "enum class E {\n    A, B;\n    fun f() = 1\n}\n\
         enum class F\n\
         private enum class G(val v: Int) { X(1) }\n\
         @Suppress(\"enum\") class NotAnEnum\n\
         @Enumerated class AlsoNot\n\
         class Plain\n\
         interface I\n",
    );
    assert_eq!(
        declared(&facts),
        strings(&[
            "E:enum",
            "f:method",
            "F:enum",
            "G:enum",
            "v:field",
            "NotAnEnum:class",
            "AlsoNot:class",
            "Plain:class",
            "I:interface",
        ])
    );
    // An Enum and a Class render one type descriptor.
    assert_eq!(symbol_of(&facts, "E"), "`a.kt`/E#");
    assert_eq!(symbol_of(&facts, "f"), "`a.kt`/E#f().");
}

#[test]
fn a_typealias_is_a_type_alias() {
    let facts = extract_kt("package p\n\ntypealias A = Int\nprivate typealias B<T> = List<T>\ntypealias C = () -> Unit\nclass K\n");
    assert_eq!(
        declared(&facts),
        strings(&["A:type_alias", "B:type_alias", "C:type_alias", "K:class"])
    );
    assert_eq!(symbol_of(&facts, "B"), "`a.kt`/B#");
}

#[test]
fn a_companion_object_is_a_node_that_holds_its_members() {
    let facts = extract_kt(
        "class C {\n    fun own() {}\n    companion object {\n        fun make() = 1\n        const val K = 2\n    }\n}\n\
         class D {\n    companion object Named {\n        fun build() = 1\n    }\n}\n\
         interface I {\n    companion object {\n        fun of() = 1\n    }\n}\n",
    );
    assert_eq!(
        declared(&facts),
        strings(&[
            "C:class",
            "own:method",
            "Companion:class",
            "make:method",
            "K:field",
            "D:class",
            "Named:class",
            "build:method",
            "I:interface",
            "Companion:class",
            "of:method",
        ])
    );
    let symbols: Vec<String> = facts
        .nodes
        .iter()
        .filter(|n| matches!(n.name.as_str(), "own" | "make" | "K" | "build" | "of"))
        .map(|n| n.symbol.as_str().trim_start_matches("logos . . . ").to_string())
        .collect();
    assert_eq!(
        symbols,
        strings(&[
            "`a.kt`/C#own().",
            "`a.kt`/C#Companion#make().",
            "`a.kt`/C#Companion#K.",
            "`a.kt`/D#Named#build().",
            "`a.kt`/I#Companion#of().",
        ])
    );
}

#[test]
fn a_primary_constructor_val_or_var_is_a_field_and_a_plain_parameter_is_not() {
    let facts = extract_kt(
        "data class Talk(\n    val id: Int,\n    var title: String,\n    plain: Int,\n    private val secret: Int = DEFAULT,\n    @Json val tagged: Int,\n    val fallback: Int = 1,\n) {\n    val body = 1\n    constructor(x: String) : this(1)\n}\n\
         class Svc(private val repo: Repo, other: Int)\n\
         fun free(val_like: Int) {}\n",
    );
    assert_eq!(
        declared(&facts),
        strings(&[
            "Talk:class",
            "id:field",
            "title:field",
            "secret:field",
            "tagged:field",
            "fallback:field",
            "body:field",
            "Svc:class",
            "repo:field",
            "free:function",
        ])
    );
    // A Field of its class, not of the constructor.
    assert_eq!(symbol_of(&facts, "secret"), "`a.kt`/Talk#secret.");
}

// The fixtures below pin defects of `tree-sitter-kotlin-ng` 1.1. If one fails
// after a grammar bump, the defect is fixed: drop its fixture and re-measure.

/// The five one-line-class-body fixtures of the 2026-10-03 inspection
/// (KOTLIN-G13, `tree-sitter-kotlin-ng` 1.1): a class body closed on the line
/// its last member ends on, then a class after it.
const MISPARSED: [(&str, &str); 5] = [
    ("V1", "open class Base { fun inherited(): Int = 1 }\nclass B1 { fun b() {} }\n"),
    ("V3", "interface Worker3 { fun work3(): String }\nclass B3 { fun b() {} }\n"),
    ("V10", "class A10 { fun f10(): Int { return 1 } }\nclass B10 { fun b() {} }\n"),
    ("V11", "class A11 { fun f11() = 1 }\nclass B11 { fun b() {} }\n"),
    ("V12", "object A12 { fun f12() = 1 }\nclass B12 { fun b() {} }\n"),
];

#[test]
fn a_one_line_class_body_misparse_carries_a_partial_extraction_warning() {
    for (id, source) in MISPARSED {
        let facts = extract_kt(source);
        assert!(facts.partial, "{id}: the grammar loses declarations, so the file is partial");
        assert!(
            facts.warnings.iter().any(|w| w.starts_with("syntax error(s) present; partial extraction")),
            "{id}: {:?}",
            facts.warnings
        );
    }
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

#[test]
fn an_indexed_misparse_reaches_the_index_warnings_and_a_clean_file_adds_none() {
    let tmp = tempfile::tempdir().unwrap();
    for (id, source) in MISPARSED {
        write(tmp.path(), &format!("src/{id}.kt"), source);
    }
    write(tmp.path(), "src/Misread.kt", MISREAD_CLASS);
    write(tmp.path(), "src/Clean.kt", "class Clean {\n    fun ok() {}\n}\n");
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.index();
    for (id, _) in MISPARSED {
        let path = format!("src/{id}.kt");
        assert!(
            result.warnings.iter().any(|w| w.starts_with(&format!("{path}: syntax error(s) present; partial extraction"))),
            "{path} is reported: {:?}",
            result.warnings
        );
    }
    assert!(
        result.warnings.iter().any(|w| w.starts_with("src/Misread.kt: partial extraction (no syntax error reported)")),
        "a misread with no parse error is reported too: {:?}",
        result.warnings
    );
    assert!(
        !result.warnings.iter().any(|w| w.contains("Clean.kt")),
        "a file that parses cleanly is not reported: {:?}",
        result.warnings
    );
}

/// What a misread annotated declaration adds to a file's warning.
const MISREAD_CLAUSE: &str = "; 1 annotated declaration(s) read as an expression (a declaration or its annotations are lost)";
/// The grammar's other silent loss (frameworks.scm): a type declaration with
/// two annotations and a declaration after it is read as an expression, and
/// the parse reports no error.
const MISREAD_CLASS: &str = "@RestController\n@RequestMapping(\"/v1\")\nclass C {\n    fun f() {}\n}\ninterface S {\n    fun all(): String\n}\n";

#[test]
fn a_multiply_annotated_type_read_as_an_expression_carries_a_partial_extraction_warning() {
    let facts = extract_kt(MISREAD_CLASS);
    assert!(
        !declared(&facts).iter().any(|d| d.starts_with("C:")),
        "the grammar reads the class as an expression, so none is declared: {:?}",
        declared(&facts)
    );
    assert!(facts.partial);
    // Partial, but no parse damage (S-571, FR-IX-14): the parse held no ERROR
    // or MISSING node.
    assert!(!facts.parse_damaged);
    assert_eq!(
        facts.warnings,
        strings(&[&format!("partial extraction (no syntax error reported){MISREAD_CLAUSE}")])
    );
    // An annotation with arguments is read apart from a declaration that
    // survives: its annotations are what the file loses.
    let detached = extract_kt("@A\n@B(1)\nclass C\nclass Z\n");
    assert_eq!(declared(&detached), strings(&["C:class", "Z:class"]));
    assert_eq!(detached.warnings.len(), 1, "{:?}", detached.warnings);
    assert!(detached.warnings[0].ends_with(MISREAD_CLAUSE));
    // An annotated `interface`, `enum class` and `annotation class` are read alike.
    for header in [
        "@A\n@B\ninterface I { fun f() }\n",
        "@A\n@B\nenum class E { X }\n",
        "@A\n@B\nannotation class Q\n",
        "@A\n@B\nopen class Base {\n    fun f() {}\n}\n",
        "@A\nexpect object Tools {\n    fun f(): Int\n}\n",
    ] {
        let facts = extract_kt(&format!("{header}class Z\n"));
        assert_eq!(facts.warnings.len(), 1, "{header:?}: {:?}", facts.warnings);
        assert!(facts.warnings[0].ends_with(MISREAD_CLAUSE), "{header:?}");
    }
}

#[test]
fn a_declaration_the_grammar_reads_whole_carries_no_misread_warning() {
    for (what, source) in [
        ("one annotation", "@RequestMapping(\"/v1\")\nclass C {\n    fun f() {}\n}\ninterface S\n"),
        ("two annotations and nothing after", "@A\n@B\nclass C {\n    fun f() {}\n}\n"),
        ("two annotations and a second modifier", "@A\n@B\ndata class D(val a: Int)\nclass Z\n"),
        ("two annotations on a function", "@A\n@B\nfun top() {}\nclass Z\n"),
        ("two annotations on an expression in a body", "class C {\n    fun f() {\n        @A @B g()\n    }\n    fun g() {}\n}\n"),
    ] {
        let facts = extract_kt(source);
        assert!(!facts.partial, "{what}: {:?}", facts.warnings);
        assert!(facts.warnings.is_empty(), "{what}: {:?}", facts.warnings);
    }
}

#[test]
fn a_file_with_both_misparses_carries_one_warning_naming_both() {
    let facts = extract_kt(&format!("{MISREAD_CLASS}class One {{ fun f() = 1 }}\nclass Two {{ fun g() {{}} }}\n"));
    assert_eq!(facts.warnings.len(), 1, "{:?}", facts.warnings);
    assert!(
        facts.warnings[0].starts_with("syntax error(s) present; partial extraction")
            && facts.warnings[0].ends_with(MISREAD_CLAUSE),
        "{:?}",
        facts.warnings
    );
}

/// Sync ≡ reindex ([NFR-RA-06]): re-kinding a declaration, adding a companion,
/// a `typealias` and a constructor property, and turning a file into a
/// misparse each leave the synced graph the fresh index of the final tree.
#[test]
fn sync_equals_a_full_reindex_as_declarations_change_kind() {
    let initial = [
        ("src/Types.kt", "package p\n\nclass Mode {\n    fun label() = 1\n}\n"),
        ("src/User.kt", "package p\n\nclass User {\n    fun use(m: Mode) { m.label() }\n}\n"),
        ("src/Odd.kt", "package p\n\nclass Odd {\n    fun f() {}\n}\n"),
    ];
    let edits = [
        (
            "src/Types.kt",
            "package p\n\nenum class Mode(val tag: Int) {\n    A(1);\n    fun label() = 1\n    companion object {\n        fun of() = 1\n    }\n}\ntypealias Modes = List<Mode>\n",
        ),
        ("src/Odd.kt", "package p\n\nclass Odd { fun f() = 1 }\nclass Next { fun g() {} }\n"),
    ];
    let tmp_a = tempfile::tempdir().unwrap();
    for (rel, text) in initial {
        write(tmp_a.path(), rel, text);
    }
    coordinate::pin(tmp_a.path());
    let engine_a = Engine::start(tmp_a.path()).expect("engine starts");
    engine_a.index();
    let root = tmp_a.path().canonicalize().expect("canonical root");
    let changed: Vec<_> = edits
        .iter()
        .map(|(rel, text)| {
            write(&root, rel, text);
            root.join(rel)
        })
        .collect();
    engine_a.sync(&changed);

    let tmp_b = tempfile::tempdir().unwrap();
    for (rel, text) in [initial[1]].into_iter().chain(edits) {
        write(tmp_b.path(), rel, text);
    }
    coordinate::pin(tmp_b.path());
    let engine_b = Engine::start(tmp_b.path()).expect("engine starts");
    engine_b.index();
    assert_eq!(
        graph_fingerprint(engine_a.runtime().unwrap()),
        graph_fingerprint(engine_b.runtime().unwrap())
    );
}

/// A Kotlin class counts its member `fun`s toward Focus: with `god_methods = 4`
/// a class of five is god and one of a single `fun` is not. (Cohesion stays n/a
/// until the Kotlin plugin declares `member_access`: a class is scored there
/// only where its queries capture field access.)
#[test]
fn kotlin_member_funs_count_toward_focus() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), ".logos/rules.toml", "[constraints]\nno_god_containers = true\n\n[metric_thresholds]\ngod_methods = 4\n");
    write(
        tmp.path(),
        "src/Big.kt",
        "class Big {\n    fun a() {}\n    fun b() {}\n    fun c() {}\n    fun d() {}\n    fun e() {}\n}\n",
    );
    write(tmp.path(), "src/Small.kt", "class Small {\n    fun a() {}\n}\n");
    write(tmp.path(), "src/free.kt", "fun x() {}\nfun y() {}\nfun z() {}\nfun w() {}\nfun v() {}\n");
    coordinate::pin(tmp.path());
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    let scanned = engine.scan(false).expect("scan runs");
    assert_eq!(scanned.metrics.focus.expect("containers exist").raw, 0.5, "Big and Small; Big god");
    let focus: Vec<(&str, &str)> =
        scanned.worst_offenders.focus.iter().map(|o| (o.name.as_str(), o.file.as_str())).collect();
    assert_eq!(focus, vec![("Big", "src/Big.kt")]);
}

#[test]
fn each_misread_annotated_declaration_is_counted() {
    let facts = extract_kt(
        "@A\n@B(\"x\")\nclass C1 {\n    fun f() {}\n}\ninterface S1 {\n    fun all(): String\n}\n\
         @A\n@B(\"y\")\nclass C2 {\n    fun g() {}\n}\ninterface S2 {\n    fun all(): String\n}\n",
    );
    assert_eq!(
        facts.warnings,
        strings(&["partial extraction (no syntax error reported); 2 annotated declaration(s) read as an expression (a declaration or its annotations are lost)"])
    );
}

#[test]
fn a_gradle_script_statement_is_no_misread_and_a_script_class_is() {
    // `@Suppress("x") plugins { … }` is a statement of a `.kts` script, whole.
    let build = extract_as(
        "build.gradle.kts",
        "fun f() {}\n@Suppress(\"DSL_SCOPE_VIOLATION\")\nplugins {\n    id(\"x\")\n}\n@Suppress(\"a\") @Foo bar()\n",
    );
    assert!(!build.partial && build.warnings.is_empty(), "{:?}", build.warnings);
    assert_eq!(declared(&build), strings(&["f:function"]));
    // The grammar's misread is the same in a script as in a `.kt` file.
    let script = extract_as("tools.kts", MISREAD_CLASS);
    assert_eq!(script.warnings.len(), 1, "{:?}", script.warnings);
    assert!(script.warnings[0].ends_with(MISREAD_CLAUSE));
}
