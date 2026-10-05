//! Inheritance binds for Python, PHP, C# and Kotlin (S-522, [FR-RS-15],
//! [NFR-RA-05], [NFR-RA-06]) — exercised end to end through the public
//! [`Engine`] against real temp-directory fixtures (`type_relations/fixtures.rs`).
//!
//! Before S-522 a type relation bound only from a package-shaped source, and an
//! `Implements` outside one only to a Rust trait; no Python, PHP, C# or Kotlin
//! query captured a supertype at all. Now each language's supertypes are
//! captured and bind exactly-one through the module model it declares — the
//! path modules for Python, the declared namespace for PHP, C# and Kotlin.
//! Where the syntax leaves the kind unsaid (C#'s `base_list`, Kotlin's
//! supertype list), the edge kind follows the bound target. Library types stay
//! in the ledger, under every policy. Rust's `Implements` binds as before, and
//! Java's type relations are pinned by `tests/java_type_relations.rs`.
//!
//! [FR-RS-15]: ../../docs/specs/requirements/FR-RS-15.md
//! [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
//! [NFR-RA-06]: ../../docs/specs/requirements/NFR-RA-06.md
#![cfg(all(
    feature = "lang-python",
    feature = "lang-php",
    feature = "lang-c-sharp",
    feature = "lang-kotlin",
    feature = "lang-rust"
))]

#[path = "type_relations/fixtures.rs"]
mod fixtures;

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId, RefForm};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// `fixture` written into a fresh directory, under `policy` when given, and
/// indexed.
fn indexed_under(fixture: fixtures::Fixture, policy: Option<&str>) -> (TempDir, Engine) {
    let tmp = TempDir::new().unwrap();
    for (rel, source) in fixture {
        write(tmp.path(), rel, source);
    }
    if let Some(policy) = policy {
        write(
            tmp.path(),
            ".logos/config.toml",
            &format!("[resolution]\npolicy = \"{policy}\"\n"),
        );
    }
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    (tmp, engine)
}

fn indexed(fixture: fixtures::Fixture) -> (TempDir, Engine) {
    indexed_under(fixture, None)
}

/// Every edge of `kind`, as `source name -> target file:name:kind`, sorted.
fn edges(rt: &Runtime, kind: EdgeKind) -> Vec<String> {
    rt.submit_read(move |store| {
        let nodes: HashMap<NodeId, _> = store.all_nodes()?.into_iter().map(|n| (n.id, n)).collect();
        let mut out: Vec<String> = store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == kind)
            .map(|e| {
                let (s, t) = (&nodes[&e.source], &nodes[&e.target]);
                let file = t.file_path.as_deref().unwrap_or_default();
                format!("{} -> {file}:{}:{}", s.name, t.name, t.kind.as_str())
            })
            .collect();
        out.sort();
        Ok(out)
    })
    .expect("read runs")
}

/// The `Calls` edges out of methods named `caller`, as `target file:name`.
fn calls_from(rt: &Runtime, caller: &str) -> Vec<String> {
    let caller = caller.to_string();
    rt.submit_read(move |store| {
        let nodes: HashMap<NodeId, _> = store.all_nodes()?.into_iter().map(|n| (n.id, n)).collect();
        let mut out: Vec<String> = store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == EdgeKind::Calls && nodes[&e.source].name == caller)
            .map(|e| {
                let t = &nodes[&e.target];
                format!("{}:{}", t.file_path.as_deref().unwrap_or_default(), t.name)
            })
            .collect();
        out.sort();
        Ok(out)
    })
    .expect("read runs")
}

/// The ledger's unbound type-relation targets (`Extends`, `Implements`), as
/// `kind target`, sorted and deduplicated.
fn unbound_relations(rt: &Runtime) -> Vec<String> {
    let mut rows: Vec<String> = rt
        .submit_read(|store| {
            Ok(store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| matches!(r.kind, EdgeKind::Extends | EdgeKind::Implements))
                .filter(|r| r.form != RefForm::Symbol && !r.resolved)
                .map(|r| format!("{} {}", r.kind.as_str(), r.target))
                .collect())
        })
        .expect("read runs");
    rows.sort();
    rows.dedup();
    rows
}

/// Every edge as `(source symbol, target symbol, kind)` and every textual ledger
/// row, sorted — what a cold index and a sync must agree on.
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
            .map(|e| (sym[&e.source].clone(), sym[&e.target].clone(), e.kind.as_str().to_string()))
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
    .expect("read runs")
}

/// The facts a cold index of `fixture`, with `tmp`'s current contents, records.
fn cold(fixture: fixtures::Fixture, tmp: &TempDir) -> (Vec<(String, String, String)>, Vec<String>) {
    let fresh = TempDir::new().unwrap();
    for (rel, _) in fixture {
        write(fresh.path(), rel, &fs::read_to_string(tmp.path().join(rel)).unwrap());
    }
    let engine = Engine::start(fresh.path()).expect("engine starts");
    engine.index();
    binding_facts(engine.runtime().unwrap())
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

/// FR-RS-15 AC (werkzeug): `class Request(_SansIORequest)` — a base renamed by
/// a relative `as` import — carries `Extends` to the sans-IO `Request`, and the
/// converters to their one `BaseConverter`. A generic base binds by its name. A
/// library base (`t.Generic`), a metaclass keyword and a class the module
/// never imports stay unbound.
#[test]
fn a_python_class_extends_the_one_base_its_module_names() {
    let (_tmp, engine) = indexed(fixtures::WERKZEUG);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, EdgeKind::Extends),
        strings(&[
            "IntBox -> src/werkzeug/routing/converters.py:Box:class",
            "Request -> src/werkzeug/sansio/request.py:Request:class",
            "UUIDConverter -> src/werkzeug/routing/converters.py:BaseConverter:class",
            "UnicodeConverter -> src/werkzeug/routing/converters.py:BaseConverter:class",
        ])
    );
    assert_eq!(
        unbound_relations(rt),
        strings(&["extends Unimported", "extends t::Generic"])
    );
    assert!(edges(rt, EdgeKind::Implements).is_empty());
}

/// A Python base binds by scope and module only: under the aggressive policy a
/// class the module never imports is still no base, though it is the one
/// class of that name in the repository.
#[test]
fn a_python_base_never_binds_by_a_workspace_name_guess() {
    let (_tmp, engine) = indexed_under(fixtures::WERKZEUG, Some("aggressive"));
    let rt = engine.runtime().unwrap();
    assert!(
        !edges(rt, EdgeKind::Extends).iter().any(|e| e.starts_with("Orphan ")),
        "{:?}",
        edges(rt, EdgeKind::Extends)
    );
    assert!(unbound_relations(rt).contains(&"extends Unimported".to_string()));
}

/// With its base proven, a Python `super().close()` binds the base's `close`
/// and `self.get_data()` the inherited method — the hierarchy walk of FR-RS-12,
/// which no Python `Extends` reached before.
#[test]
fn a_python_super_and_self_call_climb_the_proven_base() {
    let (_tmp, engine) = indexed(fixtures::WERKZEUG);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        calls_from(rt, "close"),
        strings(&["src/werkzeug/sansio/request.py:close"])
    );
    assert_eq!(
        calls_from(rt, "data"),
        strings(&["src/werkzeug/sansio/request.py:get_data"])
    );
}

/// FR-RS-15 AC (healthchecks): each test case extends `hc.test.BaseTestCase`,
/// imported by name or reached through its module; `BaseTestCase`'s own base
/// is Django's and stays unbound.
#[test]
fn healthchecks_test_cases_extend_base_test_case() {
    let (_tmp, engine) = indexed(fixtures::HEALTHCHECKS);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, EdgeKind::Extends),
        strings(&[
            "BadgeTestCase -> hc/test.py:BaseTestCase:class",
            "PingTestCase -> hc/test.py:BaseTestCase:class",
        ])
    );
    assert_eq!(unbound_relations(rt), strings(&["extends TestCase"]));
    assert_eq!(calls_from(rt, "setUp"), strings(&["hc/test.py:setUp"]));
}

/// FR-RS-15 AC (monolog): the handlers are `HandlerInterface`'s incoming
/// `Implements` — directly, and through an aliased `use` — and a trait a class
/// `use`s is `Implements` to the trait. `extends` is `Extends`, class to class
/// and interface to interface, and `parent::close()` reaches the parent's.
/// `\Exception` is the global one, never the class that extends it, and a PSR
/// interface stays unbound.
#[test]
fn php_handlers_implement_the_interface_and_extend_the_abstract_handler() {
    let (_tmp, engine) = indexed(fixtures::MONOLOG);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, EdgeKind::Implements),
        strings(&[
            "AbstractHandler -> src/Monolog/Handler/HandlerInterface.php:HandlerInterface:interface",
            "AbstractHandler -> src/Monolog/Traits/LoggableTrait.php:LoggableTrait:trait",
            "NullHandler -> src/Monolog/Handler/HandlerInterface.php:HandlerInterface:interface",
        ])
    );
    assert_eq!(
        edges(rt, EdgeKind::Extends),
        strings(&[
            "FormattableHandlerInterface -> src/Monolog/Handler/HandlerInterface.php:HandlerInterface:interface",
            "StreamHandler -> src/Monolog/Handler/AbstractHandler.php:AbstractHandler:class",
        ])
    );
    assert_eq!(
        unbound_relations(rt),
        strings(&["extends \\::Exception", "implements LoggerInterface"])
    );
    assert_eq!(
        calls_from(rt, "close"),
        strings(&["src/Monolog/Handler/AbstractHandler.php:close"])
    );
}

/// FR-RS-15 AC (Newtonsoft): one `base_list` holds the base class and an
/// interface, and each entry's edge kind follows what it binds — `Extends` to
/// `JsonReader`, `Implements` to `IJsonLineInfo`. A struct implements, an
/// interface extends, a generic base binds by its name, and an alias and a
/// `global::` name reach the reader. `IDisposable` stays unbound.
#[test]
fn csharp_supertypes_take_the_kind_of_what_they_bind() {
    let (_tmp, engine) = indexed(fixtures::NEWTONSOFT);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, EdgeKind::Extends),
        strings(&[
            "AliasedReader -> Src/Newtonsoft.Json/JsonReader.cs:JsonReader:class",
            "IJsonPositionInfo -> Src/Newtonsoft.Json/IJsonLineInfo.cs:IJsonLineInfo:interface",
            "IntConverter -> Src/Newtonsoft.Json/JsonConverter.cs:JsonConverter:class",
            "JTokenReader -> Src/Newtonsoft.Json/JsonReader.cs:JsonReader:class",
            "JsonTextReader -> Src/Newtonsoft.Json/JsonReader.cs:JsonReader:class",
            "RootedReader -> Src/Newtonsoft.Json/JsonReader.cs:JsonReader:class",
        ])
    );
    assert_eq!(
        edges(rt, EdgeKind::Implements),
        strings(&[
            "JTokenReader -> Src/Newtonsoft.Json/IJsonLineInfo.cs:IJsonLineInfo:interface",
            "JsonTextReader -> Src/Newtonsoft.Json/IJsonLineInfo.cs:IJsonLineInfo:interface",
            "LineOnly -> Src/Newtonsoft.Json/IJsonLineInfo.cs:IJsonLineInfo:interface",
            "LinePosition -> Src/Newtonsoft.Json/IJsonLineInfo.cs:IJsonLineInfo:interface",
        ])
    );
    assert_eq!(unbound_relations(rt), strings(&["extends IDisposable"]));
}

/// `base.Close()` binds the base class's `Close`; a class whose one supertype
/// is an interface has no base class, so its `base.HasLineInfo()` binds nothing.
#[test]
fn a_csharp_base_call_reaches_the_base_class_never_an_interface() {
    let (_tmp, engine) = indexed(fixtures::NEWTONSOFT);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        calls_from(rt, "Close"),
        strings(&["Src/Newtonsoft.Json/JsonReader.cs:Close"])
    );
    assert!(
        !calls_from(rt, "HasLineInfo").iter().any(|c| c.ends_with(":HasLineInfo")),
        "{:?}",
        calls_from(rt, "HasLineInfo")
    );
}

/// Kotlin's supertype list: `Base()` is `Extends` and `Iface` `Implements`, a
/// delegated interface and an object implement, an interface extends one, and
/// a companion object's supertype is never its enclosing class's.
/// `super.start()` reaches the base class; `super<Iface>` names its supertype,
/// which the caller's hierarchy does not decide. An `as`-renamed base stays
/// unbound: Kotlin import rows record no local name yet (S-520 covered Python,
/// PHP, C# and Go), so `KBase` names nothing the file imports.
#[test]
fn kotlin_supertypes_take_the_kind_of_what_they_bind() {
    let (_tmp, engine) = indexed(fixtures::KOTLIN);
    let rt = engine.runtime().unwrap();
    let iface = "src/main/kotlin/org/koin/core/Iface.kt:Iface:interface";
    let base = "src/main/kotlin/org/koin/core/Base.kt:Base:class";
    assert_eq!(
        edges(rt, EdgeKind::Extends),
        vec![format!("Impl -> {base}"), format!("Sub -> {iface}")]
    );
    assert_eq!(
        edges(rt, EdgeKind::Implements),
        vec![
            format!("Deleg -> {iface}"),
            format!("Impl -> {iface}"),
            format!("Single -> {iface}"),
        ]
    );
    assert_eq!(
        calls_from(rt, "start"),
        strings(&["src/main/kotlin/org/koin/core/Base.kt:start"])
    );
    assert!(calls_from(rt, "both").is_empty(), "{:?}", calls_from(rt, "both"));
    assert_eq!(unbound_relations(rt), strings(&["extends KBase"]));
}

/// Rust's `Implements` binds as before — the impl method to its trait (S-281) —
/// beside a Kotlin interface of the trait's name, which a Kotlin class
/// implements without ever reaching the Rust trait (the interop family).
#[test]
fn rust_implements_binds_as_before_beside_a_same_named_interface() {
    let (_tmp, engine) = indexed(fixtures::RUST_BESIDE_KOTLIN);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, EdgeKind::Implements),
        strings(&[
            "K -> src/main/kotlin/org/koin/core/Iface.kt:Iface:interface",
            "run -> src/lib.rs:Iface:trait",
        ])
    );
}

/// Indexing a fixture twice records the same type relations ([NFR-RA-06]).
#[test]
fn indexing_twice_gives_identical_relations() {
    for fixture in [fixtures::WERKZEUG, fixtures::MONOLOG, fixtures::NEWTONSOFT, fixtures::KOTLIN] {
        let (tmp, engine) = indexed(fixture);
        assert_eq!(binding_facts(engine.runtime().unwrap()), cold(fixture, &tmp));
    }
}

/// Sync ≡ reindex ([NFR-RA-06]) when a Python base's file changes: the base
/// loses the method `self` and `super()` reach, is renamed away, and returns.
#[test]
fn sync_equals_a_full_reindex_when_a_python_base_changes() {
    let fixture = fixtures::WERKZEUG;
    let (tmp, engine) = indexed(fixture);
    let rt = engine.runtime().unwrap();
    let base = "src/werkzeug/sansio/request.py";
    let original = fixture.iter().find(|(rel, _)| *rel == base).unwrap().1;
    for edit in [
        "class Request:\n    def close(self):\n        pass\n".to_string(),
        "class Other:\n    def close(self):\n        pass\n".to_string(),
        original.to_string(),
    ] {
        write(tmp.path(), base, &edit);
        engine.sync(&[base.into()]);
        assert_eq!(binding_facts(rt), cold(fixture, &tmp), "after writing:\n{edit}");
    }
}

/// Sync ≡ reindex when a Python subclass gains and loses its base — the
/// hierarchy moves under calls in another file that spell none of its names.
#[test]
fn sync_equals_a_full_reindex_when_a_python_class_changes_its_base() {
    let fixture = fixtures::WERKZEUG;
    let (tmp, engine) = indexed(fixture);
    let rt = engine.runtime().unwrap();
    let sub = "src/werkzeug/wrappers/request.py";
    let original = fixture.iter().find(|(rel, _)| *rel == sub).unwrap().1;
    for edit in [
        original.replace("class Request(_SansIORequest):", "class Request:"),
        original.to_string(),
    ] {
        write(tmp.path(), sub, &edit);
        engine.sync(&[sub.into()]);
        assert_eq!(binding_facts(rt), cold(fixture, &tmp), "after writing:\n{edit}");
    }
}

/// Sync ≡ reindex when a C# supertype changes kind: the interface a class's
/// `base_list` names becomes a class (its edge turns `Extends`), and back.
///
/// The converters file is left out. Its `base.HasLineInfo()` binds while the
/// interface is a class, and when it turns back the call's capture-before-delete
/// row re-binds the edge by symbol, where a cold index has none. That is the
/// deferred incremental-retraction gap, reproduced on Java's own implicit call
/// before this change, not this capture.
#[test]
fn sync_equals_a_full_reindex_when_a_csharp_supertype_changes_kind() {
    let (converters, fixture) = fixtures::NEWTONSOFT.split_last().unwrap();
    assert_eq!(converters.0, "Src/Newtonsoft.Json/Converters/IntConverter.cs");
    let (tmp, engine) = indexed(fixture);
    let rt = engine.runtime().unwrap();
    let file = "Src/Newtonsoft.Json/IJsonLineInfo.cs";
    let original = fixture.iter().find(|(rel, _)| *rel == file).unwrap().1;
    assert!(edges(rt, EdgeKind::Implements).iter().any(|e| e.starts_with("JsonTextReader ")));
    for edit in [
        original.replace("public interface IJsonLineInfo", "public abstract class IJsonLineInfo"),
        original.to_string(),
    ] {
        write(tmp.path(), file, &edit);
        engine.sync(&[file.into()]);
        assert_eq!(binding_facts(rt), cold(fixture, &tmp), "after writing:\n{edit}");
    }
}

/// Sync ≡ reindex when a PHP trait is renamed away and back, and when the
/// abstract handler's file changes.
#[test]
fn sync_equals_a_full_reindex_when_a_php_supertype_changes() {
    let fixture = fixtures::MONOLOG;
    let (tmp, engine) = indexed(fixture);
    let rt = engine.runtime().unwrap();
    for (file, from, to) in [
        ("src/Monolog/Traits/LoggableTrait.php", "trait LoggableTrait", "trait OtherTrait"),
        ("src/Monolog/Handler/AbstractHandler.php", "implements HandlerInterface", ""),
    ] {
        let original = fixture.iter().find(|(rel, _)| *rel == file).unwrap().1;
        for edit in [original.replace(from, to), original.to_string()] {
            write(tmp.path(), file, &edit);
            engine.sync(&[file.into()]);
            assert_eq!(binding_facts(rt), cold(fixture, &tmp), "after writing:\n{edit}");
        }
    }
}
