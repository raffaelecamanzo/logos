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
use crate::graph_store::{NewUnresolvedRef, SqliteGraphStore, UnboundCodeTarget};
use crate::model::RefForm;
use crate::models::ReferenceFigureAbsence;
use crate::Engine;

fn universe(names: &[&str]) -> NameUniverse {
    NameUniverse::build(
        &DeclaredNameSources {
            names: names.iter().map(|n| (*n).to_string()).collect(),
            aliases: Vec::new(),
            files: Vec::new(),
            facades: Vec::new(),
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
            facades: Vec::new(),
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
            facades: Vec::new(),
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

/// S-835 (CR-262 N1): only an `Imports` row's alias is a name. A Python call
/// through an imported module records its receiver as the call row's alias
/// (`requests` of `requests.get(u)`), and a call through a parameter the
/// `local-value` marker; neither declares anything, so the import of the
/// external `requests` reads provably external.
#[cfg(feature = "lang-python")]
#[test]
fn a_call_rows_alias_and_the_local_value_marker_are_never_names() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "app.py",
        "import requests\n\n\ndef fetch(u, handler):\n    handler(u)\n    return requests.get(u)\n",
    );
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.pipeline().index();
    let layout = PackageLayout::from_registry(engine.registry().expect("a registry"));
    let (ledger, population) = engine
        .runtime()
        .unwrap()
        .submit_read(|store| Ok((store.unresolved_refs()?, store.reference_population()?)))
        .expect("read runs");
    // The fixture carries both call-row aliases, or this test pins nothing.
    let call_aliases: Vec<&str> = ledger
        .iter()
        .filter(|r| r.kind == EdgeKind::Calls)
        .filter_map(|r| r.alias.as_deref())
        .collect();
    assert!(
        call_aliases.contains(&"requests") && call_aliases.contains(&crate::resolve::LOCAL_VALUE_ALIAS),
        "the fixture's calls carry a receiver alias and the local-value marker: {call_aliases:?}"
    );
    assert_eq!(
        population.names.aliases,
        vec![("requests".to_string(), "requests".to_string())],
        "only the import row's alias is collected"
    );
    let universe = NameUniverse::build(&population.names, &layout);
    for alias in ["requests", crate::resolve::LOCAL_VALUE_ALIAS] {
        assert!(!universe.could_name(alias), "{alias} is a call row's alias, not a name");
    }
    let status = engine.graph().status();
    let python = status
        .resolution_by_language
        .iter()
        .find(|row| row.language == "python")
        .expect("a python row");
    assert_eq!(
        (python.imports.references, python.imports.bound, python.imports.provably_external),
        (1, 0, Some(1)),
        "the import of the external `requests` is provably external"
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

/// A ledger row for [`plant`]: `(file, target, alias, form, kind, bound)`;
/// targets are unique, so a row is marked bound by its target.
type PlantedRow<'a> = (i64, &'a str, Option<&'a str>, RefForm, EdgeKind, bool);

/// An in-memory store holding `files` (`(path, language)`) and the ledger
/// rows `rows` builds from their ids, each marked bound or not.
fn plant(files: &[(&str, &str)], rows: impl FnOnce(&[i64]) -> Vec<PlantedRow<'static>>) -> SqliteGraphStore {
    let mut store = SqliteGraphStore::open_in_memory().expect("in-memory store");
    let bound: Vec<&str> = store
        .write_batch(|w| {
            let ids = files
                .iter()
                .map(|(path, language)| w.insert_file(path, Some(language), None))
                .collect::<anyhow::Result<Vec<_>>>()?;
            let mut bound = Vec::new();
            for (file, target, alias, form, kind, is_bound) in rows(&ids) {
                w.insert_unresolved_ref(&NewUnresolvedRef {
                    file_id: Some(file),
                    source_symbol: "planted caller",
                    target,
                    alias,
                    form,
                    kind,
                    line: None,
                    payload: None,
                    receiver: None,
                    peeled: None,
                    arg_count: None,
                    exported: None,
                })?;
                if is_bound {
                    bound.push(target);
                }
            }
            Ok(bound)
        })
        .expect("plant the ledger");
    let ids: Vec<i64> = store
        .unresolved_refs()
        .expect("read the ledger")
        .into_iter()
        .filter(|r| bound.contains(&r.target.as_str()))
        .map(|r| r.id)
        .collect();
    store
        .write_batch(|w| ids.iter().try_for_each(|id| w.mark_ref_resolved(*id, true)))
        .expect("bind the planted rows");
    store
}

/// The check fires: a bound row named nowhere is reported with its language,
/// kind and target, and each language's figures count exactly the rows they
/// name — a capture-before-delete row and a doc reference judged by none.
#[test]
fn the_soundness_check_reports_each_counterexample_with_its_language() {
    let store = plant(&[("app.py", "python"), ("lib.rs", "rust")], |ids| {
        let (py, rs) = (ids[0], ids[1]);
        vec![
            (py, "zz_declared_nowhere", None, RefForm::Path, EdgeKind::Calls, true),
            (py, "requests::get", Some("requests"), RefForm::Method, EdgeKind::Calls, false),
            (py, "zz_doc_nowhere", None, RefForm::Path, EdgeKind::DocReference, true),
            (py, "zz_captured_nowhere", None, RefForm::Symbol, EdgeKind::Calls, true),
            (rs, "app::run", None, RefForm::Path, EdgeKind::Imports, true),
        ]
    });
    let soundness = bound_row_soundness(&store, &PackageLayout::default()).expect("the check reads");
    assert_eq!(
        soundness.named_nowhere,
        vec![NamedNowhereRow {
            language: Some("python".to_string()),
            kind: EdgeKind::Calls,
            target: "zz_declared_nowhere".to_string(),
        }]
    );
    let reading = |bound_judged, named_nowhere, call_aliases| LanguageSoundness {
        bound_judged,
        named_nowhere,
        call_aliases,
    };
    assert_eq!(
        soundness.by_language,
        BTreeMap::from([
            (Some("python".to_string()), reading(1, 1, 1)),
            // `app::run` is named by the file stem `app`.
            (Some("rust".to_string()), reading(1, 0, 0)),
        ])
    );
    assert_eq!(soundness.total(), reading(2, 1, 1));
}

/// A store with more files than SQLite binds in one statement is read whole:
/// the language of every file is looked up, the last one's included.
#[test]
fn the_soundness_check_reads_a_store_with_more_files_than_one_statement_binds() {
    let paths: Vec<String> = (0..33_000).map(|i| format!("src/m{i}.py")).collect();
    let files: Vec<(&str, &str)> = paths.iter().map(|p| (p.as_str(), "python")).collect();
    let store = plant(&files, |ids| {
        vec![(ids[ids.len() - 1], "zz_declared_nowhere", None, RefForm::Path, EdgeKind::Calls, true)]
    });
    let soundness = bound_row_soundness(&store, &PackageLayout::default())
        .expect("the check reads past one statement's host parameters");
    assert_eq!(soundness.named_nowhere.len(), 1);
    assert_eq!(soundness.named_nowhere[0].language.as_deref(), Some("python"));
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// The bound, name-bound rows of `files` the name test would call external —
/// each a counterexample to its soundness — and how many bound rows it judged:
/// the [`bound_row_soundness`] check the estate harness runs too.
fn named_nowhere(files: &[(&str, &str)]) -> (Vec<String>, u64) {
    let tmp = TempDir::new().unwrap();
    for (rel, contents) in files {
        write(tmp.path(), rel, contents);
    }
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.pipeline().index();
    let layout = PackageLayout::from_registry(engine.registry().expect("a registry"));
    let soundness = engine
        .runtime()
        .unwrap()
        .submit_read(move |store| bound_row_soundness(store, &layout))
        .expect("read runs");
    let nowhere = soundness
        .named_nowhere
        .iter()
        .map(|r| format!("{:?} {}", r.kind, r.target))
        .collect();
    (nowhere, soundness.total().bound_judged)
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

    /// The `@file:JvmName` shape (S-832, CR-261): Java imports and calls the
    /// facade a file declares, a name no node, path segment or namespace
    /// spells — only the universe's file facades.
    const DECLARED_FACADE: &[(&str, &str)] = &[
        (
            "src/main/kotlin/app/text/Strings.kt",
            "@file:JvmName(\"Slugs\")\n\npackage app.text\n\nfun slug(s: String): String = s\n",
        ),
        (
            "src/main/java/app/boot/Boot.java",
            "package app.boot;\n\nimport app.text.Slugs;\n\nclass Boot {\n    void start() {\n        Slugs.slug(\"x\");\n    }\n}\n",
        ),
    ];

    /// CR-212 A1, verified per binder: a row the binder bound is never one the
    /// name test would call provably external. Measured on every code
    /// language's reach fixture — the shapes each binds across files — and on
    /// a declared file facade (S-832).
    #[test]
    fn bound_rows_are_never_named_nowhere() {
        let mut judged = 0;
        for (language, fixture) in reach::ALL.iter().copied().chain([("a declared file facade", DECLARED_FACADE)]) {
            let (nowhere, bound) = named_nowhere(fixture);
            assert!(nowhere.is_empty(), "{language}: bound rows named nowhere: {nowhere:?}");
            if fixture == DECLARED_FACADE {
                assert!(bound >= 2, "{language}: the import and the call bind ({bound} judged)");
            }
            judged += bound;
        }
        assert!(judged > 0, "the fixtures bound rows to judge");
    }

    /// FR-RS-49 (S-832): the universe the classifier reads holds the facade a
    /// file declares, in place of the one its stem derives — what the binder
    /// binds `Slugs.slug("x")` through. No bound row above needs it (an import
    /// is named by any segment, a call by its last name), so it is pinned here.
    #[test]
    fn the_universe_holds_a_declared_facade_in_place_of_the_derived_one() {
        let tmp = TempDir::new().unwrap();
        for (rel, contents) in DECLARED_FACADE {
            write(tmp.path(), rel, contents);
        }
        let engine = Engine::start(tmp.path()).expect("engine starts");
        engine.pipeline().index();
        let layout = PackageLayout::from_registry(engine.registry().expect("a registry"));
        let (_, universe) = engine
            .runtime()
            .unwrap()
            .submit_read(move |store| read_population(store, &layout))
            .expect("read runs");
        assert!(universe.names.contains("Slugs"), "the declared facade is a name");
        assert!(!universe.names.contains("StringsKt"), "the derived one is not");
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
