//! A bare C++ member read is an access, and `this->f()` is not (S-580, CR-184,
//! FR-EX-22, FR-QM-11, NFR-RA-05, NFR-RA-06) — exercised end to end through the
//! public [`Engine`] façade against temp-directory fixtures, with the SHIPPED
//! `references.scm` and `plugin.toml` of the C++ plugin (no query override).
//!
//! What the fixtures pin, over the methods of `Account`:
//!
//! - a bare `m_balance` or `m_limit` that the enclosing class declares as a
//!   field, in the value positions real code spells them in (an operand, an
//!   argument, a receiver, a return, a condition, an assignment, an index, the
//!   source of a loop), is an `Accesses` row of the method, and binds to the
//!   one field — from a method written in the class body, a constructor, and an
//!   out-of-line definition the file merges into its declaration;
//! - a parameter, local, structured binding, loop, catch or lambda variable of
//!   the same name in that method shadows the field: it records nothing, and
//!   the field read beside it in another method still does;
//! - `this->bump()` is a call and records no access, while `this->m_balance`
//!   still does;
//! - the near misses sit one token from matching and record nothing: a call of a
//!   callable field (`m_callback(1)`), the member of another object
//!   (`o.m_balance`), a name the class does not declare, a field the class
//!   inherits, a field the enclosing class of a nested one declares;
//! - C++ declares `member_access`, so Cohesion is computed from those links.
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite (`csharp_bare_access.rs`): a fixture tree checked into this
//! repository would be indexed into its own graph.

#![cfg(feature = "lang-cpp")]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::metrics::{self, RecordedSnapshot};
use logos_core::model::{EdgeKind, NodeId};
use logos_core::{Engine, Granularity, Runtime};
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

/// Every bound `Accesses` edge as `(method name, field name)`, sorted.
fn accesses(rt: &Runtime) -> Vec<(String, String)> {
    let names: HashMap<NodeId, String> = rt
        .submit_read(|store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .map(|n| (n.id, n.name))
                .collect())
        })
        .expect("read runs");
    let mut out: Vec<(String, String)> = rt
        .submit_read(|store| store.all_edges())
        .expect("read runs")
        .into_iter()
        .filter(|e| e.kind == EdgeKind::Accesses)
        .map(|e| (names[&e.source].clone(), names[&e.target].clone()))
        .collect();
    out.sort();
    out
}

/// Every `Accesses` ledger row, bound or not, as `(method name, field name)`.
fn access_rows(rt: &Runtime) -> Vec<(String, String)> {
    let by_symbol: HashMap<String, String> = rt
        .submit_read(|store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .map(|n| (n.symbol.as_str().to_string(), n.name))
                .collect())
        })
        .expect("read runs");
    let mut rows: Vec<(String, String)> = rt
        .submit_read(|store| store.unresolved_refs())
        .expect("read runs")
        .into_iter()
        .filter(|r| r.kind == EdgeKind::Accesses)
        .filter_map(|r| Some((by_symbol.get(&r.source_symbol)?.clone(), r.target)))
        .collect();
    rows.sort();
    rows.dedup();
    rows
}

fn pair(method: &str, field: &str) -> (String, String) {
    (method.to_string(), field.to_string())
}

const ACCOUNT: &str = "\
#include <vector>
class Account {
public:
  Account(int v) { m_balance = v; }
  int deposit(int amount) { m_balance += amount; m_log.push_back(amount); return m_balance; }
  int total() { return m_balance + m_limit; }
  int pick(bool b) { return b ? m_balance : m_limit; }
  bool over(int x) { return x > m_limit && check(m_limit); }
  bool check(int v) { return v > 0; }
  int at(int i) { return m_slots[i]; }
  int sum() { int t = 0; for (int s : m_slots) { t += s; } return t; }
  int gate() { if (m_limit) { return 1; } while (m_balance > 0) { m_balance--; } return 0; }
  int bump() { return ++m_count; }
  int self() { return this->m_balance + this->bump(); }
  int peek();
  int bare() { return s_total; }
  void assign(int v) { this->m_balance = v; }
  int viaParen(int x) { return (this->m_cb)(x); }
  int direct(int x) { return this->m_cb(x); }
private:
  std::function<int(int)> m_cb;
  int m_balance;
  int m_limit;
  int m_count;
  std::vector<int> m_log;
  std::vector<int> m_slots;
  static int s_total;
};
int Account::peek() { return m_balance; }
";

const SHADOWS: &str = "\
class Shadows {
public:
  int param(int m_balance) { return m_balance; }
  int pointer(int* m_limit) { return *m_limit; }
  int reference(const int& m_limit) { return m_limit; }
  int local() { int m_balance = 2; return m_balance + 1; }
  int init() { int a = 1, m_limit = 2; return a + m_limit; }
  int binding() { auto [m_balance, m_limit] = pair_; return m_balance; }
  int range() { int n = 0; for (auto& m_balance : list_) { n += m_balance; } return n; }
  int caught() { try { return 1; } catch (const E& m_limit) { return m_limit.code; } }
  int lambda() { auto f = [](int m_balance) { return m_balance + 1; }; return f(1) + m_limit; }
  int branch() { if (int m_balance = pick()) { return m_balance; } return 0; }
  int array() { int m_buf[4]; return m_buf[0]; }
  int arrayParam(int m_buf[]) { return m_buf[0]; }
  int arrayRef(int (&m_buf)[4]) { return m_buf[1]; }
  int pointerToPointer(int** m_limit) { return **m_limit; }
  int referenceToPointer(int*& m_limit) { return *m_limit; }
  int fnPointer(int (*m_cb)(int)) { if (m_cb) { return 1; } return 0; }
  int initCapture() { auto l = [m_cap = 5]() { return m_cap + 1; }; return l(); }
  int optional(int m_balance = 2) { return m_balance; }
  int pointerLocal() { int* m_limit = nullptr; return *m_limit; }
  int pointerDecl() { int* m_balance; return *m_balance; }
  int referenceLocal(int& x) { int& m_limit = x; return m_limit; }
  int functionLocal() { Widget m_balance(1); return m_balance.size(); }
  int doublePointer() { int** m_limit = nullptr; return **m_limit; }
  int rangePlain(int* xs) { int n = 0; for (int m_balance : xs) { n += m_balance; } return n; }
  int rangePointer(int** xs) { int n = 0; for (int* m_limit : xs) { n += *m_limit; } return n; }
  int untouched() { return m_balance + m_limit; }
  int pick() { return 1; }
private:
  int m_balance;
  int m_limit;
  int m_buf;
  int m_cb;
  int m_cap;
  int pair_;
  int list_;
};
";

const NEAR_MISSES: &str = "\
class Misses {
public:
  int callback() { return m_callback(1); }
  int member(Misses& o) { return o.m_balance; }
  int unknown() { return m_ghost + Other::value; }
  int viaThis() { return this->m_balance; }
  int viaThisCall() { return this->callback(); }
  int viaThisPointer() { return (*this).m_balance; }
  int declared() { int m_balance2 = 3; return m_balance2; }
private:
  int m_balance;
  std::function<int(int)> m_callback;
};
class Base { protected: int m_x; };
class Derived : public Base { public: int go() { return m_x; } };
class Outer { int m_o; public: class In { public: int go() { return m_o; } }; };
";

#[test]
fn a_bare_member_read_is_an_access() {
    let tmp = tree(&[("src/account.cpp", ACCOUNT)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let mut expected: Vec<(String, String)> = [
        ("Account", "m_balance"),
        ("assign", "m_balance"),
        ("bare", "s_total"),
        ("deposit", "m_balance"),
        ("deposit", "m_log"),
        ("gate", "m_balance"),
        ("gate", "m_limit"),
        ("over", "m_limit"),
        ("peek", "m_balance"),
        ("pick", "m_balance"),
        ("pick", "m_limit"),
        ("self", "m_balance"),
        ("sum", "m_slots"),
        ("at", "m_slots"),
        ("total", "m_balance"),
        ("total", "m_limit"),
        ("bump", "m_count"),
        ("viaParen", "m_cb"),
    ]
    .map(|(m, f)| pair(m, f))
    .to_vec();
    expected.sort();
    // `check`, the parameter reads (`v`, `x`, `i`, `s`) and `this->m_cb(x)`, a
    // call, record nothing.
    assert_eq!(accesses(rt), expected);
}

#[test]
fn this_arrow_a_call_is_not_an_access() {
    let tmp = tree(&[("src/account.cpp", ACCOUNT)]);
    let engine = index(tmp.path());
    let rows = access_rows(engine.runtime().unwrap());
    assert!(!rows.contains(&pair("self", "bump")), "`this->bump()` is a call: {rows:?}");
    assert!(rows.contains(&pair("self", "m_balance")), "`this->m_balance` still reads: {rows:?}");
}

#[test]
fn a_parameter_or_local_of_the_same_name_records_nothing() {
    let tmp = tree(&[("src/shadows.cpp", SHADOWS)]);
    let engine = index(tmp.path());
    assert_eq!(
        access_rows(engine.runtime().unwrap()),
        vec![
            // The structured binding and the loop variable hide the fields of
            // their names, and the sources they are read from are fields too.
            pair("binding", "pair_"),
            // The lambda's own parameter shadows `m_balance`; `m_limit` is not.
            pair("lambda", "m_limit"),
            pair("range", "list_"),
            // `untouched` reads both, with nothing to shadow either.
            pair("untouched", "m_balance"),
            pair("untouched", "m_limit"),
        ]
    );
}

#[test]
fn the_near_misses_record_nothing() {
    let tmp = tree(&[("src/misses.cpp", NEAR_MISSES)]);
    let engine = index(tmp.path());
    // Only `this->m_balance` — the pre-existing own-field form — records a row;
    // `(*this).m_balance`, a call, another object's member, and an unknown,
    // inherited or outer-class name record none.
    assert_eq!(
        access_rows(engine.runtime().unwrap()),
        vec![pair("viaThis", "m_balance")]
    );
}

/// One value position per method, each reading a field of its own that no other
/// method reads, so a position the query lost is a missing pair rather than one
/// another statement's read still supplies. `{f}` is the method's field.
const POSITIONS: &[&str] = &[
    "return use({f});",
    "return {f}.size();",
    "return {f} + 1;",
    "return 1 + {f};",
    "return -{f};",
    "{f}++; return 0;",
    "int y = 0; y = {f}; return y;",
    "{f} = 1; return 0;",
    "return {f} ? 1 : 0;",
    "return b ? {f} : 0;",
    "return b ? 0 : {f};",
    "int a[3] = {0}; return a[{f}];",
    "int* a = nullptr; return a[{f}];",
    "return *{f};",
    "return ({f});",
    "return (long){f};",
    "return ({f}, 1);",
    "return (use(), {f});",
    "if ({f}) { return 1; } return 0;",
    "throw {f};",
    "if (int y = {f}) { return y; } return 0;",
    "int a{{f}}; return a;",
    "int a = {f}; return a;",
    "int n = 0; for (int x : {f}) { n += x; } return n;",
    "auto l = [&r = {f}]() { return 1; }; return l();",
];

#[test]
fn a_bare_read_is_an_access_in_each_position_a_value_is_written() {
    let fields: String = (0..POSITIONS.len()).map(|i| format!("  int f{i};\n")).collect();
    let methods: String = POSITIONS
        .iter()
        .enumerate()
        .map(|(i, body)| format!("  int m{i:02}(bool b) {{ {} }}\n", body.replace("{f}", &format!("f{i}"))))
        .collect();
    let src = format!("class Positions {{\npublic:\n{methods}private:\n{fields}}};\n");
    let tmp = tree(&[("src/positions.cpp", &src)]);
    let engine = index(tmp.path());
    let mut expected: Vec<(String, String)> =
        (0..POSITIONS.len()).map(|i| (format!("m{i:02}"), format!("f{i}"))).collect();
    expected.sort();
    assert_eq!(accesses(engine.runtime().unwrap()), expected);
}

/// A pointer or array member is a field (S-582, FR-EX-31), so its bare read is
/// an access; a reference member is still no field, and its read records
/// nothing.
#[test]
fn a_pointer_or_array_member_is_a_field_and_a_reference_member_is_not() {
    let src = "class P { int* m_p; int& m_r; int m_a[3]; int m_v; public: P(int& r) : m_r(r) {} \
               int f() { return *m_p + m_r + m_a[0] + m_v; } };\n";
    let tmp = tree(&[("src/p.cpp", src)]);
    let engine = index(tmp.path());
    assert_eq!(access_rows(engine.runtime().unwrap()), vec![pair("f", "m_a"), pair("f", "m_p"), pair("f", "m_v")]);
}

#[test]
fn a_class_declared_in_a_header_is_not_seen_from_its_source_file() {
    let header = "class B { int m_n; public: int get(); int inline_get() { return m_n; } };\n";
    let source = "#include \"b.h\"\nint B::get() { return m_n; }\n";
    let tmp = tree(&[("src/b.h", header), ("src/b.cpp", source)]);
    let engine = index(tmp.path());
    // Only the in-header body reads the field: the definition in the source file
    // sits in a class this file does not declare, which a bare name cannot reach.
    assert_eq!(access_rows(engine.runtime().unwrap()), vec![pair("inline_get", "m_n")]);
}

#[test]
fn a_bare_read_is_one_row_at_its_line() {
    let src = "class A {\n  int m_n;\npublic:\n  int get() {\n    return m_n;\n  }\n};\n";
    let tmp = tree(&[("src/a.cpp", src)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let rows: Vec<(String, String, i64)> = rt
        .submit_read(|store| store.unresolved_refs())
        .expect("read runs")
        .into_iter()
        .filter(|r| r.kind == EdgeKind::Accesses)
        .map(|r| (r.source_symbol, r.target, r.line.unwrap_or(-1)))
        .collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!((rows[0].1.as_str(), rows[0].2), ("m_n", 5));
}

// ── Cohesion ─────────────────────────────────────────────────────────────────

/// A class of two field groups, each read and written by two methods: with the
/// bare accesses captured the methods of one field are one LCOM4 component and
/// the class has two; with only `this->x` captured it has four.
fn split_class(name: &str) -> String {
    format!(
        "class {name} {{ int m_a; int m_b; public: int getA() {{ return m_a; }} \
         void setA(int v) {{ m_a = v; }} int getB() {{ return m_b; }} void setB(int v) {{ m_b = v; }} }};"
    )
}

fn scan_classes(n: usize) -> (TempDir, RecordedSnapshot) {
    let files: Vec<(String, String)> = (0..n)
        .map(|i| (format!("src/box{i}.cpp"), split_class(&format!("Box{i}"))))
        .collect();
    let refs: Vec<(&str, &str)> = files.iter().map(|(p, t)| (p.as_str(), t.as_str())).collect();
    let tmp = tree(&refs);
    let engine = index(tmp.path());
    let view = engine
        .hydrate(Granularity::ExcludeContains)
        .expect("dependency view hydrates");
    let rt = engine.runtime().expect("runtime");
    let member_access = engine
        .registry()
        .expect("a started engine holds its registry")
        .member_access_extensions();
    let snapshot = metrics::snapshot(rt, &view, None, metrics::Thresholds::default(), &member_access)
        .expect("snapshot runs");
    (tmp, snapshot)
}

#[test]
fn cpp_cohesion_is_computed_from_the_bare_accesses() {
    let (_tmp, s) = scan_classes(5);
    assert!(s.metrics.cohesion.is_some(), "five C++ classes: Cohesion is scored, not n/a");
    assert!(s.metrics.cohesion_not_applicable.is_none());
    let lcom4: Vec<(&str, &str)> = s
        .worst_offenders
        .cohesion
        .iter()
        .map(|o| (o.name.as_str(), o.detail.as_str()))
        .collect();
    assert_eq!(lcom4.len(), 5, "{lcom4:?}");
    assert!(lcom4.iter().all(|(_, d)| *d == "LCOM4 2"), "{lcom4:?}");
}

// ── Sync ≡ reindex ───────────────────────────────────────────────────────────

const BEFORE: &str = "class A { int m_n; public: int get() { return m_n; } };\n";
const AFTER: &str = "class A { int m_n; int m_m; public: int get() { return m_n + m_m; } };\n";

#[test]
fn a_synced_edit_matches_a_fresh_reindex() {
    let tmp_a = tree(&[("src/a.cpp", BEFORE)]);
    let engine_a = index(tmp_a.path());
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    fs::write(tmp_a.path().join("src/a.cpp"), AFTER).unwrap();
    engine_a.sync(&[root_a.join("src/a.cpp")] as &[PathBuf]);
    let rt_a = engine_a.runtime().unwrap();

    let tmp_b = tree(&[("src/a.cpp", AFTER)]);
    let engine_b = index(tmp_b.path());
    assert_eq!(
        graph_fingerprint(rt_a),
        graph_fingerprint(engine_b.runtime().unwrap()),
        "sync-to-state must equal index-of-state"
    );
    assert_eq!(accesses(rt_a), vec![pair("get", "m_m"), pair("get", "m_n")]);
}
