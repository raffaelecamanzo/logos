//! A Go method records its receiver's base type (S-509, CR-166, FR-EX-12,
//! FR-EX-05, NFR-RA-06) — exercised end to end through the public [`Engine`]
//! façade against temp-directory fixtures.
//!
//! The Go plugin's `symbols` query tags each `method_declaration` with the
//! `@symbol.self_type` capture S-493 introduced for Rust; extraction persists it
//! beside the method's node (`nodes.self_type`, the same column — no migration).
//! `func (s *Svc[T]) Work()` and `func (s Svc) Rest()` both record `Svc`; a free
//! `func` records none. The fact rides beside the node and never in its symbol,
//! so no symbol, node or edge changes (ADR-07).
//!
//! Fixtures are written inline into temp directories, like the Rust sibling
//! (`rust_self_binding.rs`): a `.go` fixture tree checked into this repository
//! would be indexed into its own graph.

#![cfg(feature = "lang-go")]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::NodeId;
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

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

/// Every recorded self type as `label → self type`.
fn self_types(rt: &Runtime) -> BTreeMap<String, String> {
    let label = labels(rt);
    rt.submit_read(|store| store.node_self_types())
        .expect("read runs")
        .into_iter()
        .map(|(id, ty)| (label[&id].clone(), ty))
        .collect()
}

/// Every node as `(symbol, kind, label)` — the symbol is the stable identity, so
/// two runs agreeing on this set agree on every node and symbol.
fn nodes(rt: &Runtime) -> BTreeSet<(String, String, String)> {
    let label = labels(rt);
    rt.submit_read(|store| store.all_nodes())
        .expect("read runs")
        .into_iter()
        .map(|n| (n.symbol.as_str().to_string(), format!("{:?}", n.kind), label[&n.id].clone()))
        .collect()
}

/// Every edge as `(kind, source label, target label)`.
fn edges(rt: &Runtime) -> BTreeSet<(String, String, String)> {
    let label = labels(rt);
    rt.submit_read(|store| store.all_edges())
        .expect("read runs")
        .into_iter()
        .map(|e| {
            (format!("{:?}", e.kind), label[&e.source].clone(), label[&e.target].clone())
        })
        .collect()
}

fn expect(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// Every receiver form Go spells: pointer, value, generic (one and two type
/// parameters), unnamed and blank receivers — each beside a free function and a
/// method of another type.
const FORMS: &str = "\
package svc

type Svc[T any] struct{}

type Pair[K comparable, V any] struct{}

type Other struct{}

func (s *Svc[T]) Work() {}

func (s Svc[T]) Gen() {}

func (s *Svc[T]) Ptr() {}

func (s Svc[T]) Rest() {}

func (*Svc[T]) Anon() {}

func (Svc[T]) AnonV() {}

func (_ *Svc[T]) Blank() {}

func (p *Pair[K, V]) Get() {}

func (o Other) Rest() {}

func Free() {}
";

/// The plain, non-generic pointer and value forms the task text names verbatim,
/// beside every other declaration kind the Go query captures (const, var,
/// interface), which must record no self type.
const PLAIN: &str = "\
package plain

type Svc struct{}

func (s *Svc) Work() {}

func (s Svc) Rest() {}

func Helper() {}

const K = 1

var V = 2

type I interface{ Foo() }
";

#[test]
fn pointer_value_generic_and_unnamed_receivers_record_their_base_type() {
    let tmp = tree(&[("svc/forms.go", FORMS), ("plain/plain.go", PLAIN)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        self_types(rt),
        expect(&[
            ("svc/forms.go:Work@9", "Svc"),
            ("svc/forms.go:Gen@11", "Svc"),
            ("svc/forms.go:Ptr@13", "Svc"),
            ("svc/forms.go:Rest@15", "Svc"),
            ("svc/forms.go:Anon@17", "Svc"),
            ("svc/forms.go:AnonV@19", "Svc"),
            ("svc/forms.go:Blank@21", "Svc"),
            ("svc/forms.go:Get@23", "Pair"),
            ("svc/forms.go:Rest@25", "Other"),
            ("plain/plain.go:Work@5", "Svc"),
            ("plain/plain.go:Rest@7", "Svc"),
        ]),
        "every method records its receiver's base type; Svc/Pair/Other/Free/Helper record none"
    );
}

/// A free `func` and every non-method declaration kind the Go query captures
/// (struct, interface, const, var) record no self type, even in a file whose
/// methods do.
#[test]
fn a_free_function_and_the_type_declarations_record_none() {
    let tmp = tree(&[("svc/forms.go", FORMS), ("plain/plain.go", PLAIN)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let recorded = self_types(rt);
    for label in [
        "svc/forms.go:Free@27",
        "svc/forms.go:Svc@3",
        "svc/forms.go:Pair@5",
        "svc/forms.go:Other@7",
        "plain/plain.go:Helper@9",
        "plain/plain.go:Svc@3",
        "plain/plain.go:K@11",
        "plain/plain.go:V@13",
        "plain/plain.go:I@15",
    ] {
        assert!(!recorded.contains_key(label), "{label} must record no self type: {recorded:?}");
    }
    // Each label names a node that exists, so the absence above is not a typo.
    let indexed: BTreeSet<String> = labels(rt).into_values().collect();
    for label in ["svc/forms.go:Free@27", "plain/plain.go:K@11", "plain/plain.go:V@13", "plain/plain.go:I@15"] {
        assert!(indexed.contains(label), "{label} is indexed at that label: {indexed:?}");
    }
}

/// The Go `symbols` query as it stood before S-509: the same declarations, no
/// `@symbol.self_type`. Dropped on disk it overrides the embedded query
/// (FR-PL-04), giving the byte-identity baseline the fact must not move.
const SYMBOLS_BEFORE_S509: &str = "\
(function_declaration
  name: (identifier) @symbol.function)

(method_declaration
  name: (field_identifier) @symbol.method)

(type_declaration
  (type_spec
    name: (type_identifier) @symbol.struct
    type: (struct_type)))

(type_declaration
  (type_spec
    name: (type_identifier) @symbol.interface
    type: (interface_type)))

(const_declaration
  (const_spec
    name: (identifier) @symbol.constant))

(var_declaration
  (var_spec
    name: (identifier) @symbol.variable))
";

/// The fact rides beside the node: every node (symbol, kind, lines) and every
/// edge is what the pre-S509 query yields on the same files, and only the
/// self-type column differs. A receiver type leaking into a symbol, or a second
/// node for a method the extra patterns also name, fails here.
#[test]
fn the_fact_changes_no_node_symbol_or_edge() {
    let files = [("svc/forms.go", FORMS), ("plain/plain.go", PLAIN)];
    let baseline_tmp = tree(&files);
    write(baseline_tmp.path(), ".logos/plugins/go/queries/symbols.scm", SYMBOLS_BEFORE_S509);
    let baseline = index(baseline_tmp.path());
    let baseline_rt = baseline.runtime().unwrap();
    assert!(self_types(baseline_rt).is_empty(), "the pre-S509 query records no self type");

    let tmp = tree(&files);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(!self_types(rt).is_empty(), "the shipped query records them");

    let got = nodes(rt);
    assert_eq!(got, nodes(baseline_rt), "nodes and symbols are byte-identical");
    assert_eq!(edges(rt), edges(baseline_rt), "edges are byte-identical");
    let methods = got.iter().filter(|(_, k, _)| k == "Method").count();
    assert_eq!(methods, 11, "one node per method declaration: {got:?}");
}

/// Re-extracting a file whose declarations are unchanged yields the identical
/// fact. A second `index` re-extracts every file; a `sync` of a byte-identical
/// file is skipped by its content hash (FR-SY-03), so the sync leg appends a
/// trailing comment — a changed file, the same declarations.
#[test]
fn re_extraction_of_an_unchanged_file_yields_the_identical_fact() {
    let tmp = tree(&[("svc/forms.go", FORMS), ("plain/plain.go", PLAIN)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let before = self_types(rt);
    assert_eq!(before.len(), 11, "the fact is recorded to begin with: {before:?}");
    let nodes_before = nodes(rt);
    let _ = engine.index();
    assert_eq!(self_types(rt), before, "a second index records the same facts");
    assert_eq!(nodes(rt), nodes_before, "and the same nodes");
    write(tmp.path(), "svc/forms.go", &format!("{FORMS}\n// a comment, no declaration\n"));
    let synced = engine.sync(&[PathBuf::from("svc/forms.go")]);
    assert_eq!(synced.files_modified, 1, "the sync really re-extracted the file: {synced:?}");
    assert_eq!(self_types(rt), before, "a re-extracted file with the same declarations records the same facts");
    assert_eq!(nodes(rt), nodes_before);
}

/// A one-file edit re-derives only that file's rows, and the synced store equals
/// a fresh index of the edited tree.
#[test]
fn a_synced_edit_matches_a_fresh_reindex_and_rederives_only_its_file() {
    let tmp = tree(&[("svc/forms.go", FORMS), ("plain/plain.go", PLAIN)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let plain_before: BTreeMap<String, String> =
        self_types(rt).into_iter().filter(|(l, _)| l.starts_with("plain/")).collect();

    // `Work` moves to another receiver type; a method of a new type appears.
    let edited = FORMS.replace("func (s *Svc[T]) Work() {}", "func (s *Other) Work() {}")
        + "\nfunc (n *Fresh[T]) Make() {}\n";
    write(tmp.path(), "svc/forms.go", &edited);
    engine.sync(&[PathBuf::from("svc/forms.go")]);

    let synced = self_types(rt);
    assert_eq!(synced.get("svc/forms.go:Work@9").map(String::as_str), Some("Other"));
    assert_eq!(synced.get("svc/forms.go:Make@29").map(String::as_str), Some("Fresh"));
    let plain_after: BTreeMap<String, String> =
        synced.iter().filter(|(l, _)| l.starts_with("plain/")).map(|(k, v)| (k.clone(), v.clone())).collect();
    assert_eq!(plain_after, plain_before, "an untouched file's facts are unchanged");

    let fresh_tmp = tree(&[("svc/forms.go", &edited), ("plain/plain.go", PLAIN)]);
    let fresh = index(fresh_tmp.path());
    let fresh_rt = fresh.runtime().unwrap();
    assert_eq!(self_types(rt), self_types(fresh_rt), "sync ≡ reindex: self types");
    assert_eq!(nodes(rt), nodes(fresh_rt), "sync ≡ reindex: nodes");
    assert_eq!(edges(rt), edges(fresh_rt), "sync ≡ reindex: edges");
}
