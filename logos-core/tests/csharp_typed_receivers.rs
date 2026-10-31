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
    public class Mailer { public void Send() { } }
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
