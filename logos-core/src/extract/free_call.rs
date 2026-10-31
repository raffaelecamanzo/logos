//! A bare call no member of its class can answer is a free call (S-573,
//! [CR-183], [FR-RS-36]): Scala's `forkDiscard(update())` inside `object
//! RateLimiter`, which calls the top-level `ox.forkDiscard` — an object or
//! class reads an unqualified name among its own members first, and only then
//! through the imports and the package around it.
//!
//! Under `implicit_receiver = "self"` the receiver pass records every bare call
//! inside a class as a call on the instance ([`super::receiver`]), which binds
//! among the class's members and its proven supertypes only. Where the file
//! itself shows that no member answers the name, that row is turned into the
//! Path-form free call a bare call outside a class records, which the lexical
//! scope, the imports, the declared package and the wildcards bind. The
//! `references` query marks the three facts the turn reads, and records no row
//! for any of them:
//!
//! - `@ref.implicit.free` — the name of a call written with no receiver that
//!   may be a free call ([`FREE_CAPTURE`]); a name every instance has (Scala's
//!   `toString`, `synchronized`) is left unmarked by the query;
//! - `@ref.implicit.open` — a class-like body whose members the file cannot all
//!   see ([`OPEN_CAPTURE`]): one that inherits (an `extends` clause, a
//!   self-type, an anonymous `new T { … }`), or one the compiler adds members
//!   to (a case class, an enum);
//! - `@ref.implicit.value` — a name the file binds as a value ([`VALUE_CAPTURE`]):
//!   a parameter, a class parameter, a local, a pattern variable.
//!
//! A marked call stays on the instance when any body around it is open, when
//! any class-like declaration around it declares a member of the name, or when
//! the file binds the name as a value anywhere — read file-wide, as
//! `CallShadows` reads values, so it can lose a free call and never invent one
//! ([NFR-RA-05]). A row the receiver pass left on the instance (`self`) is
//! turned free under those three tests. A row it already made free — a bare
//! call outside every class, or inside `new T { … }`, whose instance has no
//! node — is turned into a call on another object (`other`, bound nowhere)
//! when it applies a value the file binds, or when the nearest open body
//! around it declares nothing of its name: `T`'s inherited member may answer
//! it, never the imports or the package.
//!
//! [CR-183]: ../../../docs/requests/CR-183-scala-resolves-imports-calls-and-relations.md
//! [FR-RS-36]: ../../../docs/specs/requirements/FR-RS-36.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md

use std::collections::{HashMap, HashSet};

use tree_sitter::Node;

use super::{Decl, RefFact};
use crate::model::{ReceiverShape, RefForm};
use crate::resolve::is_class_like;

/// The `references`-query capture of a bare call's name that may be free.
pub(super) const FREE_CAPTURE: &str = "ref.implicit.free";

/// The `references`-query capture of a body whose members are not all seen.
pub(super) const OPEN_CAPTURE: &str = "ref.implicit.open";

/// The `references`-query capture of a name the file binds as a value.
pub(super) const VALUE_CAPTURE: &str = "ref.implicit.value";

/// The free-call marks of one file.
#[derive(Default)]
pub(super) struct FreeCalls {
    /// The node ids of the call names marked free.
    candidates: HashSet<usize>,
    /// The node ids of the open bodies.
    open: HashSet<usize>,
    /// Every name the file binds as a value.
    values: HashSet<String>,
}

impl FreeCalls {
    /// Take `node` when `capture` is one of the three marks, recording no row;
    /// `false` for every other capture.
    pub(super) fn note(&mut self, capture: &str, node: Node<'_>, source: &[u8]) -> bool {
        match capture {
            FREE_CAPTURE => {
                self.candidates.insert(node.id());
            }
            OPEN_CAPTURE => {
                self.open.insert(node.id());
            }
            VALUE_CAPTURE => {
                if let Ok(name) = node.utf8_text(source) {
                    self.values.insert(name.trim().to_string());
                }
            }
            _ => return false,
        }
        true
    }

    /// Turn every marked call the receiver pass left on the instance, and no
    /// member can answer, into the Path-form free call; and turn a marked call
    /// the receiver pass already made free — outside every class, or in a body
    /// whose instance has no node (`new T { … }`) — into a call on another
    /// object (`other`, which binds nothing) when it applies a value the file
    /// binds, or sits in an open body that declares nothing of its name: an
    /// inherited member may answer it, which no import or package does.
    /// `callees` pairs each call row with its callee node; `id_to_idx` maps a
    /// declaration node's id to its index in `decls`.
    pub(super) fn finish(
        self,
        out: &mut [RefFact],
        callees: &[(usize, Node<'_>)],
        decls: &[Decl<'_>],
        id_to_idx: &HashMap<usize, usize>,
    ) {
        if self.candidates.is_empty() {
            return;
        }
        for &(row, callee) in callees {
            let fact = &mut out[row];
            if !self.candidates.contains(&callee.id()) {
                continue;
            }
            let on_instance = fact.form == RefForm::Method && fact.receiver == Some(ReceiverShape::SelfInstance);
            // A call a local binding shadows is already decided (S-594): it
            // carries the binding's mark, and stays the call through it.
            let free = fact.form == RefForm::Path
                && fact.receiver.is_none()
                && fact.alias.is_none()
                && !fact.target.contains("::");
            let value = self.values.contains(&fact.target);
            if on_instance && !value && !self.answered_around(callee, &fact.target, decls, id_to_idx) {
                fact.form = RefForm::Path;
                fact.receiver = None;
            } else if free && (value || self.open_without(callee, &fact.target, decls)) {
                fact.form = RefForm::Method;
                fact.receiver = Some(ReceiverShape::Other);
            }
        }
    }

    /// Whether the nearest open body around `callee` declares nothing named
    /// `name` — no declaration inside it, nested ones included, which the
    /// lexical scope would bind. `false` with no open body around.
    fn open_without(&self, callee: Node<'_>, name: &str, decls: &[Decl<'_>]) -> bool {
        let mut at = callee.parent();
        while let Some(node) = at {
            if self.open.contains(&node.id()) {
                let inside = |d: &Decl<'_>| d.node.start_byte() >= node.start_byte() && d.node.end_byte() <= node.end_byte();
                return !decls.iter().any(|d| d.name == name && inside(d));
            }
            at = node.parent();
        }
        false
    }

    /// Whether a member may answer `name` at `callee`: a body around it is
    /// open, or a class-like declaration around it declares a member `name`.
    fn answered_around(&self, callee: Node<'_>, name: &str, decls: &[Decl<'_>], id_to_idx: &HashMap<usize, usize>) -> bool {
        let mut at = callee.parent();
        while let Some(node) = at {
            if self.open.contains(&node.id()) {
                return true;
            }
            let declares = id_to_idx
                .get(&node.id())
                .filter(|&&idx| is_class_like(decls[idx].kind))
                .is_some_and(|&idx| decls.iter().any(|d| d.parent == Some(idx) && d.name == name));
            if declares {
                return true;
            }
            at = node.parent();
        }
        false
    }
}
