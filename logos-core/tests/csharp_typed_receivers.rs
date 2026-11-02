//! A C# call on a receiver of declared type binds among that type's members
//! (S-558, CR-177, FR-RS-30, FR-RS-10, FR-RS-12, NFR-RA-05) — exercised end
//! to end through the public [`Engine`] façade against temp-directory
//! fixtures, with the SHIPPED `references.scm` and `plugin.toml` of the C#
//! plugin (no query override).
//!
//! What the fixture pins, over methods of `Shop.Api.Handler` calling the
//! `Shop.Domain` types through a `using`:
//!
//! - a receiver whose type the file declares — a field (`_orderService`), a
//!   nullable field (`Order? _maybe`), a property read through `this.`, a
//!   parameter, a local, `var x = new T()`, a typed `foreach` variable and a
//!   primary-constructor parameter — is the Path-form `T::m`, bound among
//!   `T`'s members: an interface's own, and a base class's through the
//!   proven hierarchy (`SpecialOrder` → `Order.Ship`);
//! - a receiver the file proves no type for — `var x = Make()`, an untyped
//!   lambda parameter, a `var` loop variable, a deconstructed name, a name
//!   declared with a qualified type, whose simple name the `using`s may
//!   re-qualify to another type — stays the Method-form row of shape
//!   `other`, bound nowhere (`no-receiver-evidence`), though the file declares
//!   a same-named field whose type it does prove.
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite (`csharp_call_forms.rs`): a fixture tree checked into this
//! repository would be indexed into its own graph.

#![cfg(feature = "lang-c-sharp")]

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::{EdgeKind, NodeId, ReceiverShape, RefForm};
use logos_core::models::CallResidueReason;
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
    let result = engine.pipeline().index();
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

/// One ledger row: `(source label, target, form, receiver, resolved)`.
type Row = (String, String, RefForm, Option<ReceiverShape>, bool);

/// The `Calls` rows sourced in `file`, sorted.
fn rows(rt: &Runtime, file: &str) -> Vec<Row> {
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
    let rows: Vec<Row> = rt
        .submit_read(|store| store.unresolved_refs())
        .expect("read runs")
        .into_iter()
        .filter(|r| r.kind == EdgeKind::Calls && r.form != RefForm::Symbol)
        .filter_map(|r| {
            let label = by_symbol.get(&r.source_symbol)?;
            label
                .starts_with(&prefix)
                .then(|| (label.clone(), r.target, r.form, r.receiver, r.resolved))
        })
        .collect();
    sorted(rows)
}

fn row(source: &str, target: &str, form: RefForm, receiver: Option<ReceiverShape>, resolved: bool) -> Row {
    (source.to_string(), target.to_string(), form, receiver, resolved)
}

fn sorted(mut rows: Vec<Row>) -> Vec<Row> {
    rows.sort_by(|a, b| {
        (&a.0, &a.1, a.2.as_i32(), a.3.map(ReceiverShape::as_i32))
            .cmp(&(&b.0, &b.1, b.2.as_i32(), b.3.map(ReceiverShape::as_i32)))
    });
    rows
}

fn edge(from: &str, to: &str) -> (String, String) {
    (from.to_string(), to.to_string())
}

const OTHER: Option<ReceiverShape> = Some(ReceiverShape::Other);
const SELF: Option<ReceiverShape> = Some(ReceiverShape::SelfInstance);

const DOMAIN: &str = "\
namespace Shop.Domain {
    public interface IOrderService { void AddOrderItem(int id); }
    public class Order {
        public void AddOrderItem(int id) { }
        public void Ship() { }
    }
    public class SpecialOrder : Order { }
    public class Mailer { public void Send() { } public bool Ok() { return true; } }
}
";

const HANDLER: &str = "\
using System.Collections.Generic;
using Shop.Domain;
namespace Shop.Api {
    public class Handler {
        private readonly IOrderService _orderService;
        public Order Current { get; set; }
        private Order? _maybe;
        private Mailer x;
        private Mailer m;
        private Mailer it;
        private Mailer unused;
        public void ByField() { _orderService.AddOrderItem(1); }
        public void ByProperty() { this.Current.Ship(); }
        public void ByNullable() { _maybe.Ship(); }
        public void ByParameter(Order order) { order.Ship(); }
        public void ByLocal() { Order o = Make(); o.Ship(); }
        public void ByNew() { var made = new Order(); made.AddOrderItem(2); }
        public void ByBase(SpecialOrder s) { s.Ship(); }
        public void ByLoop(List<Order> xs) { foreach (Order each in xs) { each.Ship(); } }
        public void Untyped() { var got = Make(); got.Ship(); }
        public void Lambda(List<Order> xs) { xs.ForEach(x => x.Ship()); }
        public void VarLoop(List<Order> xs) { foreach (var m in xs) { m.Ship(); } }
        public void Tuple() { var (it, n) = Pair(); it.Ship(); }
        public void Qualified(Shop.Domain.Order u) { u.Ship(); }
        Order Make() { return null; }
        (Order, int) Pair() { return (null, 0); }
    }
    public class Primary(IOrderService service) {
        public void Run() { service.AddOrderItem(3); }
    }
}
";

#[test]
fn a_call_on_a_receiver_of_declared_type_binds_among_that_types_members() {
    let tmp = tree(&[("src/Domain.cs", DOMAIN), ("src/Handler.cs", HANDLER)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let h = |line: u32, name: &str| format!("src/Handler.cs:{name}@{line}");
    let d = |line: u32, name: &str| format!("src/Domain.cs:{name}@{line}");
    let path = |source: String, target: &str| row(&source, target, RefForm::Path, None, true);
    let untyped = |source: String, name: &str| row(&source, name, RefForm::Method, OTHER, false);
    assert_eq!(
        rows(rt, "src/Handler.cs"),
        sorted(vec![
            path(h(12, "ByField"), "IOrderService::AddOrderItem"),
            path(h(13, "ByProperty"), "Order::Ship"),
            path(h(14, "ByNullable"), "Order::Ship"),
            path(h(15, "ByParameter"), "Order::Ship"),
            row(&h(16, "ByLocal"), "Make", RefForm::Method, SELF, true),
            path(h(16, "ByLocal"), "Order::Ship"),
            path(h(17, "ByNew"), "Order::AddOrderItem"),
            path(h(18, "ByBase"), "SpecialOrder::Ship"),
            path(h(19, "ByLoop"), "Order::Ship"),
            row(&h(20, "Untyped"), "Make", RefForm::Method, SELF, true),
            untyped(h(20, "Untyped"), "Ship"),
            // `List<Order>` is a type, though no file here declares it.
            row(&h(21, "Lambda"), "List::ForEach", RefForm::Path, None, false),
            untyped(h(21, "Lambda"), "Ship"),
            untyped(h(22, "VarLoop"), "Ship"),
            row(&h(23, "Tuple"), "Pair", RefForm::Method, SELF, true),
            untyped(h(23, "Tuple"), "Ship"),
            untyped(h(24, "Qualified"), "Ship"),
            path(h(29, "Run"), "IOrderService::AddOrderItem"),
        ])
    );
    let edges = calls(rt);
    let add_interface = d(2, "AddOrderItem");
    let add_class = d(4, "AddOrderItem");
    let ship = d(5, "Ship");
    for expected in [
        edge(&h(12, "ByField"), &add_interface),
        edge(&h(13, "ByProperty"), &ship),
        edge(&h(14, "ByNullable"), &ship),
        edge(&h(15, "ByParameter"), &ship),
        edge(&h(16, "ByLocal"), &ship),
        edge(&h(17, "ByNew"), &add_class),
        // Inheritance: `SpecialOrder` declares no `Ship`; its base does.
        edge(&h(18, "ByBase"), &ship),
        edge(&h(19, "ByLoop"), &ship),
        edge(&h(29, "Run"), &add_interface),
    ] {
        assert!(edges.contains(&expected), "missing {expected:?} in {edges:#?}");
    }
    // Nothing reaches `Mailer`: every same-named field is another variable.
    assert!(!edges.iter().any(|(_, to)| to.contains("Send")), "{edges:#?}");
}

#[test]
fn impact_of_the_called_method_lists_every_typed_caller() {
    let tmp = tree(&[("src/Domain.cs", DOMAIN), ("src/Handler.cs", HANDLER)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let callers: Vec<String> = calls(rt)
        .into_iter()
        .filter(|(_, to)| to.starts_with("src/Domain.cs:AddOrderItem@"))
        .map(|(from, _)| from)
        .collect();
    assert_eq!(
        callers,
        vec![
            "src/Handler.cs:ByField@12".to_string(),
            "src/Handler.cs:ByNew@17".to_string(),
            "src/Handler.cs:Run@29".to_string(),
        ]
    );
}

// ── Every declaration form that proves a type ───────────────────────────────

const TYPED: &str = "\
using System.Collections.Generic;
using Shop.Domain;
namespace Shop.Api {
    public class Nullables {
        public void ByLoop(List<Order> xs) { foreach (Order? each in xs) { each.Ship(); } }
        public void ByOut() { Find(out Order? found); found.Ship(); }
        void Find(out Order? o) { o = null; }
        public Order? nprop { get; set; }
        public void ByParam(Order? np) { np.Ship(); }
        public void ByProperty() { nprop.Ship(); }
    }
    public struct PStruct(IOrderService s1) { public void Run() { s1.AddOrderItem(1); } }
    public record PRecord(IOrderService s2) { public void Run() { s2.AddOrderItem(2); } }
}
";

#[test]
fn every_declaration_form_c_sharp_writes_proves_its_type() {
    let tmp = tree(&[("src/Domain.cs", DOMAIN), ("src/Typed.cs", TYPED)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let t = |line: u32, name: &str| format!("src/Typed.cs:{name}@{line}");
    let ship = "src/Domain.cs:Ship@5";
    let mut from_typed: Edges = calls(rt).into_iter().filter(|(from, _)| from.starts_with("src/Typed.cs:")).collect();
    from_typed.sort();
    assert_eq!(
        from_typed,
        vec![
            edge(&t(5, "ByLoop"), ship),
            edge(&t(6, "ByOut"), ship),
            edge(&t(6, "ByOut"), &t(7, "Find")),
            edge(&t(9, "ByParam"), ship),
            edge(&t(10, "ByProperty"), ship),
            edge(&t(12, "Run"), "src/Domain.cs:AddOrderItem@2"),
            edge(&t(13, "Run"), "src/Domain.cs:AddOrderItem@2"),
        ]
    );
}

/// A nullable field read through `this.`: its stated `T?` declaration is a
/// field at its position (S-537 reads a stated field's position through the
/// same path a TypeScript parameter property takes).
const THIS_NULLABLE: &str = "\
using Shop.Domain;
namespace Shop.Api {
    public class Holder {
        private Order? _maybe;
        public void ByThis() { this._maybe.Ship(); }
    }
}
";

#[test]
fn a_nullable_field_read_through_this_binds_among_its_types_members() {
    let tmp = tree(&[("src/Domain.cs", DOMAIN), ("src/Holder.cs", THIS_NULLABLE)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(calls(rt), vec![edge("src/Holder.cs:ByThis@5", "src/Domain.cs:Ship@5")]);
}

// ── What the file does not prove ────────────────────────────────────────────

/// A base class in another file, whose `repo` hides a same-named
/// primary-constructor parameter in every type that inherits it.
const BASE: &str = "\
namespace Shop.Domain {
    public class Base { protected Mailer repo; }
}
";

/// Every call below names `Ship` or `AddOrderItem` on a receiver the file
/// proves no type for, though a plain reading would hand it one.
const UNPROVEN: &str = "\
using System.Collections.Generic;
using System.Linq;
using Shop.Domain;
namespace Shop.Api {
    public class Inheriting(Order repo) : Base {
        public void Run() { repo.Ship(); }
    }
    public class Outer(Order repo) {
        public class Inner {
            public void Run() { repo.Ship(); }
        }
    }
    public struct Owner(IOrderService service) { }
    public class Elsewhere {
        public void Leak() { service.AddOrderItem(4); }
    }
    public class Holder { }
    public class Names : Base {
        public (Order repo, int k) Tuple() { repo.Ship(); return default; }
        private Mailer e; private Mailer f; private Mailer g; private Mailer h; private Mailer v; private Mailer w;
        public object From(List<Order> xs) { return from e in xs select e.Ship(); }
        public object Let(List<Order> xs) { return from z in xs let f = z select f.Ship(); }
        public object Join(List<Order> xs, List<Order> ys) { return from z in xs join g in ys on z equals g select g.Ship(); }
        public object TypedJoin(List<Order> xs, List<object> ys) { return from z in xs join Order w in ys on z equals w select w.Ship(); }
        public object JoinInto(List<Order> xs, List<Order> ys) { return from z in xs join y in ys on z equals y into h select h.Ship(); }
        public object Into(List<Order> xs) { return from z in xs select z into v select v.Ship(); }
        private Mailer value;
        public Order Current { set { value.Ship(); } }
        private Mailer n; private Mailer b; private Mailer c;
        public void Lambda(List<Order> xs) { xs.ForEach((n) => n.Ship()); }
        public void ListPattern(object o) { if (o is [1] b) { b.Ship(); } }
        public void RecursivePattern(object o) { if (o is { } c) { c.Ship(); } }
        public void Cast(Holder src) { var t = (Order)src.item; item.Ship(); }
        public void Qualified() { Ext.Order p1 = null; p1.Ship(); global::Ext.Order p2 = null; p2.Ship(); Ext.Order? p3 = null; p3.Ship(); }
        public void QualifiedLoop(List<Ext.Order> xs) { foreach (Ext.Order p4 in xs) { p4.Ship(); } foreach (Ext.Order? p9 in xs) { p9.Ship(); } }
        public void QualifiedOut() { Try(out Ext.Order p5); p5.Ship(); }
        public void QualifiedPattern(object o) { if (o is Ext.Order p6) { p6.Ship(); } if (o is Ext.Order { } p7) { p7.Ship(); } }
        public void QualifiedCatch() { try { } catch (Ext.Order p8) { p8.Ship(); } }
        public Ext.Order qprop { get; set; }
        public void QualifiedProperty() { qprop.Ship(); }
        void Try(out Ext.Order o) { o = null; }
        public dynamic dp { get; set; }
        public void Dynamic(dynamic dy) { dy.Ship(); dynamic dl = null; dl.Ship(); dp.Ship(); }
        public void DynamicLoop(List<Order> xs) { foreach (dynamic dd in xs) { dd.Ship(); } Out(out dynamic de); de.Ship(); }
        void Out(out dynamic d) { d = null; }
        private Order ls;
        public void Labeled() { lbl: Mailer ls = null; ls.Send(); }
    }
}
";

/// The parts of one `partial` type: the other part may declare the name a
/// primary-constructor parameter spells, and hide it.
const PART1: &str = "\
using Shop.Domain;
namespace Shop.Api {
    public partial class Vm { protected Mailer repo; }
}
";

const PART2: &str = "\
using Shop.Domain;
namespace Shop.Api {
    public partial class Vm(Order repo) {
        public void Run() { repo.Ship(); }
    }
}
";

#[test]
fn a_receiver_the_file_proves_no_type_for_binds_nowhere() {
    let tmp = tree(&[
        ("src/Domain.cs", DOMAIN),
        ("src/Base.cs", BASE),
        ("src/Unproven.cs", UNPROVEN),
        ("src/Part1.cs", PART1),
        ("src/Part2.cs", PART2),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let typed = ["Ship", "Send", "AddOrderItem"];
    for file in ["src/Unproven.cs", "src/Part2.cs"] {
        let receiver_calls: Vec<Row> = rows(rt, file)
            .into_iter()
            .filter(|r| typed.iter().any(|name| r.1.ends_with(name)))
            .collect();
        assert!(!receiver_calls.is_empty(), "{file}: the fixture makes receiver calls");
        for r in &receiver_calls {
            assert_eq!((r.2, r.3, r.4), (RefForm::Method, OTHER, false), "{file}: {r:?} must stay untyped");
        }
    }
    let edges = calls(rt);
    assert!(
        !edges.iter().any(|(_, to)| to.starts_with("src/Domain.cs:")),
        "nothing here proves a domain type: {edges:#?}"
    );
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
    engine_a.pipeline().sync(&changed);
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
using Shop.Domain;
namespace Shop.Api {
    public class Handler {
        private readonly IOrderService _orderService;
        public void ByField() { _orderService.AddOrderItem(1); }
    }
}
";

#[test]
fn a_synced_edit_to_the_receivers_type_matches_a_fresh_reindex() {
    let initial = [("src/Domain.cs", DOMAIN), ("src/Handler.cs", CALLER)];
    // The member is renamed away: the edge goes, though the caller's file
    // never changes.
    let renamed = DOMAIN.replace("void AddOrderItem(int id); }", "void Add(int id); }");
    let edges = synced_equals_reindexed(&initial, &[("src/Domain.cs", &renamed)]);
    assert!(edges.is_empty(), "{edges:?}");

    // The type arrives after the caller: the call binds on sync.
    let edges = synced_equals_reindexed(&[("src/Handler.cs", CALLER)], &[("src/Domain.cs", DOMAIN)]);
    assert_eq!(edges, vec![edge("src/Handler.cs:ByField@5", "src/Domain.cs:AddOrderItem@2")]);

    // The field's declared type changes: the caller's row is re-typed.
    let retyped = CALLER.replace("IOrderService _orderService", "Order _orderService");
    let edges = synced_equals_reindexed(&initial, &[("src/Handler.cs", &retyped)]);
    assert_eq!(edges, vec![edge("src/Handler.cs:ByField@5", "src/Domain.cs:AddOrderItem@4")]);
}

// ── A binding types only the calls in its own scope (S-823, CR-256) ─────────

/// The C# reasons of the unbound-call residue, the zero ones dropped.
fn csharp_residue(engine: &Engine) -> Vec<(CallResidueReason, u64)> {
    let residue = engine
        .graph().status()
        .resolution_by_language
        .into_iter()
        .find(|row| row.language == "c-sharp")
        .and_then(|row| row.call_residue)
        .expect("the c-sharp row states its call residue");
    let mut reasons: Vec<(CallResidueReason, u64)> = residue.reasons.into_iter().filter(|(_, n)| *n > 0).collect();
    reasons.sort();
    reasons
}

/// The bound `Calls` edges whose caller sits in `file`, sorted.
fn calls_from(rt: &Runtime, file: &str) -> Edges {
    let prefix = format!("{file}:");
    calls(rt).into_iter().filter(|(from, _)| from.starts_with(&prefix)).collect()
}

/// A base class in another file: every name it declares is a member the
/// file below inherits and cannot see, so a call on it proves nothing.
const SCOPED_BASE: &str = "\
namespace Shop.Domain {
    public class Logger { public void Send() { } }
    public class Oops : System.Exception { public void Send() { } }
    public class Base {
        protected Logger svc; protected Logger p; protected Logger e; protected Logger x;
        protected Logger u; protected Logger before; protected Logger s; protected Logger pv;
        protected Logger cf; protected Logger qm; protected Logger dm; protected Logger wo; protected Logger lo;
        protected Logger aa; protected Logger sq; protected Logger lq;
    }
}
";

/// The D1 fixture (CR-256 §2): a typed local in a closed `if` block, the
/// call on the inherited field of its name after the block.
const CLOSED_BLOCK: &str = "\
using Shop.Domain;
namespace Shop.Api {
    public class C : Base {
        public void M(bool f) { if (f) { Mailer svc = new Mailer(); } svc.Send(); }
        public void N(bool f) { if (f) { Mailer svc = new Mailer(); svc.Send(); } }
    }
}
";

#[test]
fn a_local_in_a_closed_block_types_no_call_after_it() {
    let tmp = tree(&[("src/Domain.cs", DOMAIN), ("src/Base.cs", SCOPED_BASE), ("src/C.cs", CLOSED_BLOCK)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let c = |line: u32, name: &str| format!("src/C.cs:{name}@{line}");
    // `M`'s `svc` is the inherited `Logger` field: the local's block closed
    // before the call, so no binding of `svc` is in scope there.
    assert_eq!(
        rows(rt, "src/C.cs"),
        sorted(vec![
            row(&c(4, "M"), "Send", RefForm::Method, OTHER, false),
            row(&c(5, "N"), "Mailer::Send", RefForm::Path, None, true),
        ])
    );
    assert_eq!(calls_from(rt, "src/C.cs"), vec![edge(&c(5, "N"), "src/Domain.cs:Send@8")]);
    assert_eq!(csharp_residue(&engine), vec![(CallResidueReason::NoReceiverEvidence, 1)]);
}

/// Each binding below is typed, and each is called once inside its scope and
/// once outside it, where only the inherited member of its name is in scope.
const OUT_OF_SCOPE: &str = "\
using System.Collections.Generic;
using Shop.Domain;
namespace Shop.Api {
    public class D : Base {
        public void Lambda(List<Mailer> ms) { ms.ForEach((Mailer p) => p.Send()); p.Send(); }
        public void Loop(List<Mailer> ms) { foreach (Mailer e in ms) { e.Send(); } e.Send(); }
        public void Catch() { try { } catch (Oops x) { x.Send(); } x.Send(); }
        public void Using() { using (Mailer u = new Mailer()) { u.Send(); } u.Send(); }
        public void Before() { before.Send(); Mailer before = new Mailer(); before.Send(); }
        public void Sibling() { Mailer s = new Mailer(); s.Send(); }
        public void Other() { s.Send(); }
        public void Pattern(object o, bool f) { if (f) { if (o is Mailer pv) { pv.Send(); } } pv.Send(); }
    }
}
";

#[test]
fn a_binding_outside_its_construct_or_callable_types_no_call() {
    let tmp = tree(&[("src/Domain.cs", DOMAIN), ("src/Base.cs", SCOPED_BASE), ("src/D.cs", OUT_OF_SCOPE)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let d = |line: u32, name: &str| format!("src/D.cs:{name}@{line}");
    let mailer = |source: String| row(&source, "Mailer::Send", RefForm::Path, None, true);
    let untyped = |source: String| row(&source, "Send", RefForm::Method, OTHER, false);
    assert_eq!(
        rows(rt, "src/D.cs"),
        sorted(vec![
            row(&d(5, "Lambda"), "List::ForEach", RefForm::Path, None, false),
            // The lambda's `p` is its own; after it, `p` is the inherited field.
            mailer(d(5, "Lambda")),
            untyped(d(5, "Lambda")),
            // A loop variable, outside its loop.
            mailer(d(6, "Loop")),
            untyped(d(6, "Loop")),
            // A `catch` variable, outside its clause.
            row(&d(7, "Catch"), "Oops::Send", RefForm::Path, None, true),
            untyped(d(7, "Catch")),
            // A `using` variable, outside its statement.
            mailer(d(8, "Using")),
            untyped(d(8, "Using")),
            // A local, before its declaration.
            untyped(d(9, "Before")),
            mailer(d(9, "Before")),
            // A sibling method's local.
            mailer(d(10, "Sibling")),
            untyped(d(11, "Other")),
            // A pattern variable, after the block around its `if` closed.
            mailer(d(12, "Pattern")),
            untyped(d(12, "Pattern")),
        ])
    );
}

/// The binding forms that still type their calls, each beside a field of
/// its name in another class (`Holder`) that is never in scope here.
const IN_SCOPE: &str = "\
using System.Collections.Generic;
using Shop.Domain;
namespace Shop.Api {
    public class Holder { private Order pm; private Order pn; private Order om; private Order sm; private Order am; }
    public class E {
        public void Pattern(object o) { if (o is Mailer pm) { pm.Send(); } }
        public void Guard(object o) { if (!(o is Mailer pn)) { return; } pn.Send(); }
        public void Out() { Find(out Mailer om); om.Send(); }
        public void Case(object o) { switch (o) { case Mailer sm: sm.Send(); break; } }
        public void Arm(object o) { var r = o switch { Mailer am => Use(am), _ => 0 }; }
        int Use(Mailer am) { am.Send(); return 0; }
        void Find(out Mailer o) { o = null; }
        public void Compound(object o, bool b) { if (!(o is Mailer pc) || b) { return; } pc.Send(); }
        public void Chain(object o, bool b) { if (o is Mailer pd && b && pd != null) { pd.Send(); } }
        public void Lambda(List<object> xs) { xs.ForEach(o => { if (o is Mailer pl && pl != null) pl.Send(); }); }
    }
    public class Cp(Order repo) {
        private Mailer repo = null;
        public void Run() { repo.Send(); }
    }
    public class Cq(Order repo) {
        public void Run(Mailer repo) { repo.Send(); }
        public void Own() { repo.Ship(); }
    }
}
";

#[test]
fn a_pattern_out_case_or_primary_binding_types_the_calls_in_its_scope() {
    let tmp = tree(&[("src/Domain.cs", DOMAIN), ("src/E.cs", IN_SCOPE)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let e = |line: u32, name: &str| format!("src/E.cs:{name}@{line}");
    let send = "src/Domain.cs:Send@8";
    assert_eq!(
        calls_from(rt, "src/E.cs"),
        sorted_edges(vec![
            edge(&e(6, "Pattern"), send),
            // A guard's pattern variable flows into the rest of the block.
            edge(&e(7, "Guard"), send),
            edge(&e(8, "Out"), send),
            edge(&e(8, "Out"), &e(12, "Find")),
            edge(&e(9, "Case"), send),
            edge(&e(10, "Arm"), &e(11, "Use")),
            edge(&e(11, "Use"), send),
            // A pattern variable in a compound condition, a chain or a lambda.
            edge(&e(13, "Compound"), send),
            edge(&e(14, "Chain"), send),
            edge(&e(15, "Lambda"), send),
            // A member hides a primary-constructor parameter of its name...
            edge(&e(19, "Run"), send),
            // ...and so does a parameter; the type's own members read it.
            edge(&e(22, "Run"), send),
            edge(&e(23, "Own"), "src/Domain.cs:Ship@5"),
        ])
    );
}

fn sorted_edges(mut edges: Edges) -> Edges {
    edges.sort();
    edges
}

#[test]
fn a_synced_edit_closing_a_locals_block_matches_a_fresh_reindex() {
    let initial = [("src/Domain.cs", DOMAIN), ("src/Base.cs", SCOPED_BASE), ("src/C.cs", CLOSED_BLOCK)];
    // `N`'s call moves after the block that declares its local: it is unbound.
    let closed = CLOSED_BLOCK.replace("svc.Send(); } }\n    }", "} svc.Send(); }\n    }");
    assert_ne!(closed, CLOSED_BLOCK, "the edit applies");
    let edges = synced_equals_reindexed(&initial, &[("src/C.cs", &closed)]);
    assert!(!edges.iter().any(|(from, _)| from.starts_with("src/C.cs:")), "{edges:#?}");
}

/// A `case` section's local is in scope across the whole switch body: in a
/// later section it is still the local, never the field of its name.
const SWITCH_LOCAL: &str = "\
using Shop.Domain;
namespace Shop.Api {
    public class Sw {
        private Logger o;
        public void M(int k) {
            switch (k) {
                case 1: Mailer o = new Mailer(); break;
                case 2: o = new Mailer(); o.Send(); break;
            }
        }
    }
    public class Sx : Base {
        public void N(int k) {
            switch (k) {
                case 1: Mailer svc = new Mailer(); break;
                case 2: svc = new Mailer(); svc.Send(); break;
            }
        }
    }
}
";

#[test]
fn a_case_sections_local_is_in_scope_in_every_section_of_its_switch() {
    let tmp = tree(&[("src/Domain.cs", DOMAIN), ("src/Base.cs", SCOPED_BASE), ("src/Sw.cs", SWITCH_LOCAL)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    // `M`'s local and its field are both in scope in `case 2`: refused, and
    // never the field's `Logger`. `N`'s local is the one binding there.
    assert_eq!(
        rows(rt, "src/Sw.cs"),
        sorted(vec![
            row("src/Sw.cs:M@5", "Send", RefForm::Method, OTHER, false),
            row("src/Sw.cs:N@13", "Mailer::Send", RefForm::Path, None, true),
        ])
    );
    assert_eq!(calls_from(rt, "src/Sw.cs"), vec![edge("src/Sw.cs:N@13", "src/Domain.cs:Send@8")]);
}

/// Top-level statements before the file's types: a member reads its own
/// type's members — an inherited `svc`, a primary-constructor `w` — never a
/// top-level local of the name (CS8801).
const TOP_LEVEL: &str = "\
using Shop.Domain;
var svc = new Mailer();
Mailer w = new Mailer();
class Tl : Base { public void M() { svc.Send(); } }
class Wk(Logger w) : Base { public void N() { w.Send(); } }
";

#[test]
fn a_top_level_local_types_no_call_in_the_types_after_it() {
    let tmp = tree(&[("src/Domain.cs", DOMAIN), ("src/Base.cs", SCOPED_BASE), ("src/Program.cs", TOP_LEVEL)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let methods: Vec<Row> = rows(rt, "src/Program.cs").into_iter().filter(|r| !r.0.contains(":Program.cs@")).collect();
    assert_eq!(
        methods,
        sorted(vec![
            row("src/Program.cs:M@4", "Send", RefForm::Method, OTHER, false),
            row("src/Program.cs:N@5", "Send", RefForm::Method, OTHER, false),
        ])
    );
    assert!(!calls(rt).iter().any(|(from, _)| from.starts_with("src/Program.cs:")), "{:#?}", calls(rt));
}

/// Pattern and `out` variables C# scopes to their own construct: each types
/// the call inside it, and none the call after it, where only the inherited
/// member of its name is in scope.
const CONSTRUCT_SCOPED: &str = "\
using System;
using System.Collections.Generic;
using System.Linq;
using Shop.Domain;
namespace Shop.Api {
    public class W : Base {
        public void While(object o) { while (o is Mailer x) { x.Send(); break; } x.Send(); }
        public void Do(object o) { do { } while (o is Mailer dm && Go(dm)); dm.Send(); }
        public void WhileOut() { while (Try(out Mailer wo)) { wo.Send(); } wo.Send(); }
        public void ElseIf(object o, bool f) { if (f) { } else if (o is Mailer e) { e.Send(); } e.Send(); }
        public void UsingOut() { using (Make(out Mailer u)) { u.Send(); } u.Send(); }
        public void For(object o) { for (int i = 0; o is Mailer p && i < 1; i++) { p.Send(); } p.Send(); }
        public void Embedded(List<object> xs) { foreach (var q in xs) if (q is Mailer s) { s.Send(); } s.Send(); }
        public void Filter() { try { } catch (Exception ex) when (ex.Data is Mailer cf) { cf.Send(); } cf.Send(); }
        public void Lock(object o) { lock (o is Mailer lo ? lo : o) { lo.Send(); } lo.Send(); }
        public object Query(List<object> xs) { var r = from z in xs where z is Mailer qm select qm; qm.Send(); return r; }
        bool Go(Mailer m) { return true; }
        bool Try(out Mailer m) { m = null; return false; }
        IDisposable Make(out Mailer m) { m = null; return null; }
    }
}
";

#[test]
fn a_pattern_or_out_variable_of_a_loop_resource_filter_or_embedded_statement_stays_in_it() {
    let tmp = tree(&[("src/Domain.cs", DOMAIN), ("src/Base.cs", SCOPED_BASE), ("src/W.cs", CONSTRUCT_SCOPED)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let w = |line: u32, name: &str| format!("src/W.cs:{name}@{line}");
    let send: Vec<Row> = rows(rt, "src/W.cs").into_iter().filter(|r| r.1.ends_with("Send")).collect();
    let mailer = |source: String| row(&source, "Mailer::Send", RefForm::Path, None, true);
    let untyped = |source: String| row(&source, "Send", RefForm::Method, OTHER, false);
    assert_eq!(
        send,
        sorted(vec![
            mailer(w(7, "While")),
            untyped(w(7, "While")),
            untyped(w(8, "Do")),
            mailer(w(9, "WhileOut")),
            untyped(w(9, "WhileOut")),
            mailer(w(10, "ElseIf")),
            untyped(w(10, "ElseIf")),
            mailer(w(11, "UsingOut")),
            untyped(w(11, "UsingOut")),
            mailer(w(12, "For")),
            untyped(w(12, "For")),
            mailer(w(13, "Embedded")),
            untyped(w(13, "Embedded")),
            mailer(w(14, "Filter")),
            untyped(w(14, "Filter")),
            mailer(w(15, "Lock")),
            untyped(w(15, "Lock")),
            untyped(w(16, "Query")),
        ])
    );
}

/// A switch arm's, a `case` section's and an expression lambda's pattern
/// variable: typed inside its construct, and after it only the inherited
/// member of its name is in scope.
const FLOW_BOUNDS: &str = "\
using System.Collections.Generic;
using System.Linq;
using Shop.Domain;
namespace Shop.Api {
    public class F : Base {
        public void Arm(object o) { var r = o switch { Mailer aa => aa.Ok(), _ => false }; aa.Send(); }
        public void Sec(object o, int k) { switch (k) { case 1: if (o is Mailer sq) { sq.Send(); } break; } sq.Ok(); }
        public void Lam(List<object> xs) { var r = xs.Select(o => o is Mailer lq && lq.Ok()); lq.Send(); }
    }
}
";

#[test]
fn a_switch_arm_section_or_expression_lambda_bounds_its_pattern_variable() {
    let tmp = tree(&[("src/Domain.cs", DOMAIN), ("src/Base.cs", SCOPED_BASE), ("src/F.cs", FLOW_BOUNDS)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let f = |line: u32, name: &str| format!("src/F.cs:{name}@{line}");
    let receiver_calls: Vec<Row> =
        rows(rt, "src/F.cs").into_iter().filter(|r| r.1.ends_with("Send") || r.1.ends_with("Ok")).collect();
    assert_eq!(
        receiver_calls,
        sorted(vec![
            row(&f(6, "Arm"), "Mailer::Ok", RefForm::Path, None, true),
            row(&f(6, "Arm"), "Send", RefForm::Method, OTHER, false),
            row(&f(7, "Sec"), "Mailer::Send", RefForm::Path, None, true),
            row(&f(7, "Sec"), "Ok", RefForm::Method, OTHER, false),
            row(&f(8, "Lam"), "Mailer::Ok", RefForm::Path, None, true),
            row(&f(8, "Lam"), "Send", RefForm::Method, OTHER, false),
        ])
    );
}

/// The binding forms the fixtures above leave out, each typing its call: an
/// indexer parameter, an event field, a `for` variable, `var x = new T<A>()`,
/// a `T?` property read through `this.`, a field initializer's pattern
/// variable.
const MORE_FORMS: &str = "\
using System.Collections.Generic;
using Shop.Domain;
namespace Shop.Api {
    public class Repo<T> { public void Add() { } }
    public class G {
        public Mailer this[Mailer ix] { get { ix.Send(); return ix; } }
        public event Mailer ev;
        public void Event() { ev.Send(); }
        public void For() { for (Mailer fi = null; ; ) { fi.Send(); break; } }
        public void Generic() { var r = new Repo<Order>(); r.Add(); }
        public Mailer? NProp { get; set; }
        public void ThisProp() { this.NProp.Send(); }
        static object Obj;
        private static bool B = Obj is Mailer fq && fq.Ok();
    }
}
";

#[test]
fn every_other_binding_form_types_its_call() {
    let tmp = tree(&[("src/Domain.cs", DOMAIN), ("src/G.cs", MORE_FORMS)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let g = |line: u32, name: &str| format!("src/G.cs:{name}@{line}");
    let send = "src/Domain.cs:Send@8";
    assert_eq!(
        calls_from(rt, "src/G.cs"),
        sorted_edges(vec![
            // An indexer is no symbol: its call is its class's.
            edge(&g(5, "G"), send),
            edge(&g(8, "Event"), send),
            edge(&g(9, "For"), send),
            edge(&g(10, "Generic"), &g(4, "Add")),
            edge(&g(12, "ThisProp"), send),
            // A field initializer's call is the field's.
            edge(&g(14, "B"), "src/Domain.cs:Ok@8"),
        ])
    );
}
