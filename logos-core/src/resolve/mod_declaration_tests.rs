//! A bodyless Rust `mod x;` is the file module it declares (S-585, [FR-RS-41],
//! [NFR-RA-05]), over a synthetic snapshot. The extractor keeps the declaration
//! as a `Module` node of the declaring file, keyed where the file it declares is
//! keyed; a path through that key must meet the file, never the empty
//! declaration — whatever the node ids. End to end, against real Rust, in
//! `tests/rust_mod_declarations.rs`.
//!
//! Every declaring file below has a **lower** id than the file it declares, the
//! order that used to hand the key to the declaration:
//!
//! ```text
//! src/lib.rs        (module 1)  ─ mod util; (2)  mod a; (3)  mod x; (4)
//! │                               mod missing; (5)  fn alpha (6)
//! │                               mod inner { fn deep (8) } (7)
//! src/util.rs       (module 10) ─ fn run (11)
//! src/a/mod.rs  or  src/a.rs   (module 20) ─ mod b; (21)  fn helper (22)
//! src/a/b.rs        (module 30) ─ fn c (31)  fn up (32)
//! src/x_impl.rs     (module 40) ─ fn go (41)        [what `#[path = "x_impl.rs"] mod x;` names]
//! src/inner.rs      (module 50) ─ fn deep (51)      [an orphan beside the inline `mod inner`]
//! ```
//!
//! [FR-RS-41]: ../../../docs/specs/requirements/FR-RS-41.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md

use super::binder::{bind, Index, Outcome};
use crate::config::BindingPolicy;
use crate::graph_store::{EdgeRow, NodeRow, UnresolvedRefRow};
use crate::model::{EdgeKind, LogosSymbol, NodeId, NodeKind, RefForm};

const LIB_RS: i64 = 60;
const A_RS: i64 = 61;
const B_RS: i64 = 62;

const POLICIES: [BindingPolicy; 3] = [
    BindingPolicy::Strict,
    BindingPolicy::Balanced,
    BindingPolicy::Aggressive,
];

fn node(id: i64, name: &str, kind: NodeKind, file: &str) -> NodeRow {
    NodeRow {
        id: NodeId(id),
        symbol: LogosSymbol::parse(&format!("local sym{id}")).unwrap(),
        kind,
        name: name.to_string(),
        file_path: Some(file.to_string()),
        start_line: None,
        end_line: None,
    }
}

/// A node spanning `start..=end` — how a declaration (one line) and an inline
/// module (its braces) differ when both are childless.
fn spanning(mut n: NodeRow, start: i64, end: i64) -> NodeRow {
    n.start_line = Some(start);
    n.end_line = Some(end);
    n
}

fn contains(source: i64, target: i64) -> EdgeRow {
    EdgeRow {
        source: NodeId(source),
        target: NodeId(target),
        kind: EdgeKind::Contains,
    }
}

/// The crate above, with `a` declared by `a_file` (`src/a/mod.rs` or `src/a.rs`).
fn graph(a_file: &str) -> (Vec<NodeRow>, Vec<EdgeRow>) {
    let nodes = vec![
        node(1, "crate", NodeKind::Module, "src/lib.rs"),
        spanning(node(2, "util", NodeKind::Module, "src/lib.rs"), 1, 1),
        spanning(node(3, "a", NodeKind::Module, "src/lib.rs"), 2, 2),
        spanning(node(4, "x", NodeKind::Module, "src/lib.rs"), 4, 4),
        spanning(node(5, "missing", NodeKind::Module, "src/lib.rs"), 5, 5),
        node(6, "alpha", NodeKind::Function, "src/lib.rs"),
        spanning(node(7, "inner", NodeKind::Module, "src/lib.rs"), 9, 11),
        node(8, "deep", NodeKind::Function, "src/lib.rs"),
        node(10, "util", NodeKind::Module, "src/util.rs"),
        node(11, "run", NodeKind::Function, "src/util.rs"),
        node(20, "a", NodeKind::Module, a_file),
        spanning(node(21, "b", NodeKind::Module, a_file), 1, 1),
        node(22, "helper", NodeKind::Function, a_file),
        node(30, "b", NodeKind::Module, "src/a/b.rs"),
        node(31, "c", NodeKind::Function, "src/a/b.rs"),
        node(32, "up", NodeKind::Function, "src/a/b.rs"),
        node(40, "x_impl", NodeKind::Module, "src/x_impl.rs"),
        node(41, "go", NodeKind::Function, "src/x_impl.rs"),
        node(50, "inner", NodeKind::Module, "src/inner.rs"),
        node(51, "deep", NodeKind::Function, "src/inner.rs"),
    ];
    let edges = vec![
        contains(1, 2),
        contains(1, 3),
        contains(1, 4),
        contains(1, 5),
        contains(1, 6),
        contains(1, 7),
        contains(7, 8),
        contains(10, 11),
        contains(20, 21),
        contains(20, 22),
        contains(30, 31),
        contains(30, 32),
        contains(40, 41),
        contains(50, 51),
    ];
    (nodes, edges)
}

fn row(id: i64, file_id: i64, source: i64, target: &str, alias: Option<&str>, kind: EdgeKind) -> UnresolvedRefRow {
    UnresolvedRefRow {
        id,
        file_id: Some(file_id),
        source_symbol: format!("local sym{source}"),
        target: target.to_string(),
        alias: alias.map(str::to_string),
        form: RefForm::Path,
        kind,
        line: Some(1),
        resolved: false,
        payload: None,
        receiver: None,
    }
}

fn import(id: i64, file_id: i64, source: i64, target: &str) -> UnresolvedRefRow {
    let alias = target.rsplit("::").next();
    row(id, file_id, source, target, alias, EdgeKind::Imports)
}

fn call(id: i64, file_id: i64, source: i64, target: &str) -> UnresolvedRefRow {
    row(id, file_id, source, target, None, EdgeKind::Calls)
}

/// `r` bound against `graph` (with `scope` rows giving its file's imports)
/// under every policy, each of which must agree.
fn outcome_in(graph: &(Vec<NodeRow>, Vec<EdgeRow>), scope: &[UnresolvedRefRow], r: &UnresolvedRefRow) -> Outcome {
    let mut rows = scope.to_vec();
    rows.push(r.clone());
    let ix = Index::build(&graph.0, &graph.1, &rows);
    let outcomes: Vec<Outcome> = POLICIES.iter().map(|&p| bind(r, &ix, p)).collect();
    assert!(
        outcomes.windows(2).all(|w| w[0] == w[1]),
        "{}: the policies disagree: {outcomes:?}",
        r.target
    );
    outcomes[0].clone()
}

fn outcome(scope: &[UnresolvedRefRow], r: &UnresolvedRefRow) -> Outcome {
    outcome_in(&graph("src/a/mod.rs"), scope, r)
}

fn bound(source: i64, target: i64, kind: EdgeKind) -> Outcome {
    Outcome::Bound {
        source: NodeId(source),
        target: NodeId(target),
        kind,
        payload: None,
    }
}

/// FR-RS-41 AC: `pub mod util;` + `use crate::util::run;` + `run()` binds the
/// import and the call into `util.rs`.
#[test]
fn an_import_and_a_call_through_a_declared_module_bind_into_its_file() {
    let import_row = import(100, LIB_RS, 1, "crate::util::run");
    assert_eq!(outcome(&[], &import_row), bound(1, 11, EdgeKind::Imports));
    let run = call(101, LIB_RS, 6, "run");
    assert_eq!(outcome(&[import_row], &run), bound(6, 11, EdgeKind::Calls));
}

/// A path naming the declared module itself reaches the file module, never the
/// declaration: `use crate::util;`, a glob through it, and a bare-head call.
#[test]
fn the_declared_module_itself_is_its_file_module() {
    assert_eq!(
        outcome(&[], &import(100, LIB_RS, 1, "crate::util")),
        bound(1, 10, EdgeKind::Imports)
    );
    let mut glob = import(100, LIB_RS, 1, "crate::util");
    glob.form = RefForm::Glob;
    glob.alias = None;
    assert_eq!(outcome(&[glob], &call(101, LIB_RS, 6, "run")), bound(6, 11, EdgeKind::Calls));
    assert_eq!(
        outcome(&[], &call(101, LIB_RS, 6, "util::run")),
        bound(6, 11, EdgeKind::Calls)
    );
}

/// FR-RS-41 AC: `use crate::a::b::c` through `mod a;` → `a/mod.rs` (and
/// `a.rs`) → `mod b;` → `b.rs` binds at every level; `self::` from `a` and
/// `super::` from `b` bind through the declarations.
#[test]
fn chained_declarations_bind_at_every_level_in_both_layouts() {
    for a_file in ["src/a/mod.rs", "src/a.rs"] {
        let g = graph(a_file);
        assert_eq!(
            outcome_in(&g, &[], &import(100, LIB_RS, 1, "crate::a::b::c")),
            bound(1, 31, EdgeKind::Imports),
            "{a_file}"
        );
        assert_eq!(
            outcome_in(&g, &[], &call(101, A_RS, 22, "self::b::c")),
            bound(22, 31, EdgeKind::Calls),
            "{a_file}"
        );
        assert_eq!(
            outcome_in(&g, &[], &call(102, B_RS, 32, "super::helper")),
            bound(32, 22, EdgeKind::Calls),
            "{a_file}"
        );
        assert_eq!(
            outcome_in(&g, &[], &call(103, B_RS, 32, "crate::util::run")),
            bound(32, 11, EdgeKind::Calls),
            "{a_file}"
        );
    }
}

/// FR-RS-41 AC: a declaration whose file is not where its path puts it stays
/// unresolved — `#[path = "x_impl.rs"] mod x;` (the attribute is not read, so
/// `x_impl.rs` is never guessed) and `mod missing;` with no file. The
/// declaration is then the only module at the key, and it holds nothing.
#[test]
fn a_declaration_with_no_file_at_its_path_stays_unresolved() {
    for target in ["crate::x::go", "crate::missing::f"] {
        assert_eq!(outcome(&[], &import(100, LIB_RS, 1, target)), Outcome::Unbound, "{target}");
        assert_eq!(outcome(&[], &call(101, LIB_RS, 6, target)), Outcome::Unbound, "{target}");
    }
}

/// FR-RS-41: two files at a declaration's path (`dup.rs` and `dup/mod.rs`,
/// which rustc refuses as E0761) are two candidates — the declaration resolves
/// to neither, even when a file's id would have won the key first.
#[test]
fn a_declaration_with_two_candidate_files_stays_unresolved() {
    let nodes = vec![
        node(1, "dup", NodeKind::Module, "src/dup.rs"),
        node(2, "f", NodeKind::Function, "src/dup.rs"),
        node(3, "dup", NodeKind::Module, "src/dup/mod.rs"),
        node(4, "f", NodeKind::Function, "src/dup/mod.rs"),
        node(5, "crate", NodeKind::Module, "src/lib.rs"),
        spanning(node(6, "dup", NodeKind::Module, "src/lib.rs"), 1, 1),
        node(7, "alpha", NodeKind::Function, "src/lib.rs"),
    ];
    let edges = vec![contains(1, 2), contains(3, 4), contains(5, 6), contains(5, 7)];
    let g = (nodes, edges);
    assert_eq!(outcome_in(&g, &[], &import(100, LIB_RS, 5, "crate::dup::f")), Outcome::Unbound);
    assert_eq!(outcome_in(&g, &[], &call(101, LIB_RS, 7, "crate::dup::f")), Outcome::Unbound);
}

/// FR-RS-41 AC: an inline `mod inner { … }` binds as before — it is a module
/// with its own contents, never a declaration of the orphan `inner.rs` beside
/// it — and so does an empty inline module spread over several lines.
#[test]
fn an_inline_module_binds_as_before() {
    assert_eq!(
        outcome(&[], &call(100, LIB_RS, 6, "crate::inner::deep")),
        bound(6, 8, EdgeKind::Calls)
    );
    assert_eq!(
        outcome(&[], &call(100, LIB_RS, 6, "inner::deep")),
        bound(6, 8, EdgeKind::Calls)
    );
    // `mod inner {\n}`: childless, but its braces span lines — not a declaration.
    let (mut nodes, mut edges) = graph("src/a/mod.rs");
    edges.retain(|e| e.target != NodeId(8));
    nodes.retain(|n| n.id != NodeId(8));
    let g = (nodes, edges);
    assert_eq!(
        outcome_in(&g, &[], &import(100, LIB_RS, 1, "crate::inner")),
        bound(1, 7, EdgeKind::Imports)
    );
    // `mod inner { pub fn deep() {} }` on one line: its contents make it a
    // module, not a declaration. Beside it, an orphan `inner.rs` holding one
    // line and nothing else is a file, never a declaration either.
    let nodes = vec![
        node(1, "crate", NodeKind::Module, "src/lib.rs"),
        node(6, "alpha", NodeKind::Function, "src/lib.rs"),
        spanning(node(7, "inner", NodeKind::Module, "src/lib.rs"), 9, 9),
        node(8, "deep", NodeKind::Function, "src/lib.rs"),
        spanning(node(50, "inner", NodeKind::Module, "src/inner.rs"), 1, 1),
    ];
    let edges = vec![contains(1, 6), contains(1, 7), contains(7, 8)];
    let g = (nodes, edges);
    assert_eq!(
        outcome_in(&g, &[], &call(100, LIB_RS, 6, "crate::inner::deep")),
        bound(6, 8, EdgeKind::Calls)
    );
}

/// The answer never depends on node ids, which a re-extraction renews
/// ([NFR-RA-06]): with the declared file **before** its declarer, the path
/// binds the same file.
///
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
#[test]
fn the_declared_file_answers_whatever_the_node_ids() {
    let nodes = vec![
        node(1, "util", NodeKind::Module, "src/util.rs"),
        node(2, "run", NodeKind::Function, "src/util.rs"),
        node(3, "crate", NodeKind::Module, "src/lib.rs"),
        spanning(node(4, "util", NodeKind::Module, "src/lib.rs"), 1, 1),
        node(5, "alpha", NodeKind::Function, "src/lib.rs"),
    ];
    let edges = vec![contains(1, 2), contains(3, 4), contains(3, 5)];
    let g = (nodes, edges);
    assert_eq!(
        outcome_in(&g, &[], &import(100, LIB_RS, 3, "crate::util")),
        bound(3, 1, EdgeKind::Imports)
    );
    assert_eq!(
        outcome_in(&g, &[], &call(101, LIB_RS, 5, "crate::util::run")),
        bound(5, 2, EdgeKind::Calls)
    );
}

/// A declaration names a file of its own language (review fix): the path model
/// keys `src/x.js` where it keys `src/x.rs`, and a Rust `mod x;` never binds
/// into the JavaScript file — whether the JavaScript file is alone at the path
/// (and numbered first, the order that used to hand it the key, or after), or
/// beside the `x.rs` the declaration does name, which is then the one candidate.
#[test]
fn only_a_file_of_the_declarations_language_answers_it() {
    let rust_crate = |js_first: bool, with_rs: bool| {
        let (lib, decl, alpha, js, js_run) = if js_first { (3, 4, 5, 1, 2) } else { (1, 2, 3, 10, 11) };
        let mut nodes = vec![
            node(lib, "crate", NodeKind::Module, "src/lib.rs"),
            spanning(node(decl, "x", NodeKind::Module, "src/lib.rs"), 1, 1),
            node(alpha, "alpha", NodeKind::Function, "src/lib.rs"),
            node(js, "x", NodeKind::Module, "src/x.js"),
            node(js_run, "run", NodeKind::Function, "src/x.js"),
        ];
        let mut edges = vec![contains(lib, decl), contains(lib, alpha), contains(js, js_run)];
        if with_rs {
            nodes.push(node(20, "x", NodeKind::Module, "src/x.rs"));
            nodes.push(node(21, "run", NodeKind::Function, "src/x.rs"));
            edges.push(contains(20, 21));
        }
        nodes.sort_by_key(|n| n.id);
        ((nodes, edges), lib, alpha)
    };
    for js_first in [false, true] {
        let (g, lib, alpha) = rust_crate(js_first, false);
        assert_eq!(
            outcome_in(&g, &[], &import(100, LIB_RS, lib, "crate::x::run")),
            Outcome::Unbound,
            "js_first={js_first}"
        );
        assert_eq!(
            outcome_in(&g, &[], &call(101, LIB_RS, alpha, "crate::x::run")),
            Outcome::Unbound,
            "js_first={js_first}"
        );
        let (g, _, alpha) = rust_crate(js_first, true);
        assert_eq!(
            outcome_in(&g, &[], &call(101, LIB_RS, alpha, "crate::x::run")),
            bound(alpha, 21, EdgeKind::Calls),
            "js_first={js_first}"
        );
    }
}
