//! Go types implement interfaces by method set, and methods access their
//! receiver's fields (S-529, CR-172, FR-RS-18, FR-EX-12, FR-EX-15, NFR-RA-05,
//! NFR-RA-06) — exercised end to end through the public [`Engine`] façade
//! against temp-directory fixtures shaped like zap's `zapcore`.
//!
//! - A type whose methods — its own, by pointer or value receiver, and those an
//!   embedded field promotes — cover every method an in-repository interface
//!   declares, matched by name and parameter count, with the interface's
//!   embedded interfaces expanded, records `Implements` to it.
//! - A type missing one method, or declaring it with another parameter count,
//!   records none; an unexported method is matched only within its package.
//! - A generic interface, a constraint and the empty interface are never
//!   implemented: the first is not computed, the others declare no method.
//! - Inside a method, `c.enc` on the method's own receiver `c` records
//!   `Accesses` to the receiver type's `Field`, whichever file declares it; a
//!   call on the receiver, a selector on another value and a free function's
//!   parameter that happens to share the name record none.
//!
//! Fixtures are written inline into temp directories, like every sibling suite:
//! a `.go` fixture tree checked into this repository would be indexed into its
//! own graph.

#![cfg(feature = "lang-go")]

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId, NodeKind};
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

/// A node's label: `name`, and `Parent.name` for one a type contains; a
/// package's file module is no parent, so a declaration is labelled by its
/// directory instead (`zapcore/Core`).
fn labels(rt: &Runtime) -> HashMap<NodeId, String> {
    let nodes = rt.submit_read(|store| store.all_nodes()).expect("read runs");
    let edges = rt.submit_read(|store| store.all_edges()).expect("read runs");
    let by_id: HashMap<NodeId, _> = nodes.iter().map(|n| (n.id, n)).collect();
    let parent: HashMap<NodeId, NodeId> = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Contains)
        .map(|e| (e.target, e.source))
        .collect();
    nodes
        .iter()
        .map(|n| {
            let label = match parent.get(&n.id).map(|p| by_id[p]) {
                Some(p) if p.kind != NodeKind::Module => format!("{}.{}", p.name, n.name),
                _ => {
                    let dir = n.file_path.as_deref().and_then(|f| f.rsplit_once('/')).map_or("", |(d, _)| d);
                    format!("{dir}/{}", n.name)
                }
            };
            (n.id, label)
        })
        .collect()
}

/// Every edge of `kind`, as `(source, target)` labels.
fn edges(rt: &Runtime, kind: EdgeKind) -> BTreeSet<(String, String)> {
    let label = labels(rt);
    rt.submit_read(|store| store.all_edges())
        .expect("read runs")
        .into_iter()
        .filter(|e| e.kind == kind)
        .map(|e| (label[&e.source].clone(), label[&e.target].clone()))
        .collect()
}

/// Every ledger row of `kind`, bound or not, as `(source, target)`.
fn ref_rows(rt: &Runtime, kind: EdgeKind) -> BTreeSet<(String, String)> {
    let nodes = rt.submit_read(|store| store.all_nodes()).expect("read runs");
    let label = labels(rt);
    let by_symbol: HashMap<String, String> =
        nodes.iter().map(|n| (n.symbol.as_str().to_string(), label[&n.id].clone())).collect();
    rt.submit_read(|store| store.unresolved_refs())
        .expect("read runs")
        .into_iter()
        .filter(|r| r.kind == kind)
        .map(|r| (by_symbol[&r.source_symbol].clone(), r.target))
        .collect()
}

/// Every `Accesses` row of the ledger, bound or not, as `(source, field name)`.
fn access_rows(rt: &Runtime) -> BTreeSet<(String, String)> {
    ref_rows(rt, EdgeKind::Accesses)
}

fn pairs(items: &[(&str, &str)]) -> BTreeSet<(String, String)> {
    items.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

const GO_MOD: &str = "module example.com/zap\n\ngo 1.22\n";

/// zap's `level.go`: the embedded interface lives in another file of the
/// package than the interface embedding it.
const LEVEL: &str = "\
package zapcore

type Level int8

type LevelEnabler interface {
	Enabled(Level) bool
}

func (l Level) Enabled(lvl Level) bool { return lvl >= l }
";

/// zap's `core.go`: `ioCore` takes `Enabled` from its embedded `LevelEnabler`
/// and the rest from pointer receivers; `nopCore` declares all five on value
/// receivers. `partial` misses `Sync`, and `skewed` declares it with one
/// parameter too many.
const CORE: &str = "\
package zapcore

type Entry struct{}

type CheckedEntry struct{}

type Field struct{}

type Core interface {
	LevelEnabler

	With([]Field) Core
	Check(Entry, *CheckedEntry) *CheckedEntry
	Write(Entry, []Field) error
	Sync() error
}

type nopCore struct{}

func (nopCore) Enabled(Level) bool                                 { return false }
func (n nopCore) With([]Field) Core                                { return n }
func (nopCore) Check(_ Entry, ce *CheckedEntry) *CheckedEntry      { return ce }
func (nopCore) Write(Entry, []Field) error                         { return nil }
func (nopCore) Sync() error                                        { return nil }

type ioCore struct {
	LevelEnabler
	enc string
	out string
}

func (c *ioCore) With(fields []Field) Core { return c.clone() }
func (c *ioCore) Check(ent Entry, ce *CheckedEntry) *CheckedEntry { return ce }
func (c *ioCore) Sync() error { return nil }
func (c *ioCore) clone() *ioCore { return &ioCore{LevelEnabler: c.LevelEnabler, enc: c.enc} }

type partial struct{ LevelEnabler }

func (p partial) With([]Field) Core                            { return nil }
func (p partial) Check(Entry, *CheckedEntry) *CheckedEntry     { return nil }
func (p partial) Write(Entry, []Field) error                   { return nil }

type skewed struct{ LevelEnabler }

func (s skewed) With([]Field) Core                             { return nil }
func (s skewed) Check(Entry, *CheckedEntry) *CheckedEntry      { return nil }
func (s skewed) Write(Entry, []Field) error                    { return nil }
func (s skewed) Sync(force bool) error                         { return nil }
";

/// `ioCore.Write` sits in a file of its own, apart from the struct whose
/// fields it reads, as zap's methods often do.
const WRITE: &str = "\
package zapcore

func (c *ioCore) Write(ent Entry, fields []Field) error {
	out := c.out
	_ = out
	c.enc = c.enc + \"x\"
	var other ioCore
	_ = other.enc
	return nil
}

func free(c *ioCore) string { return c.enc }
";

fn zap_tree() -> TempDir {
    tree(&[
        ("go.mod", GO_MOD),
        ("zapcore/level.go", LEVEL),
        ("zapcore/core.go", CORE),
        ("zapcore/write.go", WRITE),
    ])
}

/// FR-RS-18: a type implements an interface its method set covers — its own
/// methods by either receiver and those its embedded field promotes, against
/// the interface's methods with its embedded interface expanded — and no
/// other.
#[test]
fn a_type_implements_each_interface_its_method_set_covers_and_no_other() {
    let tmp = zap_tree();
    let engine = index(tmp.path());
    assert_eq!(
        edges(engine.runtime().unwrap(), EdgeKind::Implements),
        pairs(&[
            ("zapcore/Level", "zapcore/LevelEnabler"),
            ("zapcore/nopCore", "zapcore/Core"),
            ("zapcore/nopCore", "zapcore/LevelEnabler"),
            ("zapcore/ioCore", "zapcore/Core"),
            ("zapcore/ioCore", "zapcore/LevelEnabler"),
            ("zapcore/partial", "zapcore/LevelEnabler"),
            ("zapcore/skewed", "zapcore/LevelEnabler"),
        ]),
        "`partial` misses `Sync` and `skewed` takes one parameter too many: neither is a Core"
    );
}

/// The embedded interface is an `Extends` of the interface embedding it, bound
/// like a struct's embedded field (S-525).
#[test]
fn an_embedded_interface_is_an_extends_of_the_interface_embedding_it() {
    let tmp = zap_tree();
    let engine = index(tmp.path());
    let rows: BTreeSet<(String, String)> = ref_rows(engine.runtime().unwrap(), EdgeKind::Extends)
        .into_iter()
        .filter(|(source, _)| source == "zapcore/Core")
        .collect();
    assert_eq!(rows, pairs(&[("zapcore/Core", "LevelEnabler")]));
}

const UNEXPORTED_A: &str = "\
package a

type sealed interface {
	seal()
	Name() string
}

type inside struct{}

func (inside) seal()         {}
func (inside) Name() string  { return \"\" }
";

const UNEXPORTED_B: &str = "\
package b

type outside struct{}

func (outside) seal()        {}
func (outside) Name() string { return \"\" }
";

/// An unexported method name is the package's own: a type of another package
/// declaring one of the same spelling has another method, and implements
/// nothing through it.
#[test]
fn an_unexported_method_is_matched_only_within_its_package() {
    let tmp = tree(&[
        ("go.mod", GO_MOD),
        ("a/a.go", UNEXPORTED_A),
        ("b/b.go", UNEXPORTED_B),
    ]);
    let engine = index(tmp.path());
    assert_eq!(
        edges(engine.runtime().unwrap(), EdgeKind::Implements),
        pairs(&[("a/inside", "a/sealed")])
    );
}

const NOT_COMPUTED: &str = "\
package gen

type Getter[T any] interface {
	Get() T
}

type Number interface {
	~int | ~float64
}

type Any interface{}

type Marshaler interface {
	Marshal() error
}

// zap's `ObjectMarshalerPtr`: generic, with no method spec of its own.
type MarshalerPtr[T any] interface {
	*T
	Marshaler
}

type box struct{ v int }

func (b box) Get() int { return b.v }
func (b *box) Marshal() error { return nil }
";

/// A generic interface is not computed — its methods' parameter types wait on
/// its type arguments, and one built only by embedding (`*T; Marshaler`) is a
/// constraint — and a constraint or the empty interface declares no method a
/// type could be matched on: none is implemented, by anything. `box` still
/// implements the plain `Marshaler`.
#[test]
fn a_generic_interface_a_constraint_and_the_empty_interface_are_never_implemented() {
    let tmp = tree(&[("go.mod", GO_MOD), ("gen/gen.go", NOT_COMPUTED)]);
    let engine = index(tmp.path());
    assert_eq!(
        edges(engine.runtime().unwrap(), EdgeKind::Implements),
        pairs(&[("gen/box", "gen/Marshaler")])
    );
}

/// FR-RS-18 / FR-EX-12: a selector on the method's own receiver that names a
/// field of the receiver's type records `Accesses` to that `Field` — from a
/// file other than the struct's — and a call on the receiver, a selector on
/// another value and a free function's same-named parameter record no row.
#[test]
fn a_read_of_the_receivers_field_is_an_access_and_nothing_else_is() {
    let tmp = zap_tree();
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        access_rows(rt),
        pairs(&[
            ("zapcore/Write", "enc"),
            ("zapcore/Write", "out"),
            ("zapcore/clone", "LevelEnabler"),
            ("zapcore/clone", "enc"),
        ]),
        "`other.enc`, `c.clone()` and `free`'s `c.enc` are no access of the receiver"
    );
    assert_eq!(
        edges(rt, EdgeKind::Accesses),
        pairs(&[
            ("zapcore/Write", "ioCore.enc"),
            ("zapcore/Write", "ioCore.out"),
            ("zapcore/clone", "ioCore.LevelEnabler"),
            ("zapcore/clone", "ioCore.enc"),
        ])
    );
}

/// The `ioCore` methods once `Sync` is gone: `ioCore` no longer covers `Core`.
const CORE_WITHOUT_SYNC: &str = "\
package zapcore

type Entry struct{}

type CheckedEntry struct{}

type Field struct{}

type Core interface {
	LevelEnabler

	With([]Field) Core
	Check(Entry, *CheckedEntry) *CheckedEntry
	Write(Entry, []Field) error
	Sync() error
}

type nopCore struct{}

func (nopCore) Enabled(Level) bool                                 { return false }
func (n nopCore) With([]Field) Core                                { return n }
func (nopCore) Check(_ Entry, ce *CheckedEntry) *CheckedEntry      { return ce }
func (nopCore) Write(Entry, []Field) error                         { return nil }
func (nopCore) Sync() error                                        { return nil }

type ioCore struct {
	LevelEnabler
	enc string
}

func (c *ioCore) With(fields []Field) Core { return c.clone() }
func (c *ioCore) Check(ent Entry, ce *CheckedEntry) *CheckedEntry { return ce }
func (c *ioCore) clone() *ioCore { return &ioCore{LevelEnabler: c.LevelEnabler, enc: c.enc} }
";

/// The interface gains a method, so no type covers it any more.
const LEVEL_WIDER: &str = "\
package zapcore

type Level int8

type LevelEnabler interface {
	Enabled(Level) bool
	Level() Level
}

func (l Level) Enabled(lvl Level) bool { return lvl >= l }
";

/// NFR-RA-06: a sync that drops a method, a field and an interface's coverage —
/// in the implementer's file and in the interface's — builds exactly the graph
/// a fresh index of the edited tree builds; then restoring them does too.
#[test]
fn a_synced_edit_equals_a_fresh_index_of_the_edited_tree() {
    let tmp_a = zap_tree();
    let engine_a = index(tmp_a.path());
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    let core = root_a.join("zapcore/core.go");
    let level = root_a.join("zapcore/level.go");

    let fresh = |core_text: &str, level_text: &str| {
        let tmp_b = tree(&[
            ("go.mod", GO_MOD),
            ("zapcore/level.go", level_text),
            ("zapcore/core.go", core_text),
            ("zapcore/write.go", WRITE),
        ]);
        let engine_b = index(tmp_b.path());
        graph_fingerprint(engine_b.runtime().unwrap())
    };

    fs::write(&core, CORE_WITHOUT_SYNC).unwrap();
    engine_a.sync(std::slice::from_ref(&core));
    assert_eq!(
        graph_fingerprint(engine_a.runtime().unwrap()),
        fresh(CORE_WITHOUT_SYNC, LEVEL),
        "dropping `Sync` and `out`: sync-to-state must equal index-of-state"
    );
    assert!(
        !edges(engine_a.runtime().unwrap(), EdgeKind::Implements)
            .contains(&("zapcore/ioCore".to_string(), "zapcore/Core".to_string())),
        "`ioCore` without `Sync` is no Core"
    );

    fs::write(&level, LEVEL_WIDER).unwrap();
    engine_a.sync(std::slice::from_ref(&level));
    assert_eq!(
        graph_fingerprint(engine_a.runtime().unwrap()),
        fresh(CORE_WITHOUT_SYNC, LEVEL_WIDER),
        "widening the embedded interface: sync-to-state must equal index-of-state"
    );

    fs::write(&core, CORE).unwrap();
    fs::write(&level, LEVEL).unwrap();
    engine_a.sync(&[core, level]);
    assert_eq!(
        graph_fingerprint(engine_a.runtime().unwrap()),
        fresh(CORE, LEVEL),
        "restoring both files: sync-to-state must equal index-of-state"
    );
    assert!(edges(engine_a.runtime().unwrap(), EdgeKind::Implements)
        .contains(&("zapcore/ioCore".to_string(), "zapcore/Core".to_string())));
}

/// Another package of the module: `wrapped` embeds `zapcore.Core` through its
/// import, and `Wrapped` embeds it into an interface of its own.
const WRAPPER: &str = "\
package wrapper

import \"example.com/zap/zapcore\"

type Wrapped interface {
	zapcore.Core
	Name() string
}

type wrapped struct {
	zapcore.Core
}

func (w wrapped) Name() string { return \"w\" }
";

/// A package-qualified embedding is read through the import that names it: the
/// embedded interface's methods are promoted into a struct of another package
/// and expanded into an interface of another package.
#[test]
fn a_qualified_embedding_is_read_through_its_import() {
    let tmp = tree(&[
        ("go.mod", GO_MOD),
        ("zapcore/level.go", LEVEL),
        ("zapcore/core.go", CORE),
        ("zapcore/write.go", WRITE),
        ("wrapper/wrapper.go", WRAPPER),
    ]);
    let engine = index(tmp.path());
    let got = edges(engine.runtime().unwrap(), EdgeKind::Implements);
    for want in [
        ("wrapper/wrapped", "zapcore/Core"),
        ("wrapper/wrapped", "zapcore/LevelEnabler"),
        ("wrapper/wrapped", "wrapper/Wrapped"),
        ("zapcore/ioCore", "zapcore/Core"),
    ] {
        assert!(got.contains(&(want.0.to_string(), want.1.to_string())), "{want:?} missing from {got:#?}");
    }
    assert!(
        !got.contains(&("zapcore/ioCore".to_string(), "wrapper/Wrapped".to_string())),
        "`ioCore` has no `Name`: it is no `Wrapped`"
    );
}

/// A build-tag twin of `ioCore` in the same package: two types of the name
/// leave a receiver read no one type to bind among.
const IO_CORE_TWIN: &str = "\
//go:build windows

package zapcore

type ioCore struct {
	handle uintptr
}
";

/// NFR-RA-06: a sync that adds a second `ioCore` to the package re-binds the
/// receiver reads of every `ioCore` method — whose rows spell only the field's
/// name — exactly as a fresh index of the edited tree binds them: unbound.
#[test]
fn a_synced_twin_of_the_receiver_type_rebinds_its_reads_as_a_fresh_index_does() {
    let tmp_a = zap_tree();
    let engine_a = index(tmp_a.path());
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    assert!(edges(engine_a.runtime().unwrap(), EdgeKind::Accesses)
        .contains(&("zapcore/Write".to_string(), "ioCore.enc".to_string())));
    write(&root_a, "zapcore/core_windows.go", IO_CORE_TWIN);
    engine_a.sync(&[root_a.join("zapcore/core_windows.go")]);

    let tmp_b = tree(&[
        ("go.mod", GO_MOD),
        ("zapcore/level.go", LEVEL),
        ("zapcore/core.go", CORE),
        ("zapcore/write.go", WRITE),
        ("zapcore/core_windows.go", IO_CORE_TWIN),
    ]);
    let engine_b = index(tmp_b.path());
    assert_eq!(
        edges(engine_b.runtime().unwrap(), EdgeKind::Accesses),
        BTreeSet::new(),
        "two `ioCore`s in the package: no read binds"
    );
    assert_eq!(
        graph_fingerprint(engine_a.runtime().unwrap()),
        graph_fingerprint(engine_b.runtime().unwrap()),
        "sync-to-state must equal index-of-state"
    );
}

/// Go's promotion rule: the shallowest depth decides. `S` takes `M()` from `B`
/// at depth 1, which hides `Inner`'s `M(x int)` at depth 2 — so `S` is no
/// `Iface`, although `A` and `Inner` are.
const SHALLOWER_WINS: &str = "\
package p

type Iface interface { M(x int) }

type Inner struct{}

func (Inner) M(x int) {}

type A struct{ Inner }

type B struct{}

func (B) M() {}

type S struct {
	A
	B
}
";

/// Two promotions of one name at one depth are an ambiguous selector, so the
/// method is in neither's method set: `Both` is no `Doer`.
const SAME_DEPTH: &str = "\
package q

type Doer interface{ Do() }

type X struct{}

func (X) Do() {}

type Y struct{}

func (Y) Do() {}

type Both struct {
	X
	Y
}
";

/// FR-RS-18 / NFR-RA-05: promotion follows Go's depth rule — the shallowest
/// method of a name wins, and two at the shallowest depth promote none.
#[test]
fn promotion_takes_the_shallowest_method_and_none_of_an_ambiguous_depth() {
    let tmp = tree(&[("go.mod", GO_MOD), ("p/p.go", SHALLOWER_WINS), ("q/q.go", SAME_DEPTH)]);
    let engine = index(tmp.path());
    assert_eq!(
        edges(engine.runtime().unwrap(), EdgeKind::Implements),
        pairs(&[
            ("p/A", "p/Iface"),
            ("p/Inner", "p/Iface"),
            ("q/X", "q/Doer"),
            ("q/Y", "q/Doer"),
        ])
    );
}

/// A receiver name the method's body rebinds (S-594) is another value at a
/// read, as it is at a call: `s.name` inside `for _, s := range xs` reads an
/// `Other`'s field, so `Run` records no access of `Svc.name` — a false
/// cohesion link — while `Own`'s `s.name` on the receiver itself does.
#[test]
fn a_read_through_a_rebound_receiver_name_is_no_access_of_the_receiver() {
    let tmp = tree(&[
        ("go.mod", GO_MOD),
        (
            "p/p.go",
            "package p\n\ntype Svc struct{ name string }\n\ntype Other struct{ name string }\n\n\
             func (o *Other) Close() {}\n\n\
             func (s *Svc) Run(xs []*Other) {\n\tfor _, s := range xs {\n\t\t_ = s.name\n\t\ts.Close()\n\t}\n}\n\n\
             func (s *Svc) Own() string { return s.name }\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(access_rows(rt), pairs(&[("p/Own", "name")]));
    assert_eq!(edges(rt, EdgeKind::Accesses), pairs(&[("p/Own", "Svc.name")]));
}

/// NFR-RA-05: a receiver read binds only among its own package's types. `a.Foo`
/// is a defined type with no field of its own; the one `Foo` struct of the
/// repository lives in package `b`, and a read in `a` never reaches it.
#[test]
fn a_receiver_read_never_binds_into_another_package() {
    let tmp = tree(&[
        ("go.mod", GO_MOD),
        (
            "a/a.go",
            "package a\n\ntype Bar struct{ name string }\n\ntype Foo Bar\n\nfunc (f Foo) N() string { return f.name }\n",
        ),
        ("b/b.go", "package b\n\ntype Foo struct{ name string }\n"),
    ]);
    let engine = index(tmp.path());
    assert_eq!(edges(engine.runtime().unwrap(), EdgeKind::Accesses), BTreeSet::new());
}

const TWIN_TYPES: &str = "\
package t

type T struct{}

type I interface{ M() }

type J interface{ M(a int) }
";

/// NFR-RA-06: one type declaring a method twice under two build tags, with two
/// parameter counts, implements neither interface — whichever file was
/// re-extracted last — so a sync builds what a fresh index builds.
#[test]
fn build_tag_twin_methods_implement_nothing_and_sync_equals_reindex() {
    let files = [
        ("go.mod", GO_MOD),
        ("t/t.go", TWIN_TYPES),
        ("t/x_a.go", "//go:build linux\n\npackage t\n\nfunc (t T) M() {}\n"),
        ("t/x_b.go", "//go:build !linux\n\npackage t\n\nfunc (t T) M(a int) {}\n"),
    ];
    let tmp_a = tree(&files);
    let engine_a = index(tmp_a.path());
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    assert_eq!(edges(engine_a.runtime().unwrap(), EdgeKind::Implements), BTreeSet::new());
    write(&root_a, "t/x_a.go", "//go:build linux\n\npackage t\n\nfunc (t T) M() {}\n\n// touched\n");
    engine_a.sync(&[root_a.join("t/x_a.go")]);
    assert_eq!(edges(engine_a.runtime().unwrap(), EdgeKind::Implements), BTreeSet::new());
}

/// NFR-RA-05: two types of one name in one package (a build-tag twin) give its
/// methods no owner, so neither implements anything through them.
#[test]
fn a_method_of_a_twinned_type_has_no_owner_and_implements_nothing() {
    let tmp = tree(&[
        ("go.mod", GO_MOD),
        ("p/p.go", "package p\n\ntype I interface{ M() }\n\ntype T struct{}\n\nfunc (T) M() {}\n"),
        ("p/p_windows.go", "//go:build windows\n\npackage p\n\ntype T struct{ h int }\n"),
    ]);
    let engine = index(tmp.path());
    assert_eq!(edges(engine.runtime().unwrap(), EdgeKind::Implements), BTreeSet::new());
}

/// FR-RS-18: two packages each declaring a `T` — the same name in one module —
/// each bind their methods' receiver reads to their own `T`'s field.
#[test]
fn same_named_receiver_types_of_two_packages_each_bind_their_own_fields() {
    let tmp = tree(&[
        ("go.mod", GO_MOD),
        ("a/a.go", "package a\n\ntype T struct{ f int }\n\nfunc (t *T) Get() int { return t.f }\n"),
        ("b/b.go", "package b\n\ntype T struct{ g int }\n\nfunc (t *T) Get() int { return t.g }\n"),
    ]);
    let engine = index(tmp.path());
    assert_eq!(
        edges(engine.runtime().unwrap(), EdgeKind::Accesses),
        pairs(&[("a/Get", "T.f"), ("b/Get", "T.g")])
    );
}

/// An interface reached twice through two embeddings (a diamond) is expanded
/// each time, never read as a cycle: `t` implements all four.
const DIAMOND: &str = "\
package d

type D interface{ A() }

type B interface {
\tD
\tX()
}

type C interface {
\tD
\tY()
}

type I interface {
\tB
\tC
}

type t struct{}

func (t) A() {}
func (t) X() {}
func (t) Y() {}
";

#[test]
fn a_diamond_of_embedded_interfaces_is_expanded_not_read_as_a_cycle() {
    let tmp = tree(&[("go.mod", GO_MOD), ("d/d.go", DIAMOND)]);
    let engine = index(tmp.path());
    assert_eq!(
        edges(engine.runtime().unwrap(), EdgeKind::Implements),
        pairs(&[("d/t", "d/B"), ("d/t", "d/C"), ("d/t", "d/D"), ("d/t", "d/I")])
    );
}

/// A type's own method hides a promoted one of the same name: `T`'s `M()`
/// hides its embedded `Inner`'s `M(x int)`, so `T` is a `Zero` and no `Inner`.
#[test]
fn an_own_method_hides_a_promoted_one_of_the_same_name() {
    let tmp = tree(&[(
        "go.mod",
        GO_MOD,
    ), (
        "p/p.go",
        "package p\n\ntype Inner interface{ M(x int) }\n\ntype Zero interface{ M() }\n\ntype T struct{ Inner }\n\nfunc (T) M() {}\n",
    )]);
    let engine = index(tmp.path());
    assert_eq!(edges(engine.runtime().unwrap(), EdgeKind::Implements), pairs(&[("p/T", "p/Zero")]));
}

/// A field only ever written (`c.w = 1`) is accessed too.
#[test]
fn a_write_to_the_receivers_field_is_an_access() {
    let tmp = tree(&[(
        "go.mod",
        GO_MOD,
    ), (
        "p/p.go",
        "package p\n\ntype T struct{ w int }\n\nfunc (c *T) Set() { c.w = 1 }\n",
    )]);
    let engine = index(tmp.path());
    assert_eq!(edges(engine.runtime().unwrap(), EdgeKind::Accesses), pairs(&[("p/Set", "T.w")]));
}
