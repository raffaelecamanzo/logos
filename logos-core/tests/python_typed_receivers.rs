//! A Python call on an annotated or constructed receiver binds among its
//! type's methods (S-546, CR-175, FR-RS-24, FR-RS-10, FR-RS-12, NFR-RA-05) —
//! exercised end to end through the public [`Engine`] façade against
//! temp-directory fixtures, with the SHIPPED `references.scm` and
//! `plugin.toml` of the Python plugin (no query override).
//!
//! What the fixtures pin:
//!
//! - a receiver whose type the file proves — a parameter annotation
//!   (`svc: Service`, with or without a default), `Optional[T]`,
//!   `typing.Optional[T]`, `T | None`, `None | T` and a string annotation
//!   (`"T"`, `Optional["T"]`), which unwrap to `T`; a generic `G[A]`, which is
//!   a `G`; an annotated local; `x = T(…)` — is the Path-form `T::m` of shape
//!   `other`, `T` as the file writes it, and binds among `T`'s methods or at
//!   the nearest level of its proven `Extends` chain holding one;
//! - `self.x.m()` is typed by the field `x` the enclosing class declares
//!   itself — a class-level annotation, an annotated `self.x: T = …` write or
//!   a constructed `self.x = T(…)` — when every declaration of it in the class
//!   agrees ([FR-RS-24]'s `self.transport.notify()`);
//! - a receiver the file proves no type for — untyped, inferred, rebound by any
//!   binding form in scope, a module-level name read in a function, a dotted
//!   or other-union annotation, an inherited or untyped attribute — stays the
//!   Method-form row of shape `other`, which the binder binds nowhere
//!   (`no-receiver-evidence`);
//! - a type the repository does not declare binds nothing, and neither does a
//!   capitalised callee that is a function (their reasons, `external-type` and
//!   `no-receiver-evidence`, are pinned by the binder's `path_module_tests`).
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite (`typescript_typed_receivers.rs`): a fixture tree checked into
//! this repository would be indexed into its own graph.
//!
//! [FR-RS-24]: ../../docs/specs/requirements/FR-RS-24.md

#![cfg(feature = "lang-python")]

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::{EdgeKind, NodeId, ReceiverShape, RefForm};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

#[path = "support/graph_fingerprint.rs"]
mod graph_fingerprint;
use graph_fingerprint::graph_fingerprint;

#[path = "support/coordinate.rs"]
mod coordinate;

fn tree(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (rel, text) in files {
        let path = tmp.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    coordinate::pin(tmp.path());
    tmp
}

fn index(root: &Path) -> Engine {
    let engine = Engine::start(root).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    engine
}

/// A node's label: `file:name@line`.
fn labels(rt: &Runtime) -> HashMap<NodeId, String> {
    rt.submit_read(|store| {
        Ok(store
            .all_nodes()?
            .into_iter()
            .map(|n| {
                let file = n.file_path.unwrap_or_default();
                (n.id, format!("{file}:{}@{}", n.name, n.start_line.unwrap_or(0)))
            })
            .collect())
    })
    .expect("read runs")
}

/// Bound edges as `(source label, target label)`.
type Edges = Vec<(String, String)>;

/// Every bound `Calls` edge as `(source label, target label)`, sorted.
fn calls(rt: &Runtime) -> Edges {
    let label = labels(rt);
    let mut out: Edges = rt
        .submit_read(|store| store.all_edges())
        .expect("read runs")
        .into_iter()
        .filter(|e| e.kind == EdgeKind::Calls)
        .map(|e| (label[&e.source].clone(), label[&e.target].clone()))
        .collect();
    out.sort();
    out
}

/// The bound `Calls` edges sourced in `file`.
fn calls_from(rt: &Runtime, file: &str) -> Edges {
    calls(rt).into_iter().filter(|(from, _)| from.starts_with(&format!("{file}:"))).collect()
}

/// One ledger row: `(source label, target, form, receiver, resolved)`.
type Row = (String, String, RefForm, Option<ReceiverShape>, bool);

/// The `Calls` rows sourced in `file` whose target ends in one of `names`,
/// sorted.
fn rows(rt: &Runtime, file: &str, names: &[&str]) -> Vec<Row> {
    let label = labels(rt);
    let by_symbol: HashMap<String, String> = rt
        .submit_read(|store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .map(|n| (n.symbol.as_str().to_string(), label[&n.id].clone()))
                .collect())
        })
        .expect("read runs");
    let prefix = format!("{file}:");
    let mut rows: Vec<Row> = rt
        .submit_read(|store| store.unresolved_refs())
        .expect("read runs")
        .into_iter()
        .filter(|r| r.kind == EdgeKind::Calls && r.form != RefForm::Symbol)
        .filter(|r| names.iter().any(|n| r.target == *n || r.target.ends_with(&format!("::{n}"))))
        .filter_map(|r| {
            let label = by_symbol.get(&r.source_symbol)?;
            label
                .starts_with(&prefix)
                .then(|| (label.clone(), r.target, r.form, r.receiver, r.resolved))
        })
        .collect();
    rows.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    rows
}

fn edge(from: &str, to: &str) -> (String, String) {
    (from.to_string(), to.to_string())
}

const OTHER: Option<ReceiverShape> = Some(ReceiverShape::Other);

const SERVICES: &str = "\
class Base:
    def ping(self): return 1
class Service(Base):
    def run(self): return 0
class Transport:
    def notify(self, message): return message
class Repo:
    def find(self): return None
def make(): return None
";

const HANDLERS: &str = "\
import typing
from typing import Optional
from requests import Session
from app.services import Base, Repo, Service, make
class Mailer:
    def send(self, to): return to
def by_param(svc: Service): return svc.run()
def by_default(svc: Service = None): return svc.run()
def by_optional(svc: Optional[Service] = None): return svc.run()
def by_typing_optional(svc: typing.Optional[Service]): return svc.run()
def by_union(svc: Service | None): return svc.run()
def by_union_first(svc: None | Service): return svc.run()
def by_string(svc: \"Service\"): return svc.run()
def by_optional_string(svc: Optional[\"Service\"]): return svc.run()
def by_generic(repo: Repo[int]): return repo.find()
def by_local():
    s: Service = make()
    return s.run()
def by_constructed():
    s = Service()
    return s.run()
def by_base(svc: Service): return svc.ping()
def by_same_file(m: Mailer): return m.send(\"a\")
def by_external(session: Session): return session.get(\"x\")
def untyped(svc): return svc.run()
def by_widened():
    s: Base = Service()
    return s.ping()
def by_outer_param(svc: Service):
    def inner(svc): return svc
    return svc.run()
def by_outer_lambda(svc: Service):
    f = lambda svc: svc
    return svc.run()
def by_closure():
    made = Service()
    def inner():
        return made.run()
    return inner
class Box:
    held = Service()
    result = held.run()
";

const NAMES: &[&str] = &["run", "ping", "find", "send", "get"];

#[test]
fn a_call_on_an_annotated_or_constructed_receiver_binds_among_its_types_methods() {
    let tmp = tree(&[("app/__init__.py", ""), ("app/services.py", SERVICES), ("app/handlers.py", HANDLERS)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let h = |line: u32, name: &str| format!("app/handlers.py:{name}@{line}");
    let proven = |line: u32, name: &str, target: &str, resolved: bool| {
        (h(line, name), target.to_string(), RefForm::Path, OTHER, resolved)
    };
    assert_eq!(
        rows(rt, "app/handlers.py", NAMES),
        vec![
            // A class body's own call reads the class body's name.
            ("app/handlers.py:Box@40".to_string(), "Service::run".to_string(), RefForm::Path, OTHER, true),
            proven(22, "by_base", "Service::ping", true),
            proven(19, "by_constructed", "Service::run", true),
            proven(8, "by_default", "Service::run", true),
            proven(24, "by_external", "Session::get", false),
            proven(15, "by_generic", "Repo::find", true),
            proven(16, "by_local", "Service::run", true),
            proven(9, "by_optional", "Service::run", true),
            proven(14, "by_optional_string", "Service::run", true),
            proven(32, "by_outer_lambda", "Service::run", true),
            proven(29, "by_outer_param", "Service::run", true),
            proven(7, "by_param", "Service::run", true),
            proven(23, "by_same_file", "Mailer::send", true),
            proven(13, "by_string", "Service::run", true),
            proven(10, "by_typing_optional", "Service::run", true),
            proven(11, "by_union", "Service::run", true),
            proven(12, "by_union_first", "Service::run", true),
            // The annotation is the proof, not the value it is given.
            proven(26, "by_widened", "Base::ping", true),
            // A nested function reads its enclosing function's local.
            proven(37, "inner", "Service::run", true),
            (h(25, "untyped"), "run".to_string(), RefForm::Method, OTHER, false),
        ]
    );
    let edges = calls_from(rt, "app/handlers.py");
    let s = |line: u32, name: &str| format!("app/services.py:{name}@{line}");
    for (caller, line) in [
        ("by_param", 7),
        ("by_default", 8),
        ("by_optional", 9),
        ("by_typing_optional", 10),
        ("by_union", 11),
        ("by_union_first", 12),
        ("by_string", 13),
        ("by_optional_string", 14),
        ("by_local", 16),
        ("by_constructed", 19),
    ] {
        let expected = edge(&h(line, caller), &s(4, "run"));
        assert!(edges.contains(&expected), "missing {expected:?} in {edges:#?}");
    }
    for expected in [
        edge(&h(15, "by_generic"), &s(8, "find")),
        // Inheritance: `Service` declares no `ping`; its proven base does.
        edge(&h(22, "by_base"), &s(2, "ping")),
        edge(&h(23, "by_same_file"), &h(6, "send")),
    ] {
        assert!(edges.contains(&expected), "missing {expected:?} in {edges:#?}");
    }
    for caller in ["by_external", "untyped"] {
        assert!(
            !edges.iter().any(|(from, _)| from.contains(&format!(":{caller}@"))),
            "{caller} proves no in-repository type: {edges:#?}"
        );
    }
}

// ── Every annotation shape, in every position ──────────────────────────────

/// Each annotation shape the query reads down to its type, with that type and
/// the method a call on it names.
const SHAPES: [(&str, &str, &str); 9] = [
    ("Service", "Service", "run"),
    ("Repo[int]", "Repo", "find"),
    ("Optional[Service]", "Service", "run"),
    ("Optional[\"Service\"]", "Service", "run"),
    ("typing.Optional[Service]", "Service", "run"),
    ("typing.Optional[\"Service\"]", "Service", "run"),
    ("Service | None", "Service", "run"),
    ("None | Service", "Service", "run"),
    ("\"Service\"", "Service", "run"),
];

/// Every shape proves its type as a parameter, a defaulted parameter, an
/// annotated local and an annotated `self.x` write — the four places the query
/// reads an annotation, each its own pattern.
#[test]
fn every_annotation_shape_proves_its_type_in_every_position() {
    let mut src = String::from("import typing\nfrom typing import Optional\nfrom app.services import Repo, Service, make\n");
    for (i, (annotation, _, method)) in SHAPES.iter().enumerate() {
        src.push_str(&format!(
            "def param_{i}(s: {annotation}): return s.{method}()\n\
             def default_{i}(s: {annotation} = None): return s.{method}()\n\
             def local_{i}():\n    s: {annotation} = make()\n    return s.{method}()\n\
             class Attrs{i}:\n    def __init__(self):\n        self.s: {annotation} = None\n    def attr(self): return self.s.{method}()\n"
        ));
    }
    let tmp = tree(&[("app/__init__.py", ""), ("app/services.py", SERVICES), ("app/shapes.py", &src)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let found = rows(rt, "app/shapes.py", &["run", "find"]);
    assert!(found.iter().all(|r| (r.2, r.3, r.4) == (RefForm::Path, OTHER, true)), "{found:#?}");
    let mut expected: Vec<String> = SHAPES
        .iter()
        .flat_map(|(_, ty, method)| std::iter::repeat_n(format!("{ty}::{method}"), 4))
        .collect();
    expected.sort();
    let mut got: Vec<String> = found.into_iter().map(|r| r.1).collect();
    got.sort();
    assert_eq!(got, expected);
    let edges = calls_from(rt, "app/shapes.py");
    let to = |target: &str| edges.iter().filter(|(_, t)| t == target).count();
    assert_eq!((to("app/services.py:run@4"), to("app/services.py:find@8")), (32, 4), "{edges:#?}");
}

// ── `self.x` (FR-RS-24's `self.transport.notify()`) ─────────────────────────

const CHANNELS: &str = "\
from typing import Optional
from app.services import Base, Service, Transport, make
class Channel:
    transport: Transport
    mixed = make()
    dotted: \"app.services.Service\"
    def __init__(self):
        self.backup: Optional[Transport] = None
        self.service = Service()
        self.other = make()
        self.twice = Service()
        self.mixed = Service()
        self.pair = Service()
        self.paren = Service()
        self.written: \"app.services.Service\" = None
        self.widened: Base = Service()
    def reset(self):
        self.twice = make()
        self.pair, self.spare = make()
        (self.paren, self.spare) = make()
    def by_class_annotation(self): return self.transport.notify(\"a\")
    def by_annotated_write(self): return self.backup.notify(\"b\")
    def by_constructed_write(self): return self.service.run()
    def by_untyped_write(self): return self.other.run()
    def by_two_writes(self): return self.twice.run()
    def by_undeclared(self): return self.missing.run()
    def by_class_untyped(self): return self.mixed.run()
    def by_tuple_write(self): return self.pair.run()
    def by_paren_write(self): return self.paren.run()
    def by_dotted_class(self): return self.dotted.run()
    def by_dotted_write(self): return self.written.run()
    def by_widened_write(self): return self.widened.ping()
    def by_annotated_self(self: \"Channel\"): return self.reset()
    @classmethod
    def by_annotated_cls(cls: \"Channel\"): return cls.reset()
class Sub(Channel):
    def by_inherited(self): return self.service.run()
class Stranger:
    transport = make()
    a, b = make()
    (c, d) = make()
    def __init__(self):
        self.a = Service()
        self.c = Service()
    def by_class_value(self): return self.transport.notify(\"c\")
    def by_class_tuple(self): return self.a.run()
    def by_class_paren(self): return self.c.run()
def factory():
    class Local:
        transport: Transport
        def by_local_class(self): return self.transport.notify(\"d\")
    return Local
class WithProperty:
    svc: Service
    @property
    def svc(self): return make()
    def by_property(self): return self.svc.run()
class WithMethod:
    svc: Service
    def svc(self): return make()
    def by_method(self): return self.svc.run()
class WithInner:
    svc: Service
    class svc: pass
    def by_inner_class(self): return self.svc.run()
";

/// `self.x` is typed by the attributes the enclosing class declares itself —
/// a class-level annotation, an annotated or constructed write — when every
/// declaration of it in the class agrees; an untyped, inherited, undeclared,
/// dotted or disagreeing one proves nothing, and an annotated `self`/`cls` keeps
/// its own shape.
#[test]
fn an_annotated_or_constructed_attribute_binds_and_an_unproven_one_stays_unbound() {
    let tmp = tree(&[("app/__init__.py", ""), ("app/services.py", SERVICES), ("app/channels.py", CHANNELS)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let c = |line: u32, name: &str| format!("app/channels.py:{name}@{line}");
    let s = |line: u32, name: &str| format!("app/services.py:{name}@{line}");
    assert_eq!(
        calls_from(rt, "app/channels.py")
            .into_iter()
            .filter(|(_, to)| !to.ends_with(":make@9") && !to.ends_with(":Service@3"))
            .collect::<Edges>(),
        vec![
            edge(&c(35, "by_annotated_cls"), &c(17, "reset")),
            edge(&c(33, "by_annotated_self"), &c(17, "reset")),
            edge(&c(22, "by_annotated_write"), &s(6, "notify")),
            edge(&c(21, "by_class_annotation"), &s(6, "notify")),
            edge(&c(23, "by_constructed_write"), &s(4, "run")),
            // A class declared in a function: its body's declarations sit
            // under a callable, and are its attributes all the same.
            edge(&c(51, "by_local_class"), &s(6, "notify")),
            edge(&c(32, "by_widened_write"), &s(2, "ping")),
        ]
    );
    let own = |line: u32, name: &str| (c(line, name), "reset".to_string(), RefForm::Method, Some(ReceiverShape::SelfInstance), true);
    let proven = |line: u32, name: &str, target: &str| (c(line, name), target.to_string(), RefForm::Path, OTHER, true);
    let untyped = |line: u32, name: &str, target: &str| (c(line, name), target.to_string(), RefForm::Method, OTHER, false);
    assert_eq!(
        rows(rt, "app/channels.py", &["run", "notify", "ping", "reset"]),
        vec![
            own(35, "by_annotated_cls"),
            own(33, "by_annotated_self"),
            proven(22, "by_annotated_write", "Transport::notify"),
            proven(21, "by_class_annotation", "Transport::notify"),
            untyped(47, "by_class_paren", "run"),
            untyped(46, "by_class_tuple", "run"),
            untyped(27, "by_class_untyped", "run"),
            untyped(45, "by_class_value", "notify"),
            proven(23, "by_constructed_write", "Service::run"),
            untyped(30, "by_dotted_class", "run"),
            untyped(31, "by_dotted_write", "run"),
            untyped(37, "by_inherited", "run"),
            untyped(65, "by_inner_class", "run"),
            proven(51, "by_local_class", "Transport::notify"),
            untyped(61, "by_method", "run"),
            untyped(29, "by_paren_write", "run"),
            // A `def` of the class body — a property — is what `self.svc` reads.
            untyped(57, "by_property", "run"),
            untyped(28, "by_tuple_write", "run"),
            untyped(25, "by_two_writes", "run"),
            untyped(26, "by_undeclared", "run"),
            untyped(24, "by_untyped_write", "run"),
            // The annotation is the proof, not the value it is given.
            proven(32, "by_widened_write", "Base::ping"),
        ]
    );
}

// ── What the file does not prove ────────────────────────────────────────────

/// Every call below names `run` on a receiver the file proves no type for,
/// though a plain reading would hand it one: a proven binding of the same name
/// sits beside another binding in scope, at module level, or behind an
/// annotation that is no single type.
const UNPROVEN: &str = "\
import app.services
from typing import Annotated, ClassVar, Final, Union
from app.services import Service, make
top: Service = make()
shadowed: Service = make()
def shadowed(): return None
shadowed.run()
decorated: Service = make()
@staticmethod
def decorated(): return None
decorated.run()
def untyped(svc): return svc.run()
def inferred():
    s = make()
    return s.run()
def rebound(svc: Service):
    svc = make()
    return svc.run()
def augmented(svc: Service):
    svc += make()
    return svc.run()
def looped(svc: Service, xs):
    for svc in xs: svc.run()
def with_as(svc: Service, ctx):
    with ctx as svc: svc.run()
def caught(svc: Service):
    try: make()
    except Exception as svc: svc.run()
def walrus(svc: Service):
    if (svc := make()): svc.run()
def comprehension(svc: Service, xs): return [svc.run() for svc in xs]
def nested_def(svc: Service):
    def svc(): return None
    return svc.run()
def nested_decorated(svc: Service):
    @staticmethod
    def svc(): return None
    return svc.run()
def nested_class(svc: Service):
    class svc: pass
    return svc.run()
def imported(svc: Service):
    import svc
    return svc.run()
def imported_as(svc: Service):
    import os as svc
    return svc.run()
def imported_from(svc: Service):
    from app import svc
    return svc.run()
def imported_from_as(svc: Service):
    from app import services as svc
    return svc.run()
def matched(svc: Service, value):
    match value:
        case svc: return svc.run()
def tuple_target(svc: Service, pair):
    svc, other = pair
    return svc.run()
def inner_param(svc: Service):
    def inner(svc): return svc.run()
    return inner
def inner_typed(svc: Service):
    def inner(svc: int): return svc.run()
    return inner
def inner_default(svc: Service):
    def inner(svc=None): return svc.run()
    return inner
def inner_typed_default(svc: Service):
    def inner(svc: int = 0): return svc.run()
    return inner
def inner_star(svc: Service):
    def inner(*svc): return svc.run()
    return inner
def inner_kwargs(svc: Service):
    def inner(**svc): return svc.run()
    return inner
def inner_typed_star(svc: Service):
    def inner(*svc: int): return svc.run()
    return inner
def lambda_param(svc: Service): return (lambda svc: svc.run())(make())
def lambda_default(svc: Service): return (lambda svc=None: svc.run())()
def lambda_star(svc: Service): return (lambda *svc: svc.run())()
def lambda_kwargs(svc: Service): return (lambda **svc: svc.run())()
def module_level(): return top.run()
def dotted(svc: app.services.Service): return svc.run()
def dotted_generic(svc: app.services.Repo[Service]): return svc.run()
def other_union(svc: Union[Service, None]): return svc.run()
def wide_union(svc: Service | int): return svc.run()
def dotted_string(svc: \"app.services.Service\"): return svc.run()
def annotated(svc: Annotated[Service, 0]): return svc.run()
def class_var(svc: ClassVar[Service]): return svc.run()
def final(svc: Final[Service]): return svc.run()
def other_union_default(svc: Union[Service, None] = None): return svc.run()
def class_var_local():
    s: ClassVar[Service] = make()
    return s.run()
def lower_constructor():
    s = make()
    return s.run()
def siblings():
    def setup():
        sibling = Service()
        return sibling
    def use():
        return sibling.run()
    return setup, use
def class_in_function():
    class Holder:
        held = Service()
        def handle(self):
            return held.run()
    return Holder
def in_default(svc: Service, other=svc.run()): return other
def reads_global():
    global made_global
    made_global.run()
    made_global = Service()
def rebinds_nonlocal():
    shared = Service()
    def reset():
        nonlocal shared
        shared = make()
    reset()
    return shared.run()
";

#[test]
fn a_receiver_the_file_proves_no_type_for_binds_nowhere() {
    let tmp = tree(&[("app/__init__.py", ""), ("app/services.py", SERVICES), ("app/unproven.py", UNPROVEN)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let receiver_calls = rows(rt, "app/unproven.py", &["run"]);
    // 48 calls: the module's two (`shadowed.run()`, `decorated.run()`) are one
    // row of one source and target — a typed one would be a row of its own.
    // A sibling function's local and a class body's name are in scope in no
    // other function, and a parameter is not in scope in the defaults beside
    // it, which the enclosing scope evaluates. A `global` or `nonlocal` name
    // proves nothing anywhere.
    assert_eq!(receiver_calls.len(), 47, "{receiver_calls:#?}");
    let typed: Vec<&Row> = receiver_calls
        .iter()
        .filter(|r| (&r.1[..], r.2, r.3, r.4) != ("run", RefForm::Method, OTHER, false))
        .collect();
    assert!(typed.is_empty(), "every receiver here must stay untyped: {typed:#?}");
    let edges = calls(rt);
    assert!(!edges.iter().any(|(_, to)| to.ends_with(":run@4")), "nothing here proves a type: {edges:#?}");
}

// ── A capitalised callee that is a function ─────────────────────────────────

const FACTORY: &str = "\
def Factory(): return None
def untyped(svc): return svc.run()
def by_factory():
    s = Factory()
    return s.run()
";

/// `x = T(…)` is read as a construction when `T` is capitalised; the binder
/// finds `Factory` is a function, and the typed row binds nothing (its reason,
/// `no-receiver-evidence`, is pinned by the binder's own tests).
#[test]
fn a_capitalised_callee_that_is_a_function_proves_nothing() {
    let tmp = tree(&[("app/__init__.py", ""), ("app/factory.py", FACTORY)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        rows(rt, "app/factory.py", &["run"]),
        vec![
            ("app/factory.py:by_factory@3".to_string(), "Factory::run".to_string(), RefForm::Path, OTHER, false),
            ("app/factory.py:untyped@2".to_string(), "run".to_string(), RefForm::Method, OTHER, false),
        ]
    );
    assert_eq!(calls_from(rt, "app/factory.py"), vec![edge("app/factory.py:by_factory@3", "app/factory.py:Factory@1")]);
}

// ── An attribute hides the method of its name ───────────────────────────────

const SHADOW: &str = "\
from app.services import Service
class Aliased(Service):
    def other(self): return None
    run = other
class Assigned(Service):
    def __init__(self, cb):
        self.run = cb
class Own(Service):
    def run(self): return 1
def through_alias(a: Aliased): return a.run()
def through_attribute(a: Assigned): return a.run()
def through_own(a: Own): return a.run()
";

const REDECLARED: &str = "\
from app.services import Service
class Service(Service):
    def run(self): return 2
def through_redeclared(svc: Service): return svc.run()
";

/// Python keeps attributes and methods in one namespace: `run = other` in a
/// class body, or `self.run = cb`, is what `a.run()` calls, never the `run` its
/// base declares. A class's own method of the name still binds.
#[test]
fn an_attribute_of_the_name_hides_the_method_its_base_declares() {
    let tmp = tree(&[("app/__init__.py", ""), ("app/services.py", SERVICES), ("app/shadow.py", SHADOW)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        calls_from(rt, "app/shadow.py"),
        vec![edge("app/shadow.py:through_own@12", "app/shadow.py:run@9")]
    );
    let typed: Vec<(String, bool)> = rows(rt, "app/shadow.py", &["run"]).into_iter().map(|r| (r.1, r.4)).collect();
    assert_eq!(
        typed,
        [("Aliased::run", false), ("Assigned::run", false), ("Own::run", true)].map(|(t, b)| (t.to_string(), b))
    );
}

/// `class Service(Service)` beside `from app.services import Service`: which
/// `Service` an annotation names is the statements' order, so the call binds
/// neither.
#[test]
fn a_type_the_file_imports_and_redeclares_binds_neither() {
    let tmp = tree(&[("app/__init__.py", ""), ("app/services.py", SERVICES), ("app/redeclared.py", REDECLARED)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(
        !calls_from(rt, "app/redeclared.py").iter().any(|(from, _)| from.contains(":through_redeclared@")),
        "{:#?}",
        calls_from(rt, "app/redeclared.py")
    );
    let typed: Vec<(String, bool)> = rows(rt, "app/redeclared.py", &["run"]).into_iter().map(|r| (r.1, r.4)).collect();
    assert_eq!(typed, [("Service::run".to_string(), false)]);
}

// ── Sync ≡ reindex ───────────────────────────────────────────────────────────

/// Index `initial`, overwrite `edits` and sync them; then index the post-edit
/// tree from scratch. The two graphs must be identical (NFR-RA-06); the synced
/// one's `Calls` edges are returned.
fn synced_equals_reindexed(initial: &[(&str, &str)], edits: &[(&str, &str)]) -> Edges {
    let tmp_a = tree(initial);
    let engine_a = index(tmp_a.path());
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    let mut changed: Vec<PathBuf> = Vec::new();
    for (rel, text) in edits {
        let path = tmp_a.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
        changed.push(root_a.join(rel));
    }
    engine_a.sync(&changed);
    let rt_a = engine_a.runtime().unwrap();

    let mut final_state: BTreeMap<&str, &str> = initial.iter().copied().collect();
    final_state.extend(edits.iter().copied());
    let files: Vec<(&str, &str)> = final_state.into_iter().collect();
    let tmp_b = tree(&files);
    let engine_b = index(tmp_b.path());

    assert_eq!(
        graph_fingerprint(rt_a),
        graph_fingerprint(engine_b.runtime().unwrap()),
        "sync-to-state must equal index-of-state"
    );
    calls(rt_a)
}

const CALLER: &str = "\
from app.services import Service
class Caller:
    def __init__(self):
        self.service = Service()
    def go(self): return self.service.ping()
";

#[test]
fn a_synced_edit_to_the_receivers_type_matches_a_fresh_reindex() {
    let go = "app/caller.py:go@5";
    let without = "class Base:\n    pass\nclass Service(Base):\n    pass\n";
    let with_base = "class Base:\n    def ping(self): return 1\nclass Service(Base):\n    pass\n";
    let ping = "app/services.py:ping@2";
    let initial = [("app/__init__.py", ""), ("app/services.py", with_base), ("app/caller.py", CALLER)];
    let from_go = |edges: Edges| edges.into_iter().filter(|(from, _)| from == go).collect::<Edges>();

    // The base's method is renamed away: the edge goes, though the caller's
    // file never changes.
    let renamed = with_base.replace("def ping", "def pong");
    assert!(from_go(synced_equals_reindexed(&initial, &[("app/services.py", &renamed)])).is_empty());

    // The type arrives after the caller: the call binds on sync.
    let edges = synced_equals_reindexed(
        &[("app/__init__.py", ""), ("app/caller.py", CALLER)],
        &[("app/services.py", with_base)],
    );
    assert_eq!(from_go(edges), vec![edge(go, ping)]);

    // A base class gains the method its subclass is typed by.
    let edges = synced_equals_reindexed(
        &[("app/__init__.py", ""), ("app/services.py", without), ("app/caller.py", CALLER)],
        &[("app/services.py", with_base)],
    );
    assert_eq!(from_go(edges), vec![edge(go, ping)]);

    // The type is renamed away under its import: the call binds nothing.
    let gone = with_base.replace("class Service", "class Other");
    assert!(from_go(synced_equals_reindexed(&initial, &[("app/services.py", &gone)])).is_empty());

    // A base two modules away gains the base that declares the method: the
    // row spells no name the edited file or its imports spell (top-level
    // modules, no shared package), so the hierarchy alone re-selects it.
    let caller = CALLER.replace("from app.services import", "from svc import");
    let edges = synced_equals_reindexed(
        &[
            ("base.py", "class Base:\n    def ping(self): return 1\n"),
            ("mid.py", "class Mid:\n    pass\n"),
            ("svc.py", "from mid import Mid\nclass Service(Mid):\n    pass\n"),
            ("caller.py", &caller),
        ],
        &[("mid.py", "from base import Base\nclass Mid(Base):\n    pass\n")],
    );
    let go = "caller.py:go@5";
    assert_eq!(edges.into_iter().filter(|(from, _)| from == go).collect::<Edges>(), vec![edge(go, "base.py:ping@2")]);
}
