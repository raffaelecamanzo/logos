//! Every code language's declared cross-file reach is **verified**
//! ([FR-PL-09], [S-570], [CR-180]).
//!
//! `logos languages` used to list every code plugin with the same
//! `symbols`/`references` capabilities, so a language that binds nothing across
//! files (Scala) read like one that binds calls, imports and type relations
//! (Java). Each `plugin.toml` now declares a `[reach]` table; this suite indexes
//! a small fixture per language and compares what the graph actually holds with
//! what the descriptor claims, in **both** directions:
//!
//! - a declared relation must bind at least one edge across a file boundary on
//!   the fixture (an over-claim fails);
//! - an undeclared relation must bind none (an under-claim fails — a story that
//!   makes a language resolve more must raise its declaration in the same
//!   change);
//! - a `symbols` language binds no import, type, member or route relation, in
//!   or across files. Its same-file **calls** are tolerated: C and C++ bind a
//!   call to a function of the same file (7/7 on the 2026-10-03 inspection), and
//!   a descriptor that claimed otherwise would be the over-claim this suite
//!   exists to refuse.
//!
//! Each fixture is deliberately the *same set of shapes* in the language's own
//! idiom — a call into another file, an import, a subclass / implementation of a
//! type declared in another file, a field read through a typed receiver — so that
//! the `same-file` and `symbols` rows are measured against inputs that *would*
//! bind if the plugin could, not against an empty corpus. The levels are coarse
//! by design: the per-project `resolution_by_language` readout ([FR-RS-09])
//! remains the authority on a real repository.
//!
//! [FR-PL-09]: ../../docs/specs/requirements/FR-PL-09.md
//! [FR-RS-09]: ../../docs/specs/requirements/FR-RS-09.md
//! [S-570]: ../../docs/planning/journal.md#s-570-every-language-declares-its-verified-cross-file-reach-and-scala-is-declared-same-file
//! [CR-180]: ../../docs/requests/CR-180-scala-is-declared-as-limited-support-and-every-language-declares-its-reach.md

#![cfg(all(
    feature = "lang-rust",
    feature = "lang-c",
    feature = "lang-cpp",
    feature = "lang-scala",
    feature = "lang-java",
    feature = "lang-kotlin",
    feature = "lang-go",
    feature = "lang-python",
    feature = "lang-typescript",
    feature = "lang-c-sharp",
    feature = "lang-ruby",
    feature = "lang-php"
))]

#[path = "reach_declared/fixtures.rs"]
mod fixtures;

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use logos_core::model::EdgeKind;
use logos_core::models::quality::{LanguageDescriptor, LanguageReach};
use logos_core::Engine;
use rusqlite::{Connection, OpenFlags};
use tempfile::TempDir;

/// Edge kinds per relation of the `[reach]` vocabulary, spelled through
/// [`EdgeKind`] so a renumbered or mistyped kind cannot leave a relation
/// measured against nothing. `Instantiates` rides with `type_relations`: a
/// construction names the type it builds.
const CALLS: &[i64] = &[EdgeKind::Calls as i64];
const IMPORTS: &[i64] = &[EdgeKind::Imports as i64];
const TYPE_RELATIONS: &[i64] = &[
    EdgeKind::Implements as i64,
    EdgeKind::Extends as i64,
    EdgeKind::Instantiates as i64,
    EdgeKind::TypeUses as i64,
];
const MEMBER_ACCESS: &[i64] = &[EdgeKind::Accesses as i64];
const ROUTES: &[i64] = &[EdgeKind::RoutesTo as i64];

const RELATIONS: &[(&str, &[i64])] = &[
    ("calls", CALLS),
    ("imports", IMPORTS),
    ("type_relations", TYPE_RELATIONS),
    ("member_access", MEMBER_ACCESS),
    ("routes", ROUTES),
];

/// Edge kinds a `symbols` language must bind **none** of, anywhere: every
/// relational kind except lexical containment (1), which is declaration
/// structure rather than a reference, and calls (2), whose same-file binding C
/// and C++ do perform — their *cross-file* calls are covered by the declared-set
/// comparison above.
const NON_CALL_RELATIONAL_KINDS: &[i64] = &[
    EdgeKind::Imports as i64,
    EdgeKind::References as i64,
    EdgeKind::Implements as i64,
    EdgeKind::Extends as i64,
    EdgeKind::Instantiates as i64,
    EdgeKind::TypeUses as i64,
    EdgeKind::RoutesTo as i64,
    EdgeKind::Accesses as i64,
];

fn write_tree(root: &Path, files: &[(&str, &str)]) {
    for (rel, contents) in files {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
}

fn indexed(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    write_tree(tmp.path(), files);
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.index();
    assert!(result.files_indexed > 0, "the fixture indexed: {result:?}");
    tmp
}

/// Resolved edges per `(language, edge kind)`, split into same-file and
/// cross-file — the same endpoint rule `resolution_by_language` uses (both
/// endpoints in an indexed file, compared by `file_id`).
fn edge_locality(root: &Path) -> BTreeMap<(String, i64), (u64, u64)> {
    let conn = Connection::open_with_flags(
        root.join(".logos/logos.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT fs.language, e.kind, \
                    COALESCE(SUM(ns.file_id = nt.file_id), 0), \
                    COALESCE(SUM(ns.file_id <> nt.file_id), 0) \
             FROM edges e \
             JOIN nodes ns ON ns.id = e.source \
             JOIN nodes nt ON nt.id = e.target \
             JOIN files fs ON fs.id = ns.file_id \
             JOIN files ft ON ft.id = nt.file_id \
             WHERE fs.language IS NOT NULL \
             GROUP BY fs.language, e.kind",
        )
        .unwrap();
    stmt.query_map([], |r| {
        Ok((
            (r.get::<_, String>(0)?, r.get::<_, i64>(1)?),
            (r.get::<_, i64>(2)? as u64, r.get::<_, i64>(3)? as u64),
        ))
    })
    .unwrap()
    .collect::<rusqlite::Result<_>>()
    .unwrap()
}

/// Cross-file edges bound by `language`, per relation of the vocabulary.
fn cross_file_by_relation(
    locality: &BTreeMap<(String, i64), (u64, u64)>,
    language: &str,
) -> BTreeMap<&'static str, u64> {
    RELATIONS
        .iter()
        .map(|(name, kinds)| {
            let total = kinds
                .iter()
                .filter_map(|k| locality.get(&(language.to_string(), *k)))
                .map(|(_, cross)| *cross)
                .sum();
            (*name, total)
        })
        .collect()
}

/// Every non-call relational edge `language` bound, in or across files.
fn non_call_relational_edges(
    locality: &BTreeMap<(String, i64), (u64, u64)>,
    language: &str,
) -> u64 {
    NON_CALL_RELATIONAL_KINDS
        .iter()
        .filter_map(|k| locality.get(&(language.to_string(), *k)))
        .map(|(same, cross)| same + cross)
        .sum()
}

fn descriptor(name: &str) -> LanguageDescriptor {
    let tmp = TempDir::new().unwrap();
    Engine::open(tmp.path())
        .languages()
        .languages
        .into_iter()
        .find(|d| d.name == name)
        .unwrap_or_else(|| panic!("{name} is compiled in by default"))
}

/// Compare one declaration with what the fixture binds; `Some(why)` is a
/// violation. A function rather than a panic so the negative tests below can
/// prove a wrong declaration is *refused* without editing a descriptor.
fn violation(
    language: &str,
    declared: &LanguageReach,
    files: &[(&str, &str)],
) -> Option<String> {
    let tmp = indexed(files);
    let locality = edge_locality(tmp.path());
    let bound = cross_file_by_relation(&locality, language);

    for (relation, count) in &bound {
        let is_declared = declared.cross_file.iter().any(|d| d == relation);
        if is_declared != (*count > 0) {
            let verdict = if is_declared { "over-claim" } else { "under-claim" };
            return Some(format!(
                "{verdict}: {language} declares level `{}` with cross_file {:?}, but binds \
                 {count} cross-file `{relation}` edge(s) on its fixture (all bound: {bound:?}; \
                 edge locality by kind: {:?}) — update `plugins/{language}/plugin.toml` \
                 `[reach]` in the same change that alters the binding",
                declared.level,
                declared.cross_file,
                locality
                    .iter()
                    .filter(|((l, _), _)| l == language)
                    .map(|((_, k), v)| (*k, *v))
                    .collect::<Vec<_>>(),
            ));
        }
    }

    if declared.level == "symbols" {
        return symbols_violation(&locality, language);
    }
    None
}

/// The `symbols` clause on its own: a `symbols` language binds no import, type,
/// member or route relation, in or across files. A function of its own so a
/// test can reach it with a synthetic locality — through [`violation`] the
/// cross-file comparison above decides first, and for C and C++ (which bind only
/// same-file calls) nothing here would ever fire.
fn symbols_violation(
    locality: &BTreeMap<(String, i64), (u64, u64)>,
    language: &str,
) -> Option<String> {
    (non_call_relational_edges(locality, language) != 0).then(|| {
        format!(
            "{language} is declared `symbols` (no import, type, member or route relation), yet \
             binds some: {locality:?}"
        )
    })
}

/// Verify one language's declaration against what its fixture binds.
///
/// `declared` is what the **descriptor** says (read through `languages()`, the
/// surface users read), never a copy of it in this file — raising a descriptor
/// without raising the plugin is what must fail here.
fn verify(language: &str, files: &[(&str, &str)]) {
    let declared = descriptor(language)
        .reach
        .unwrap_or_else(|| panic!("{language} declares no `[reach]`"));
    if let Some(why) = violation(language, &declared, files) {
        panic!("{why}");
    }
}

fn reach(level: &str, cross_file: &[&str]) -> LanguageReach {
    LanguageReach {
        level: level.to_string(),
        cross_file: cross_file.iter().map(|r| (*r).to_string()).collect(),
    }
}

#[test]
fn rust_declared_resolved_binds_across_files() {
    verify("rust", fixtures::RUST);
}

#[test]
fn java_declared_resolved_binds_across_files() {
    verify("java", fixtures::JAVA);
}

#[test]
fn go_declared_partial_binds_across_files() {
    verify("go", fixtures::GO);
}

#[test]
fn typescript_declared_partial_binds_across_files() {
    verify("typescript", fixtures::TYPESCRIPT);
}

#[test]
fn tsx_declared_partial_binds_across_files() {
    verify("tsx", fixtures::TSX);
}

#[test]
fn python_declared_same_file_binds_nothing_across_files() {
    verify("python", fixtures::PYTHON);
}

#[test]
fn php_declared_same_file_binds_nothing_across_files() {
    verify("php", fixtures::PHP);
}

#[test]
fn csharp_declared_same_file_binds_nothing_across_files() {
    verify("c-sharp", fixtures::C_SHARP);
}

#[test]
fn kotlin_declared_partial_binds_imports_only() {
    verify("kotlin", fixtures::KOTLIN);
}

#[test]
fn ruby_declared_same_file_binds_nothing_across_files() {
    verify("ruby", fixtures::RUBY);
}

#[test]
fn scala_declared_same_file_binds_nothing_across_files() {
    verify("scala", fixtures::SCALA);
}

#[test]
fn c_declared_symbols_binds_nothing_across_files() {
    verify("c", fixtures::C);
}

#[test]
fn cpp_declared_symbols_binds_nothing_across_files() {
    verify("cpp", fixtures::CPP);
}

// ── the suite refuses a wrong declaration, in both directions ────────────────

/// The deliberate-raise proof ([S-570] AC): Scala resolves nothing across files,
/// so declaring it anything more must fail the suite.
///
/// [S-570]: ../../docs/planning/journal.md#s-570-every-language-declares-its-verified-cross-file-reach-and-scala-is-declared-same-file
#[test]
fn a_raised_scala_declaration_fails_the_suite() {
    for raised in [
        reach("partial", &["calls"]),
        reach("partial", &["imports"]),
        reach("resolved", &["calls", "imports", "type_relations"]),
    ] {
        let why = violation("scala", &raised, fixtures::SCALA)
            .unwrap_or_else(|| panic!("a Scala declaration of {raised:?} passed the suite"));
        assert!(why.contains("over-claim"), "{why}");
    }
    // …and the declaration the descriptor actually carries is the one that passes.
    assert_eq!(
        violation("scala", &reach("same-file", &[]), fixtures::SCALA),
        None
    );
}

/// The other direction: a language that binds across files cannot be
/// under-declared, so a later story that makes a language resolve more fails
/// here until it raises its descriptor in the same change.
#[test]
fn an_under_claimed_declaration_fails_the_suite() {
    let why = violation("java", &reach("partial", &["imports"]), fixtures::JAVA)
        .expect("Java binds calls and type relations across files; declaring only imports fails");
    assert!(why.contains("under-claim"), "{why}");

    // A plugin that binds across files cannot be declared `same-file` either:
    // Rust binds calls, imports and implementations across them.
    let why = violation("rust", &reach("same-file", &[]), fixtures::RUST)
        .expect("a same-file declaration is refused for a language that binds across files");
    assert!(why.contains("under-claim"), "{why}");
}

/// A `symbols` declaration is refused when the plugin binds a non-call relation
/// — even a same-file one the cross-file comparison cannot see — and tolerates
/// the same-file calls C and C++ do bind.
#[test]
fn a_symbols_declaration_is_refused_when_a_relation_binds() {
    let locality = |kind: i64, same: u64, cross: u64| {
        BTreeMap::from([(("c".to_string(), kind), (same, cross))])
    };
    // A same-file import, type relation, member access or route: refused.
    for kind in NON_CALL_RELATIONAL_KINDS.iter().copied() {
        let why = symbols_violation(&locality(kind, 1, 0), "c")
            .unwrap_or_else(|| panic!("a same-file edge of kind {kind} passed as `symbols`"));
        assert!(why.contains("declared `symbols`"), "{why}");
    }
    // Same-file calls and containment are what C and C++ really bind: tolerated.
    assert_eq!(symbols_violation(&locality(EdgeKind::Calls as i64, 3, 0), "c"), None);
    assert_eq!(symbols_violation(&locality(EdgeKind::Contains as i64, 9, 0), "c"), None);
    // Another language's edges are not this language's.
    assert_eq!(symbols_violation(&locality(EdgeKind::Imports as i64, 1, 0), "cpp"), None);

    // Through the whole check, on a real fixture: Rust binds across files, so a
    // `symbols` declaration is refused (by the cross-file comparison, first).
    assert!(violation("rust", &reach("symbols", &[]), fixtures::RUST).is_some());
}

// ── the declarations are surfaced, for code languages only ───────────────────

/// Every code language declares a reach and no other plugin does; Scala reads
/// `same-file` with an empty set ([FR-PL-09] AC).
///
/// A code language is one with a `symbols` capability — the documentation and
/// artifact plugins ship none and bind no code reference.
///
/// [FR-PL-09]: ../../docs/specs/requirements/FR-PL-09.md
#[test]
fn every_code_language_declares_a_reach_and_nothing_else_does() {
    let tmp = TempDir::new().unwrap();
    let info = Engine::open(tmp.path()).languages();
    assert!(info.load_error.is_none(), "{:?}", info.load_error);

    let mut declared: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
    for lang in &info.languages {
        let is_code = lang.capabilities.iter().any(|c| c == "symbols");
        match (&lang.reach, is_code) {
            (Some(r), true) => {
                declared.insert(lang.name.clone(), (r.level.clone(), r.cross_file.clone()));
            }
            (None, true) => panic!("code language {} declares no `[reach]`", lang.name),
            (Some(_), false) => panic!("non-code plugin {} declares a `[reach]`", lang.name),
            (None, false) => {}
        }
    }

    let level = |name: &str| declared.get(name).unwrap_or_else(|| panic!("{name}")).0.as_str();
    assert_eq!(level("scala"), "same-file");
    assert!(declared["scala"].1.is_empty(), "Scala binds no relation across files");
    for name in ["rust", "java"] {
        assert_eq!(level(name), "resolved", "{name}");
    }
    for name in ["go", "typescript", "tsx", "kotlin"] {
        assert_eq!(level(name), "partial", "{name}");
    }
    for name in ["python", "php", "c-sharp", "ruby"] {
        assert_eq!(level(name), "same-file", "{name}");
    }
    for name in ["c", "cpp"] {
        assert_eq!(level(name), "symbols", "{name}");
    }
    assert_eq!(declared.len(), 13, "thirteen code-language rows: {declared:?}");

    // Every one of them has a fixture, and the fixture set names no other.
    let mut fixture_names: Vec<&str> = fixtures::ALL.iter().map(|(n, _)| *n).collect();
    fixture_names.sort_unstable();
    let declared_names: Vec<&str> = declared.keys().map(String::as_str).collect();
    assert_eq!(fixture_names, declared_names, "a fixture per declared language");
}

/// `reach` is a plain object in the `languages` payload — `level` and
/// `cross_file` — and absent (not `null`) from a plugin that declares none.
#[test]
fn the_languages_payload_carries_reach_as_level_and_cross_file() {
    let tmp = TempDir::new().unwrap();
    let json = serde_json::to_value(Engine::open(tmp.path()).languages()).unwrap();
    let rows = json["languages"].as_array().unwrap();
    let row = |name: &str| {
        rows.iter()
            .find(|r| r["name"] == name)
            .unwrap_or_else(|| panic!("{name} row"))
    };
    assert_eq!(
        row("scala")["reach"],
        serde_json::json!({ "level": "same-file", "cross_file": [] })
    );
    assert_eq!(
        row("java")["reach"],
        serde_json::json!({ "level": "resolved", "cross_file": ["calls", "imports", "type_relations"] })
    );
    assert!(
        row("markdown").get("reach").is_none(),
        "a documentation plugin has no reach — the key is absent, not null"
    );
    assert!(row("yaml").get("reach").is_none());
}

/// A scratch dump of what each fixture binds, for authoring and reviewing a
/// declaration (`cargo test --test reach_declared -- --ignored --nocapture`).
#[test]
#[ignore = "authoring aid: prints the measured matrix, asserts nothing"]
fn print_the_measured_matrix() {
    for (language, files) in fixtures::ALL {
        let tmp = indexed(files);
        let locality = edge_locality(tmp.path());
        println!("{language}: relations {:?}", cross_file_by_relation(&locality, language));
        for ((l, k), (same, cross)) in &locality {
            if l == language {
                println!("    kind {k}: same {same} cross {cross}");
            }
        }
    }
}
