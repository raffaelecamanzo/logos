//! A Java file's module identity follows its package, so its imports bind
//! (S-465, CR-149, FR-RS-01, FR-RS-03, NFR-RA-05) — exercised end-to-end through
//! the public [`Engine`] façade against real temp-directory fixtures.
//!
//! Before S-465 every Java file was keyed by the Rust module model:
//! `src/main/java/com/x/svc/Svc.java` was `main::java::com::x::svc::Svc`, so the
//! import `com.x.svc.Svc` could never descend to it and 18 of the reference
//! estate's 25,132 Java imports bound — by accident, through the workspace
//! suffix match, to a static-imported *member*. The Java descriptor now declares
//! `[package_modules]` and each fixture here pins one import shape to the
//! **type** (or member) it names, in a single-module and a multi-module Maven
//! layout, with the never-fabricate negatives beside them.
//!
//! [FR-RS-01]: ../../docs/specs/requirements/FR-RS-01.md
//! [FR-RS-03]: ../../docs/specs/requirements/FR-RS-03.md
//! [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md

#![cfg(feature = "lang-java")]

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId, NodeKind, RefForm};
use logos_core::Engine;
use logos_core::Runtime;
use tempfile::TempDir;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn index(tmp: &TempDir) -> Engine {
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    engine
}

/// Every edge of `kind` as `(source file:name, target file:name:kind)`, sorted.
/// The target's kind is in the label because the defect this story fixes is a
/// *kind* confusion: an import must reach the class, not its file module.
fn edges_of(rt: &Runtime, kind: EdgeKind) -> Vec<(String, String)> {
    rt.submit_read(move |store| {
        let label: HashMap<NodeId, (String, NodeKind)> = store
            .all_nodes()?
            .into_iter()
            .map(|n| {
                let file = n.file_path.unwrap_or_default();
                (n.id, (format!("{file}:{}", n.name), n.kind))
            })
            .collect();
        let mut out: Vec<(String, String)> = store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == kind)
            .map(|e| {
                let (target, target_kind) = &label[&e.target];
                (
                    label[&e.source].0.clone(),
                    format!("{target}:{}", target_kind.as_str()),
                )
            })
            .collect();
        out.sort();
        Ok(out)
    })
    .expect("read runs")
}

/// The `Imports` edge targets out of the file `rel`, sorted.
fn imports_of(rt: &Runtime, rel: &str) -> Vec<String> {
    let prefix = format!("{rel}:");
    edges_of(rt, EdgeKind::Imports)
        .into_iter()
        .filter(|(s, _)| s.starts_with(&prefix))
        .map(|(_, t)| t)
        .collect()
}

/// The `Calls` edges whose target is `file:name:kind`, as source labels.
fn callers_of(rt: &Runtime, target: &str) -> Vec<String> {
    edges_of(rt, EdgeKind::Calls)
        .into_iter()
        .filter(|(_, t)| t == target)
        .map(|(s, _)| s)
        .collect()
}

/// The ledger's `Imports` rows for `target`: `(form, resolved)` pairs, sorted.
fn import_rows(rt: &Runtime, target: &str) -> Vec<(RefForm, bool)> {
    let wanted = target.to_string();
    let mut rows: Vec<(RefForm, bool)> = rt
        .submit_read(move |store| {
            Ok(store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| r.kind == EdgeKind::Imports && r.target == wanted)
                .map(|r| (r.form, r.resolved))
                .collect())
        })
        .expect("read runs");
    rows.sort_by_key(|(f, r)| (f.as_i32(), *r));
    rows
}

const SVC: &str = "package com.x.svc;\n\
\n\
public class Svc {\n\
    public static final String K = \"k\";\n\
    public String run() { return helper(); }\n\
    public static String helper() { return \"h\"; }\n\
    public static String util() { return \"u\"; }\n\
    public static class Inner { public void go() {} }\n\
}\n";

/// The single-module fixture: one Maven source root, two packages.
fn single_module(ctl: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/main/java/com/x/svc/Svc.java", SVC);
    write(tmp.path(), "src/main/java/com/x/web/Ctl.java", ctl);
    tmp
}

const SVC_FILE: &str = "src/main/java/com/x/svc/Svc.java";
const CTL_FILE: &str = "src/main/java/com/x/web/Ctl.java";

#[test]
fn a_single_type_import_binds_to_the_class_not_its_file_module() {
    let tmp = single_module(
        "package com.x.web;\n\nimport com.x.svc.Svc;\n\npublic class Ctl {\n    private Svc svc;\n}\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(imports_of(rt, CTL_FILE), [format!("{SVC_FILE}:Svc:class")]);
    assert_eq!(import_rows(rt, "com::x::svc::Svc"), [(RefForm::Path, true)]);
}

#[test]
fn a_single_type_import_of_a_nested_type_binds_to_the_nested_class() {
    let tmp =
        single_module("package com.x.web;\n\nimport com.x.svc.Svc.Inner;\n\npublic class Ctl {}\n");
    let engine = index(&tmp);
    assert_eq!(
        imports_of(engine.runtime().unwrap(), CTL_FILE),
        [format!("{SVC_FILE}:Inner:class")]
    );
}

#[test]
fn a_single_type_import_binds_across_the_modules_of_a_multi_module_maven_repository() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "mailbox-core/src/main/java/com/x/svc/Svc.java",
        SVC,
    );
    write(
        tmp.path(),
        "mailbox-api/src/main/java/com/x/api/Api.java",
        "package com.x.api;\n\nimport com.x.svc.Svc;\n\npublic class Api {\n    public String get() { return new Svc().run(); }\n}\n",
    );
    let engine = index(&tmp);
    assert_eq!(
        imports_of(
            engine.runtime().unwrap(),
            "mailbox-api/src/main/java/com/x/api/Api.java"
        ),
        ["mailbox-core/src/main/java/com/x/svc/Svc.java:Svc:class"]
    );
}

#[test]
fn a_static_import_binds_to_the_member_and_the_call_through_it_binds() {
    let tmp = single_module(
        "package com.x.web;\n\nimport static com.x.svc.Svc.helper;\n\npublic class Ctl {\n    public String get() { return helper(); }\n}\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        imports_of(rt, CTL_FILE),
        [format!("{SVC_FILE}:helper:method")]
    );
    // The imported rung (FR-RS-03): the bare `helper()` resolves through the
    // file's static import, not by name across the workspace.
    assert_eq!(
        callers_of(rt, &format!("{SVC_FILE}:helper:method")),
        [format!("{SVC_FILE}:run"), format!("{CTL_FILE}:get")]
    );
}

#[test]
fn a_wildcard_import_is_a_glob_and_brings_the_types_members_into_scope() {
    let tmp = single_module(
        "package com.x.web;\n\nimport com.x.svc.*;\nimport static com.x.svc.Svc.*;\n\npublic class Ctl {\n    public String get() { return util(); }\n}\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    // Both wildcards are recorded as globs. The static one names the type `Svc`
    // and binds to it; the package one names a package, which has no node, so
    // its row stays unbound — never bound to some file of the package.
    assert_eq!(import_rows(rt, "com::x::svc::Svc"), [(RefForm::Glob, true)]);
    assert_eq!(import_rows(rt, "com::x::svc"), [(RefForm::Glob, false)]);
    assert_eq!(imports_of(rt, CTL_FILE), [format!("{SVC_FILE}:Svc:class")]);
    // `util()` is visible only through `import static com.x.svc.Svc.*`.
    assert_eq!(
        callers_of(rt, &format!("{SVC_FILE}:util:method")),
        [format!("{CTL_FILE}:get")]
    );
}

#[test]
fn the_static_and_wildcard_shapes_bind_in_a_multi_module_maven_repository_too() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "core/src/main/java/com/x/svc/Svc.java", SVC);
    write(
        tmp.path(),
        "web/src/main/java/com/x/web/Ctl.java",
        "package com.x.web;\n\nimport static com.x.svc.Svc.helper;\nimport static com.x.svc.Svc.*;\n\npublic class Ctl {\n    public String a() { return helper(); }\n    public String b() { return util(); }\n}\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    let ctl = "web/src/main/java/com/x/web/Ctl.java";
    let svc = "core/src/main/java/com/x/svc/Svc.java";
    assert_eq!(
        imports_of(rt, ctl),
        [format!("{svc}:Svc:class"), format!("{svc}:helper:method")]
    );
    assert_eq!(
        callers_of(rt, &format!("{svc}:helper:method")),
        [format!("{svc}:run"), format!("{ctl}:a")]
    );
    assert_eq!(
        callers_of(rt, &format!("{svc}:util:method")),
        [format!("{ctl}:b")]
    );
}

#[test]
fn a_test_type_imports_its_production_class_across_the_two_source_roots() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), SVC_FILE, SVC);
    write(
        tmp.path(),
        "src/test/java/com/x/web/CtlTest.java",
        "package com.x.web;\n\nimport com.x.svc.Svc;\n\npublic class CtlTest {}\n",
    );
    let engine = index(&tmp);
    assert_eq!(
        imports_of(
            engine.runtime().unwrap(),
            "src/test/java/com/x/web/CtlTest.java"
        ),
        [format!("{SVC_FILE}:Svc:class")]
    );
}

#[test]
fn a_type_declared_under_one_name_in_main_and_in_test_stays_unresolved() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), SVC_FILE, SVC);
    write(tmp.path(), "src/test/java/com/x/svc/Svc.java", SVC);
    write(
        tmp.path(),
        CTL_FILE,
        "package com.x.web;\n\nimport com.x.svc.Svc;\nimport static com.x.svc.Svc.helper;\n\npublic class Ctl {\n    public String get() { return helper(); }\n}\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    // Two declarations of `com.x.svc.Svc`: the import picks neither (NFR-RA-05).
    assert!(
        imports_of(rt, CTL_FILE).is_empty(),
        "{:?}",
        imports_of(rt, CTL_FILE)
    );
    assert_eq!(
        import_rows(rt, "com::x::svc::Svc"),
        [(RefForm::Path, false)]
    );
    assert_eq!(
        import_rows(rt, "com::x::svc::Svc::helper"),
        [(RefForm::Path, false)]
    );
    assert!(callers_of(rt, &format!("{SVC_FILE}:helper:method"))
        .iter()
        .all(|c| !c.starts_with(CTL_FILE)));
}

#[test]
fn jdk_spring_and_lombok_imports_stay_unbound_even_beside_a_same_named_workspace_type() {
    // Each external import has a workspace type of the same simple name in
    // another package — the near miss a name-only or suffix reading would bind.
    let tmp = TempDir::new().unwrap();
    for (pkg, name) in [
        ("com.x.util", "List"),
        ("com.x.stereotype", "Service"),
        ("com.x.model", "Data"),
    ] {
        let dir = pkg.replace('.', "/");
        write(
            tmp.path(),
            &format!("src/main/java/{dir}/{name}.java"),
            &format!("package {pkg};\n\npublic class {name} {{}}\n"),
        );
    }
    write(
        tmp.path(),
        CTL_FILE,
        "package com.x.web;\n\nimport java.util.List;\nimport java.util.*;\nimport org.springframework.stereotype.Service;\nimport lombok.Data;\n\n@Service\n@Data\npublic class Ctl {\n    private List<String> xs;\n}\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert!(
        imports_of(rt, CTL_FILE).is_empty(),
        "{:?}",
        imports_of(rt, CTL_FILE)
    );
    for target in [
        "java::util::List",
        "org::springframework::stereotype::Service",
        "lombok::Data",
    ] {
        assert_eq!(
            import_rows(rt, target),
            [(RefForm::Path, false)],
            "{target}"
        );
    }
    assert_eq!(import_rows(rt, "java::util"), [(RefForm::Glob, false)]);
}

#[test]
fn a_spring_controller_behind_a_wildcard_import_is_still_a_framework_candidate() {
    // Spring candidacy reads import-prefix rows in the ledger by target text
    // (FR-FW-04), whatever their form: recording the wildcard as a glob must not
    // cost the controller its route.
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/main/java/com/x/web/Hello.java",
        "package com.x.web;\n\nimport org.springframework.web.bind.annotation.*;\n\n@RestController\npublic class Hello {\n    @GetMapping(\"/hello\")\n    public String hello() { return \"hi\"; }\n}\n",
    );
    let engine = index(&tmp);
    let routes: Vec<String> = engine
        .runtime()
        .unwrap()
        .submit_read(|store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .filter(|n| n.kind == NodeKind::Route)
                .map(|n| n.name)
                .collect())
        })
        .unwrap();
    assert_eq!(routes, ["GET /hello"]);
    assert_eq!(
        import_rows(
            engine.runtime().unwrap(),
            "org::springframework::web::bind::annotation"
        ),
        [(RefForm::Glob, false)]
    );
}

#[test]
fn a_doc_link_to_a_java_file_reaches_that_file_even_when_its_package_key_is_shared() {
    // `src/main/…/Svc.java` and `src/test/…/Svc.java` share one package-shaped
    // module key; a link names a PATH, so each must reach its own file.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), SVC_FILE, SVC);
    write(
        tmp.path(),
        "src/test/java/com/x/svc/Svc.java",
        "package com.x.svc;\n\npublic class Svc {}\n",
    );
    write(
        tmp.path(),
        "README.md",
        "# Svc\n\n[main](src/main/java/com/x/svc/Svc.java) and [test](src/test/java/com/x/svc/Svc.java)\n",
    );
    let engine = index(&tmp);
    let targets: Vec<String> = edges_of(engine.runtime().unwrap(), EdgeKind::DocReference)
        .into_iter()
        .map(|(_, t)| t)
        .collect();
    assert_eq!(
        targets,
        [
            format!("{SVC_FILE}:Svc:module"),
            "src/test/java/com/x/svc/Svc.java:Svc:module".to_string()
        ]
    );
}

/// Every binding fact of the graph in an id-free form — `(source symbol, target
/// symbol, kind)` edges and `(source, target, form, kind, resolved)` ledger rows
/// — for the sync ≡ reindex comparison (NFR-RA-06).
///
/// Capture-before-delete rows (`RefForm::Symbol`, ADR-10) are excluded exactly
/// as the CR-015 net in `tests/indexing.rs` excludes them: a sync-only
/// bookkeeping artifact a cold index never produces. The edges they preserve are
/// compared, so a mis-bound capture still fails the edge half.
fn binding_facts(rt: &Runtime) -> (Vec<(String, String, String)>, Vec<String>) {
    rt.submit_read(|store| {
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
            .map(|r| {
                format!(
                    "{} {} {:?} {:?} {}",
                    r.source_symbol, r.target, r.form, r.kind, r.resolved
                )
            })
            .collect();
        refs.sort();
        Ok((edges, refs))
    })
    .expect("read runs")
}

/// Index a fresh copy of `files` from `tmp` and return its binding facts.
fn cold_facts(tmp: &TempDir, files: &[&str]) -> (Vec<(String, String, String)>, Vec<String>) {
    let cold = TempDir::new().unwrap();
    for rel in files {
        if let Ok(text) = fs::read_to_string(tmp.path().join(rel)) {
            write(cold.path(), rel, &text);
        }
    }
    let engine = index(&cold);
    binding_facts(engine.runtime().unwrap())
}

const CTL_ALL_SHAPES: &str = "package com.x.web;\n\nimport com.x.svc.Svc;\nimport static com.x.svc.Svc.helper;\nimport static com.x.svc.Svc.*;\n\npublic class Ctl {\n    private Svc svc;\n    public String a() { return helper(); }\n    public String b() { return util(); }\n}\n";

#[test]
fn sync_equals_a_full_reindex_after_adding_a_java_file() {
    // The importer exists first; the imported file arrives on sync.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), CTL_FILE, CTL_ALL_SHAPES);
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert!(
        imports_of(rt, CTL_FILE).is_empty(),
        "precondition: nothing to bind yet"
    );
    write(tmp.path(), SVC_FILE, SVC);
    engine.sync(&[SVC_FILE.into()]);
    assert_eq!(
        imports_of(rt, CTL_FILE),
        [
            format!("{SVC_FILE}:Svc:class"),
            format!("{SVC_FILE}:helper:method")
        ]
    );
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &[SVC_FILE, CTL_FILE]));
}

#[test]
fn sync_equals_a_full_reindex_after_editing_a_java_file() {
    let tmp = single_module(CTL_ALL_SHAPES);
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        callers_of(rt, &format!("{SVC_FILE}:util:method")),
        [format!("{CTL_FILE}:b")]
    );
    // `util` leaves `Svc` and `helper` moves to a nested type: the static
    // import of `helper` and the calls through both imports must unbind.
    write(
        tmp.path(),
        SVC_FILE,
        "package com.x.svc;\n\npublic class Svc {\n    public static class Inner { public static String helper() { return \"h\"; } }\n}\n",
    );
    engine.sync(&[SVC_FILE.into()]);
    assert_eq!(imports_of(rt, CTL_FILE), [format!("{SVC_FILE}:Svc:class")]);
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &[SVC_FILE, CTL_FILE]));
}

#[test]
fn sync_equals_a_full_reindex_after_deleting_a_java_file() {
    let tmp = single_module(CTL_ALL_SHAPES);
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert!(
        !imports_of(rt, CTL_FILE).is_empty(),
        "precondition: the imports bind"
    );
    fs::remove_file(tmp.path().join(SVC_FILE)).unwrap();
    engine.sync(&[SVC_FILE.into()]);
    assert!(
        imports_of(rt, CTL_FILE).is_empty(),
        "{:?}",
        imports_of(rt, CTL_FILE)
    );
    assert_eq!(
        import_rows(rt, "com::x::svc::Svc"),
        [(RefForm::Path, false), (RefForm::Glob, false)]
    );
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &[SVC_FILE, CTL_FILE]));
}

#[test]
fn sync_re_decides_a_wildcard_call_when_its_imported_type_becomes_ambiguous() {
    // The call `util()` spells no token of the file that makes it ambiguous: a
    // second `com.x.svc.Svc` under `src/test`. Only the glob's own tokens can
    // select the row for re-binding (`Index::ref_affected`).
    let tmp = single_module(
        "package com.x.web;\n\nimport static com.x.svc.Svc.*;\n\npublic class Ctl {\n    public String b() { return util(); }\n}\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        callers_of(rt, &format!("{SVC_FILE}:util:method")),
        [format!("{CTL_FILE}:b")]
    );
    let dup = "src/test/java/com/x/svc/Svc.java";
    write(
        tmp.path(),
        dup,
        "package com.x.svc;\n\npublic class Svc {}\n",
    );
    engine.sync(&[dup.into()]);
    // The ledger is re-decided exactly as a cold index decides it. (A row that
    // flips to unbound keeps the edge it committed — the resolution pass's
    // commit semantics for every edge kind, recorded at S-439/S-440 review — so
    // the comparison is over the ledger, not the edge set.)
    assert_eq!(
        binding_facts(rt).1,
        cold_facts(&tmp, &[SVC_FILE, CTL_FILE, dup]).1
    );
}
