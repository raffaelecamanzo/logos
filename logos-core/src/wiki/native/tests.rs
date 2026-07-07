//! Unit tests for the native (extracted) wiki read-model ([FR-WK-10], [ADR-32]).
//!
//! These drive [`render`](super::render) over an in-memory `logos.db`
//! ([`SqliteGraphStore::open_in_memory`]) seeded with files, code nodes, config
//! artifacts, and edges — so the three sections, exclusion parity, determinism,
//! and the no-store-write guarantee are exercised without a real tree or
//! pipeline. The end-to-end engine/memoization fixtures live in
//! `tests/wiki_store.rs`.
//!
//! [FR-WK-10]: ../../../../docs/specs/requirements/FR-WK-10.md
//! [ADR-32]: ../../../../docs/specs/architecture/decisions/ADR-32.md

use super::*;
use crate::graph_store::{NewNode, SqliteGraphStore};
use crate::model::{EdgeKind, LogosSymbol, NodeId, NodeKind};

/// A counter-free symbol minter — each call mints a distinct `local N` symbol so
/// nodes never collide on identity, exactly as `graph_store::tests::seed` does.
struct Fixture {
    store: SqliteGraphStore,
    next_symbol: i64,
}

impl Fixture {
    fn new() -> Self {
        Fixture {
            store: SqliteGraphStore::open_in_memory().expect("in-memory store"),
            next_symbol: 0,
        }
    }

    fn file(&self, path: &str) -> i64 {
        self.store.insert_file(path, None, None).expect("file inserts")
    }

    /// Insert a node bound to `file_id` with the given kind/name/start-line.
    fn node(&mut self, file_id: i64, kind: NodeKind, name: &str, start_line: i64) -> NodeId {
        let n = self.next_symbol;
        self.next_symbol += 1;
        let sym = LogosSymbol::parse(&format!("local {n}")).expect("symbol parses");
        let symbol_id = self.store.upsert_symbol(&sym).expect("symbol upserts");
        self.store
            .insert_node(&NewNode {
                file_id: Some(file_id),
                start_line: Some(start_line),
                ..NewNode::plain(symbol_id, kind, name)
            })
            .expect("node inserts")
    }

    fn edge(&self, src: NodeId, dst: NodeId, kind: EdgeKind) {
        self.store.insert_edge(src, dst, kind).expect("edge inserts");
    }
}

/// A representative two-crate fixture: a `logos-core` crate with a code file and
/// a config file (`Cargo.toml` with two `[sections]`), and a `web` crate whose
/// node imports a `logos-core` node (a cross-crate dependency).
fn fixture() -> Fixture {
    let mut f = Fixture::new();

    let lib = f.file("logos-core/src/lib.rs");
    let core_fn = f.node(lib, NodeKind::Function, "run", 10);
    f.node(lib, NodeKind::Struct, "Engine", 3);
    // A field is a member, not a top-level declaration — must not list as a symbol.
    f.node(lib, NodeKind::Field, "root", 4);

    let web = f.file("web/src/main.rs");
    let web_fn = f.node(web, NodeKind::Function, "serve", 1);

    // A cross-crate dependency: web::serve imports logos-core::run.
    f.edge(web_fn, core_fn, EdgeKind::Imports);

    // A config artifact: Cargo.toml with a `[package]` and a nested `[lib]` table.
    let cargo = f.file("logos-core/Cargo.toml");
    let cfg = f.node(cargo, NodeKind::ConfigFile, "Cargo.toml", 1);
    let package = f.node(cargo, NodeKind::ConfigSection, "package", 2);
    let lib_sec = f.node(cargo, NodeKind::ConfigSection, "lib", 6);
    f.edge(cfg, package, EdgeKind::Contains);
    f.edge(cfg, lib_sec, EdgeKind::Contains);

    f
}

// ── FR-WK-10: the three sections render, byte-identical, with the label ──────

#[test]
fn render_is_byte_identical_at_a_fixed_revision() {
    let f = fixture();
    let first = render(&f.store, 7).unwrap();
    let second = render(&f.store, 7).unwrap();
    assert_eq!(first, second, "render is deterministic at a fixed revision");

    // All three sections are populated by the fixture.
    assert!(!first.structure.is_empty(), "codebase structure renders");
    assert!(!first.files.is_empty(), "files view renders");
    assert!(
        first.dependency_mermaid.starts_with("graph LR\n"),
        "dependency Mermaid renders"
    );
}

#[test]
fn render_carries_the_extracted_at_revision_label() {
    let f = fixture();
    let native = render(&f.store, 42).unwrap();
    assert_eq!(native.revision, 42);
    assert_eq!(native.label, "extracted — live from graph @revision 42");
    assert_eq!(native.label, native_label(42));
}

#[test]
fn a_revision_advance_re_renders_with_the_new_label() {
    let f = fixture();
    let at_one = render(&f.store, 1).unwrap();
    let at_two = render(&f.store, 2).unwrap();
    assert_ne!(at_one.label, at_two.label, "the label tracks the revision");
    assert_eq!(at_two.label, "extracted — live from graph @revision 2");
    // The structural content is the same graph snapshot — only the revision moved.
    assert_eq!(at_one.structure, at_two.structure);
}

// ── FR-WK-10: exclusion parity — only the admitted set appears ───────────────

#[test]
fn only_admitted_files_appear_in_every_section() {
    let f = fixture();
    let native = render(&f.store, 1).unwrap();

    let admitted = ["logos-core/src/lib.rs", "web/src/main.rs", "logos-core/Cargo.toml"];
    let listed: Vec<&str> = native.files.iter().map(|e| e.path.as_str()).collect();
    for path in admitted {
        assert!(listed.contains(&path), "{path} is in the files view");
    }
    // A file the graph never indexed (config exclusion) appears nowhere — the
    // native tier derives only from `indexed_files()`, never a second walk.
    let excluded = "vendor/skip.rs";
    assert!(
        !native.files.iter().any(|e| e.path == excluded),
        "an excluded file is absent from the files view"
    );
    assert!(
        !native.dependency_mermaid.contains("vendor"),
        "an excluded crate is absent from the dependency diagram"
    );
    assert!(
        !structure_contains_path(&native.structure, excluded),
        "an excluded file is absent from the codebase structure"
    );
}

/// Whether any node in the structure tree carries `path`.
fn structure_contains_path(nodes: &[StructureNode], path: &str) -> bool {
    nodes
        .iter()
        .any(|n| n.path == path || structure_contains_path(&n.children, path))
}

// ── FR-WK-10: codebase structure (crate→module→file) ─────────────────────────

#[test]
fn structure_groups_crate_then_module_then_file() {
    let f = fixture();
    let native = render(&f.store, 1).unwrap();

    // Top-level entries are crates, name-ordered.
    let crates: Vec<&str> = native.structure.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(crates, ["logos-core", "web"], "crates are name-ordered");
    assert!(native.structure.iter().all(|n| n.kind == StructureKind::Crate));

    // logos-core nests src/ (a module) and Cargo.toml (a file).
    let core = &native.structure[0];
    let src = core
        .children
        .iter()
        .find(|n| n.name == "src")
        .expect("src module under logos-core");
    assert_eq!(src.kind, StructureKind::Module);
    assert_eq!(src.path, "logos-core/src");
    assert!(core.children.iter().any(|n| n.name == "Cargo.toml" && n.kind == StructureKind::File));

    // The code-node count rolls up every code node (run + Engine + the root
    // Field): only the non-code doc/config layers are excluded from the count.
    let lib = src
        .children
        .iter()
        .find(|n| n.name == "lib.rs")
        .expect("lib.rs file");
    assert_eq!(lib.node_count, 3, "all three code nodes counted");
}

// ── FR-WK-10: files view (declarations + related files) ──────────────────────

#[test]
fn files_view_lists_declarations_and_related_files() {
    let f = fixture();
    let native = render(&f.store, 1).unwrap();

    let lib = native
        .files
        .iter()
        .find(|e| e.path == "logos-core/src/lib.rs")
        .expect("lib.rs entry");
    // Declarations ordered by start line then name: Engine (line 3) before run (line 10).
    let decls: Vec<(&str, &str)> = lib.symbols.iter().map(|s| (s.name.as_str(), s.kind)).collect();
    assert_eq!(decls, [("Engine", "struct"), ("run", "function")]);
    assert!(
        !lib.symbols.iter().any(|s| s.name == "root"),
        "a Field member is not a top-level declaration"
    );

    let web = native
        .files
        .iter()
        .find(|e| e.path == "web/src/main.rs")
        .expect("web entry");
    assert_eq!(
        web.related,
        ["logos-core/src/lib.rs"],
        "the cross-crate import is surfaced as a related file"
    );
}

// ── FR-WK-10: dependency Mermaid ─────────────────────────────────────────────

#[test]
fn dependency_mermaid_renders_crate_level_edges() {
    let f = fixture();
    let native = render(&f.store, 1).unwrap();

    let m = &native.dependency_mermaid;
    assert!(m.starts_with("graph LR\n"));
    // Crates get stable positional ids (sorted): logos-core = c0, web = c1; the
    // real name is the label, so distinct names can never collide on an id.
    assert!(m.contains("c0[\"logos-core\"]"), "crate node labelled by name");
    assert!(m.contains("c1[\"web\"]"));
    assert!(m.contains("c1 --> c0"), "web depends on logos-core");
    // No self-edge for an intra-crate dependency.
    assert!(!m.contains("c1 --> c1"));
}

#[test]
fn dependency_mermaid_keeps_collision_prone_crate_names_distinct() {
    // `my-crate` and `my.crate` both sanitize to `my_crate`; positional ids keep
    // them distinct boxes so neither silently overwrites the other.
    let mut f = Fixture::new();
    let a = f.file("my-crate/src/lib.rs");
    let b = f.file("my.crate/src/lib.rs");
    f.node(a, NodeKind::Function, "a", 1);
    f.node(b, NodeKind::Function, "b", 1);

    let native = render(&f.store, 1).unwrap();
    let m = &native.dependency_mermaid;
    assert!(m.contains("[\"my-crate\"]"), "both crate labels render verbatim");
    assert!(m.contains("[\"my.crate\"]"));
    // Two distinct positional ids — the boxes do not collapse into one.
    assert!(m.contains("c0[") && m.contains("c1["), "two distinct node ids");
}

// ── NFR-RA-06 / NFR-SE-01: render writes nothing ─────────────────────────────

#[test]
fn render_performs_no_store_write() {
    let f = fixture();
    let before = f.store.counts().unwrap();
    let _ = render(&f.store, 1).unwrap();
    let _ = render(&f.store, 2).unwrap();
    let after = f.store.counts().unwrap();
    assert_eq!(before, after, "render mutates no row in the graph store");
}

// (StoreCounts is the inferred return type of `counts()`; no explicit use needed.)

// ── Empty / un-indexed root ──────────────────────────────────────────────────

#[test]
fn empty_graph_renders_empty_sections_with_the_label() {
    let store = SqliteGraphStore::open_in_memory().unwrap();
    let native = render(&store, 0).unwrap();
    assert!(native.structure.is_empty());
    assert!(native.files.is_empty());
    assert_eq!(native.dependency_mermaid, "graph LR\n", "no crates, just the header");
    assert_eq!(native.label, "extracted — live from graph @revision 0");
}
