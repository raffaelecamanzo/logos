//! A Spring path built from a constant **another file of the member** declares
//! folds (S-470, [CR-151] §3.2 (b)/(c), [FR-FW-05], [NFR-RA-05]) — exercised
//! end-to-end through the public [`Engine`] façade against temp-directory
//! Maven layouts.
//!
//! [S-469] folds a constant the handler's own type declares. The reference
//! estate's other shape — 14 of its 16 concatenated paths — reaches the constant
//! through `import static …controller.GlobalControllerAdvice.EMAIL_ADDRESS_PARAMETER_NAME;`,
//! so the declaring file has to be found. It is found by type, through the
//! package-shaped module key [S-465] built; each fixture below pins one shape
//! that folds, or one that is refused and counted in `routes_not_composed`
//! rather than dropped.
//!
//! [CR-151]: ../../docs/requests/CR-151-provider-routes-composed-from-string-constants.md
//! [FR-FW-05]: ../../docs/specs/requirements/FR-FW-05.md
//! [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
//! [S-465]: ../../docs/planning/journal.md#s-465-a-java-files-module-identity-follows-its-package-so-its-imports-bind
//! [S-469]: ../../docs/planning/journal.md#s-469-a-refused-spring-method-path-is-counted-and-a-same-type-constant-concatenation-folds

#![cfg(feature = "lang-java")]

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId, NodeKind, RefForm};
use logos_core::models::pipeline::FrameworkStats;
use logos_core::Engine;
use logos_core::Runtime;
use tempfile::TempDir;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// The file that declares the constant, in package `a.b` — the estate's
/// `GlobalControllerAdvice`, whose constant is `= "emailAddress"`.
const ADVICE_FILE: &str = "src/main/java/a/b/GlobalControllerAdvice.java";

fn advice(value: &str) -> String {
    format!(
        "package a.b;\n\n\
         import org.springframework.web.bind.annotation.ControllerAdvice;\n\n\
         @ControllerAdvice\n\
         public class GlobalControllerAdvice {{\n    \
             public static final String EMAIL = {value};\n\
         }}\n"
    )
}

/// The estate's handler shape: a `@RequestMapping("/v1")` interface in
/// `package`, with `imports` above the Spring ones and `path` as the
/// `value =` of one `@GetMapping`.
fn api(package: &str, imports: &str, path: &str) -> String {
    format!(
        "package {package};\n\n\
         {imports}\
         import org.springframework.web.bind.annotation.GetMapping;\n\
         import org.springframework.web.bind.annotation.RequestMapping;\n\n\
         @RequestMapping(\"/v1\")\n\
         public interface MailboxApiV1 {{\n\n    \
             @GetMapping(value = {path})\n    \
             String size(String emailAddress);\n\
         }}\n"
    )
}

/// The handler's file in package `a.<leaf>`.
fn api_file(leaf: &str) -> String {
    format!("src/main/java/a/{leaf}/MailboxApiV1.java")
}

const STATIC_IMPORT: &str = "import static a.b.GlobalControllerAdvice.EMAIL;\n";
const CONCATENATED: &str = r#""/m/{" + EMAIL + "}/size""#;
const FOLDED_ROUTE: &str = "GET /v1/m/{emailAddress}/size";

/// Index `files` in a fresh project; the engine and the run's framework stats.
fn index(files: &[(&str, &str)]) -> (TempDir, Engine, FrameworkStats) {
    let tmp = TempDir::new().unwrap();
    for (rel, text) in files {
        write(tmp.path(), rel, text);
    }
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.index();
    (tmp, engine, result.framework)
}

/// Every node of `kind` as `(id, name)`.
fn nodes_of(rt: &Runtime, kind: NodeKind) -> Vec<(NodeId, String)> {
    rt.submit_read(move |store| {
        Ok(store
            .all_nodes()?
            .into_iter()
            .filter(|n| n.kind == kind)
            .map(|n| (n.id, n.name))
            .collect())
    })
    .expect("read runs")
}

fn route_names(rt: &Runtime) -> Vec<String> {
    let mut names: Vec<String> = nodes_of(rt, NodeKind::Route)
        .into_iter()
        .map(|(_, name)| name)
        .collect();
    names.sort();
    names
}

/// `(route name, handler name)` for every `RoutesTo` edge.
fn routes_to(rt: &Runtime) -> Vec<(String, String)> {
    rt.submit_read(|store| {
        let name: HashMap<NodeId, String> = store
            .all_nodes()?
            .into_iter()
            .map(|n| (n.id, n.name))
            .collect();
        let mut out: Vec<(String, String)> = store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == EdgeKind::RoutesTo)
            .map(|e| (name[&e.source].clone(), name[&e.target].clone()))
            .collect();
        out.sort();
        Ok(out)
    })
    .expect("read runs")
}

/// Assert that `files` promote no route and count exactly one refusal.
fn assert_refused_once(files: &[(&str, &str)], why: &str) {
    let (_tmp, engine, stats) = index(files);
    let rt = engine.runtime().unwrap();
    assert_eq!(route_names(rt), Vec::<String>::new(), "{why}: no route is promoted");
    assert_eq!(stats.routes, 0, "{why}");
    assert_eq!(stats.routes_not_composed, 1, "{why}: refused and counted, never dropped");
}

// ── Shapes that fold ─────────────────────────────────────────────────────────

/// The acceptance criterion's shape, and the estate's: `import static
/// a.b.GlobalControllerAdvice.EMAIL;` beside `value = "/m/{" + EMAIL +
/// "}/size"` — from the declaring type's own package (the estate's layout) and
/// from another package (where only the import can name it).
#[test]
fn a_static_imported_constant_of_another_type_in_the_member_folds() {
    for leaf in ["b", "c"] {
        let handler = api_file(leaf);
        let (_tmp, engine, stats) = index(&[
            (ADVICE_FILE, &advice(r#""emailAddress""#)),
            (&handler, &api(&format!("a.{leaf}"), STATIC_IMPORT, CONCATENATED)),
        ]);
        let rt = engine.runtime().unwrap();
        assert_eq!(route_names(rt), [FOLDED_ROUTE], "handler in a.{leaf}");
        assert_eq!(stats.routes_not_composed, 0, "handler in a.{leaf}");
        // A folded route links to its handler exactly as a written one does.
        assert_eq!(
            routes_to(rt),
            [(FOLDED_ROUTE.to_string(), "size".to_string())],
            "handler in a.{leaf}"
        );
    }
}

/// `GlobalControllerAdvice.EMAIL`, written qualified with no import, from a
/// file of the same package: Java finds a type of the file's own package
/// without an import, and the fold finds it the same way.
#[test]
fn a_qualified_constant_of_a_same_package_type_folds_identically() {
    let (_tmp, engine, stats) = index(&[
        (ADVICE_FILE, &advice(r#""emailAddress""#)),
        (
            &api_file("b"),
            &api("a.b", "", r#""/m/{" + GlobalControllerAdvice.EMAIL + "}/size""#),
        ),
    ]);
    let rt = engine.runtime().unwrap();
    assert_eq!(route_names(rt), [FOLDED_ROUTE]);
    assert_eq!(stats.routes_not_composed, 0);
}

/// The same qualified form from another package, through a single-type
/// `import a.b.GlobalControllerAdvice;` — and, beside it, the near miss: the
/// same file without the import, whose own package has no such type.
#[test]
fn a_qualified_constant_of_a_single_type_imported_type_folds() {
    let path = r#""/m/{" + GlobalControllerAdvice.EMAIL + "}/size""#;
    let (_tmp, engine, stats) = index(&[
        (ADVICE_FILE, &advice(r#""emailAddress""#)),
        (
            &api_file("c"),
            &api("a.c", "import a.b.GlobalControllerAdvice;\n", path),
        ),
    ]);
    let rt = engine.runtime().unwrap();
    assert_eq!(route_names(rt), [FOLDED_ROUTE]);
    assert_eq!(stats.routes_not_composed, 0);

    assert_refused_once(
        &[
            (ADVICE_FILE, &advice(r#""emailAddress""#)),
            (&api_file("c"), &api("a.c", "", path)),
        ],
        "another package's type named without an import",
    );
}

/// A constant whose own value is built from a constant of its file folds
/// through both, and a repeated import is one import, not two declarations.
#[test]
fn a_constant_built_in_its_own_file_folds_and_a_repeated_import_is_one_import() {
    let advice_file = "package a.b;\n\n\
        public class GlobalControllerAdvice {\n    \
            static final String PARAM = \"email\";\n    \
            public static final String EMAIL = PARAM + \"Address\";\n\
        }\n";
    let (_tmp, engine, stats) = index(&[
        (ADVICE_FILE, advice_file),
        (
            &api_file("c"),
            &api("a.c", &format!("{STATIC_IMPORT}{STATIC_IMPORT}"), CONCATENATED),
        ),
    ]);
    let rt = engine.runtime().unwrap();
    assert_eq!(route_names(rt), [FOLDED_ROUTE]);
    assert_eq!(stats.routes_not_composed, 0);
}

// ── Shapes refused and counted — one fixture per shape ──────────────────────

/// `import static a.b.GlobalControllerAdvice.*;` could supply `EMAIL`, but a
/// wildcard proves nothing about which declaration a name binds to.
#[test]
fn a_wildcard_static_import_is_refused_and_counted() {
    assert_refused_once(
        &[
            (ADVICE_FILE, &advice(r#""emailAddress""#)),
            (
                &api_file("c"),
                &api("a.c", "import static a.b.GlobalControllerAdvice.*;\n", CONCATENATED),
            ),
        ],
        "a static wildcard",
    );
}

/// The constant lives in another member (a sibling directory the engine does
/// not index) or in a library (Spring's own `HttpHeaders.ACCEPT`): no file of
/// this member declares its type, so nothing is folded.
#[test]
fn a_constant_of_another_member_or_a_library_is_refused_and_counted() {
    let workspace = TempDir::new().unwrap();
    write(
        workspace.path(),
        &format!("lib/{ADVICE_FILE}"),
        &advice(r#""emailAddress""#),
    );
    write(
        workspace.path(),
        &format!("svc/{}", api_file("c")),
        &api("a.c", STATIC_IMPORT, CONCATENATED),
    );
    let engine = Engine::start(workspace.path().join("svc")).expect("engine starts");
    let stats = engine.index().framework;
    assert_eq!(route_names(engine.runtime().unwrap()), Vec::<String>::new());
    assert_eq!(stats.routes_not_composed, 1, "another member's constant is counted");

    assert_refused_once(
        &[(
            &api_file("c"),
            &api(
                "a.c",
                "import static org.springframework.http.HttpHeaders.ACCEPT;\n",
                r#""/m/{" + ACCEPT + "}/size""#,
            ),
        )],
        "a library constant",
    );
}

/// Two declarations of `EMAIL` visible at once: two static imports of the name
/// from two types, and one import whose type the member declares twice under
/// one name (a `src/main` and a `src/test` copy).
#[test]
fn two_visible_declarations_of_one_name_are_refused_and_counted() {
    let other = "package a.b;\n\npublic class Other {\n    public static final String EMAIL = \"other\";\n}\n";
    assert_refused_once(
        &[
            (ADVICE_FILE, &advice(r#""emailAddress""#)),
            ("src/main/java/a/b/Other.java", other),
            (
                &api_file("c"),
                &api(
                    "a.c",
                    &format!("{STATIC_IMPORT}import static a.b.Other.EMAIL;\n"),
                    CONCATENATED,
                ),
            ),
        ],
        "two static imports of one name",
    );
    assert_refused_once(
        &[
            (ADVICE_FILE, &advice(r#""emailAddress""#)),
            ("src/test/java/a/b/GlobalControllerAdvice.java", &advice(r#""test""#)),
            (&api_file("c"), &api("a.c", STATIC_IMPORT, CONCATENATED)),
        ],
        "one type name declared twice in the member",
    );
}

/// A body that may inherit — a class with a supertype — could hide the
/// imported constant behind an inherited field of the same name, and hide the
/// imported type behind an inherited member type; both are refused. A class
/// without one folds, which is the near miss.
#[test]
fn an_import_is_not_followed_where_an_inherited_member_could_shadow_it() {
    let controller = |header: &str, path: &str| {
        format!(
            "package a.c;\n\n\
             {STATIC_IMPORT}\
             import a.b.GlobalControllerAdvice;\n\
             import org.springframework.web.bind.annotation.GetMapping;\n\
             import org.springframework.web.bind.annotation.RestController;\n\n\
             @RestController\n\
             public class {header} {{\n    \
                 @GetMapping(value = {path})\n    \
                 public String size() {{ return \"\"; }}\n\
             }}\n"
        )
    };
    let qualified = r#""/m/{" + GlobalControllerAdvice.EMAIL + "}/size""#;
    for path in [CONCATENATED, qualified] {
        assert_refused_once(
            &[
                (ADVICE_FILE, &advice(r#""emailAddress""#)),
                (
                    "src/main/java/a/c/MailboxController.java",
                    &controller("MailboxController extends Base", path),
                ),
            ],
            &format!("an inheriting class: {path}"),
        );
        let (_tmp, engine, stats) = index(&[
            (ADVICE_FILE, &advice(r#""emailAddress""#)),
            (
                "src/main/java/a/c/MailboxController.java",
                &controller("MailboxController", path),
            ),
        ]);
        assert_eq!(route_names(engine.runtime().unwrap()), ["GET /m/{emailAddress}/size"], "{path}");
        assert_eq!(stats.routes_not_composed, 0, "{path}");
    }
}

// ── Incremental: the declaring file is an input of the fold ─────────────────

/// Route names, `(source, target, kind)` edges by symbol, and the ledger rows —
/// the sync ≡ reindex comparison ([NFR-RA-06]). Capture-before-delete rows
/// (`RefForm::Symbol`) are a sync-only bookkeeping artifact and excluded, as
/// in `tests/java_imports.rs`.
///
/// [NFR-RA-06]: ../../docs/specs/requirements/NFR-RA-06.md
type Facts = (Vec<String>, Vec<(String, String, String)>, Vec<String>);

fn facts(rt: &Runtime) -> Facts {
    let routes = route_names(rt);
    let (edges, refs) = rt
        .submit_read(|store| {
            let sym: HashMap<NodeId, String> = store
                .all_nodes()?
                .into_iter()
                .map(|n| (n.id, n.symbol.as_str().to_string()))
                .collect();
            let mut edges: Vec<(String, String, String)> = store
                .all_edges()?
                .into_iter()
                .map(|e| {
                    (
                        sym[&e.source].clone(),
                        sym[&e.target].clone(),
                        e.kind.as_str().to_string(),
                    )
                })
                .collect();
            edges.sort();
            let mut refs: Vec<String> = store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| r.form != RefForm::Symbol)
                .map(|r| format!("{} {} {:?} {:?} {}", r.source_symbol, r.target, r.form, r.kind, r.resolved))
                .collect();
            refs.sort();
            Ok((edges, refs))
        })
        .expect("read runs");
    (routes, edges, refs)
}

/// A full index of the project's current files: its facts and its framework
/// counts, to compare a synced store against.
fn cold(tmp: &TempDir, files: &[&str]) -> (Facts, FrameworkStats) {
    let copy = TempDir::new().unwrap();
    for rel in files {
        if let Ok(text) = fs::read_to_string(tmp.path().join(rel)) {
            write(copy.path(), rel, &text);
        }
    }
    let engine = Engine::start(copy.path()).expect("engine starts");
    let stats = engine.index().framework;
    (facts(engine.runtime().unwrap()), stats)
}

/// The route counts a run published — its duration aside, which no two runs
/// share.
fn counts(stats: &FrameworkStats) -> (u64, u64, u64) {
    (stats.routes, stats.components, stats.routes_not_composed)
}

/// Editing **only** the declaring file re-folds the routes built from it on
/// `logos sync`, and the synced store equals a full re-index: first a new
/// value (the route moves), then the constant renamed away (the route goes,
/// and is counted), then restored (it comes back).
#[test]
fn editing_only_the_constants_file_re_folds_its_routes_and_equals_a_full_reindex() {
    let handler = api_file("c");
    let (tmp, engine, stats) = index(&[
        (ADVICE_FILE, &advice(r#""emailAddress""#)),
        (&handler, &api("a.c", STATIC_IMPORT, CONCATENATED)),
    ]);
    let rt = engine.runtime().unwrap();
    assert_eq!(route_names(rt), [FOLDED_ROUTE]);
    assert_eq!(stats.routes_not_composed, 0);
    let files = [ADVICE_FILE, handler.as_str()];

    for (edit, expected_routes, expected_refused) in [
        (advice(r#""mail""#), vec!["GET /v1/m/{mail}/size"], 0),
        (
            advice(r#""emailAddress""#).replace("EMAIL =", "MAIL ="),
            vec![],
            1,
        ),
        (advice(r#""emailAddress""#), vec![FOLDED_ROUTE], 0),
    ] {
        write(tmp.path(), ADVICE_FILE, &edit);
        let synced = engine.sync(&[ADVICE_FILE.into()]).framework;
        assert_eq!(route_names(rt), expected_routes, "after editing to {edit}");
        assert_eq!(synced.routes_not_composed, expected_refused, "after editing to {edit}");
        let (cold_facts, cold_stats) = cold(&tmp, &files);
        assert_eq!(facts(rt), cold_facts, "sync must equal a full re-index after {edit}");
        assert_eq!(counts(&synced), counts(&cold_stats), "after {edit}");
    }
}

/// Deleting the declaring file refuses the route on sync, exactly as a full
/// index of the remaining files does.
#[test]
fn deleting_the_constants_file_refuses_its_routes_and_equals_a_full_reindex() {
    let handler = api_file("c");
    let (tmp, engine, _) = index(&[
        (ADVICE_FILE, &advice(r#""emailAddress""#)),
        (&handler, &api("a.c", STATIC_IMPORT, CONCATENATED)),
    ]);
    let rt = engine.runtime().unwrap();
    fs::remove_file(tmp.path().join(ADVICE_FILE)).unwrap();
    let synced = engine.sync(&[ADVICE_FILE.into()]).framework;
    assert_eq!(route_names(rt), Vec::<String>::new());
    assert_eq!(synced.routes_not_composed, 1);
    let (cold_facts, cold_stats) = cold(&tmp, &[handler.as_str()]);
    assert_eq!(facts(rt), cold_facts);
    assert_eq!(counts(&synced), counts(&cold_stats));
}

/// `import a.b.GlobalControllerAdvice.*;` imports the type's **member types**,
/// not the type: `GlobalControllerAdvice.EMAIL` from package `a.c` names
/// nothing it declares, so it is refused rather than read as the single-type
/// import it is not.
#[test]
fn a_type_wildcard_import_does_not_name_the_type_itself() {
    assert_refused_once(
        &[
            (ADVICE_FILE, &advice(r#""emailAddress""#)),
            (
                &api_file("c"),
                &api(
                    "a.c",
                    "import a.b.GlobalControllerAdvice.*;\n",
                    r#""/m/{" + GlobalControllerAdvice.EMAIL + "}/size""#,
                ),
            ),
        ],
        "a wildcard over the type's members",
    );
}

/// An import the parser could not read might import anything — a static
/// wildcard that could supply the name, a second `EMAIL` — so a file holding
/// one reaches through no import at all. The same file without the broken line
/// folds, which is the near miss.
#[test]
fn an_import_the_parser_could_not_read_stops_the_reach() {
    for broken in [
        // Recovered inside the declaration.
        "import static a.b.Other.;\n",
        "import static a.b.Other.EMAIL\n",
        // Recovered at the top of the file: no declaration left to mark.
        "impot static a.b.Other.*;\n",
        // No path at all, and a typo that parses as something else entirely.
        "import static ;\n",
        "static import a.b.Other.EMAIL;\n",
    ] {
        assert_refused_once(
            &[
                (ADVICE_FILE, &advice(r#""emailAddress""#)),
                (
                    &api_file("c"),
                    &api("a.c", &format!("{STATIC_IMPORT}{broken}"), CONCATENATED),
                ),
            ],
            &format!("a broken import {broken:?}"),
        );
    }
}

/// A cross-file fold goes one file deep: a constant whose own value names a
/// third file's constant through its file's static import is refused rather
/// than chased — which also leaves no cycle between files to guard. Pinned as
/// a limitation, so widening it is a decision rather than an accident.
#[test]
fn a_constant_built_from_a_third_files_constant_is_refused_and_counted() {
    let chained = "package a.b;\n\n\
        import static a.b.Base.PARAM;\n\n\
        public class GlobalControllerAdvice {\n    \
            public static final String EMAIL = PARAM + \"Address\";\n\
        }\n";
    let base = "package a.b;\n\npublic class Base {\n    public static final String PARAM = \"email\";\n}\n";
    assert_refused_once(
        &[
            (ADVICE_FILE, chained),
            ("src/main/java/a/b/Base.java", base),
            (&api_file("c"), &api("a.c", STATIC_IMPORT, CONCATENATED)),
        ],
        "a constant two files away",
    );
}
