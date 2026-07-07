//! The **native (extracted) wiki tier** ([FR-WK-10], [ADR-32], [CR-027]) — a
//! stateless read-model that live-renders deterministic wiki sections **directly
//! from the graph**, beside the agent-authored store ([ADR-24]).
//!
//! # The tier in one sentence
//! [`render`] is a **pure function of `logos.db` at a given graph revision**: it
//! reads nodes, edges, and the admitted-file set out of the graph and folds them
//! into three deterministic sections — Codebase structure, a Files view, and a
//! dependency Mermaid diagram — and returns them with the
//! "extracted — live from graph @revision N" provenance label. It performs **no
//! `wiki.db` write, no second filesystem walk, and no LLM/network call**
//! ([NFR-SE-01], [NFR-RA-06]).
//!
//! # Exclusion parity, by construction ([FR-WK-10])
//! Every file the native tier names comes from the graph's own admitted-file set
//! ([`GraphStore::indexed_files`]) and every symbol from a node already in the
//! graph — both of which only ever exist for files the graph admitted. There is
//! **no independent file walk**, so a file excluded by `config.toml`
//! (`gitignore`, `exclude` globs, `ignored_dirs`, or the `languages` gate) is
//! never indexed and therefore can never appear in any native section. The
//! admitted set is the single gate ([CR-027] §3.2, the [wiki-engine] graph-store
//! dependency).
//!
//! # Determinism ([NFR-RA-06])
//! The graph read order is not contractually fixed, so every collection this
//! module emits is **explicitly sorted** (paths lexicographically, Mermaid edges
//! by endpoint). Given the same `logos.db` and the same `revision`, [`render`] returns a
//! byte-identical [`NativeWiki`]; the memoization the [`Engine`](crate::Engine)
//! layers on top keys on `revision`, so a cache miss simply re-renders.
//!
//! [FR-WK-10]: ../../../docs/specs/requirements/FR-WK-10.md
//! [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
//! [ADR-24]: ../../../docs/specs/architecture/decisions/ADR-24.md
//! [ADR-32]: ../../../docs/specs/architecture/decisions/ADR-32.md
//! [CR-027]: ../../../docs/requests/CR-027-structured-wiki-native-tier-freshness.md
//! [wiki-engine]: ../../../docs/specs/architecture/components/wiki-engine.md

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;

use crate::graph_store::{EdgeRow, GraphStore, NodeRow};
use crate::model::{EdgeKind, NodeId, NodeKind};

/// The provenance label every native unit carries ([FR-WK-10]) — distinct from
/// the agent tier's [`GENERATED_CONTENT_MARKER`](super::GENERATED_CONTENT_MARKER).
/// The text is part of the contract: surfaces render it verbatim so native
/// (extracted-from-graph) content can never be mistaken for agent prose, and the
/// `@revision N` suffix tells the reader exactly which graph snapshot it reflects.
pub fn native_label(revision: u64) -> String {
    format!("extracted — live from graph @revision {revision}")
}

/// What level of the codebase-structure tree a [`StructureNode`] sits at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StructureKind {
    /// A top-level directory — the "crate" (or top-level package) grouping.
    Crate,
    /// A nested directory — a module within a crate.
    Module,
    /// A leaf file.
    File,
}

/// One node of the Codebase-structure tree ([FR-WK-10] "crate→module→file").
///
/// Directories nest via [`children`](StructureNode::children); files are leaves.
/// `node_count` is the number of **code** nodes (every node except the non-code
/// doc/config layers) the graph records under this subtree — for a file, its own
/// nodes; for a directory, the sum over its descendants.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StructureNode {
    /// The path segment (a directory or file name), never the whole path.
    pub name: String,
    /// Whether this is a crate (top-level dir), module (nested dir), or file.
    pub kind: StructureKind,
    /// The full repo-relative path of this node (the cumulative prefix).
    pub path: String,
    /// Code nodes under this subtree (own, for a file; aggregate, for a dir).
    pub node_count: usize,
    /// Child directories (first, name-ordered) then child files (name-ordered).
    pub children: Vec<StructureNode>,
}

/// One declared symbol in a [`FileEntry`] — a structural fact Logos owns ([FR-WK-10]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FileSymbol {
    /// The declaration's human-facing name.
    pub name: String,
    /// Its ontology kind label (`function`, `struct`, `trait`, …).
    pub kind: &'static str,
}

/// One file in the Files view ([FR-WK-10]) — grouped (by path order) crate→
/// module→file, carrying the structural facts the graph owns: the top-level
/// declarations defined in the file, and the other files it depends on via graph
/// edges.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FileEntry {
    /// The repo-relative file path.
    pub path: String,
    /// Top-level declarations defined in the file, ordered by start line then name.
    pub symbols: Vec<FileSymbol>,
    /// Other admitted files this file depends on (via non-containment code
    /// edges), de-duplicated and path-ordered.
    pub related: Vec<String>,
}

/// The native (extracted) wiki read-model ([FR-WK-10], [ADR-32]) — the three
/// deterministic sections plus the revision they were rendered at and the
/// provenance label. A pure function of `logos.db` at `revision`; nothing here is
/// ever persisted to `wiki.db`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct NativeWiki {
    /// The graph revision this render reflects ([FR-SY-09]).
    pub revision: u64,
    /// The "extracted — live from graph @revision N" provenance label.
    pub label: String,
    /// Codebase structure — the crate→module→file tree.
    pub structure: Vec<StructureNode>,
    /// The Files view — every admitted file with its declarations and dependencies.
    pub files: Vec<FileEntry>,
    /// A crate-level dependency diagram in Mermaid `graph LR` syntax.
    pub dependency_mermaid: String,
}

/// Live-render the three native sections from the graph at `revision` ([FR-WK-10]).
///
/// A **pure read** of the graph: it reads the admitted-file set, nodes, and edges
/// and folds them into the deterministic read-model. It never writes `wiki.db`,
/// never walks the filesystem a second time, and never calls an LLM/network
/// ([NFR-SE-01]). `revision` is supplied by the caller (the engine reads the
/// persisted [`graph_revision`](GraphStore::graph_revision) and memoizes on it),
/// so the render is a deterministic function of `(graph, revision)` ([NFR-RA-06]).
///
/// # Errors
/// Returns an error only on an unexpected backing-store failure.
pub fn render(store: &dyn GraphStore, revision: u64) -> Result<NativeWiki> {
    // The admitted-file set IS the graph's record of what it indexed — the same
    // source of truth `config::discover()`/`indexed_files()` feeds the pipeline,
    // so a config-excluded file is absent here without a second file walk
    // ([FR-WK-10] exclusion parity).
    let admitted: BTreeSet<String> = store
        .indexed_files()?
        .into_iter()
        .map(|f| f.path)
        .collect();
    let nodes = store.all_nodes()?;
    let edges = store.all_edges()?;

    let structure = build_structure(&admitted, &nodes);
    let files = build_files(&admitted, &nodes, &edges);
    let dependency_mermaid = build_dependency_mermaid(&admitted, &nodes, &edges);

    Ok(NativeWiki {
        revision,
        label: native_label(revision),
        structure,
        files,
        dependency_mermaid,
    })
}

// ── Shared helpers ────────────────────────────────────────────────────────────

/// The "crate" a repo-relative path belongs to: its first path segment, or `None`
/// for a root-level file (no separator) which has no enclosing crate.
fn crate_of(path: &str) -> Option<&str> {
    path.split_once('/').map(|(head, _)| head)
}

/// Whether an edge is a **code dependency** for the crate-level diagram and the
/// per-file related list: any edge except lexical containment ([`Contains`]),
/// the derived governance marker ([`ForbiddenDependency`]), and the non-code
/// documentation/config-reference layers. Mirrors the canonical dependency view
/// graph hydration runs metrics over ([ADR-19], [ADR-26], [FR-CG-05]).
///
/// [`Contains`]: EdgeKind::Contains
/// [`ForbiddenDependency`]: EdgeKind::ForbiddenDependency
fn is_dependency_edge(kind: EdgeKind) -> bool {
    !matches!(kind, EdgeKind::Contains | EdgeKind::ForbiddenDependency)
        && !kind.is_documentation()
        && !kind.is_config_reference()
}

/// `id → file_path` for every node bound to an **admitted** file — the lookup the
/// related-files and crate-dependency folds share. A node whose file is not in
/// the admitted set is dropped, preserving exclusion parity even if the graph
/// somehow carried a dangling reference.
fn node_file_index<'a>(
    admitted: &BTreeSet<String>,
    nodes: &'a [NodeRow],
) -> BTreeMap<NodeId, &'a str> {
    nodes
        .iter()
        .filter_map(|n| {
            let path = n.file_path.as_deref()?;
            admitted.contains(path).then_some((n.id, path))
        })
        .collect()
}

// ── Codebase structure (crate→module→file) ──────────────────────────────────

/// An intermediate, name-ordered tree assembled from the admitted file paths —
/// directories nest, files are leaves carrying their code-declaration count.
#[derive(Default)]
struct TreeBuilder {
    dirs: BTreeMap<String, TreeBuilder>,
    files: BTreeMap<String, usize>,
}

impl TreeBuilder {
    fn insert(&mut self, segments: &[&str], count: usize) {
        match segments {
            [] => {}
            [file] => {
                self.files.insert((*file).to_string(), count);
            }
            [dir, rest @ ..] => {
                self.dirs.entry((*dir).to_string()).or_default().insert(rest, count);
            }
        }
    }

    /// Emit the children of this builder at `prefix`/`depth`, directories first
    /// (each name-ordered) then files (name-ordered) — a deterministic, stable
    /// shape ([NFR-RA-06]).
    fn into_nodes(self, prefix: &str, depth: usize) -> (Vec<StructureNode>, usize) {
        let mut out = Vec::new();
        let mut subtotal = 0usize;
        for (name, builder) in self.dirs {
            let path = join(prefix, &name);
            let (children, count) = builder.into_nodes(&path, depth + 1);
            subtotal += count;
            out.push(StructureNode {
                name,
                kind: if depth == 0 { StructureKind::Crate } else { StructureKind::Module },
                path,
                node_count: count,
                children,
            });
        }
        for (name, count) in self.files {
            let path = join(prefix, &name);
            subtotal += count;
            out.push(StructureNode {
                name,
                kind: StructureKind::File,
                path,
                node_count: count,
                children: Vec::new(),
            });
        }
        (out, subtotal)
    }
}

/// Join a path `prefix` and a `segment` without a leading slash for the root.
fn join(prefix: &str, segment: &str) -> String {
    if prefix.is_empty() {
        segment.to_string()
    } else {
        format!("{prefix}/{segment}")
    }
}

/// The Codebase-structure tree: every admitted file placed under its
/// crate→module path, each file carrying its count of **code** nodes
/// (non-doc, non-config), directories carrying the aggregate.
fn build_structure(admitted: &BTreeSet<String>, nodes: &[NodeRow]) -> Vec<StructureNode> {
    let mut code_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for node in nodes {
        if node.kind.is_non_code() {
            continue;
        }
        if let Some(path) = node.file_path.as_deref() {
            if admitted.contains(path) {
                *code_counts.entry(path).or_default() += 1;
            }
        }
    }

    let mut tree = TreeBuilder::default();
    for path in admitted {
        let segments: Vec<&str> = path.split('/').collect();
        tree.insert(&segments, code_counts.get(path.as_str()).copied().unwrap_or(0));
    }
    tree.into_nodes("", 0).0
}

// ── Files view (declarations + related files) ────────────────────────────────

/// Whether a node kind is a **top-level declaration** the Files view surfaces as
/// an "interface/export" — the nominal types, callables, and framework anchors,
/// excluding members (fields/variables) and the non-code layers.
fn is_declaration(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Module
            | NodeKind::Class
            | NodeKind::Interface
            | NodeKind::Trait
            | NodeKind::Struct
            | NodeKind::Enum
            | NodeKind::Function
            | NodeKind::TypeAlias
            | NodeKind::Macro
            | NodeKind::Constant
            | NodeKind::Route
            | NodeKind::Component
    )
}

/// The Files view: every admitted file with its top-level declarations (ordered
/// by start line then name) and the other admitted files it depends on (via
/// non-containment code edges, de-duplicated and path-ordered).
fn build_files(
    admitted: &BTreeSet<String>,
    nodes: &[NodeRow],
    edges: &[EdgeRow],
) -> Vec<FileEntry> {
    let id_to_file = node_file_index(admitted, nodes);

    // Declarations per admitted file, gathered then sorted per file below.
    let mut symbols: BTreeMap<&str, Vec<(i64, &str, &'static str)>> = BTreeMap::new();
    for node in nodes {
        if !is_declaration(node.kind) {
            continue;
        }
        if let Some(path) = node.file_path.as_deref() {
            if admitted.contains(path) {
                symbols.entry(path).or_default().push((
                    node.start_line.unwrap_or(0),
                    node.name.as_str(),
                    node.kind.as_str(),
                ));
            }
        }
    }

    // File→file dependencies: an edge between two nodes in different admitted
    // files contributes a related-file pair.
    let mut related: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for edge in edges {
        if !is_dependency_edge(edge.kind) {
            continue;
        }
        let (Some(&src), Some(&dst)) = (id_to_file.get(&edge.source), id_to_file.get(&edge.target))
        else {
            continue;
        };
        if src != dst {
            related.entry(src).or_default().insert(dst);
        }
    }

    admitted
        .iter()
        .map(|path| {
            let key = path.as_str();
            let mut decls = symbols.remove(key).unwrap_or_default();
            decls.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
            let symbols = decls
                .into_iter()
                .map(|(_, name, kind)| FileSymbol { name: name.to_string(), kind })
                .collect();
            let related = related
                .get(key)
                .map(|set| set.iter().map(|s| s.to_string()).collect())
                .unwrap_or_default();
            FileEntry { path: path.clone(), symbols, related }
        })
        .collect()
}

// ── Dependency Mermaid (crate-level) ─────────────────────────────────────────

/// A crate-level dependency diagram in Mermaid `graph LR` syntax: one node per
/// crate present in the admitted set, and one edge per ordered crate pair that a
/// code dependency crosses. Node declarations and edges are both sorted, so the
/// diagram string is byte-identical at a fixed revision ([NFR-RA-06]).
fn build_dependency_mermaid(
    admitted: &BTreeSet<String>,
    nodes: &[NodeRow],
    edges: &[EdgeRow],
) -> String {
    let id_to_file = node_file_index(admitted, nodes);

    // Every crate present (top-level directory of an admitted path).
    let crates: BTreeSet<&str> = admitted.iter().filter_map(|p| crate_of(p)).collect();
    // A stable, collision-free Mermaid node id per crate, assigned by sorted
    // position (`c0`, `c1`, …). Deriving the id by sanitizing the name would
    // collapse distinct crates (`my-crate` and `my.crate` → `my_crate`) and
    // silently overwrite a box; the positional id keeps every crate distinct
    // while the real name stays the label.
    let id_for: BTreeMap<&str, String> = crates
        .iter()
        .enumerate()
        .map(|(i, c)| (*c, format!("c{i}")))
        .collect();

    // Ordered crate→crate dependency pairs.
    let mut pairs: BTreeSet<(&str, &str)> = BTreeSet::new();
    for edge in edges {
        if !is_dependency_edge(edge.kind) {
            continue;
        }
        let (Some(&src), Some(&dst)) = (id_to_file.get(&edge.source), id_to_file.get(&edge.target))
        else {
            continue;
        };
        if let (Some(sc), Some(dc)) = (crate_of(src), crate_of(dst)) {
            if sc != dc {
                pairs.insert((sc, dc));
            }
        }
    }

    let mut out = String::from("graph LR\n");
    for c in &crates {
        out.push_str(&format!("  {}[\"{}\"]\n", id_for[c], mermaid_label(c)));
    }
    for (sc, dc) in &pairs {
        out.push_str(&format!("  {} --> {}\n", id_for[sc], id_for[dc]));
    }
    out
}

/// A Mermaid-safe node **label** for a crate name: the characters that would
/// terminate a `["…"]` label (`"`, `[`, `]`) are neutralized so the label always
/// renders verbatim. The node *id* is positional (`cN`), so labels need not be
/// unique — only safe to embed.
fn mermaid_label(name: &str) -> String {
    name.replace('"', "'").replace(['[', ']'], "_")
}

#[cfg(test)]
mod tests;
