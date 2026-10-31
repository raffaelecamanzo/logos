//! Method → struct attribution for Rust and Go (S-510, [CR-166], metric
//! semantics v9) — pure, no I/O.
//!
//! Extraction makes the crate/package *module*, not the struct, the `Contains`
//! parent of a Rust `impl` method and of a Go receiver method, so a struct never
//! had a method count and Focus could flag it by span alone. Each such method
//! records the base name of the type it belongs to (`nodes.self_type`: a Rust
//! impl's self type, [FR-RS-11]; a Go receiver's base type, [FR-EX-12]). Focus
//! reads that record to count the method toward the one `Struct` it names:
//!
//! - **Rust** — a method inside a recorded impl block belongs to the struct of
//!   that name anywhere in its **crate** (the impl may sit in any module of it):
//!   the files under the same directory prefix before their last `src/`, or in
//!   the same directory when there is none.
//! - **Go** — a receiver method belongs to the struct of that name in its
//!   **package**, the directory of its file.
//!
//! A candidate struct must also be declared in a file of the method's own
//! extension, so a same-named struct of another language sharing a directory or
//! a crate-less path is never a candidate.
//!
//! A method names exactly one struct or none: zero same-named structs in scope
//! (a type alias, an enum, a type from another crate or package) or two or more
//! (two files of one directory declaring it twice) leave it **unattributed**,
//! and the count of those is reported with the count of every method that
//! records a self type ([`MethodAttribution`]). A same-named struct of another
//! crate or package is never a candidate. The attribution feeds Focus only: node
//! ids, symbols and edges are untouched and Cohesion (which scores `Class` alone)
//! reads nothing of it.
//!
//! [CR-166]: ../../../docs/requests/CR-166-rust-and-go-methods-count-toward-their-type-for-focus.md
//! [FR-RS-11]: ../../../docs/specs/requirements/FR-RS-11.md
//! [FR-EX-12]: ../../../docs/specs/requirements/FR-EX-12.md
//! [`MethodAttribution`]: crate::models::quality::MethodAttribution

use std::collections::HashMap;

use crate::graph_store::{ImplBlockRow, NodeRow};
use crate::model::{NodeId, NodeKind};

use super::directory_of;

/// The store facts that unite declarations into the containers Focus and
/// Cohesion score: the part links of partial types (S-557) and the methods
/// attributed to their struct (S-510). Both reach `ContainerIndex::build`
/// as one input so a caller cannot pass one and forget the other.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContainerLinks {
    /// `(part, first part)` of every later part of a partial type
    /// ([`GraphStore::partial_type_links`](crate::graph_store::GraphStore::partial_type_links),
    /// S-557, [FR-RS-29]).
    ///
    /// [FR-RS-29]: ../../../docs/specs/requirements/FR-RS-29.md
    pub partial_types: Vec<(NodeId, NodeId)>,
    /// Every method recording a self type, in id order, with the one struct it
    /// is attributed to — `None` when it names zero or two-plus structs of its
    /// crate/package (`attribute`).
    pub typed_methods: Vec<(NodeId, Option<NodeId>)>,
}

impl ContainerLinks {
    /// The links of a graph snapshot: `partial_types` as the store reads them,
    /// and the attribution of every node of `self_types` (`attribute`).
    pub fn new(
        partial_types: Vec<(NodeId, NodeId)>,
        nodes: &[NodeRow],
        self_types: &[(NodeId, String)],
        impl_blocks: &[ImplBlockRow],
    ) -> Self {
        Self {
            partial_types,
            typed_methods: attribute(nodes, self_types, impl_blocks),
        }
    }
}

/// Attribute each method of `self_types` (`(node, self type)`, id-ordered) to
/// the one same-crate or same-package `Struct` its self type names.
///
/// The scope is the crate ([`crate_root`]) when the method lies inside one of
/// `impl_blocks` (the language places a type's methods in impl blocks, which may
/// sit in any module of the crate), and the directory of its file otherwise (a
/// receiver method, bound to a type of its own package); a candidate is also of
/// the method file's extension. Only a [`NodeKind::Method`] is a member
/// that Focus counts; any other node carrying a self type is skipped. The result
/// is in `self_types` order.
pub(super) fn attribute(
    nodes: &[NodeRow],
    self_types: &[(NodeId, String)],
    impl_blocks: &[ImplBlockRow],
) -> Vec<(NodeId, Option<NodeId>)> {
    let node_of: HashMap<NodeId, &NodeRow> = nodes.iter().map(|n| (n.id, n)).collect();
    let mut structs: HashMap<&str, Vec<&NodeRow>> = HashMap::new();
    for n in nodes.iter().filter(|n| n.kind == NodeKind::Struct && n.file_path.is_some()) {
        structs.entry(n.name.as_str()).or_default().push(n);
    }
    let mut blocks: HashMap<&str, Vec<(i64, i64)>> = HashMap::new();
    for b in impl_blocks {
        blocks
            .entry(b.file_path.as_str())
            .or_default()
            .push((i64::from(b.start_line), i64::from(b.end_line)));
    }

    self_types
        .iter()
        .filter_map(|(id, self_type)| {
            let method = node_of.get(id).filter(|n| n.kind == NodeKind::Method)?;
            let Some(path) = method.file_path.as_deref() else {
                return Some((*id, None));
            };
            let in_impl = method.start_line.is_some_and(|line| {
                blocks
                    .get(path)
                    .is_some_and(|spans| spans.iter().any(|&(start, end)| start <= line && line <= end))
            });
            let scope = |file: &str| {
                if in_impl {
                    crate_root(file)
                } else {
                    directory_of(file)
                }
            };
            let wanted = (scope(path), extension(path));
            let mut candidates = structs
                .get(self_type.as_str())
                .into_iter()
                .flatten()
                .filter(|s| s.file_path.as_deref().is_some_and(|p| (scope(p), extension(p)) == wanted));
            let owner = match (candidates.next(), candidates.next()) {
                (Some(only), None) => Some(only.id),
                _ => None,
            };
            Some((*id, owner))
        })
        .collect()
}

/// The crate a Rust file belongs to, as a path: the directories before the last
/// `src/` segment (`libs/util/src/a/b.rs` → `libs/util`), or the file's own
/// directory when it has none (`examples/demo.rs` → `examples`). The full prefix,
/// not the last segment, so `libs/util` and `tools/util` are two crates.
fn crate_root(path: &str) -> String {
    let dirs: Vec<&str> = path.split('/').collect();
    let dirs = &dirs[..dirs.len().saturating_sub(1)];
    match dirs.iter().rposition(|d| *d == "src") {
        Some(at) => dirs[..at].join("/"),
        None => dirs.join("/"),
    }
}

/// The lower-cased extension of a path's file name — the language a candidate
/// struct must share with the method; empty when the file has none.
fn extension(path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.rsplit_once('.').map_or(String::new(), |(_, ext)| ext.to_ascii_lowercase())
}
