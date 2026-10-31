//! The names a callable's body binds locally (S-593, [CR-191], [FR-EX-33]):
//! every parameter, local declaration, loop / closure / `catch` / pattern
//! binding and local import, each with its kind, the scope it binds in and its
//! position.
//!
//! These are **facts only**: no symbol, node, edge or reference is derived from
//! them here. The rules that consult them — a rebound receiver is `other`, a
//! bare call to a local import resolves through it, a bare call to a local
//! value binds nothing — are [FR-RS-44]'s.
//!
//! Every fact is read from a plugin's own `locals` query; no language is named
//! here ([NFR-MA-01]). A plugin that declares no `locals` capability records no
//! fact, and one whose query captures a form records exactly that form.
//!
//! # The `locals`-query vocabulary (the `local.` group)
//!
//! - `@local.parameter`, `@local.variable`, `@local.import` — a **binding
//!   site** of that [`LocalBindingKind`]: a name, a pattern, or a node holding
//!   several (a parameter list, a declaration). The site binds the
//!   `@local.name` captures inside it, each attributed to the innermost site
//!   enclosing it; a site with no name inside binds nothing.
//! - `@local.name` — one bound name. A name no site encloses is dropped, so a
//!   query may capture names by pattern shape alone (`tuple_pattern`'s
//!   identifiers) and let the sites decide which of them bind.
//! - `@local.callable` — a node opening a callable body: a function, method,
//!   closure or lambda. A binding no callable encloses — a top-level
//!   declaration — is not a callable's, and is not recorded.
//! - `@local.block` — a node opening a block scope, read by `.block` below.
//!
//! A site's scope is named by one companion in its **same match**, spelled
//! `<site>.<role>` (`@local.variable.declaration`):
//!
//! - `.scope` — this node: a parameter's callable, a loop, a `catch` clause, a
//!   match arm, a conditional statement whose condition binds;
//! - `.declaration` — this node's parent: the block a declaration statement
//!   sits in;
//! - `.block` — the innermost `@local.block` or `@local.callable` around the
//!   site: the block a binding nested inside an expression leaks into (a Java
//!   `instanceof` pattern, a C# `out var`);
//! - none — the innermost `@local.callable` around the site: a function-scoped
//!   binding (a JavaScript `var`, any Python local).
//!
//! `.hoisted` (any node of the match) marks the binding **in force throughout
//! its scope**, before its position too: a JavaScript `var` or function
//! declaration, a JavaScript `let` / `const` (whose temporal dead zone throws
//! rather than reaching an outer name), a name a Python function assigns
//! anywhere, a C# or Scala block local, a Rust item. Without it a binding is in
//! force from its position on.
//!
//! When several patterns capture one node as a site, the first in the query
//! file wins: a specific pattern (a `for` initializer's declaration, scoped to
//! the loop) is written before the general one (a declaration, scoped to its
//! block).
//!
//! [CR-191]: ../../../docs/requests/CR-191-a-local-binding-shadows-the-receiver-and-the-callee.md
//! [FR-EX-33]: ../../../docs/specs/requirements/FR-EX-33.md
//! [FR-RS-44]: ../../../docs/specs/requirements/FR-RS-44.md
//! [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md

use std::collections::{HashMap, HashSet};

use tree_sitter::{Node, Query, QueryCapture, QueryCursor, StreamingIterator};

use crate::model::LocalBindingKind;

/// The capture-name prefix of the vocabulary.
const GROUP: &str = "local.";

/// One name a callable's body binds locally, as [`collect`] records it.
///
/// Lines are 1-based like every line the graph stores; columns are 0-based
/// byte offsets within their line, as the parser reports them. The derived
/// order — position first — is the order [`collect`] returns.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LocalBindingFact {
    /// 1-based line of the bound name.
    pub line: u32,
    /// 0-based byte column of the bound name.
    pub column: u32,
    /// The name as written (a PHP variable keeps its `$`).
    pub name: String,
    /// What binds it.
    pub kind: LocalBindingKind,
    /// 1-based first line of the scope the name binds in.
    pub scope_start_line: u32,
    /// 0-based byte column the scope starts at.
    pub scope_start_column: u32,
    /// 1-based last line of the scope.
    pub scope_end_line: u32,
    /// 0-based byte column the scope ends at (exclusive).
    pub scope_end_column: u32,
    /// Whether the binding is in force throughout its scope rather than from
    /// its position on (`.hoisted`).
    pub hoisted: bool,
}

/// Where a binding site binds its names: the companion of its match.
#[derive(Clone, Copy)]
enum Scope<'tree> {
    /// `.scope`: this node.
    Node(Node<'tree>),
    /// `.declaration`: this node's parent.
    Parent(Node<'tree>),
    /// `.block`: the innermost block or callable around the site.
    Block,
    /// No companion: the innermost callable around the site.
    Callable,
}

/// One binding site and what its match says about it.
struct Site<'tree> {
    kind: LocalBindingKind,
    scope: Scope<'tree>,
    hoisted: bool,
    /// The query pattern that captured it: the lowest wins a node captured
    /// twice.
    pattern: usize,
}

/// Every local binding the `locals` query `query` captures in the tree at
/// `root`, sorted by position ([`LocalBindingFact`]'s order).
pub(super) fn collect(query: &Query, root: Node<'_>, source: &[u8]) -> Vec<LocalBindingFact> {
    let mut captured = Captured::default();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, source);
    while let Some(m) = matches.next() {
        captured.record(m.captures, m.pattern_index, query.capture_names());
    }
    let mut facts: Vec<LocalBindingFact> = captured
        .names
        .values()
        .filter_map(|&name| captured.fact(name, source))
        .collect();
    facts.sort();
    facts
}

/// What a file's matches captured: its binding sites, the names they bind, and
/// the callables and blocks that scope them — each keyed by node id.
#[derive(Default)]
struct Captured<'tree> {
    sites: HashMap<usize, Site<'tree>>,
    names: HashMap<usize, Node<'tree>>,
    callables: HashSet<usize>,
    blocks: HashSet<usize>,
}

impl<'tree> Captured<'tree> {
    /// Record one match's captures: a name, a callable, a block, or a site with
    /// the companions its match gives it. A site already captured by an earlier
    /// pattern keeps that pattern's reading.
    fn record(&mut self, captures: &[QueryCapture<'tree>], pattern: usize, capture_names: &[&str]) {
        for cap in captures {
            let Some(role) = capture_names[cap.index as usize].strip_prefix(GROUP) else {
                continue;
            };
            match role {
                "name" => {
                    self.names.insert(cap.node.id(), cap.node);
                }
                "callable" => {
                    self.callables.insert(cap.node.id());
                }
                "block" => {
                    self.blocks.insert(cap.node.id());
                }
                _ => self.site_at(cap.node, site(role, captures, pattern, capture_names)),
            }
        }
    }

    /// Record `site` at `node`, unless an earlier pattern already captured it.
    fn site_at(&mut self, node: Node<'tree>, site: Option<Site<'tree>>) {
        let Some(site) = site else { return };
        if self.sites.get(&node.id()).is_some_and(|kept| kept.pattern <= site.pattern) {
            return;
        }
        self.sites.insert(node.id(), site);
    }

    /// The fact `name` records, when a site encloses it and the scope that
    /// site gives it is a callable's.
    fn fact(&self, name: Node<'tree>, source: &[u8]) -> Option<LocalBindingFact> {
        let (site_node, site) = innermost(name, |n| self.sites.get(&n.id()))?;
        let is_callable = |n: Node<'_>| self.callables.contains(&n.id());
        let scope = match site.scope {
            Scope::Node(node) => node,
            Scope::Parent(node) => node.parent()?,
            Scope::Block => outer(site_node, |n| self.blocks.contains(&n.id()) || is_callable(n))?,
            Scope::Callable => outer(site_node, is_callable)?,
        };
        // A callable's binding: its scope is a callable or lies in one.
        innermost(scope, |n| is_callable(n).then_some(()))?;
        let text = name.utf8_text(source).ok()?.trim();
        (!text.is_empty()).then(|| fact(text, name, site, scope))
    }
}

/// The site a `@local.<role>` capture names, read from the companions of its
/// match (`<role>.scope`, …), or `None` for a role outside the kinds.
fn site<'tree>(role: &str, captures: &[QueryCapture<'tree>], pattern: usize, capture_names: &[&str]) -> Option<Site<'tree>> {
    let kind = LocalBindingKind::ALL.into_iter().find(|k| k.as_str() == role)?;
    let companion = |suffix: &str| {
        let name = format!("{GROUP}{role}.{suffix}");
        captures.iter().find(|c| capture_names[c.index as usize] == name).map(|c| c.node)
    };
    let scope = companion("scope")
        .map(Scope::Node)
        .or_else(|| companion("declaration").map(Scope::Parent))
        .or_else(|| companion("block").map(|_| Scope::Block))
        .unwrap_or(Scope::Callable);
    Some(Site { kind, scope, hoisted: companion("hoisted").is_some(), pattern })
}

/// The first of `node` and its ancestors `find` answers for, with its answer.
fn innermost<'tree, T>(node: Node<'tree>, find: impl Fn(Node<'tree>) -> Option<T>) -> Option<(Node<'tree>, T)> {
    let mut at = Some(node);
    while let Some(n) = at {
        if let Some(found) = find(n) {
            return Some((n, found));
        }
        at = n.parent();
    }
    None
}

/// The nearest strict ancestor of `node` that `is` accepts.
fn outer<'tree>(node: Node<'tree>, is: impl Fn(Node<'tree>) -> bool) -> Option<Node<'tree>> {
    innermost(node.parent()?, |n| is(n).then_some(())).map(|(n, ())| n)
}

/// The fact `name` (spelled `text`), bound by `site` in `scope`, records.
fn fact(text: &str, name: Node<'_>, site: &Site<'_>, scope: Node<'_>) -> LocalBindingFact {
    let line = |row: usize| u32::try_from(row + 1).unwrap_or(u32::MAX);
    let column = |col: usize| u32::try_from(col).unwrap_or(u32::MAX);
    let (at, start, end) = (name.start_position(), scope.start_position(), scope.end_position());
    LocalBindingFact {
        line: line(at.row),
        column: column(at.column),
        name: text.to_string(),
        kind: site.kind,
        scope_start_line: line(start.row),
        scope_start_column: column(start.column),
        scope_end_line: line(end.row),
        scope_end_column: column(end.column),
        hoisted: site.hoisted,
    }
}

#[cfg(test)]
#[path = "locals_tests.rs"]
mod tests;
