//! Java type relations are captured and bound (S-466, CR-149 §3.2 B, FR-EX-05,
//! FR-EX-10, NFR-RA-05, NFR-RA-06) — exercised end-to-end through the public
//! [`Engine`] façade against real temp-directory fixtures.
//!
//! Before S-466 a Java store held no `Extends`, `Implements`, `Instantiates` or
//! `TypeUses` edge: `references.scm` captured calls, imports and `this.x` only.
//! Each fixture here pins one bound edge of each kind to a **unique
//! in-repository** type, reached through S-465's package key, with the
//! never-fabricate negatives beside it — the JDK, Kafka, Lombok and a generated
//! (Avro) type stay in `unresolved_refs`, even beside a same-named in-repository
//! type the file never imports.
//!
//! [FR-EX-05]: ../../docs/specs/requirements/FR-EX-05.md
//! [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md

#![cfg(all(feature = "lang-java", feature = "lang-rust"))]

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use logos_core::hydrate::{build_view, Granularity};
use logos_core::model::{EdgeKind, NodeId, NodeKind, RefForm};
use logos_core::Engine;
use logos_core::Runtime;
use tempfile::TempDir;

/// The four kinds this story captures and binds.
const TYPE_RELATIONS: [EdgeKind; 4] = [
    EdgeKind::Extends,
    EdgeKind::Implements,
    EdgeKind::Instantiates,
    EdgeKind::TypeUses,
];

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

/// The distinct ledger rows of `kind` whose source symbol lies in `file`:
/// `(target, form, resolved)`, sorted — one entry however many of the file's
/// declarations record it.
fn rows_of(rt: &Runtime, kind: EdgeKind, file: &str) -> Vec<(String, RefForm, bool)> {
    let needle = file.to_string();
    let mut rows: Vec<(String, RefForm, bool)> = rt
        .submit_read(move |store| {
            let files: HashMap<String, String> = store
                .all_nodes()?
                .into_iter()
                .map(|n| (n.symbol.as_str().to_string(), n.file_path.unwrap_or_default()))
                .collect();
            Ok(store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| r.kind == kind && r.form != RefForm::Symbol)
                .filter(|r| files.get(&r.source_symbol).is_some_and(|f| *f == needle))
                .map(|r| (r.target, r.form, r.resolved))
                .collect())
        })
        .expect("read runs");
    rows.sort_by(|a, b| (&a.0, a.1.as_i32(), a.2).cmp(&(&b.0, b.1.as_i32(), b.2)));
    rows.dedup();
    rows
}

const BASE_FILE: &str = "src/main/java/com/x/base/Base.java";
const PORT_FILE: &str = "src/main/java/com/x/base/Port.java";
const WIDE_FILE: &str = "src/main/java/com/x/base/Wide.java";
const DTO_FILE: &str = "src/main/java/com/x/svc/Dto.java";
const REQ_FILE: &str = "src/main/java/com/x/svc/Req.java";
const SVC_FILE: &str = "src/main/java/com/x/svc/Svc.java";

const BASE: &str = "package com.x.base;\n\npublic abstract class Base {}\n";
const PORT: &str = "package com.x.base;\n\npublic interface Port {}\n";
const WIDE: &str = "package com.x.base;\n\npublic interface Wide extends Port {}\n";
const DTO: &str = "package com.x.svc;\n\npublic class Dto {}\n";
const REQ: &str = "package com.x.svc;\n\npublic class Req {}\n";
/// One declaration per capture shape: a superclass and a super-interface named
/// through single-type imports, a field typed by a same-package class, a field
/// whose type ARGUMENT is that class, a parameter, a return type, a local and a
/// `new`.
const SVC: &str = "package com.x.svc;\n\
\n\
import com.x.base.Base;\n\
import com.x.base.Port;\n\
import java.util.List;\n\
\n\
public class Svc extends Base implements Port {\n\
    private Dto dto;\n\
    private List<Dto> all;\n\
    public Dto make(Req req) {\n\
        Dto made = new Dto();\n\
        return made;\n\
    }\n\
}\n";

/// The one-module fixture every positive test reads.
fn fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (rel, text) in [
        (BASE_FILE, BASE),
        (PORT_FILE, PORT),
        (WIDE_FILE, WIDE),
        (DTO_FILE, DTO),
        (REQ_FILE, REQ),
        (SVC_FILE, SVC),
    ] {
        write(tmp.path(), rel, text);
    }
    tmp
}

#[test]
fn a_class_extends_its_imported_superclass_and_an_interface_extends_its_super_interface() {
    let tmp = fixture();
    let engine = index(&tmp);
    assert_eq!(
        edges_of(engine.runtime().unwrap(), EdgeKind::Extends),
        [
            (
                format!("{WIDE_FILE}:Wide"),
                format!("{PORT_FILE}:Port:interface")
            ),
            (
                format!("{SVC_FILE}:Svc"),
                format!("{BASE_FILE}:Base:class")
            ),
        ]
    );
}

#[test]
fn a_class_implements_its_imported_interface() {
    let tmp = fixture();
    let engine = index(&tmp);
    assert_eq!(
        edges_of(engine.runtime().unwrap(), EdgeKind::Implements),
        [(
            format!("{SVC_FILE}:Svc"),
            format!("{PORT_FILE}:Port:interface")
        )]
    );
}

#[test]
fn a_method_instantiates_the_class_it_news() {
    let tmp = fixture();
    let engine = index(&tmp);
    assert_eq!(
        edges_of(engine.runtime().unwrap(), EdgeKind::Instantiates),
        [(
            format!("{SVC_FILE}:make"),
            format!("{DTO_FILE}:Dto:class")
        )]
    );
}

#[test]
fn a_declaration_type_uses_its_field_parameter_local_return_and_argument_types() {
    // The field `dto` and the field `all` (through `List<Dto>`'s argument) each
    // use `Dto`; `make` uses `Dto` (return and local — one row per distinct type
    // per declaration) and `Req` (parameter). `List` is the JDK's.
    let tmp = fixture();
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges_of(rt, EdgeKind::TypeUses),
        [
            (format!("{SVC_FILE}:all"), format!("{DTO_FILE}:Dto:class")),
            (format!("{SVC_FILE}:dto"), format!("{DTO_FILE}:Dto:class")),
            (format!("{SVC_FILE}:make"), format!("{DTO_FILE}:Dto:class")),
            (format!("{SVC_FILE}:make"), format!("{REQ_FILE}:Req:class")),
        ]
    );
    assert_eq!(
        rows_of(rt, EdgeKind::TypeUses, SVC_FILE),
        [
            ("Dto".to_string(), RefForm::Path, true),
            ("List".to_string(), RefForm::Path, false),
            ("Req".to_string(), RefForm::Path, true),
        ]
    );
}

/// A file whose every type relation names a type with no source here — the JDK,
/// Kafka, Lombok and an Avro-generated class — beside in-repository types of
/// the SAME simple names in another package (the near miss): a name match is
/// not the package key, so nothing binds, under any policy.
const PUB_FILE: &str = "src/main/java/com/x/svc/Pub.java";
const PUB: &str = "package com.x.svc;\n\
\n\
import org.apache.kafka.clients.producer.KafkaProducer;\n\
import lombok.Data;\n\
import com.x.avro.MailEvent;\n\
\n\
@Data\n\
public class Pub extends KafkaProducer<String, MailEvent> implements java.io.Serializable {\n\
    private String name;\n\
    private MailEvent last;\n\
    public void send() { Object o = new StringBuilder(); }\n\
}\n";

fn external_fixture(policy: Option<&str>) -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), PUB_FILE, PUB);
    write(
        tmp.path(),
        "src/main/java/com/y/KafkaProducer.java",
        "package com.y;\n\npublic class KafkaProducer<K, V> {}\n",
    );
    write(
        tmp.path(),
        "src/main/java/com/y/MailEvent.java",
        "package com.y;\n\npublic class MailEvent {}\n",
    );
    write(
        tmp.path(),
        "src/main/java/com/y/Serializable.java",
        "package com.y;\n\npublic interface Serializable {}\n",
    );
    write(
        tmp.path(),
        "src/main/java/com/y/StringBuilder.java",
        "package com.y;\n\npublic class StringBuilder {}\n",
    );
    if let Some(policy) = policy {
        write(
            tmp.path(),
            ".logos/config.toml",
            &format!("[resolution]\npolicy = \"{policy}\"\n"),
        );
    }
    tmp
}

#[test]
fn external_and_generated_types_stay_in_unresolved_refs_beside_same_named_in_repo_types() {
    for policy in [None, Some("aggressive")] {
        let tmp = external_fixture(policy);
        let engine = index(&tmp);
        let rt = engine.runtime().unwrap();
        for kind in TYPE_RELATIONS {
            let out_of_pub: Vec<_> = edges_of(rt, kind)
                .into_iter()
                .filter(|(s, _)| s.starts_with(PUB_FILE))
                .collect();
            assert!(out_of_pub.is_empty(), "{policy:?} {kind:?}: {out_of_pub:?}");
        }
        assert_eq!(
            rows_of(rt, EdgeKind::Extends, PUB_FILE),
            [("KafkaProducer".to_string(), RefForm::Path, false)],
            "{policy:?}"
        );
        assert_eq!(
            rows_of(rt, EdgeKind::Implements, PUB_FILE),
            [("java::io::Serializable".to_string(), RefForm::Path, false)],
            "{policy:?}"
        );
        assert_eq!(
            rows_of(rt, EdgeKind::Instantiates, PUB_FILE),
            [("StringBuilder".to_string(), RefForm::Path, false)],
            "{policy:?}"
        );
        // The Kafka type arguments are type uses of the class; `Object` is the
        // local's; `@Data` is an annotation, which this story does not capture.
        assert_eq!(
            rows_of(rt, EdgeKind::TypeUses, PUB_FILE),
            [
                ("MailEvent".to_string(), RefForm::Path, false),
                ("Object".to_string(), RefForm::Path, false),
                ("String".to_string(), RefForm::Path, false),
            ],
            "{policy:?}"
        );
    }
}

#[test]
fn a_relation_binds_only_a_type_of_the_kind_it_names() {
    // A class cannot extend an interface nor implement a class, and an
    // interface is not instantiated: a same-package type of the wrong kind is
    // not a candidate, so each row stays unresolved rather than bind it.
    let tmp = fixture();
    write(
        tmp.path(),
        "src/main/java/com/x/base/Odd.java",
        "package com.x.base;\n\npublic class Odd extends Port implements Base {\n    public Object run() { return new Port() {}; }\n}\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    let odd = "src/main/java/com/x/base/Odd.java";
    for kind in [EdgeKind::Extends, EdgeKind::Implements, EdgeKind::Instantiates] {
        assert!(
            edges_of(rt, kind).iter().all(|(s, _)| !s.starts_with(odd)),
            "{kind:?}: {:?}",
            edges_of(rt, kind)
        );
    }
    assert_eq!(
        rows_of(rt, EdgeKind::Extends, odd),
        [("Port".to_string(), RefForm::Path, false)]
    );
    assert_eq!(
        rows_of(rt, EdgeKind::Implements, odd),
        [("Base".to_string(), RefForm::Path, false)]
    );
    assert_eq!(
        rows_of(rt, EdgeKind::Instantiates, odd),
        [("Port".to_string(), RefForm::Path, false)]
    );
}

#[test]
fn a_type_parameter_is_not_a_type_use_even_beside_a_same_named_class() {
    // `T` and `E` name the declarations' own type variables; the same-package
    // classes `T` and `E` are other types, and the capture records no row.
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/main/java/com/x/svc/Box.java",
        "package com.x.svc;\n\npublic class Box<T> {\n    private T value;\n    public <E> E get(E e, T t) { return e; }\n}\n",
    );
    write(tmp.path(), "src/main/java/com/x/svc/T.java", "package com.x.svc;\n\npublic class T {}\n");
    write(tmp.path(), "src/main/java/com/x/svc/E.java", "package com.x.svc;\n\npublic class E {}\n");
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert!(edges_of(rt, EdgeKind::TypeUses).is_empty());
    assert!(rows_of(rt, EdgeKind::TypeUses, "src/main/java/com/x/svc/Box.java").is_empty());
}

#[test]
fn every_java_type_relation_row_is_path_form_never_method_form() {
    // A Method-form `::` target is the binder's trait-dispatch branch (S-281,
    // FR-RS-08); a Java type relation must never reach it.
    let tmp = fixture();
    write(tmp.path(), PUB_FILE, PUB);
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    let forms: Vec<(EdgeKind, RefForm)> = rt
        .submit_read(|store| {
            Ok(store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| TYPE_RELATIONS.contains(&r.kind) && r.form != RefForm::Symbol)
                .map(|r| (r.kind, r.form))
                .collect())
        })
        .expect("read runs");
    assert!(
        forms.len() >= 12,
        "the fixture records a row of every kind: {forms:?}"
    );
    for kind in TYPE_RELATIONS {
        assert!(forms.iter().any(|(k, _)| *k == kind), "no {kind:?} row");
    }
    assert!(
        forms.iter().all(|(_, f)| *f == RefForm::Path),
        "{forms:?}"
    );
}

#[test]
fn a_java_implements_never_binds_a_rust_trait_nor_joins_its_dyn_fan_out() {
    // The widened Implements arm is keyed by the package: a Java class that
    // names `Port` with no Java `Port` in scope must not reach the one Rust
    // trait `Port`, which the Rust trait rule alone would bind. Nor may its row
    // enter the `dyn Port` fan-out universe (S-281), where a class named like
    // the trait's method would become a `Calls` target of `p.go()`.
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        "pub trait Port { fn go(&self); }\npub struct S;\nimpl Port for S { fn go(&self) {} }\npub fn call(p: &dyn Port) { p.go(); }\n",
    );
    let java = "src/main/java/com/x/go.java";
    write(
        tmp.path(),
        java,
        "package com.x;\n\npublic class go implements Port {}\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges_of(rt, EdgeKind::Implements),
        [("src/lib.rs:go".to_string(), "src/lib.rs:Port:trait".to_string())],
        "only the Rust impl method binds its trait"
    );
    assert_eq!(
        rows_of(rt, EdgeKind::Implements, java),
        [("Port".to_string(), RefForm::Path, false)]
    );
    let fan_out: Vec<String> = edges_of(rt, EdgeKind::Calls)
        .into_iter()
        .filter(|(s, _)| s == "src/lib.rs:call")
        .map(|(_, t)| t)
        .collect();
    assert_eq!(fan_out, ["src/lib.rs:go:method"], "the Rust impl only");
}

#[test]
fn the_type_relations_are_fenced_out_of_the_dependency_view_and_kept_in_the_symbol_view() {
    // CR-149 §10's default: all four kinds are structural facts, fenced like
    // `Implements` and `Accesses` — the gated metrics' views are what they
    // would be without them, and the full symbol view keeps them navigable.
    let tmp = fixture();
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    let (nodes, edges) = rt
        .submit_read(|store| Ok((store.all_nodes()?, store.all_edges()?)))
        .expect("read runs");
    let relations = edges
        .iter()
        .filter(|e| TYPE_RELATIONS.contains(&e.kind))
        .count();
    assert_eq!(relations, 8, "the fixture's bound relations");
    let without: Vec<_> = edges
        .iter()
        .filter(|e| !TYPE_RELATIONS.contains(&e.kind))
        .cloned()
        .collect();
    for granularity in [
        Granularity::ExcludeContains,
        Granularity::File,
        Granularity::Module,
    ] {
        let with = build_view(granularity, &nodes, &edges);
        let bare = build_view(granularity, &nodes, &without);
        assert_eq!(
            (with.node_count(), with.edge_count()),
            (bare.node_count(), bare.edge_count()),
            "{granularity:?}"
        );
    }
    let symbol = build_view(Granularity::Symbol, &nodes, &edges);
    let symbol_bare = build_view(Granularity::Symbol, &nodes, &without);
    assert_eq!(symbol.edge_count(), symbol_bare.edge_count() + relations);
}

/// Every binding fact of the graph in an id-free form, for the sync ≡ reindex
/// comparison (NFR-RA-06) — the `tests/java_imports.rs` shape: every ledger row,
/// capture-before-delete rows (ADR-10) included (CR-187).
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

/// Index a fresh copy of every fixture file still present under `tmp`.
fn cold_facts(tmp: &TempDir) -> (Vec<(String, String, String)>, Vec<String>) {
    let cold = TempDir::new().unwrap();
    for rel in [BASE_FILE, PORT_FILE, WIDE_FILE, DTO_FILE, REQ_FILE, SVC_FILE] {
        if let Ok(text) = fs::read_to_string(tmp.path().join(rel)) {
            write(cold.path(), rel, &text);
        }
    }
    let engine = index(&cold);
    binding_facts(engine.runtime().unwrap())
}

fn relation_count(rt: &Runtime) -> usize {
    TYPE_RELATIONS.iter().map(|k| edges_of(rt, *k).len()).sum()
}

#[test]
fn sync_equals_a_full_reindex_after_adding_the_related_types() {
    // `Svc` exists first; every type it relates to arrives on sync.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), SVC_FILE, SVC);
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(relation_count(rt), 0, "precondition: nothing to bind yet");
    let added = [BASE_FILE, PORT_FILE, WIDE_FILE, DTO_FILE, REQ_FILE];
    for (rel, text) in added.iter().zip([BASE, PORT, WIDE, DTO, REQ]) {
        write(tmp.path(), rel, text);
    }
    engine.sync(&added.map(Into::into));
    assert_eq!(relation_count(rt), 8);
    assert_eq!(binding_facts(rt), cold_facts(&tmp));
}

#[test]
fn sync_equals_a_full_reindex_after_editing_the_relating_file() {
    let tmp = fixture();
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    // `Svc` drops its superclass and its field, and now also news a `Req`.
    write(
        tmp.path(),
        SVC_FILE,
        "package com.x.svc;\n\nimport com.x.base.Port;\n\npublic class Svc implements Port {\n    public Dto make(Req req) { return new Dto(); }\n    public Req again() { return new Req(); }\n}\n",
    );
    engine.sync(&[SVC_FILE.into()]);
    assert!(edges_of(rt, EdgeKind::Extends)
        .iter()
        .all(|(s, _)| !s.starts_with(SVC_FILE)));
    assert_eq!(
        edges_of(rt, EdgeKind::Instantiates),
        [
            (format!("{SVC_FILE}:again"), format!("{REQ_FILE}:Req:class")),
            (format!("{SVC_FILE}:make"), format!("{DTO_FILE}:Dto:class")),
        ]
    );
    assert_eq!(binding_facts(rt), cold_facts(&tmp));
}

#[test]
fn sync_equals_a_full_reindex_after_deleting_a_related_type() {
    let tmp = fixture();
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(relation_count(rt), 8, "precondition: every relation binds");
    fs::remove_file(tmp.path().join(DTO_FILE)).unwrap();
    engine.sync(&[DTO_FILE.into()]);
    for kind in TYPE_RELATIONS {
        assert!(
            edges_of(rt, kind).iter().all(|(_, t)| !t.starts_with(DTO_FILE)),
            "{kind:?}"
        );
    }
    assert_eq!(binding_facts(rt), cold_facts(&tmp));
}

#[test]
fn sync_equals_a_full_reindex_after_a_related_type_changes_kind() {
    // `Base` becomes an interface: `Svc extends Base` names a class no longer.
    // The edge a capture-before-delete row carries back must meet the same kind
    // rule a cold index applies, so it is not restored.
    let tmp = fixture();
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(edges_of(rt, EdgeKind::Extends).len(), 2, "precondition");
    write(
        tmp.path(),
        BASE_FILE,
        "package com.x.base;\n\npublic interface Base {}\n",
    );
    engine.sync(&[BASE_FILE.into()]);
    assert_eq!(
        edges_of(rt, EdgeKind::Extends),
        [(
            format!("{WIDE_FILE}:Wide"),
            format!("{PORT_FILE}:Port:interface")
        )]
    );
    assert_eq!(binding_facts(rt), cold_facts(&tmp));
}

/// The type-relation edges out of `file`, across the four kinds, as
/// `(kind, source, target)`, sorted.
fn relations_out_of(rt: &Runtime, file: &str) -> Vec<(EdgeKind, String, String)> {
    let mut out: Vec<(EdgeKind, String, String)> = TYPE_RELATIONS
        .iter()
        .flat_map(|k| {
            edges_of(rt, *k)
                .into_iter()
                .filter(|(s, _)| s.starts_with(file))
                .map(move |(s, t)| (*k, s, t))
        })
        .collect();
    out.sort_by(|a, b| (a.0.as_i32(), &a.1, &a.2).cmp(&(b.0.as_i32(), &b.1, &b.2)));
    out
}

#[test]
fn an_import_of_an_external_type_shadows_a_same_package_type_of_that_name() {
    // JLS §6.4.1: `import org.lib.Message` shadows the package's own `Message`,
    // and `import java.util.Map` its own `Map` — even though neither import
    // binds here. The package's types are the near miss (S-466 review).
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/main/java/com/x/Message.java", "package com.x;\n\npublic class Message {}\n");
    write(
        tmp.path(),
        "src/main/java/com/x/Map.java",
        "package com.x;\n\npublic class Map { public static class Entry {} }\n",
    );
    let handler = "src/main/java/com/x/Handler.java";
    write(
        tmp.path(),
        handler,
        "package com.x;\n\nimport org.lib.Message;\nimport java.util.Map;\n\npublic class Handler {\n    private Message msg;\n    private Map.Entry<String, String> e;\n    public void on() { Object o = new Message(); }\n}\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(relations_out_of(rt, handler), []);
    assert_eq!(
        rows_of(rt, EdgeKind::TypeUses, handler),
        [
            ("Map::Entry".to_string(), RefForm::Path, false),
            ("Message".to_string(), RefForm::Path, false),
            ("Object".to_string(), RefForm::Path, false),
            ("String".to_string(), RefForm::Path, false),
        ]
    );
    assert_eq!(
        rows_of(rt, EdgeKind::Instantiates, handler),
        [("Message".to_string(), RefForm::Path, false)]
    );
}

#[test]
fn a_declarations_header_never_names_its_own_member_type() {
    // JLS §6.3: a class's member types are in scope in its body, not in its
    // `extends`/`implements` clause. `Client implements Callback` is the
    // top-level interface, never `Client.Callback`; `Svc extends Base<Item>`'s
    // `Item` row reads the top-level `Item` in the header and `Svc.Item` in the
    // body, so it binds neither. A constructor parameter typed by a nested
    // `Builder` — a body use with nothing to shadow — still binds it.
    let tmp = TempDir::new().unwrap();
    let dir = "src/main/java/com/x";
    write(tmp.path(), &format!("{dir}/Item.java"), "package com.x;\n\npublic class Item {}\n");
    write(tmp.path(), &format!("{dir}/Base.java"), "package com.x;\n\npublic class Base<T> {}\n");
    write(tmp.path(), &format!("{dir}/Callback.java"), "package com.x;\n\npublic interface Callback {}\n");
    write(
        tmp.path(),
        &format!("{dir}/Svc.java"),
        "package com.x;\n\npublic class Svc extends Base<Item> { public static class Item {} }\n",
    );
    write(
        tmp.path(),
        &format!("{dir}/Client.java"),
        "package com.x;\n\npublic class Client implements Callback { public interface Callback {} }\n",
    );
    write(
        tmp.path(),
        &format!("{dir}/Foo.java"),
        "package com.x;\n\npublic class Foo {\n    public Foo(Builder b) {}\n    public static class Builder {}\n}\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        relations_out_of(rt, &format!("{dir}/Client.java")),
        [(
            EdgeKind::Implements,
            format!("{dir}/Client.java:Client"),
            format!("{dir}/Callback.java:Callback:interface")
        )]
    );
    assert_eq!(
        relations_out_of(rt, &format!("{dir}/Svc.java")),
        [(
            EdgeKind::Extends,
            format!("{dir}/Svc.java:Svc"),
            format!("{dir}/Base.java:Base:class")
        )]
    );
    assert_eq!(
        rows_of(rt, EdgeKind::TypeUses, &format!("{dir}/Svc.java")),
        [("Item".to_string(), RefForm::Path, false)]
    );
    assert_eq!(
        relations_out_of(rt, &format!("{dir}/Foo.java")),
        [(
            EdgeKind::TypeUses,
            format!("{dir}/Foo.java:Foo"),
            format!("{dir}/Foo.java:Builder:class")
        )]
    );
}

#[test]
fn a_qualified_type_name_reads_its_head_as_a_member_type_in_scope_first() {
    // JLS §6.5.5: `Inner.Deep` inside `Outer` is `Outer.Inner.Deep`, never the
    // same-package `Inner`'s `Deep`.
    let tmp = TempDir::new().unwrap();
    let outer = "src/main/java/com/x/Outer.java";
    write(
        tmp.path(),
        outer,
        "package com.x;\n\npublic class Outer {\n    static class Inner { static class Deep {} }\n    private Inner.Deep d;\n}\n",
    );
    write(
        tmp.path(),
        "src/main/java/com/x/Inner.java",
        "package com.x;\n\npublic class Inner { public static class Deep {} }\n",
    );
    let engine = index(&tmp);
    assert_eq!(
        relations_out_of(engine.runtime().unwrap(), outer),
        [(
            EdgeKind::TypeUses,
            format!("{outer}:d"),
            format!("{outer}:Deep:class")
        )]
    );
}

// ── Shapes the first fixtures left unpinned (S-466 review, surviving mutants) ──

#[test]
fn an_enum_and_a_record_implement_their_interface() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), PORT_FILE, PORT);
    let color = "src/main/java/com/x/svc/Color.java";
    let pt = "src/main/java/com/x/svc/Pt.java";
    write(
        tmp.path(),
        color,
        "package com.x.svc;\n\nimport com.x.base.Port;\n\npublic enum Color implements Port { RED }\n",
    );
    write(
        tmp.path(),
        pt,
        "package com.x.svc;\n\nimport com.x.base.Port;\n\npublic record Pt(int x) implements Port {}\n",
    );
    let engine = index(&tmp);
    assert_eq!(
        edges_of(engine.runtime().unwrap(), EdgeKind::Implements),
        [
            (format!("{color}:Color"), format!("{PORT_FILE}:Port:interface")),
            (format!("{pt}:Pt"), format!("{PORT_FILE}:Port:interface")),
        ]
    );
}

#[test]
fn an_interface_constants_type_and_an_annotated_type_argument_are_type_uses() {
    // `Dto DEFAULT = null;` in an interface is a `constant_declaration`, not a
    // field; `List<@NonNull Dto>` wraps its argument in an annotated type.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), DTO_FILE, DTO);
    let consts = "src/main/java/com/x/svc/Consts.java";
    let holder = "src/main/java/com/x/svc/Holder.java";
    write(
        tmp.path(),
        consts,
        "package com.x.svc;\n\npublic interface Consts {\n    Dto DEFAULT = null;\n}\n",
    );
    write(
        tmp.path(),
        holder,
        "package com.x.svc;\n\nimport java.util.List;\n\npublic class Holder {\n    private List<@NonNull Dto> xs;\n}\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        rows_of(rt, EdgeKind::TypeUses, consts),
        [("Dto".to_string(), RefForm::Path, true)]
    );
    assert_eq!(
        rows_of(rt, EdgeKind::TypeUses, holder),
        [
            ("Dto".to_string(), RefForm::Path, true),
            ("List".to_string(), RefForm::Path, false),
        ]
    );
}

#[test]
fn a_type_use_binds_an_interface_and_a_nested_type_but_never_a_same_named_method() {
    // `TypeUses` admits every type-like kind — an interface, a nested class
    // reached through the lexical chain — and nothing else: the method `Req()`
    // in `Svc`'s own scope is not the type `Req`.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), PORT_FILE, PORT);
    write(tmp.path(), REQ_FILE, REQ);
    let user = "src/main/java/com/x/svc/User.java";
    write(
        tmp.path(),
        user,
        "package com.x.svc;\n\nimport com.x.base.Port;\n\npublic class User {\n    static class Inner {}\n    private Port port;\n    private Inner inner;\n    void Req() {}\n    void use(Req r) {}\n}\n",
    );
    let engine = index(&tmp);
    assert_eq!(
        relations_out_of(engine.runtime().unwrap(), user),
        [
            (
                EdgeKind::TypeUses,
                format!("{user}:inner"),
                format!("{user}:Inner:class")
            ),
            (
                EdgeKind::TypeUses,
                format!("{user}:port"),
                format!("{PORT_FILE}:Port:interface")
            ),
            (
                EdgeKind::TypeUses,
                format!("{user}:use"),
                format!("{REQ_FILE}:Req:class")
            ),
        ]
    );
}

#[test]
fn a_wildcard_import_binds_a_relation_cold_and_on_sync() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        SVC_FILE,
        "package com.x.svc;\n\nimport com.x.base.*;\n\npublic class Svc implements Port {}\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert!(edges_of(rt, EdgeKind::Implements).is_empty(), "precondition");
    write(tmp.path(), PORT_FILE, PORT);
    engine.sync(&[PORT_FILE.into()]);
    let bound = [(
        format!("{SVC_FILE}:Svc"),
        format!("{PORT_FILE}:Port:interface"),
    )];
    assert_eq!(edges_of(rt, EdgeKind::Implements), bound);
    let cold = TempDir::new().unwrap();
    for rel in [SVC_FILE, PORT_FILE] {
        write(cold.path(), rel, &fs::read_to_string(tmp.path().join(rel)).unwrap());
    }
    let engine = index(&cold);
    assert_eq!(edges_of(engine.runtime().unwrap(), EdgeKind::Implements), bound);
}
