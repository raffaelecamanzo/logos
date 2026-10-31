//! Go declares every type, top-level variable, interface method and struct field
//! (S-525, CR-172, FR-EX-15, FR-EX-05, NFR-RA-05, NFR-RA-06) — exercised end to
//! end through the public [`Engine`] façade against temp-directory fixtures, and
//! at the extraction seam for the facts no store column holds.
//!
//! - Every top-level `type` is a node: a struct a `Struct`, an interface an
//!   `Interface`, any other defined type or alias a `TypeAlias` — the alias
//!   (`type A = B`) flagged apart from the defined type (`type L int8`).
//! - A top-level `var` / `const` is a node, single, grouped (`var ( … )`) and
//!   every name of a multi-name spec; a variable or constant declared in a
//!   function body is none.
//! - An interface's method specs are bodyless `Method` nodes under it, and a
//!   struct's fields are `Field` nodes under it, each name of `a, b int` one.
//! - An embedded field is a `Field` named for the embedded type and records an
//!   `Extends` row from the embedding struct.
//!
//! Fixtures are written inline into temp directories, like every sibling suite:
//! a `.go` fixture tree checked into this repository would be indexed into its
//! own graph.

#![cfg(feature = "lang-go")]

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::Path;

use logos_core::extract::{extract, FileInput, SymbolContext};
use logos_core::model::{EdgeKind, NodeId, NodeKind};
use logos_core::plugin::LanguageRegistry;
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

#[path = "support/coordinate.rs"]
mod coordinate;

#[path = "support/graph_fingerprint.rs"]
mod graph_fingerprint;
use graph_fingerprint::graph_fingerprint;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn tree(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (rel, text) in files {
        write(tmp.path(), rel, text);
    }
    tmp
}

fn index(root: &Path) -> Engine {
    coordinate::pin(root);
    let engine = Engine::start(root).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    engine
}

/// A node's label: `Kind name`, and `Kind Parent.name` for one a type contains.
fn declarations(rt: &Runtime) -> BTreeSet<String> {
    let nodes = rt.submit_read(|store| store.all_nodes()).expect("read runs");
    let edges = rt.submit_read(|store| store.all_edges()).expect("read runs");
    let by_id: HashMap<NodeId, (NodeKind, String)> =
        nodes.iter().map(|n| (n.id, (n.kind, n.name.clone()))).collect();
    let parent: HashMap<NodeId, NodeId> = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Contains)
        .map(|e| (e.target, e.source))
        .collect();
    nodes
        .iter()
        .filter(|n| n.kind != NodeKind::Module)
        .map(|n| match parent.get(&n.id).map(|p| &by_id[p]) {
            Some((NodeKind::Module, _)) | None => format!("{:?} {}", n.kind, n.name),
            Some((_, owner)) => format!("{:?} {owner}.{}", n.kind, n.name),
        })
        .collect()
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// Every `Extends` row an embedded field records, as `(owner, target)`: bound
/// ones become edges, the rest stay in the ledger.
fn extends(rt: &Runtime) -> BTreeSet<(String, String)> {
    let nodes = rt.submit_read(|store| store.all_nodes()).expect("read runs");
    let name_of: HashMap<NodeId, String> = nodes.iter().map(|n| (n.id, n.name.clone())).collect();
    let mut out: BTreeSet<(String, String)> = rt
        .submit_read(|store| store.all_edges())
        .expect("read runs")
        .into_iter()
        .filter(|e| e.kind == EdgeKind::Extends)
        .map(|e| (name_of[&e.source].clone(), name_of[&e.target].clone()))
        .collect();
    let symbol_name: HashMap<String, String> =
        nodes.iter().map(|n| (n.symbol.as_str().to_string(), n.name.clone())).collect();
    out.extend(
        rt.submit_read(|store| store.unresolved_refs())
            .expect("read runs")
            .into_iter()
            .filter(|r| r.kind == EdgeKind::Extends && !r.resolved)
            .map(|r| (symbol_name[&r.source_symbol].clone(), r.target)),
    );
    out
}

const TYPES: &str = "\
package zapcore

type Level int8

type Capability string

type A = B

type B struct{}

type (
	Hook   func(int)
	Level2 = Level
)

type Pair[K comparable, V any] map[K]V

type Ptr *int

type Arr [4]int

type Sl []int

type Ch chan int

type Qual sync.Locker

type Inst Pair[string, int]

func f() {
	type localDefined int
	type localAlias = int
}
";

#[test]
fn every_top_level_type_is_a_node_and_the_alias_is_flagged_apart_from_the_defined_type() {
    let tmp = tree(&[("zapcore/types.go", TYPES)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let got = declarations(rt);
    for want in [
        "TypeAlias Level",
        "TypeAlias Capability",
        "TypeAlias A",
        "Struct B",
        "TypeAlias Hook",
        "TypeAlias Level2",
        "TypeAlias Pair",
        "TypeAlias Ptr",
        "TypeAlias Arr",
        "TypeAlias Sl",
        "TypeAlias Ch",
        "TypeAlias Qual",
        "TypeAlias Inst",
        "Function f",
    ] {
        assert!(got.contains(want), "{want} is declared; got {got:#?}");
    }
    for local in ["localDefined", "localAlias"] {
        assert!(
            !got.iter().any(|d| d.contains(local)),
            "{local} is declared in a function body and is no top-level type; got {got:#?}"
        );
    }
}

/// The facts of one Go file, straight from the extraction seam.
fn facts_of(path: &str, source: &str) -> logos_core::extract::Facts {
    let tmp = tempfile::tempdir().expect("tempdir");
    let registry = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
    let plugin = registry.for_extension("go").expect("the Go plugin is embedded");
    extract(&FileInput::new(path, source), plugin, &SymbolContext::default())
}

#[test]
fn a_type_alias_is_flagged_and_a_defined_type_is_not() {
    let facts = facts_of("zapcore/types.go", TYPES);
    let aliased: BTreeSet<&str> = facts
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::TypeAlias && n.alias)
        .map(|n| n.name.as_str())
        .collect();
    let defined: BTreeSet<&str> = facts
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::TypeAlias && !n.alias)
        .map(|n| n.name.as_str())
        .collect();
    assert_eq!(aliased, BTreeSet::from(["A", "Level2"]), "`=` declares an alias");
    assert_eq!(
        defined,
        BTreeSet::from(["Level", "Capability", "Hook", "Pair", "Ptr", "Arr", "Sl", "Ch", "Qual", "Inst"]),
        "no `=` is a defined type"
    );
    assert!(
        facts.nodes.iter().filter(|n| n.kind != NodeKind::TypeAlias).all(|n| !n.alias),
        "only a TypeAlias is ever an alias"
    );
}

const VARS: &str = "\
package zap

import \"errors\"

var _globalL = 1

var single int

var (
	one       = 1
	two, three = 2, 3
	typed     string
)

const Top = 1

const (
	C1 = iota
	C2, C3 = 2, 3
)

var _ = errors.New

func work() {
	var local = 1
	var (
		grouped = 2
	)
	const lc = 3
	short := 4
	_ = local + grouped + lc + short
}
";

#[test]
fn a_top_level_var_or_const_is_a_node_single_grouped_or_multi_name_and_a_local_one_is_none() {
    let tmp = tree(&[("zap/vars.go", VARS)]);
    let engine = index(tmp.path());
    let got = declarations(engine.runtime().unwrap());
    for want in [
        "Variable _globalL",
        "Variable single",
        "Variable one",
        "Variable two",
        "Variable three",
        "Variable typed",
        "Constant Top",
        "Constant C1",
        "Constant C2",
        "Constant C3",
        "Function work",
    ] {
        assert!(got.contains(want), "{want} is declared; got {got:#?}");
    }
    for local in ["local", "grouped", "lc", "short"] {
        assert!(
            !got.iter().any(|d| d.ends_with(&format!(" {local}")) || d.ends_with(&format!(".{local}"))),
            "{local} is declared in a function body and is no node; got {got:#?}"
        );
    }
}

const FIELDS: &str = "\
package zapcore

import (
	\"io\"
	\"sync\"
	\"sync/atomic\"
)

type Core interface {
	Check(e int) bool
	Write(e int, fields []int) error
	Sync() error
}

type Logger struct {
	core   Core
	name   string
	x, y   int
	w      io.Writer
	g      Gen[int]
	hooks  struct{ inner int }
	tagged string `json:\"t\"`
}

type hooked struct {
	Core
	*Logger
	io.Writer
	sync.Locker
	Gen[int]
	atomic.Pointer[int]
	extra int
}

type Gen[T any] struct{}

func local() {
	type shape struct{ side int }
	var anon struct{ field int }
	_ = anon
}
";

#[test]
fn an_interfaces_method_specs_and_a_structs_fields_are_nodes_under_their_type() {
    let tmp = tree(&[("zapcore/core.go", FIELDS)]);
    let engine = index(tmp.path());
    let got = declarations(engine.runtime().unwrap());
    let want = set(&[
        "Interface Core",
        "Method Core.Check",
        "Method Core.Write",
        "Method Core.Sync",
        "Struct Logger",
        "Field Logger.core",
        "Field Logger.name",
        "Field Logger.x",
        "Field Logger.y",
        "Field Logger.w",
        "Field Logger.g",
        "Field Logger.hooks",
        "Field Logger.tagged",
        "Struct hooked",
        "Field hooked.Core",
        "Field hooked.Logger",
        "Field hooked.Writer",
        "Field hooked.Locker",
        "Field hooked.Gen",
        "Field hooked.Pointer",
        "Field hooked.extra",
        "Struct Gen",
        "Function local",
        "Struct local.shape",
        "Field shape.side",
    ]);
    assert_eq!(got, want, "an anonymous struct's field is no node; a named local struct keeps its own");
}

#[test]
fn an_interface_method_spec_has_no_body() {
    let facts = facts_of("zapcore/core.go", FIELDS);
    let spec = facts.nodes.iter().find(|n| n.name == "Check").expect("Check is declared");
    assert_eq!(spec.kind, NodeKind::Method);
    assert!(spec.signature, "a spec is a required signature of its interface");
    assert_eq!(spec.metrics.map(|m| m.has_body), Some(false), "a spec has no body");
    let bodied = |facts: &logos_core::extract::Facts| -> Vec<(String, Option<bool>)> {
        facts
            .nodes
            .iter()
            .filter(|n| !n.signature && matches!(n.kind, NodeKind::Function | NodeKind::Method))
            .map(|n| (n.name.clone(), n.metrics.map(|m| m.has_body)))
            .collect()
    };
    let funcs = bodied(&facts);
    assert!(!funcs.is_empty(), "the fixture declares a func");
    let methods = bodied(&facts_of("zapcore/entry.go", PRE_EXISTING));
    assert_eq!(methods.len(), 3, "New, Write and Write2: {methods:?}");
    for (name, has_body) in funcs.into_iter().chain(methods) {
        assert_eq!(has_body, Some(true), "{name} has a body and keeps it");
    }
}

#[test]
fn an_embedded_field_records_extends_to_the_embedded_type() {
    let tmp = tree(&[("zapcore/core.go", FIELDS)]);
    let engine = index(tmp.path());
    let got = extends(engine.runtime().unwrap());
    assert_eq!(
        got,
        BTreeSet::from([
            ("hooked".to_string(), "Core".to_string()),
            ("hooked".to_string(), "Logger".to_string()),
            ("hooked".to_string(), "io::Writer".to_string()),
            ("hooked".to_string(), "sync::Locker".to_string()),
            ("hooked".to_string(), "Gen".to_string()),
            ("hooked".to_string(), "atomic::Pointer".to_string()),
        ]),
        "every embedded field is an Extends of its struct, a package-qualified one by its path"
    );
}

/// What a Go file declared **before** S-525, in every form the old queries
/// captured: functions, methods, a struct, an interface, a single top-level
/// `var` and a grouped `const`. Their symbols are pinned below from the base
/// commit, so a query change that renumbers or re-kinds any of them fails here.
const PRE_EXISTING: &str = "\
package zapcore

type Entry struct{}

type Checker interface{ Check() bool }

const (
	DebugLevel = iota
	InfoLevel
)

var _globalL = 1

func New() *Entry { return nil }

func (e *Entry) Write() {}

func (e Entry) Write2() {}
";

#[test]
fn every_declaration_the_old_queries_captured_keeps_its_scip_id() {
    let tmp = tree(&[("zapcore/entry.go", PRE_EXISTING)]);
    let engine = index(tmp.path());
    let symbols: BTreeSet<String> = engine
        .runtime()
        .unwrap()
        .submit_read(|store| store.all_nodes())
        .expect("read runs")
        .into_iter()
        .filter(|n| n.kind != NodeKind::Module)
        .map(|n| n.symbol.as_str().to_string())
        .collect();
    let pinned = |descriptors: &str| coordinate::symbol(&format!("zapcore/`entry.go`/{descriptors}"));
    let want: BTreeSet<String> = [
        "Entry#",
        "Checker#",
        "DebugLevel.",
        "InfoLevel.",
        "_globalL.",
        "New().",
        "Write().",
        "Write2().",
    ]
    .iter()
    .map(|d| pinned(d))
    .chain([pinned("Checker#Check().")])
    .collect();
    assert_eq!(symbols, want, "the old declarations keep their symbols; the interface's spec is the one addition");
}

const SYNC_BEFORE: &str = "\
package zapcore

type Core interface{ Check() bool }

type hooked struct {
	n int
}

var one = 1
";

const SYNC_AFTER: &str = "\
package zapcore

type Core interface{ Check() bool }

type hooked struct {
	Core
	n, m int
}

type Level = int8

var (
	one = 1
	two = 2
)

func f() {
	var local = 3
	_ = local
}
";

/// NFR-RA-06: a file edited into embedding, aliasing and grouping declarations is
/// synced to exactly the graph a fresh index of the edited tree builds.
#[test]
fn a_synced_edit_equals_a_fresh_index_of_the_edited_tree() {
    let tmp_a = tree(&[("zapcore/core.go", SYNC_BEFORE)]);
    let engine_a = index(tmp_a.path());
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    fs::write(tmp_a.path().join("zapcore/core.go"), SYNC_AFTER).unwrap();
    engine_a.sync(&[root_a.join("zapcore/core.go")]);

    let tmp_b = tree(&[("zapcore/core.go", SYNC_AFTER)]);
    let engine_b = index(tmp_b.path());
    assert_eq!(
        graph_fingerprint(engine_a.runtime().unwrap()),
        graph_fingerprint(engine_b.runtime().unwrap()),
        "sync-to-state must equal index-of-state"
    );
    let got = declarations(engine_a.runtime().unwrap());
    for want in ["Field hooked.Core", "Field hooked.m", "TypeAlias Level", "Variable two"] {
        assert!(got.contains(want), "{want} is declared after the sync; got {got:#?}");
    }
    assert_eq!(extends(engine_a.runtime().unwrap()), BTreeSet::from([("hooked".to_string(), "Core".to_string())]));
}

/// Go's export rule is capitalisation, and the new kinds follow it by name: an
/// exported spec, field, variable or constant is a live root, an unexported one
/// is not. (No production change backs this: it pins the rule onto the new kinds.)
#[test]
fn the_new_kinds_are_exported_by_capitalisation() {
    let facts = facts_of("zapcore/core.go", FIELDS);
    let exported = |name: &str, kind: NodeKind| {
        facts.nodes.iter().find(|n| n.name == name && n.kind == kind).unwrap_or_else(|| panic!("{name} is declared")).exported
    };
    assert!(exported("Check", NodeKind::Method), "an exported spec");
    assert!(exported("Core", NodeKind::Field), "an embedded field is named for its type");
    assert!(!exported("core", NodeKind::Field), "an unexported field");
    assert!(!exported("extra", NodeKind::Field), "an unexported field");
    let vars = facts_of("zap/vars.go", VARS);
    let var = |name: &str| vars.nodes.iter().find(|n| n.name == name).unwrap_or_else(|| panic!("{name} is declared")).exported;
    assert!(var("Top"), "an exported constant");
    assert!(!var("_globalL"), "an unexported variable");
}

/// The first name owns a multi-name declaration's initialiser and anything nested
/// in it, as the one name the old queries captured did: `a` (not `b`) holds the
/// calls, and the local type keeps the symbol `c.M#` it had.
const MULTI_NAME: &str = "\
package m

func f() int { return 1 }

func g() int { return 2 }

var a, b = f(), g()

var c, d = func() int { type M struct{ k int }; return 1 }, 2
";

#[test]
fn the_first_name_of_a_multi_name_declaration_owns_its_initialiser_and_what_it_nests() {
    let tmp = tree(&[("m/m.go", MULTI_NAME)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let symbols: BTreeSet<String> = rt
        .submit_read(|store| store.all_nodes())
        .expect("read runs")
        .into_iter()
        .map(|n| n.symbol.as_str().to_string())
        .collect();
    let pinned = |descriptors: &str| coordinate::symbol(&format!("m/`m.go`/{descriptors}"));
    for want in ["c.M#", "c.M#k.", "a.", "b.", "c.", "d."] {
        assert!(symbols.contains(&pinned(want)), "{want} is a symbol; got {symbols:#?}");
    }
    assert!(!symbols.iter().any(|s| s.contains("d.M#")), "the local type is not the second name's");
    let calls: BTreeSet<(String, String)> = rt
        .submit_read(|store| store.unresolved_refs())
        .expect("read runs")
        .into_iter()
        .filter(|r| r.kind == EdgeKind::Calls)
        .map(|r| (r.source_symbol.rsplit('/').next().unwrap().to_string(), r.target))
        .collect();
    assert_eq!(
        calls,
        BTreeSet::from([("a.".to_string(), "f".to_string()), ("a.".to_string(), "g".to_string())]),
        "both calls belong to `a`"
    );
}

/// Multi-name declarations of other languages are untouched by S-525: a C
/// file-scope declaration of several names declares its first only, as it did
/// before (a limitation, pinned so the change stays Go's own). A struct member
/// line is the exception S-582 made (FR-EX-31): each of its names is a field.
#[test]
fn a_multi_name_c_or_cpp_declaration_declares_its_first_name_only_as_before() {
    let c = "int g1, g2 = 3;\ntypedef int T1, T2;\n";
    let cpp = "typedef int T1, T2;\nstruct S { int a, b; static int c, d; };\n";
    for (ext, path, src, want, absent) in [
        ("c", "src/g.c", c, vec!["g1", "T1"], vec!["g2", "T2"]),
        ("cpp", "src/s.cpp", cpp, vec!["T1", "a", "b", "c", "d"], vec!["T2"]),
    ] {
        let tmp = tempfile::tempdir().expect("tempdir");
        let registry = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
        let Some(plugin) = registry.for_extension(ext) else { continue };
        let facts = extract(&FileInput::new(path, src), plugin, &SymbolContext::default());
        let names: BTreeSet<&str> = facts.nodes.iter().map(|n| n.name.as_str()).collect();
        for name in want {
            assert!(names.contains(name), "{path}: {name} is declared; got {names:?}");
        }
        for name in absent {
            assert!(!names.contains(name), "{path}: {name} is the second name of a declaration; got {names:?}");
        }
    }
}

/// A blank name declares as it always has: each `_` is a node of its own,
/// numbered apart by ordinal (a compile-time interface assertion, an ignored
/// field). Pinned so skipping them stays a deliberate change, not a drift.
#[test]
fn blank_names_are_declared_as_before_each_its_own_node() {
    let src = "package b\n\nvar _ = 1\n\nvar _ = 2\n\ntype T struct {\n\t_ int\n\t_ string\n}\n";
    let facts = facts_of("b/b.go", src);
    let count = |kind| facts.nodes.iter().filter(|n| n.name == "_" && n.kind == kind).count();
    assert_eq!(count(NodeKind::Variable), 2, "two blank variables");
    assert_eq!(count(NodeKind::Field), 2, "two blank fields");
    let symbols: BTreeSet<&str> =
        facts.nodes.iter().filter(|n| n.name == "_").map(|n| n.symbol.as_str()).collect();
    assert_eq!(symbols.len(), 4, "each blank name has a symbol of its own: {symbols:?}");
}
