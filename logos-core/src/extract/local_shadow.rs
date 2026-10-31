//! A local binding shadows the receiver and the callee (S-594, [CR-191],
//! [FR-RS-44]): the rules that read the facts [`super::locals`] records.
//!
//! A local binding is **in force** at a point when its scope span encloses the
//! point and it is declared before it, or is hoisted ([`LocalBindingFact`]).
//! "Before" means before the statement that declares it: a point inside that
//! statement — the right-hand side of Go's `s := s.Clone()`, Rust's `let len =
//! len();`, a Python default `h=g()` — still reads the outer name. The
//! innermost binding in force decides — the one whose scope starts last, then
//! the one written last:
//!
//! - a **local import** (Python's `from m import f`, a C++ block `using
//!   ns::f`, a Rust block `use`) makes a bare call of its name a call through
//!   that import: the row is marked [`LOCAL_IMPORT_ALIAS`], which the binder
//!   resolves through the file's import of the name alone, never the lexical
//!   scope that would reach a same-named member or module function;
//! - a **local value** — a parameter or a variable — makes a bare call of its
//!   name a call through a value: the row is marked [`LOCAL_VALUE_ALIAS`],
//!   which binds nothing and is counted `local-binding` — except in a language
//!   whose plugin declares `separate_call_namespace` (Java, Ruby), where a
//!   call never names a value;
//! - a receiver identifier rebound by either is no longer the method's own
//!   receiver ([`super::receiver`] reads [`LocalShadows::rebinds`]).
//!
//! A binding that **is** a declaration the graph captured — a nested function,
//! a local class, a `const f = () => …` the plugin declares — is not a local
//! value: the lexical scope binds the bare call to that declaration, as it did
//! before. Every other binding without a declaration is data flow this graph
//! does not follow, so it binds nothing rather than something it hides
//! ([NFR-RA-05]).
//!
//! [CR-191]: ../../../docs/requests/CR-191-a-local-binding-shadows-the-receiver-and-the-callee.md
//! [FR-RS-44]: ../../../docs/specs/requirements/FR-RS-44.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md

use std::collections::HashMap;

use tree_sitter::Node;

use super::{Decl, LocalBindingFact, RefFact};
use crate::model::LocalBindingKind;
use crate::resolve::{LOCAL_IMPORT_ALIAS, LOCAL_VALUE_ALIAS};

/// What the local binding in force at a bare call makes of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Shadow {
    /// A call through a local import.
    Import,
    /// A call through a local value: a parameter or a variable.
    Value,
}

impl Shadow {
    /// The alias a bare Path-form call of the shadowed name carries: the mark
    /// of a call through this binding.
    pub(super) fn alias(self) -> String {
        match self {
            Shadow::Import => LOCAL_IMPORT_ALIAS,
            Shadow::Value => LOCAL_VALUE_ALIAS,
        }
        .to_string()
    }

    /// Mark `row` — a bare Path-form call of the shadowed name — as a call
    /// through this binding.
    pub(super) fn mark(self, row: &mut RefFact) {
        row.alias = Some(self.alias());
    }
}

/// A position as the facts spell one: a 1-based line, a 0-based byte column.
type Point = (u32, u32);

/// The file's local bindings, by name, ready to answer which is in force at a
/// node. Built only by [`LocalShadows::new`]: no default, whose
/// `values_shadow_calls` would silently turn the value rule off.
pub(super) struct LocalShadows<'a> {
    by_name: HashMap<&'a str, Vec<&'a LocalBindingFact>>,
    /// Whether a local value shadows a bare call: not where calls and values
    /// are separate namespaces.
    values_shadow_calls: bool,
}

impl<'a> LocalShadows<'a> {
    /// The facts of `facts` that can shadow: every local import, and every
    /// local value that no declaration of `decls` captures at its position — a
    /// declaration of its name, inside its scope, around the bound name or
    /// held directly by the binding's own declarator (JavaScript's `var f =
    /// exports.f = function …`, whose function the graph records at the
    /// assignment; never one nested deeper in the value). (The
    /// callable a parameter belongs to is its scope, not inside it: `function
    /// f(f) { f() }` calls the parameter.)
    pub(super) fn new(facts: &'a [LocalBindingFact], decls: &[Decl<'_>], separate_call_namespace: bool) -> Self {
        let mut declarations: HashMap<&str, Vec<Node<'_>>> = HashMap::new();
        for d in decls {
            declarations.entry(d.name.as_str()).or_default().push(d.node);
        }
        let mut by_name: HashMap<&'a str, Vec<&'a LocalBindingFact>> = HashMap::new();
        for fact in facts {
            let scope = ((fact.scope_start_line, fact.scope_start_column), (fact.scope_end_line, fact.scope_end_column));
            let bound = (fact.line, fact.column);
            // The scope test first: it is the cheap one, and it rules out every
            // same-named declaration elsewhere in the file before the tree is
            // walked (`held_at`) — minified code declares one name hundreds of
            // times.
            let declared = || {
                declarations.get(fact.name.as_str()).into_iter().flatten().any(|&node| {
                    let (start, end) = span(node);
                    (start, end) != scope
                        && scope.0 <= start
                        && end <= scope.1
                        && (encloses((start, end), bound) || held_at(node, bound))
                })
            };
            if fact.kind == LocalBindingKind::Import || !declared() {
                by_name.entry(fact.name.as_str()).or_default().push(fact);
            }
        }
        LocalShadows { by_name, values_shadow_calls: !separate_call_namespace }
    }

    /// What a local binding in force makes of the `@ref.call` at `node`
    /// written as `segments`: `None` for a qualified call or a path, which no
    /// local binding names, and for a bare name none is in force for. The
    /// shadowed call is a call through that binding, never through an import
    /// of the file nor typed by the receiver pass.
    pub(super) fn bare(&self, node: Node<'_>, segments: &[String], qualified: bool) -> Option<Shadow> {
        match segments {
            [name] if !qualified => self.at(node, name),
            _ => None,
        }
    }

    /// What the innermost binding of `name` in force at `node` makes of a bare
    /// call of that name there, or `None` when none is.
    pub(super) fn at(&self, node: Node<'_>, name: &str) -> Option<Shadow> {
        match self.in_force(node, name)?.kind {
            LocalBindingKind::Import => Some(Shadow::Import),
            LocalBindingKind::Parameter | LocalBindingKind::Variable => self.values_shadow_calls.then_some(Shadow::Value),
        }
    }

    /// Whether a local binding of `name` is in force at `node` — a receiver
    /// identifier the method's body has rebound (Go's `for _, s := range xs`
    /// inside `func (s *Svc) …`), which is no longer the receiver.
    pub(super) fn rebinds(&self, node: Node<'_>, name: &str) -> bool {
        self.in_force(node, name).is_some()
    }

    fn in_force(&self, node: Node<'_>, name: &str) -> Option<&'a LocalBindingFact> {
        let at = point(node);
        self.by_name
            .get(name)?
            .iter()
            .copied()
            .filter(|f| {
                let scope = ((f.scope_start_line, f.scope_start_column), (f.scope_end_line, f.scope_end_column));
                let bound = (f.line, f.column);
                encloses(scope, at) && (f.hoisted || (bound < at && !in_declaring_statement(node, scope, bound)))
            })
            .max_by_key(|f| (f.scope_start_line, f.scope_start_column, f.line, f.column))
    }
}

/// Whether `node` — a declaration — is the value of the declarator naming the
/// binding at `bound`, or sits directly in it (`var f = exports.f = function`,
/// whose assignment the declarator holds): the declaration or its parent
/// starts at the bound name. A declaration deeper inside the value is not the
/// binding's: `var f = { f: function () {} }` binds an object.
fn held_at(node: Node<'_>, bound: Point) -> bool {
    point(node) == bound || node.parent().is_some_and(|parent| point(parent) == bound)
}

/// Whether `node` lies in the statement of `scope` that declares the binding
/// at `bound`: the child of the scope's node that holds `node` holds `bound`
/// too.
fn in_declaring_statement(node: Node<'_>, scope: (Point, Point), bound: Point) -> bool {
    let mut child = node;
    while let Some(parent) = child.parent() {
        if span(parent) == scope {
            return encloses(span(child), bound);
        }
        child = parent;
    }
    false
}

/// Where `node` starts, as the facts spell a position.
fn point(node: Node<'_>) -> Point {
    let at = node.start_position();
    (u32::try_from(at.row + 1).unwrap_or(u32::MAX), u32::try_from(at.column).unwrap_or(u32::MAX))
}

/// `node`'s span, its end exclusive.
fn span(node: Node<'_>) -> (Point, Point) {
    let end = node.end_position();
    (point(node), (u32::try_from(end.row + 1).unwrap_or(u32::MAX), u32::try_from(end.column).unwrap_or(u32::MAX)))
}

/// Whether `at` lies in `[start, end)`.
fn encloses((start, end): (Point, Point), at: Point) -> bool {
    start <= at && at < end
}
