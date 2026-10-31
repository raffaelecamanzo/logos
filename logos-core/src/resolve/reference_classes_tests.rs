//! The reference classes' name test, universe and classification (S-635,
//! [FR-RS-49]), and the soundness of the name test on every code language's
//! reach fixture.
//!
//! [FR-RS-49]: ../../../docs/specs/requirements/FR-RS-49.md

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

use tempfile::TempDir;

use super::*;
use crate::graph_store::UnboundCodeTarget;
use crate::model::RefForm;
use crate::models::ReferenceFigureAbsence;
use crate::Engine;

fn universe(names: &[&str]) -> NameUniverse {
    NameUniverse::build(
        &DeclaredNameSources {
            names: names.iter().map(|n| (*n).to_string()).collect(),
            aliases: Vec::new(),
            files: Vec::new(),
        },
        &PackageLayout::default(),
    )
}

fn files(files: &[(&str, Option<&str>)], layout: &PackageLayout) -> NameUniverse {
    NameUniverse::build(
        &DeclaredNameSources {
            names: Vec::new(),
            aliases: Vec::new(),
            files: files
                .iter()
                .map(|(path, namespace)| ((*path).to_string(), namespace.map(str::to_string)))
                .collect(),
        },
        layout,
    )
}

#[test]
fn a_target_is_named_by_its_whole_or_a_suffix_after_any_separator() {
    let u = universe(&["Child", "method", "fixture.copy"]);
    for named in [
        "Child",
        "std::process::Child",
        "a.b.method",
        "pkg/sub/Child",
        "App\\Models\\Child",
        ".::fixture.copy",
    ] {
        assert!(u.could_name(named), "{named} ends on a declared name");
    }
    // Near misses: a suffix counts only where a separator starts it, and only
    // whole — never a substring, a prefix or a name with a character more.
    for nowhere in [
        "xmethod",
        "obj.methodX",
        "method::other",
        "std::process::ChildOf",
        "copy",
        "Chil",
    ] {
        assert!(!u.could_name(nowhere), "{nowhere} names nothing declared");
    }
}

#[test]
fn a_target_ending_on_a_separator_names_a_place_and_is_never_external() {
    let u = universe(&[]);
    for place in [".", "..", "./", "a::", ""] {
        assert!(u.could_name(place), "{place:?} cannot be judged by a name");
    }
    assert!(!u.could_name("serde_json::to_string"));
}

#[test]
fn the_universe_holds_path_segments_stems_namespaces_and_facades() {
    let layout = PackageLayout::default()
        .with_file_facades(HashMap::from([(".KT".to_string(), "Kt".to_string())]));
    let u = files(
        &[
            ("web/ui/src/copy/fixture.copy.ts", None),
            ("app/components/layouts/index.tsx", None),
            ("src/Helpers.cs", Some("Acme.Json.Utilities")),
            ("src/main/kotlin/org/x/ScopeJVM.kt", None),
        ],
        &layout,
    );
    let unnamed: Vec<String> = [
        (".::fixture.copy", "a file name less its last extension"),
        ("..::copy::fixture", "a file name less every extension"),
        ("fixture.copy.ts", "a file name"),
        ("@::components::layouts", "a directory"),
        ("Acme::Json::Utilities", "a declared namespace's segment"),
        ("org::x::ScopeJVMKt", "a file facade"),
    ]
    .into_iter()
    .filter(|(target, _)| !u.could_name(target))
    .map(|(target, why)| format!("{target} ({why})"))
    .collect();
    assert!(unnamed.is_empty(), "named by no source: {unnamed:?}");
    // A facade only where the file's language declares one.
    let plain = files(&[("src/main/kotlin/org/x/ScopeJVM.kt", None)], &PackageLayout::default());
    assert!(!plain.could_name("org::x::ScopeJVMKt"));
    // No empty name is ever admitted, so a target is never named by one.
    assert!(!files(&[("/a//b", None)], &layout).names.contains(""));
}

#[test]
fn an_alias_is_a_name_only_where_it_renames() {
    let u = NameUniverse::build(
        &DeclaredNameSources {
            names: Vec::new(),
            aliases: vec![
                ("_Base".to_string(), "pkg::base::Base".to_string()),
                ("HashMap".to_string(), "std::collections::HashMap".to_string()),
            ],
            files: Vec::new(),
        },
        &PackageLayout::default(),
    );
    assert!(u.could_name("_Base"), "`import Base as _Base` declares `_Base`");
    assert!(!u.could_name("Base"), "the renamed target is not declared by the alias");
    assert!(
        !u.could_name("std::collections::HashMap") && !u.could_name("HashMap"),
        "an alias that is its target's own last segment renames nothing"
    );
}

#[test]
fn an_import_is_named_by_any_segment_and_a_go_import_is_never_judged() {
    let u = universe(&["hc"]);
    let kind = |kind: EdgeKind| i64::from(kind.as_i32());
    assert!(
        u.could_be_internal(Some("python"), kind(EdgeKind::Imports), "hc::local_settings"),
        "an import binds its parent package"
    );
    assert!(
        !u.could_be_internal(Some("python"), kind(EdgeKind::Calls), "hc::local_settings"),
        "a call binds no prefix of its path"
    );
    assert!(!u.could_be_internal(Some("rust"), kind(EdgeKind::Imports), "std::collections::HashMap"));
    assert!(!u.could_be_internal(None, kind(EdgeKind::Imports), "std::collections::HashMap"));
    assert!(
        u.could_be_internal(Some("go"), kind(EdgeKind::Imports), "github.com::x::y"),
        "a Go import may bind the module root through go.mod"
    );
    assert!(!u.could_be_internal(Some("go"), kind(EdgeKind::Calls), "fmt::Println"));
    assert!(
        u.could_be_internal(Some("yaml"), kind(EdgeKind::ArtifactBinding), "POST /nothing"),
        "an artifact binding binds through its own matcher"
    );
}

#[test]
fn every_name_bound_kind_is_judged_and_no_other_kind_is() {
    // Spelled out here rather than read from `NAME_BOUND`, so dropping a kind
    // from the production list moves this test instead of the population it
    // judges.
    let judged = [
        EdgeKind::Calls,
        EdgeKind::Imports,
        EdgeKind::References,
        EdgeKind::Implements,
        EdgeKind::Extends,
        EdgeKind::Instantiates,
        EdgeKind::TypeUses,
        EdgeKind::Accesses,
    ];
    let u = universe(&["declared"]);
    let wrong: Vec<EdgeKind> = EdgeKind::ALL
        .into_iter()
        .filter(|kind| {
            let external = !u.could_be_internal(Some("c-sharp"), i64::from(kind.as_i32()), "named_nowhere");
            external != judged.contains(kind)
        })
        .collect();
    assert!(wrong.is_empty(), "kinds judged against the list: {wrong:?}");
}

fn group(language: Option<&str>, kind: EdgeKind, target: &str, rows: u64) -> UnboundCodeTarget {
    UnboundCodeTarget {
        language: language.map(str::to_string),
        kind: i64::from(kind.as_i32()),
        target: target.to_string(),
        rows,
    }
}

#[test]
fn only_name_bound_kinds_are_classified_and_each_group_counts_once() {
    let population = ReferencePopulation {
        doc_references: 4,
        doc_resolved: 3,
        code_references: 20,
        code_resolved: 5,
        unbound_code: vec![
            group(Some("rust"), EdgeKind::Calls, "iter", 3),
            group(Some("rust"), EdgeKind::Calls, "new", 2),
            group(Some("rust"), EdgeKind::Imports, "std::process::Child", 1),
            group(Some("rust"), EdgeKind::TypeUses, "Duration", 1),
            group(Some("yaml"), EdgeKind::ArtifactBinding, "POST /api/blobs/{digest}", 4),
            group(None, EdgeKind::Calls, "orphaned", 2),
        ],
        unbound_invocations: Vec::new(),
        names: DeclaredNameSources::default(),
    };
    let classes = ReferenceClasses::classify(&population, &universe(&["new"]));
    // iter 3 + Child 1 + Duration 1 + orphaned 2; `new` is named here, and an
    // artifact binding binds through its own matcher.
    assert_eq!(classes.provably_external, 7);
    assert_eq!(classes.external_of("rust"), ExternalByClass { calls: 3, imports: 1 });
    assert_eq!(classes.external_of("yaml"), ExternalByClass::default());
    assert_eq!(classes.external_of("go"), ExternalByClass::default());

    let headline = classes.internal_resolution();
    assert_eq!(
        (headline.code_references, headline.internal_candidates, headline.resolved),
        (20, 13, 5)
    );
    assert_eq!(headline.lower_bound, Some(5.0 / 13.0));
    assert_eq!(classes.doc_resolution().ratio, Some(0.75));
}

#[test]
fn the_binders_external_type_rows_add_to_the_name_test_once() {
    let u = universe(&["new", "len"]);
    let mut classes = ReferenceClasses::default();
    classes.add_external_type(&u, "rust", &BTreeMap::new());
    assert!(classes.by_language.is_empty(), "nothing added makes no row");
    // `HashMap::new` and `len` name the graph's own names, so only the binder
    // proves them external; `serde_json::to_string` the name test already
    // counted, and is not counted again.
    let rows = |n: u64| BTreeMap::from([(vec![vec!["Elsewhere".to_string()]], n)]);
    let targets = BTreeMap::from([
        ("HashMap::new".to_string(), rows(2)),
        ("len".to_string(), rows(1)),
        ("serde_json::to_string".to_string(), rows(4)),
    ]);
    classes.add_external_type(&u, "rust", &targets);
    assert_eq!(classes.provably_external, 3);
    assert_eq!(classes.external_of("rust"), ExternalByClass { calls: 3, imports: 0 });
}

#[test]
fn an_empty_denominator_reads_absent_and_never_a_ratio() {
    let none = InternalResolution::measured(0, 0, 0);
    assert_eq!(
        (none.lower_bound, none.absence),
        (None, Some(ReferenceFigureAbsence::NoReferencesRecorded))
    );
    let all_external = InternalResolution::measured(4, 0, 4);
    assert_eq!(
        (all_external.lower_bound, all_external.absence),
        (None, Some(ReferenceFigureAbsence::NoInternalCandidates { provably_external: 4 }))
    );
    let complete = InternalResolution::measured(4, 2, 2);
    assert_eq!((complete.lower_bound, complete.absence), (Some(1.0), None));
    assert_eq!(InternalResolution::default(), none);
    let docs = DocResolution::measured(0, 0);
    assert_eq!(
        (docs.ratio, docs.absence),
        (None, Some(ReferenceFigureAbsence::NoReferencesRecorded))
    );
    assert_eq!(DocResolution::default(), docs);
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// The bound, name-bound rows of `files` the name test would call external —
/// each a counterexample to its soundness — and how many bound rows it judged.
fn named_nowhere(files: &[(&str, &str)]) -> (Vec<String>, usize) {
    let tmp = TempDir::new().unwrap();
    for (rel, contents) in files {
        write(tmp.path(), rel, contents);
    }
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    let layout = PackageLayout::from_registry(engine.registry().expect("a registry"));
    engine
        .runtime()
        .unwrap()
        .submit_read(move |store| {
            let universe = NameUniverse::build(&store.reference_population()?.names, &layout);
            let paths: Vec<String> = store.indexed_files()?.into_iter().map(|f| f.path).collect();
            let by_path: HashMap<String, String> = store
                .file_languages(&paths)?
                .into_iter()
                .filter_map(|(path, language)| Some((path, language?)))
                .collect();
            let languages: HashMap<i64, String> = store
                .indexed_files()?
                .into_iter()
                .filter_map(|f| Some((f.id, by_path.get(&f.path)?.clone())))
                .collect();
            let bound: Vec<_> = store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| r.resolved && r.form != RefForm::Symbol && NAME_BOUND.contains(&r.kind))
                .collect();
            let nowhere = bound
                .iter()
                .filter(|r| {
                    let language = languages.get(&r.file_id.unwrap_or_default()).map(String::as_str);
                    !universe.could_be_internal(language, i64::from(r.kind.as_i32()), &r.target)
                })
                .map(|r| format!("{:?} {}", r.kind, r.target))
                .collect();
            Ok((nowhere, bound.len()))
        })
        .expect("read runs")
}

#[cfg(all(
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
#[path = "../../tests/reach_declared/fixtures.rs"]
mod reach;

#[cfg(all(
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
mod soundness {
    use super::*;

    /// CR-212 A1, verified per binder: a row the binder bound is never one the
    /// name test would call provably external. Measured on every code
    /// language's reach fixture — the shapes each binds across files.
    #[test]
    fn bound_rows_are_never_named_nowhere() {
        let mut judged = 0;
        for (language, fixture) in reach::ALL {
            let (nowhere, bound) = named_nowhere(fixture);
            assert!(nowhere.is_empty(), "{language}: bound rows named nowhere: {nowhere:?}");
            judged += bound;
        }
        assert!(judged > 0, "the fixtures bound rows to judge");
    }

    /// Three sources of the universe beyond node names, each on the shape that
    /// needs it: a bound row whose target only that source spells, which would
    /// read provably external without it. The file stems and facades are
    /// pinned by the universe test above instead: the two dotted
    /// file imports tried here still passed with the stems removed, so they
    /// proved nothing about them.
    #[test]
    fn each_name_source_admits_the_bound_shape_that_needs_it() {
        let shapes: &[(&str, &[(&str, &str)])] = &[
            (
                "an import alias",
                &[
                    ("pkg/__init__.py", ""),
                    ("pkg/base.py", "class Base:\n    pass\n"),
                    (
                        "pkg/child.py",
                        "from pkg.base import Base as _Base\n\n\nclass Child(_Base):\n    pass\n",
                    ),
                ],
            ),
            (
                "a declared namespace",
                &[
                    (
                        "src/Helpers.cs",
                        "namespace Acme.Json\n{\n    public class Tool\n    {\n        public void Run() {}\n    }\n}\n",
                    ),
                    (
                        "src/App.cs",
                        "using Acme.Json;\n\nnamespace Shop\n{\n    public class App\n    {\n        public void Go()\n        {\n            var tool = new Tool();\n            tool.Run();\n        }\n    }\n}\n",
                    ),
                ],
            ),
            (
                "a directory",
                &[
                    ("src/layouts/index.ts", "export const layout = 1;\n"),
                    ("src/app.ts", "import { layout } from './layouts';\n\nexport const a = layout;\n"),
                ],
            ),
        ];
        let mut failures = Vec::new();
        for (source, files) in shapes {
            let (nowhere, bound) = named_nowhere(files);
            assert!(bound > 0, "{source}: the shape binds a row to judge");
            if !nowhere.is_empty() {
                failures.push(format!("{source}: {nowhere:?}"));
            }
        }
        assert!(failures.is_empty(), "bound rows named nowhere: {failures:?}");
    }
}
