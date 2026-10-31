//! A C++ call on a declared-type receiver binds among that type's members
//! (S-581, CR-184, FR-RS-39, FR-RS-10, FR-RS-12, NFR-RA-05) — exercised end to
//! end through the public [`Engine`] façade against temp-directory fixtures,
//! with the SHIPPED `references.scm` and `plugin.toml` of the C++ plugin (no
//! query override).
//!
//! What the fixtures pin, over functions of `handler.cpp` calling the
//! `shop` types `domain.hpp` declares and `handler.cpp` includes:
//!
//! - a receiver whose one binding in scope at the call proves its type — a
//!   parameter or local declared `T`, `T&`, `const T&` or `ns::T`, a data
//!   member read by an inline method of its class, a lambda, range-`for`,
//!   `catch` or condition variable — read through `.`, or one declared `T*`,
//!   `std::unique_ptr<T>` or `std::shared_ptr<T>` read through `->`, is the
//!   Path-form `T::m`, bound to `T`'s `m` or, failing that, a base's;
//! - a receiver with no binding that proves a type — `auto`, a template
//!   parameter, a structured binding — one whose typed binding a nearer one
//!   hides (a block, an `#ifdef` branch, a parameter over a member), a pointer
//!   read through `.` and a value read through `->` stay the Method-form row of
//!   shape `other`, bound nowhere (`no-receiver-evidence`).
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite: a fixture tree checked into this repository would be
//! indexed into its own graph.

#![cfg(feature = "lang-cpp")]

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
    let rows: Vec<Row> = rt
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

const DOMAIN_PATH: &str = "src/domain.hpp";

/// `Order` and `Mailer` each declare a `reset`, as `std::unique_ptr` does: a
/// call on the smart pointer itself must never reach them. `Box` shares its
/// name with a wrapper Rust's receiver typing peels.
const DOMAIN: &str = "\
namespace shop {
class Order {
public:
    void ship() {}
    void reset() {}
};
class SpecialOrder : public Order {
public:
    void mark() {}
};
class Mailer {
public:
    void send() {}
    void reset() {}
};
Order make();
Order* find();
template <class T> class Box { public: void ship() {} };
}
";

const HANDLER_PATH: &str = "src/handler.cpp";

const HANDLER: &str = "\
#include \"domain.hpp\"
#include <memory>

namespace shop {
void byValue(Order o) { o.ship(); }
void byConstRef(const Order& o) { o.ship(); }
void byRvalue(Order&& o) { o.ship(); }
void byQualified(shop::Order o) { o.ship(); }
void byPointer(Order* o) { o->ship(); }
void byUnique(std::unique_ptr<Order> o) { o->ship(); }
void bySharedRef(const std::shared_ptr<Mailer>& m) { m->send(); }
void byLocal() { Order o; o.ship(); }
void byInitialized() { Order o = make(); o.ship(); }
void byLocalPointer() { Order* p = find(); p->ship(); }
void byBase(SpecialOrder s) { s.ship(); }
void byAuto() { auto o = make(); o.ship(); }
void byAutoPointer() { auto* p = find(); p->ship(); }
void bySmartItself(std::unique_ptr<Order> o) { o.reset(); }
void byArrowOnValue(Order o) { o->ship(); }
void byBox(Box<Order> b) { b.ship(); }
void bySharedValue(std::shared_ptr<Order> p) { p->ship(); }
void byUniqueLocal() { std::unique_ptr<Order> p = nullptr; p->ship(); }
void byUniqueRef(std::unique_ptr<Order>& q) { std::unique_ptr<Order>& r = q; r->ship(); }
void byLocalRef(Order& q) { Order& r = q; r.ship(); }
void byOtherUniquePtr(my::unique_ptr<Order> p) { p->ship(); }
}
";

#[test]
fn a_call_on_a_declared_type_receiver_binds_among_that_types_members() {
    let tmp = tree(&[(DOMAIN_PATH, DOMAIN), (HANDLER_PATH, HANDLER)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let h = |line: u32, name: &str| format!("{HANDLER_PATH}:{name}@{line}");
    let d = |line: u32, name: &str| format!("{DOMAIN_PATH}:{name}@{line}");
    let typed = |source: String, target: &str| row(&source, target, RefForm::Path, None, true);
    let untyped = |source: String, name: &str| row(&source, name, RefForm::Method, OTHER, false);
    assert_eq!(
        rows(rt, HANDLER_PATH, &["ship", "send", "reset"]),
        sorted(vec![
            typed(h(5, "byValue"), "Order::ship"),
            typed(h(6, "byConstRef"), "Order::ship"),
            typed(h(7, "byRvalue"), "Order::ship"),
            typed(h(8, "byQualified"), "shop::Order::ship"),
            typed(h(9, "byPointer"), "Order::ship"),
            typed(h(10, "byUnique"), "Order::ship"),
            typed(h(11, "bySharedRef"), "Mailer::send"),
            typed(h(12, "byLocal"), "Order::ship"),
            typed(h(13, "byInitialized"), "Order::ship"),
            typed(h(14, "byLocalPointer"), "Order::ship"),
            typed(h(15, "byBase"), "SpecialOrder::ship"),
            // `auto` is no proof.
            untyped(h(16, "byAuto"), "ship"),
            untyped(h(17, "byAutoPointer"), "ship"),
            // `std::unique_ptr::reset`, never `Order`'s.
            untyped(h(18, "bySmartItself"), "reset"),
            // `Order`'s `operator->`, never `Order::ship`.
            untyped(h(19, "byArrowOnValue"), "ship"),
            // A template is its own type, never a wrapper of its argument.
            typed(h(20, "byBox"), "Box::ship"),
            typed(h(21, "bySharedValue"), "Order::ship"),
            typed(h(22, "byUniqueLocal"), "Order::ship"),
            typed(h(23, "byUniqueRef"), "Order::ship"),
            typed(h(24, "byLocalRef"), "Order::ship"),
            // Only `std`'s smart pointers are read through.
            untyped(h(25, "byOtherUniquePtr"), "ship"),
        ])
    );
    let ship = d(4, "ship");
    let edges = calls(rt);
    for expected in [
        edge(&h(5, "byValue"), &ship),
        edge(&h(6, "byConstRef"), &ship),
        edge(&h(7, "byRvalue"), &ship),
        edge(&h(8, "byQualified"), &ship),
        edge(&h(9, "byPointer"), &ship),
        edge(&h(10, "byUnique"), &ship),
        edge(&h(11, "bySharedRef"), &d(13, "send")),
        edge(&h(12, "byLocal"), &ship),
        edge(&h(13, "byInitialized"), &ship),
        edge(&h(14, "byLocalPointer"), &ship),
        // `SpecialOrder` declares no `ship`: its proven base's.
        edge(&h(15, "byBase"), &ship),
        edge(&h(20, "byBox"), &d(18, "ship")),
        edge(&h(21, "bySharedValue"), &ship),
        edge(&h(22, "byUniqueLocal"), &ship),
        edge(&h(23, "byUniqueRef"), &ship),
        edge(&h(24, "byLocalRef"), &ship),
    ] {
        assert!(edges.contains(&expected), "missing {expected:?} in {edges:?}");
    }
    // Each still calls `make()` or `find()`; its receiver's call binds nothing.
    let members = [d(4, "ship"), d(5, "reset"), d(14, "reset")];
    for caller in ["byAuto", "byAutoPointer", "bySmartItself", "byArrowOnValue", "byOtherUniquePtr"] {
        assert!(
            !edges.iter().any(|(from, to)| from.contains(&format!(":{caller}@")) && members.contains(to)),
            "{caller}'s receiver call binds nothing in {edges:?}"
        );
    }
}

const MEMBERS_PATH: &str = "src/members.cpp";

/// Inline methods reading data members, and the out-of-line definition whose
/// class this file may not hold.
const MEMBERS: &str = "\
#include \"domain.hpp\"
#include <memory>

namespace shop {
class Handler {
public:
    Order current;
    Order* pending;
    std::unique_ptr<Mailer> mailer;
    void byMember() { current.ship(); }
    void byMemberPointer() { pending->ship(); }
    void bySmartMember() { mailer->send(); }
    void byLambda() { auto f = [this]() { current.ship(); }; f(); }
    void byShadowingParameter(Mailer current) { current.send(); }
    void outOfLine();
};
void Handler::outOfLine() { current.ship(); }
class Nested {
    Order inner;
    struct Deep {
        void call() { inner.ship(); }
    };
};
struct SHolder { Order cur; void sm() { cur.ship(); } };
union UHolder { Order* cur; void um() { cur->ship(); } };
void localStruct(Order o) { struct L { Mailer o; void g() { o.reset(); } }; }
struct PtrShadow { Mailer current; Order* byShadowPtr(Order current) { current.ship(); return nullptr; } };
}
";

#[test]
fn a_data_member_types_a_call_in_its_own_classs_inline_methods_only() {
    let tmp = tree(&[(DOMAIN_PATH, DOMAIN), (MEMBERS_PATH, MEMBERS)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let m = |line: u32, name: &str| format!("{MEMBERS_PATH}:{name}@{line}");
    let typed = |source: String, target: &str| row(&source, target, RefForm::Path, None, true);
    let untyped = |source: String, name: &str| row(&source, name, RefForm::Method, OTHER, false);
    let got: Vec<Row> = rows(rt, MEMBERS_PATH, &["ship", "send", "reset"]).into_iter().filter(|r| !r.0.contains(":outOfLine@")).collect();
    assert_eq!(
        got,
        sorted(vec![
            typed(m(10, "byMember"), "Order::ship"),
            typed(m(11, "byMemberPointer"), "Order::ship"),
            typed(m(12, "bySmartMember"), "Mailer::send"),
            typed(m(13, "byLambda"), "Order::ship"),
            // The parameter and the member are two bindings: neither is picked.
            untyped(m(14, "byShadowingParameter"), "send"),
            // `Deep` is not `Nested`: its method never sees `inner`.
            untyped(m(21, "call"), "ship"),
            typed(m(24, "sm"), "Order::ship"),
            typed(m(25, "um"), "Order::ship"),
            // A local struct's member and the enclosing function's parameter
            // are two bindings: refused, never typed by the parameter.
            untyped(m(26, "g"), "reset"),
            // The parameter and the member are two bindings, whatever the
            // function's declarator wraps them in.
            untyped(m(27, "byShadowPtr"), "ship"),
        ])
    );
    // The out-of-line definition sits in no class body: `current` is not
    // typed there, whichever node owns its row.
    let out_of_line: Vec<Row> = rows(rt, MEMBERS_PATH, &["ship"]).into_iter().filter(|r| r.0.contains(":outOfLine@")).collect();
    assert!(!out_of_line.is_empty(), "the out-of-line call is recorded");
    assert!(out_of_line.iter().all(|r| r.1 == "ship" && r.2 == RefForm::Method), "{out_of_line:?}");
}

const SCOPES_PATH: &str = "src/scopes.cpp";

/// Every other binding form C++ writes, each typing its own call — and, in
/// `hidden*`, hiding a typed binding of the same name around it.
const SCOPES: &str = "\
#include \"domain.hpp\"
#include <vector>

namespace shop {
void byLambdaParameter() { auto f = [](Order x) { x.ship(); }; }
void byRangeFor(std::vector<Order>& xs) { for (Order& x : xs) { x.ship(); } }
void byCatch() { try { make(); } catch (Order& e) { e.ship(); } }
void byCondition() { if (Order* p = find()) { p->ship(); } }
void byInitStatement() { if (Order o = make(); true) { o.ship(); } }
void byForInit() { for (Order* p = find(); p; p = nullptr) { p->ship(); } }
void byCase(int k) { switch (k) { case 1: Order o; o.ship(); break; } }
void hiddenByBlock(Order o) { { auto o = make(); o.ship(); } }
void hiddenByLambda(Order o) { auto f = [](auto o) { o.ship(); }; }
void hiddenByRangeFor(Order o, std::vector<Order>& xs) { for (auto& o : xs) { o.ship(); } }
void hiddenByCapture(Order o) { auto f = [o = make()]() { o.ship(); }; }
void hiddenByStructured(Order o) { { auto [o, n] = pair(); o.ship(); } }
void hiddenByBranch(Order o) {
    {
#ifdef FAST
        auto o = make();
#endif
        o.ship();
    }
}
void afterBlock(Order o) { { auto o = make(); } o.ship(); }
template <typename T> void byTemplate(T t) { t.ship(); }
Order* byPtrReturn(Order o) { o.ship(); return nullptr; }
Order** byPtrPtrReturn(Order o) { o.ship(); return nullptr; }
Order& byRefReturn(Order o) { o.ship(); return o; }
void hiddenByElse(Order o) {
    {
#ifdef FAST
        int x = 0;
#else
        Mailer o;
#endif
        o.ship();
    }
}
void beforeShadow(Order o) { { o.ship(); auto o = make(); } }
}
";

#[test]
fn every_binding_form_types_its_own_call_and_hides_an_outer_one() {
    let tmp = tree(&[(DOMAIN_PATH, DOMAIN), (SCOPES_PATH, SCOPES)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let s = |line: u32, name: &str| format!("{SCOPES_PATH}:{name}@{line}");
    let typed = |source: String| row(&source, "Order::ship", RefForm::Path, None, true);
    let untyped = |source: String| row(&source, "ship", RefForm::Method, OTHER, false);
    assert_eq!(
        rows(rt, SCOPES_PATH, &["ship"]),
        sorted(vec![
            // A lambda's row is its enclosing function's.
            typed(s(5, "byLambdaParameter")),
            typed(s(6, "byRangeFor")),
            typed(s(7, "byCatch")),
            typed(s(8, "byCondition")),
            typed(s(9, "byInitStatement")),
            typed(s(10, "byForInit")),
            // A `case` local is in scope in every later label: it hides,
            // and types nothing.
            untyped(s(11, "byCase")),
            untyped(s(12, "hiddenByBlock")),
            untyped(s(13, "hiddenByLambda")),
            untyped(s(14, "hiddenByRangeFor")),
            untyped(s(15, "hiddenByCapture")),
            untyped(s(16, "hiddenByStructured")),
            untyped(s(17, "hiddenByBranch")),
            // The block's `o` ended with the block: the parameter is the one.
            typed(s(25, "afterBlock")),
            // A template parameter is no type.
            untyped(s(26, "byTemplate")),
            typed(s(27, "byPtrReturn")),
            typed(s(28, "byPtrPtrReturn")),
            typed(s(29, "byRefReturn")),
            untyped(s(30, "hiddenByElse")),
            // A local is in scope from its declaration on: the call before it
            // reads the parameter.
            typed(s(40, "beforeShadow")),
        ])
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

const CALLER_PATH: &str = "src/caller.cpp";

const CALLER: &str = "\
#include \"domain.hpp\"

void run(shop::Mailer& m) { m.send(); }
";

#[test]
fn a_synced_edit_to_the_receivers_type_matches_a_fresh_reindex() {
    let initial = [(DOMAIN_PATH, DOMAIN), (CALLER_PATH, CALLER)];
    let run = format!("{CALLER_PATH}:run@3");
    // The member is renamed away: the edge goes, though the caller's file
    // never changes.
    let renamed = DOMAIN.replace("void send() {}", "void post() {}");
    let edges = synced_equals_reindexed(&initial, &[(DOMAIN_PATH, &renamed)]);
    assert!(!edges.iter().any(|(from, _)| *from == run), "{edges:?}");

    // The type arrives after the caller: the call binds on sync.
    let edges = synced_equals_reindexed(&[(CALLER_PATH, CALLER)], &[(DOMAIN_PATH, DOMAIN)]);
    assert!(edges.contains(&edge(&run, &format!("{DOMAIN_PATH}:send@13"))), "{edges:?}");

    // The parameter's declared type changes: the caller's row is re-typed.
    let retyped = CALLER.replace("shop::Mailer& m) { m.send(); }", "shop::Order& m) { m.ship(); }");
    let edges = synced_equals_reindexed(&initial, &[(CALLER_PATH, &retyped)]);
    assert!(edges.contains(&edge(&run, &format!("{DOMAIN_PATH}:ship@4"))), "{edges:?}");
}

const BASE_PATH: &str = "src/base.hpp";
const BASE: &str = "namespace shop {\nclass Root {\npublic:\n    void ping() {}\n};\n}\n";
const LEAF_PATH: &str = "src/leaf.hpp";
const LEAF: &str = "#include \"base.hpp\"\nnamespace shop {\nclass Leaf {};\n}\n";
const USER_PATH: &str = "src/user.cpp";
const USER: &str = "#include \"leaf.hpp\"\n\nvoid use(shop::Leaf& l) { l.ping(); }\n";

#[test]
fn a_base_gained_in_another_file_binds_the_typed_call_on_sync() {
    let initial = [(BASE_PATH, BASE), (LEAF_PATH, LEAF), (USER_PATH, USER)];
    let used = edge(&format!("{USER_PATH}:use@3"), &format!("{BASE_PATH}:ping@4"));
    let edges = synced_equals_reindexed(&initial, &[]);
    assert!(!edges.contains(&used), "{edges:?}");
    // Neither the caller's file nor the base's changes.
    let derived = LEAF.replace("class Leaf {};", "class Leaf : public Root {};");
    let edges = synced_equals_reindexed(&initial, &[(LEAF_PATH, &derived)]);
    assert!(edges.contains(&used), "{edges:?}");
}

const WRITTEN_PATH: &str = "src/written.cpp";

const WRITTEN: &str = "\
#include \"domain.hpp\"

namespace shop {
class Audit : public SpecialOrder {
public:
    void check() { SpecialOrder::ship(); SpecialOrder::fly(); }
};
}
";

/// A written `Derived::m()` reads `Derived`'s scope as C++ does, its bases
/// included — the lookup a typed receiver's `T::m` shares.
#[test]
fn a_written_qualified_call_reaches_a_member_its_class_inherits() {
    let tmp = tree(&[(DOMAIN_PATH, DOMAIN), (WRITTEN_PATH, WRITTEN)]);
    let engine = index(tmp.path());
    let from_file: Edges = calls(engine.runtime().unwrap())
        .into_iter()
        .filter(|(from, _)| from.starts_with(WRITTEN_PATH))
        .collect();
    assert_eq!(from_file, vec![edge(&format!("{WRITTEN_PATH}:check@6"), &format!("{DOMAIN_PATH}:ship@4"))]);
}

const GLOBAL_PATH: &str = "src/global.hpp";
const GLOBAL: &str = "struct Order {\n    void ship() {}\n};\n";
const INNER_PATH: &str = "src/inner.cpp";

/// `a::Order` hides `::Order` inside `a`: a call it inherits binds at its base,
/// never at the outer class of its name (S-581 review).
const INNER: &str = "\
#include \"global.hpp\"

namespace a {
struct Base {
    void ship() {}
};
struct Order : Base {};
void typed(Order o) { o.ship(); }
void written() { Order::ship(); }
}
";

#[test]
fn a_nearer_class_hides_an_outer_one_of_its_name() {
    let tmp = tree(&[(GLOBAL_PATH, GLOBAL), (INNER_PATH, INNER)]);
    let engine = index(tmp.path());
    let from_file: Edges = calls(engine.runtime().unwrap())
        .into_iter()
        .filter(|(from, _)| from.starts_with(INNER_PATH))
        .collect();
    let base_ship = format!("{INNER_PATH}:ship@5");
    assert_eq!(
        from_file,
        vec![
            edge(&format!("{INNER_PATH}:typed@8"), &base_ship),
            edge(&format!("{INNER_PATH}:written@9"), &base_ship),
        ]
    );
}

const HIDERS_PATH: &str = "src/hiders.cpp";

/// Declarations no `binding` pattern scopes, each a nearer `o` than the data
/// member `Order o` (S-581 review): each must hide the member, so no `reset`
/// call is typed `Order::reset`.
const HIDERS: &str = "\
#include \"domain.hpp\"

namespace shop {
struct Holder {
    Order o;
    Mailer* held;
    void labelled() { { again: Mailer o; o.reset(); } }
    void fallthrough(int k) { switch (k) { case 1: Mailer o; [[fallthrough]]; case 2: o.reset(); } }
    Mailer*& slot(Mailer o) { o.reset(); return held; }
    void (*pick(Mailer o))(int) { o.reset(); return nullptr; }
    Mailer*** deep(Mailer o) { o.reset(); return nullptr; }
    void nested() {
#ifdef A
#ifdef B
        Mailer o;
#endif
#endif
        o.reset();
    }
    void chain() {
#if A
        int x = 0;
#elif B
        int y = 0;
#elif C
        Mailer o;
#endif
        o.reset();
    }
    void inRequires() { if constexpr (requires(Mailer o) { o.reset(); }) {} }
};
}
";

#[test]
fn a_declaration_no_binding_scopes_still_hides_an_outer_one() {
    let tmp = tree(&[(DOMAIN_PATH, DOMAIN), (HIDERS_PATH, HIDERS)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let got = rows(rt, HIDERS_PATH, &["reset"]);
    let mut callers: Vec<&str> = got.iter().map(|r| r.0.rsplit(':').next().unwrap().split('@').next().unwrap()).collect();
    callers.sort_unstable();
    assert_eq!(
        callers,
        ["chain", "deep", "fallthrough", "inRequires", "labelled", "nested", "pick", "slot"],
        "every hider's call is recorded once: {got:?}"
    );
    for r in &got {
        assert_eq!((r.1.as_str(), r.2, r.3, r.4), ("reset", RefForm::Method, OTHER, false), "{got:?}");
    }
}
