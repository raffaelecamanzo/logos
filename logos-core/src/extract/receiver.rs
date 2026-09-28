//! A call site carries its receiver's proven type (S-467, [CR-150] §3.2 A).
//!
//! A receiver call used to record its bare name as a Method-form `send`, and
//! the receiver was discarded, so the binder could not tell one class's
//! `send()` from another's. Where the FILE proves the receiver's type `T`, the
//! row [`collect_refs`](super::collect_refs) pushed for that site is retyped
//! in place to a type-qualified PATH-form `T::send`, which the binder resolves
//! among `T`'s members (S-468). One row per site: the typed row replaces the
//! bare one, never joins it.
//!
//! **Path form deliberately.** A Method-form target containing `::` is the
//! binder's Rust trait-object dispatch branch (S-281, [FR-RS-08]); a Java row
//! must never reach it, and the [CR-066] receiver-method guard that governs a
//! bare Method row is left exactly as it was.
//!
//! **Which calls, and what proves `T`** — the query names each receiver shape
//! with an `@ref.receiver.*` marker capture (Java's `references.scm`); a grammar
//! that captures none is untouched, which is what keeps every other language's
//! output byte-identical. The proofs:
//!
//! | marker | receiver | `T` |
//! |---|---|---|
//! | `name` | `x.send()` | [`DeclaredTypes::get`]: the one type the file declares `x` with; a name the file never declares as a variable is a static type name when its single-type import or its own declaration names that type |
//! | `field` | `this.x.send()` | [`DeclaredTypes::field`] (S-398) |
//! | `this` | `this.send()` | the enclosing class |
//! | `super` | `super.send()` | the enclosing class's `extends`, as S-466's row records it ([`declared_superclass`]) |
//! | `implicit` | `send()` | the enclosing class, when nothing else in scope could supply `send` |
//!
//! Everything else keeps its bare row: a chained call (no marker), a name
//! bound by an untyped lambda parameter, a name declared with two disagreeing
//! types, a generic type variable, an array, `var`, and any `this` / `super` /
//! bare call inside an anonymous class body, whose class has no name.
//!
//! `T` is written as the file names it (`Mailer`, or `a::b::Mailer` from a
//! qualified `extends`). Its scope — the file's single-type imports, then the
//! same package — is the binder's package rung (S-465), the one FQN derivation
//! there is; this pass never re-derives it.
//!
//! [CR-150]: ../../../docs/requests/CR-150-java-receiver-typing-for-method-calls.md
//! [CR-066]: ../../../docs/requests/CR-066-receiver-method-overbinding.md
//! [FR-RS-08]: ../../../docs/specs/requirements/FR-RS-08.md

use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};

use tree_sitter::Node;

use super::broker::anonymous_class_body;
use super::config::accessor::DeclaredTypes;
use super::{declared_superclass, type_parameters_in_scope, Decl, RefFact};
use crate::model::{EdgeKind, NodeKind, RefForm};
use crate::resolve::STATIC_WILDCARD_ALIAS;

/// The prefix every receiver marker capture carries.
const MARKER: &str = "ref.receiver.";

/// A receiver shape a marker names.
enum Shape {
    /// `x.send()` — a simple name.
    Name(String),
    /// `this.x.send()` — a field of the enclosing class.
    Field(String),
    /// `this.send()`.
    This,
    /// `super.send()`.
    Super,
    /// `send()` — no receiver at all.
    Implicit,
}

/// What `collect_refs` knows about a file's receivers while it walks the query
/// matches — built only for a query that declares a receiver marker.
pub(super) struct Receivers<'tree> {
    /// invocation node id → the shape of its receiver.
    shapes: HashMap<usize, Shape>,
    /// Every name an untyped lambda parameter binds. File-scoped, like
    /// [`DeclaredTypes`]: such a name's type is not the one the file declares
    /// for it elsewhere, so it is poisoned for the whole file.
    untyped: HashSet<String>,
    /// The names the file's NON-static single-type imports bring into scope — a
    /// static import names a member, never a type.
    type_imports: HashSet<String>,
    /// `(row index in the refs, invocation)` for every row a receiver could
    /// retype.
    sites: Vec<(usize, Node<'tree>)>,
}

/// What a file's declarations say, for the retyping pass.
pub(super) struct FileDecls<'a, 'tree> {
    pub root: Node<'tree>,
    pub source: &'a [u8],
    pub decls: &'a [Decl<'tree>],
    pub symbols: &'a [Option<crate::model::LogosSymbol>],
    pub id_to_idx: &'a HashMap<usize, usize>,
}

/// Whether `capture` is a receiver marker.
pub(super) fn is_marker(capture: &str) -> bool {
    capture.starts_with(MARKER)
}

impl<'tree> Receivers<'tree> {
    /// The receiver state for a query, or [`None`] when it declares no marker —
    /// then nothing is recorded and no row is ever retyped.
    pub(super) fn for_query(capture_names: &[&str]) -> Option<Self> {
        capture_names.iter().any(|c| is_marker(c)).then(|| Self {
            shapes: HashMap::new(),
            untyped: HashSet::new(),
            type_imports: HashSet::new(),
            sites: Vec::new(),
        })
    }

    /// Record one marker capture. Matched on the whole capture name: this
    /// module names no grammar node kind or field, only the query's markers.
    pub(super) fn mark(&mut self, capture: &str, node: Node<'tree>, source: &[u8]) {
        let text = || node.utf8_text(source).ok().map(|t| t.trim().to_string());
        if capture == "ref.receiver.untyped" {
            if let Some(name) = text() {
                self.untyped.insert(name);
            }
            return;
        }
        // The captured node's parent is the invocation — for `this.x`, the
        // captured `x` sits one level deeper, inside the field access.
        let invocation = match capture {
            "ref.receiver.field" => node.parent().and_then(|p| p.parent()),
            _ => node.parent(),
        };
        let shape = match capture {
            "ref.receiver.name" => text().map(Shape::Name),
            "ref.receiver.field" => text().map(Shape::Field),
            "ref.receiver.this" => Some(Shape::This),
            "ref.receiver.super" => Some(Shape::Super),
            "ref.receiver.implicit" => Some(Shape::Implicit),
            _ => None,
        };
        if let (Some(invocation), Some(shape)) = (invocation, shape) {
            self.shapes.insert(invocation.id(), shape);
        }
    }

    /// Note that the row about to be pushed at `row` is a call from
    /// `invocation`, which its receiver may retype.
    pub(super) fn site(&mut self, row: usize, invocation: Option<Node<'tree>>) {
        if let Some(invocation) = invocation {
            self.sites.push((row, invocation));
        }
    }

    /// Note a non-static single-type import's name.
    pub(super) fn type_import(&mut self, name: &str) {
        self.type_imports.insert(name.to_string());
    }

    /// Retype every site whose receiver the file proves, in place. Runs once,
    /// after every match: a `super` receiver reads the `Extends` rows this same
    /// pass recorded, and a bare call the file's imports.
    pub(super) fn retype(self, refs: &mut [RefFact], file: &FileDecls<'_, 'tree>) {
        if self.sites.is_empty() {
            return;
        }
        // The one per-file walk this costs, paid only by a file with a site
        // that asks it (a simple-name or `this.x` receiver) — NFR-PE-02.
        let declared: OnceCell<DeclaredTypes> = OnceCell::new();
        let types = || declared.get_or_init(|| DeclaredTypes::build(file.root, file.source));
        let imported: HashSet<&str> = refs
            .iter()
            .filter(|r| r.kind == EdgeKind::Imports && r.form == RefForm::Path)
            .filter_map(|r| r.alias.as_deref())
            .collect();
        let static_wildcard = refs.iter().any(|r| {
            r.kind == EdgeKind::Imports
                && r.form == RefForm::Glob
                && r.alias.as_deref() == Some(STATIC_WILDCARD_ALIAS)
        });

        let mut typed: Vec<(usize, String)> = Vec::new();
        for &(row, invocation) in &self.sites {
            let Some(shape) = self.shapes.get(&invocation.id()) else {
                continue;
            };
            let name = refs[row].target.as_str();
            let provable = |t: &str| provable_type(t, invocation, file.source);
            let head = match shape {
                Shape::Name(x) if self.untyped.contains(x) => None,
                Shape::Name(x) if types().declares(x) => types().get(x).filter(|t| provable(t)).map(str::to_string),
                Shape::Name(x) => (self.type_imports.contains(x) || declares_type(file.decls, x)).then(|| x.clone()),
                Shape::Field(x) => types().field(x).filter(|t| provable(t)).map(str::to_string),
                Shape::This => enclosing_class(invocation, file).map(|i| file.decls[i].name.clone()),
                Shape::Super => enclosing_class(invocation, file)
                    .and_then(|i| file.symbols[i].as_ref())
                    .and_then(|class| declared_superclass(refs, class))
                    .map(str::to_string),
                Shape::Implicit => enclosing_class(invocation, file).and_then(|i| {
                    let own = declares_member(file.decls, i, name);
                    // Anything else in scope that could supply `name` — an
                    // enclosing class's member, a static import — keeps the
                    // bare row and the lexical / import rungs that bind it.
                    let elsewhere = nested(file.decls, i) || imported.contains(name) || static_wildcard;
                    (own || !elsewhere).then(|| file.decls[i].name.clone())
                }),
            };
            if let Some(head) = head {
                typed.push((row, format!("{head}::{name}")));
            }
        }
        for (row, target) in typed {
            refs[row].target = target;
            refs[row].form = RefForm::Path;
        }
    }
}

/// Whether the declared type `t` names a type a call at `invocation` can be
/// typed by: one identifier (not an array, whose recorded name keeps its
/// `[]`; not a union), not Java's `var`, and not a type parameter of any
/// declaration enclosing the call — a type variable is no type ([NFR-RA-05]).
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn provable_type(t: &str, invocation: Node<'_>, source: &[u8]) -> bool {
    let mut chars = t.chars();
    let identifier = chars
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$');
    identifier && t != "var" && !type_parameters_in_scope(invocation, source).contains(t)
}

/// A class-like declaration: the only kind a receiver's type can be.
fn class_like(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Class | NodeKind::Interface | NodeKind::Enum | NodeKind::Struct | NodeKind::Trait
    )
}

/// The index of the class-like declaration enclosing `invocation` — [`None`]
/// inside an anonymous class body, whose `this` has no name the file can
/// write, and at file scope.
fn enclosing_class(invocation: Node<'_>, file: &FileDecls<'_, '_>) -> Option<usize> {
    let mut at = invocation.parent();
    while let Some(node) = at {
        if anonymous_class_body(node) {
            return None;
        }
        if let Some(&i) = file.id_to_idx.get(&node.id()) {
            if class_like(file.decls[i].kind) {
                return Some(i);
            }
        }
        at = node.parent();
    }
    None
}

/// Whether the class at `class` declares a callable named `name` itself.
fn declares_member(decls: &[Decl<'_>], class: usize, name: &str) -> bool {
    decls.iter().any(|d| {
        d.parent == Some(class) && d.name == name && matches!(d.kind, NodeKind::Method | NodeKind::Function)
    })
}

/// Whether the class at `class` sits inside another class-like declaration —
/// a nested, inner or local class, whose bare calls the outer class's members
/// can also answer.
fn nested(decls: &[Decl<'_>], class: usize) -> bool {
    let mut at = decls[class].parent;
    while let Some(i) = at {
        if class_like(decls[i].kind) {
            return true;
        }
        at = decls[i].parent;
    }
    false
}

/// Whether the file declares a class-like type named `name`.
fn declares_type(decls: &[Decl<'_>], name: &str) -> bool {
    decls.iter().any(|d| class_like(d.kind) && d.name == name)
}
