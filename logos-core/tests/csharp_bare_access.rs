//! A bare C# field or property read is an access (S-556, CR-177, FR-EX-22,
//! FR-QM-11, NFR-RA-05, NFR-RA-06) — exercised end to end through the public
//! [`Engine`] façade against temp-directory fixtures, with the SHIPPED
//! `references.scm` and `plugin.toml` of the C# plugin (no query override).
//!
//! What the fixture pins, over methods of `App.Account`:
//!
//! - a bare `_balance`, `Name` or `Limit` that the enclosing class declares as
//!   a field or property, in the value positions real code spells them in (an
//!   operand, an argument, a receiver, a return, an expression body, an
//!   interpolation, a loop's source), is an `Accesses` row of the method, and
//!   binds to the one field;
//! - a local, parameter, lambda parameter, pattern variable or `out var` of the
//!   same name in that method shadows the field: it records nothing, and the
//!   field read beside it in another method still does;
//! - the near misses sit one token from matching and record nothing: a member
//!   name (`o._balance`), a type spelled like a property (`Status Status`), a
//!   `nameof(Name)`, the member of an object initializer (`new Foo { Name = … }`),
//!   a name the class does not declare, a name only a sibling `partial` part
//!   declares;
//! - C# declares `member_access`, so Cohesion is computed from those links.
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite (`csharp_call_forms.rs`): a fixture tree checked into this
//! repository would be indexed into its own graph.

#![cfg(feature = "lang-c-sharp")]

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
using System;
using System.Collections.Generic;
namespace App {
    public class Account {
        private int _balance;
        private readonly List<int> _log = new List<int>();
        public string Name { get; set; }
        public int Limit;
        public int[] Slots;

        public int Deposit(int amount) { _balance += amount; _log.Add(amount); return _balance; }
        public string Show() { return Name + Limit; }
        public string Say() { return $\"{Name}\"; }
        public int Total() => _balance + Limit;
        public int Sum() { var t = 0; foreach (var s in Slots) { t += s; } return t; }
        public bool Over(int x) { return x > Limit && Check(Limit); }
        public bool Check(int v) { return v > 0; }
        public int Pick(bool b) { return b ? _balance : Limit; }
        public int Ref() { return Wrap(Limit) + Wrap(value: _balance); }
        public int Wrap(int value) { return value; }
        public Foo Init() { return new Foo { Name = Name }; }
    }
    public class Foo { public string Name { get; set; } }
}
";

const SHADOWS: &str = "\
using System;
namespace App {
    public class Shadows {
        private int _balance;
        public string Name { get; set; }
        public int Limit;

        public int Param(int _balance) { return _balance; }
        public int Local() { var Name = \"x\"; return Name.Length; }
        public int Lambda() { Func<int, int> f = _balance => _balance + 1; return f(1) + Limit; }
        public int Pattern(object o) { if (o is int Limit) { return Limit; } return 0; }
        public int OutVar(string s) { int.TryParse(s, out var _balance); return _balance; }
        public int ForeachVar(int[] xs) { var n = 0; foreach (var _balance in xs) { n += _balance; } return n; }
        public int CatchVar() { try { return 1; } catch (Exception Name) { return Name.Message.Length; } }
        public int Untouched() { return _balance + Limit; }
    }
}
";

/// One shadowing form per method, each reading a field of the same name in a
/// position the query captures, so a form whose declaration the query forgot
/// would leak a row. `Later` declares its local after (and beside) the read.
const SHADOW_FORMS: &str = "\
using System;
using System.Linq;
namespace App {
    public class Pt { public int X; }
    public class Forms {
        private int a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12, a13, a14, a16;

        public int Later(bool b) { if (b) { return a1; } else { var a1 = 3; return a1; } }
        public int Recursive(object o) { return o is Pt { X: 1 } a2 ? a2.GetHashCode() : 0; }
        public int ListP(object o) { return o is [1, 2] a3 ? a3.GetHashCode() : 0; }
        public int Deconstruct() { var (a4, b) = (1, 2); return a4; }
        public int From(int[] xs) { return (from a5 in xs where a5 > 0 select 1).Count(); }
        public int Let(int[] xs) { return (from x in xs let a6 = x where a6 > 0 select x).Count(); }
        public int Join(int[] xs) { return (from x in xs join a7 in xs on x equals a7 where a7 > 0 select x).Count(); }
        public int LocalFn() { int Inner(int a8) { return a8; } return Inner(1); }
        public int Anon() { Func<int, int> f = delegate (int a9) { return a9; }; return f(1); }
        public int ForDecl() { var n = 0; for (int a10 = 0; a10 < 3; a10++) { n += a10; } return n; }
        public int Case(object o) { switch (o) { case int a11: return a11; } return 0; }
        public int VarP(object o) { return o is var a12 && a12 != null ? 1 : 0; }
        public int Nested(object t) { var (x, (a13, b)) = t; return a13; }
        public int Sw(object o) { switch (o) { case var a16 when a16 > 0: return 1; } return 0; }
        public int Into(int[] xs) { return (from x in xs join y in xs on x equals y into a14 where a14.Any() select x).Count(); }
    }
}
";

const NEAR_MISSES: &str = "\
using System;
namespace App {
    public enum Status { Open, Closed }
    public class Qualified {
        public App.Status Status { get; set; }
        public int Q() { return Status.Open.GetHashCode(); }
    }
    public class Aliased {
        public global::App.Status Status { get; set; }
        public int G() { return Status.Closed.GetHashCode(); }
    }
    public class Misses {
        private int _balance;
        public Status Status { get; set; }
        public string Name { get; set; }

        public int Member(Misses o) { return o._balance; }
        public Status Typed() { Status s = Status.Open; return s; }
        public Status Current() { return Status; }
        public string Named() { return nameof(Name); }
        public string Path() { return nameof(Name.Length) + nameof(_balance.ToString); }
        public Misses Object() { return new Misses { Name = \"x\" }; }
        public int Unknown() { return _ghost + Other.Value; }
        public int Field() { return this._balance; }
        public string Declared() { string Name2 = \"y\"; return Name2; }
        public int Pp => _balance;
        public int Acc { get { return _balance; } }
        private static int A = 1;
        private static int B = A + 1;
    }
    public class Base { protected int _x; }
    public class Derived : Base { public int Go() { return _x; } }
    public class Outer { private int _o; public class In { public int Go() { return _o; } } }
    public class Maybe {
        public Status? Status { get; set; }
        public int N() { return Status.Open.GetHashCode(); }
    }
}
";

#[test]
fn a_bare_field_or_property_read_is_an_access() {
    let tmp = tree(&[("src/Account.cs", ACCOUNT)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let mut expected: Vec<(String, String)> = [
        ("Deposit", "_balance"),
        ("Deposit", "_log"),
        ("Init", "Name"),
        ("Over", "Limit"),
        ("Pick", "Limit"),
        ("Pick", "_balance"),
        ("Ref", "Limit"),
        ("Ref", "_balance"),
        ("Say", "Name"),
        ("Show", "Limit"),
        ("Show", "Name"),
        ("Sum", "Slots"),
        ("Total", "Limit"),
        ("Total", "_balance"),
    ]
    .map(|(m, f)| pair(m, f))
    .to_vec();
    expected.sort();
    // `Check`, `Wrap` and the parameter reads (`v`, `value`, `x`) record nothing.
    assert_eq!(accesses(rt), expected);
}

/// One value position per member, each reading a field of its own that no
/// other member reads, so a position the query lost is a missing pair rather
/// than one another member's read still supplies. `@` is the member's name.
const POSITIONS: &[(&str, &str)] = &[
    ("f01", "void @() { Take(f01); }"),
    ("f02", "void @() { Take(v: f02); }"),
    ("f03", "void @() { f03.ToString(); }"),
    ("f04", "void @() { f04?.ToString(); }"),
    ("f05", "void @() { var x = f05[0]; }"),
    ("f06", "void @() { f06 = 1; }"),
    ("f07", "void @(int p) { p = f07; }"),
    ("f08", "void @() { var x = f08 + 1; }"),
    ("f09", "void @() { var x = 1 + f09; }"),
    ("f10", "void @() { var x = f10 ? 1 : 2; }"),
    ("f11", "void @() { var x = true ? f11 : 2; }"),
    ("f12", "void @() { var x = false ? 1 : f12; }"),
    ("f13", "void @() { var x = !f13; }"),
    ("f14", "void @() { f14++; }"),
    ("f15", "void @() { var x = (f15); }"),
    ("f16", "async Task<int> @() { return await f16; }"),
    ("f17", "void @() { var x = (long)f17; }"),
    ("f18", "void @() { var x = f18 as string; }"),
    ("f19", "void @() { var x = f19 is string; }"),
    ("f20", "void @() { if (f20 is string s) { } }"),
    ("f21", "void @() { var x = f21..3; }"),
    ("f22", "void @() { var x = f22; }"),
    ("f23", "void @() { var x = new[] { f23 }; }"),
    ("f24", "void @() { Func<int> g = () => f24; }"),
    ("f25", "object @() => f25;"),
    ("f26", "IEnumerable<int> @() { yield return f26; }"),
    ("f27", "void @() { throw f27; }"),
    ("f28", "void @() { var x = f28 ?? 1; }"),
    ("f29", "void @() { var x = true ? 1 : throw f29; }"),
    ("f30", "void @() { if (f30) { } }"),
    ("f31", "void @() { while (f31) { } }"),
    ("f32", "void @() { do { } while (f32); }"),
    ("f33", "void @() { for (;f33;) { } }"),
    ("f34", "void @() { switch (f34) { default: break; } }"),
    ("f35", "void @() { var x = f35 switch { _ => 1 }; }"),
    ("f36", "void @() { var x = 1 switch { _ => f36 }; }"),
    (
        "f37",
        "void @(object o) { switch (o) { case int i when f37: break; } }",
    ),
    ("f38", "void @() { lock (f38) { } }"),
    ("f39", "void @() { using (f39) { } }"),
    ("f40", "void @() { foreach (var x in f40) { } }"),
    ("f41", "void @() { var s = $\"{f41}\"; }"),
];

#[test]
fn a_name_is_an_access_in_each_position_a_value_is_written() {
    let fields: Vec<&str> = POSITIONS.iter().map(|(field, _)| *field).collect();
    let mut src = format!(
        "namespace App {{ public class Forms {{ object {}; void Take(object v) {{ }}\n",
        fields.join(", ")
    );
    let mut expected: Vec<(String, String)> = Vec::new();
    for (i, (field, member)) in POSITIONS.iter().enumerate() {
        let name = format!("M{i:02}");
        src.push_str(&format!("{}\n", member.replace('@', &name)));
        expected.push(pair(&name, field));
    }
    src.push_str("} }\n");
    expected.sort();
    let tmp = tree(&[("src/Forms.cs", &src)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(accesses(rt), expected);
}

/// Each read is one ledger row on the line it is written on, however often
/// the method repeats it.
#[test]
fn a_bare_read_is_one_row_at_its_line() {
    let tmp = tree(&[("src/Account.cs", ACCOUNT)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let line_of =
        |needle: &str| 1 + ACCOUNT.lines().position(|l| l.contains(needle)).unwrap() as u32;
    let mut got: Vec<(String, Option<i64>)> = rt
        .submit_read(|store| store.unresolved_refs())
        .expect("read runs")
        .into_iter()
        .filter(|r| {
            r.kind == EdgeKind::Accesses
                && r.target == "_balance"
                && r.source_symbol.contains("Deposit")
        })
        .map(|r| (r.target, r.line))
        .collect();
    got.sort();
    // `_balance += amount;` and `return _balance;` share the line of `Deposit`.
    assert_eq!(
        got,
        vec![(
            "_balance".to_string(),
            Some(i64::from(line_of("int Deposit")))
        )]
    );
}

#[test]
fn a_local_or_parameter_of_the_same_name_records_nothing() {
    let tmp = tree(&[("src/Shadows.cs", SHADOWS)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        access_rows(rt),
        vec![
            // The lambda's own parameter shadows `_balance`; `Limit` does not.
            pair("Lambda", "Limit"),
            // Untouched reads both, with nothing to shadow either.
            pair("Untouched", "Limit"),
            pair("Untouched", "_balance"),
        ]
    );
}

#[test]
fn every_shadowing_form_hides_the_field_whichever_way_round_it_is_written() {
    let tmp = tree(&[("src/Forms.cs", SHADOW_FORMS)]);
    let engine = index(tmp.path());
    assert_eq!(access_rows(engine.runtime().unwrap()), vec![]);
}

#[test]
fn the_near_misses_record_nothing() {
    let tmp = tree(&[("src/Misses.cs", NEAR_MISSES)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    // The written `this._balance` — the pre-existing own-field form — and the
    // property `Status` read as a value. `Status.Open` is the enum's, though a
    // property is spelled alike: the syntax cannot say which, so it records nothing.
    assert_eq!(
        access_rows(rt),
        vec![pair("Current", "Status"), pair("Field", "_balance")]
    );
}

#[test]
fn a_field_only_a_sibling_partial_part_declares_is_not_seen() {
    let a = "namespace App { public partial class P { public int Go() { return _hidden; } } }";
    let b = "namespace App { public partial class P { private int _hidden; } }";
    let tmp = tree(&[("src/A.cs", a), ("src/B.cs", b)]);
    let engine = index(tmp.path());
    assert_eq!(access_rows(engine.runtime().unwrap()), vec![]);
}

// ── Cohesion ─────────────────────────────────────────────────────────────────

/// A class of two field groups, each read and written by two methods: with
/// the bare accesses captured the methods of one field are one LCOM4 component
/// and the class has two; with only `this.x` captured it has four.
fn split_class(name: &str) -> String {
    format!(
        "namespace App {{ public class {name} {{ private int _a; private int _b; \
         public int GetA() => _a; public void SetA(int v) {{ _a = v; }} \
         public int GetB() {{ return _b; }} public void SetB(int v) {{ _b = v; }} }} }}"
    )
}

fn scan_classes(n: usize) -> (TempDir, RecordedSnapshot) {
    let files: Vec<(String, String)> = (0..n)
        .map(|i| (format!("src/Box{i}.cs"), split_class(&format!("Box{i}"))))
        .collect();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, t)| (p.as_str(), t.as_str()))
        .collect();
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
    let snapshot = metrics::snapshot(
        rt,
        &view,
        None,
        metrics::Thresholds::default(),
        &member_access,
    )
    .expect("snapshot runs");
    (tmp, snapshot)
}

#[test]
fn c_sharp_cohesion_is_computed_from_the_bare_accesses() {
    let (_tmp, s) = scan_classes(5);
    assert!(
        s.metrics.cohesion.is_some(),
        "five C# classes: Cohesion is scored, not n/a"
    );
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

const BEFORE: &str = "\
namespace App { public class A { private int _n; public int Get() { return _n; } } }
";
const AFTER: &str = "\
namespace App { public class A { private int _n; private int _m; public int Get() { return _n + _m; } } }
";

#[test]
fn a_synced_edit_matches_a_fresh_reindex() {
    let tmp_a = tree(&[("src/A.cs", BEFORE)]);
    let engine_a = index(tmp_a.path());
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    fs::write(tmp_a.path().join("src/A.cs"), AFTER).unwrap();
    engine_a.sync(&[root_a.join("src/A.cs")] as &[PathBuf]);
    let rt_a = engine_a.runtime().unwrap();

    let tmp_b = tree(&[("src/A.cs", AFTER)]);
    let engine_b = index(tmp_b.path());
    assert_eq!(
        graph_fingerprint(rt_a),
        graph_fingerprint(engine_b.runtime().unwrap()),
        "sync-to-state must equal index-of-state"
    );
    assert_eq!(accesses(rt_a), vec![pair("Get", "_m"), pair("Get", "_n")]);
}
