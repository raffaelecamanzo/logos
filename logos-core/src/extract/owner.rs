//! Declarations placed by name rather than by nesting (S-539, [FR-EX-18]):
//! the functions JavaScript defines by assignment. `User.exists = function …`
//! is written wherever the file likes — at file level after `const User =
//! module.exports`, inside a mixin `module.exports = function (User) { … }` —
//! and belongs to the declaration its assignment names, not the one it sits
//! in.
//!
//! Which declarations take part is read off captures a plugin's own `symbols`
//! query declares ([NFR-MA-01]): a plugin that declares none of them extracts
//! exactly what it did before.
//!
//! # The `symbols`-query vocabulary (the `owner.` group)
//!
//! - `@owner.name` — the identifier naming the declaration's owner (`User` in
//!   `User.exists = …`, `Foo` in `Foo.prototype.y = …`). The name is read the
//!   way JavaScript scopes it: from the function the assignment is written in,
//!   outwards through each enclosing function to the file, the owner is the
//!   function, class, `const`, `let` or `var` of that name the graph declares
//!   in the first of those scopes that declares one — never one declared in a
//!   function the assignment is not written in. A scope that binds the name
//!   where the graph captures nothing — a parameter, a nested `const` — hides
//!   every declaration further out: the assignment is to that binding. With no
//!   owner, the declaration is at file level ([NFR-RA-05]).
//! - `@owner.file` — on the declaration node: at file level, wherever it is
//!   written (`module.exports.y = …`).
//! - `@owner.enclosing` — on the declaration node: owned by the nearest
//!   declaration enclosing it, a value binding included (`{ y: … }` in
//!   `const api = { … }` is `api`'s).
//!
//! # Identity
//!
//! A placed declaration owns no scope ([`super::owns_no_scope`]): a
//! declaration written inside its body keeps the parent — and so the symbol —
//! it had before the placed one was captured ([ADR-07]). It does own the
//! references of its body. Among the declarations of its parent, it is
//! numbered after every one that is not placed ([`super::assign_ordinals`]),
//! so a sibling of its name keeps its ordinal.
//!
//! [FR-EX-18]: ../../../docs/specs/requirements/FR-EX-18.md
//! [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [ADR-07]: ../../../docs/specs/architecture/decisions/ADR-07.md

use std::collections::{HashMap, HashSet};

use tree_sitter::Node;

use crate::model::NodeKind;

use super::arity::owning_decl;
use super::Decl;

/// The capture-name prefix of the vocabulary.
const GROUP: &str = "owner.";

/// The kinds a name can own a placed declaration by: what a JavaScript name
/// binds at a declaration the graph holds.
const OWNER_KINDS: [NodeKind; 4] = [NodeKind::Function, NodeKind::Class, NodeKind::Constant, NodeKind::Variable];

/// Where a placed declaration belongs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Owner {
    /// The declaration on this node, its name resolved (`@owner.name`); `None`
    /// when no declaration the assignment can see bears it: the file.
    Declared(Option<usize>),
    /// The file (`@owner.file`).
    File,
    /// The nearest enclosing declaration (`@owner.enclosing`).
    Enclosing,
}

/// The `owner.` captures of one file, gathered during the declaration walk
/// ([`note`](Self::note)) and resolved once it ends.
#[derive(Default)]
pub(super) struct OwnerCaptures<'tree> {
    names: Vec<Node<'tree>>,
    files: HashSet<usize>,
    enclosing: HashSet<usize>,
}

impl<'tree> OwnerCaptures<'tree> {
    /// Record `capture` when it belongs to the vocabulary; `false` for any
    /// other capture, which the caller then reads itself.
    pub(super) fn note(&mut self, capture: &str, node: Node<'tree>) -> bool {
        let Some(name) = capture.strip_prefix(GROUP) else {
            return false;
        };
        match name {
            "name" => self.names.push(node),
            "file" => {
                self.files.insert(node.id());
            }
            "enclosing" => {
                self.enclosing.insert(node.id());
            }
            _ => {}
        }
        true
    }

    /// The owner of each declaration, by index into `decls`. A marker names
    /// the declaration node itself; a name sits inside the declaration it
    /// owns, and is resolved here, by node, so the declarations dropped before
    /// parents are assigned move no index it records.
    pub(super) fn per_decl(&self, decls: &[Decl<'tree>], source: &[u8]) -> Vec<Option<Owner>> {
        let mut out: Vec<Option<Owner>> = decls
            .iter()
            .map(|d| {
                if self.files.contains(&d.node.id()) {
                    Some(Owner::File)
                } else if self.enclosing.contains(&d.node.id()) {
                    Some(Owner::Enclosing)
                } else {
                    None
                }
            })
            .collect();
        let decl_at: HashMap<usize, usize> = decls.iter().enumerate().map(|(i, d)| (d.node.id(), i)).collect();
        let named: Vec<(usize, &str)> = self
            .names
            .iter()
            .filter_map(|&node| Some((owning_decl(node, &decl_at)?, node.utf8_text(source).ok()?)))
            .collect();
        if named.is_empty() {
            return out;
        }
        let placed = |i: usize| out[i].is_some() || named.iter().any(|&(j, _)| j == i);
        let owners = owners_by_scope(decls, placed);
        let mut root = decls[named[0].0].node;
        while let Some(parent) = root.parent() {
            root = parent;
        }
        let bound = super::local_bindings(root, source, false);
        for &(i, name) in &named {
            out[i] = Some(Owner::Declared(visible(decls[i].node, name, &owners, &bound)));
        }
        out
    }
}

/// Place each declaration that has an [`Owner`], on the parents
/// [`super::assign_parents`] resolved by nesting. It reads no parent, so the
/// result is the same in any order.
pub(super) fn place(decls: &mut [Decl<'_>], decl_at: &HashMap<usize, usize>) {
    let placed: Vec<(usize, Option<usize>)> = decls
        .iter()
        .enumerate()
        .filter_map(|(i, d)| {
            let parent = match d.owner.as_ref()? {
                Owner::File => None,
                Owner::Enclosing => owning_decl(d.node, decl_at),
                Owner::Declared(owner) => owner.and_then(|node| decl_at.get(&node).copied()),
            };
            Some((i, parent))
        })
        .collect();
    for (i, parent) in placed {
        decls[i].parent = parent;
    }
}

/// The function scope a node is written in: the nearest enclosing function
/// node, or `None` for the file.
fn scope_of(node: Node<'_>) -> Option<usize> {
    let mut ancestor = node.parent();
    while let Some(n) = ancestor {
        if super::FUNCTION_SCOPES.contains(&n.kind()) {
            return Some(n.id());
        }
        ancestor = n.parent();
    }
    None
}

/// The declarations a name can own a placed one by, keyed by the function
/// scope they are written in ([`scope_of`]) and their name, each to its node:
/// the first written, when one scope declares a name more than once.
fn owners_by_scope<'d>(decls: &'d [Decl<'_>], placed: impl Fn(usize) -> bool) -> HashMap<(Option<usize>, &'d str), (usize, usize)> {
    let mut owners: HashMap<(Option<usize>, &str), (usize, usize)> = HashMap::new();
    for (j, d) in decls.iter().enumerate() {
        if placed(j) || !OWNER_KINDS.contains(&d.kind) {
            continue;
        }
        let first = owners.entry((scope_of(d.node), d.name.as_str())).or_insert((d.start_byte, d.node.id()));
        if d.start_byte < first.0 {
            *first = (d.start_byte, d.node.id());
        }
    }
    owners
}

/// The node of the owner `name` resolves to from `assignment`: the one
/// declared in the function the assignment is written in, else in each
/// enclosing function outwards, else at file level — `None` when a function
/// on the way binds the name to what the graph does not declare (`bound`, the
/// file's [`super::local_bindings`]), or when nothing declares it.
fn visible(
    assignment: Node<'_>,
    name: &str,
    owners: &HashMap<(Option<usize>, &str), (usize, usize)>,
    bound: &HashMap<usize, HashSet<String>>,
) -> Option<usize> {
    let mut ancestor = assignment.parent();
    while let Some(n) = ancestor {
        if super::FUNCTION_SCOPES.contains(&n.kind()) {
            if let Some(&(_, node)) = owners.get(&(Some(n.id()), name)) {
                return Some(node);
            }
            if bound.get(&n.id()).is_some_and(|names| names.contains(name)) {
                return None;
            }
        }
        ancestor = n.parent();
    }
    owners.get(&(None, name)).map(|&(_, node)| node)
}
