//! Fields declared at their name (S-837, [CR-264] decision 35, [FR-EX-20],
//! [FR-EX-26]): an attribute an assignment declares — Python's class-body
//! `x = …` and a method's `self.x = …`, JavaScript's `this.x = …` — and a
//! class's or module's state — Ruby's `attr_accessor :x` and its `@x = …`.
//!
//! Such a declaration is taken at its name node, never lifted to the
//! assignment or call it sits in, so what that holds — an initialiser's calls,
//! the method's own access — stays its scope's. It is owned by a class, and
//! each name is declared once per owner. Which owner and which of several
//! writes are read off the plugin's own `symbols` query ([NFR-MA-01]): no
//! language is named here.
//!
//! # The `symbols`-query vocabulary
//!
//! - `@symbol.at_name` ([`CAPTURE`]) — a marker in the same match as a
//!   `@symbol.<kind>` capture: the declaration is a field declared at its name.
//!   Not a [`NodeKind`], so `kind_for_capture` never mistakes it for one.
//! - `(#set! symbol.field_owner "…")` ([`OWNER_SETTING`]) — who owns it:
//!   - `"scope"` (the default): the class it is written in, or the class of the
//!     method it is written in, unless a function between the two rebinds
//!     `this` ([`THIS_REBINDING_KINDS`]). One written anywhere else — a nested
//!     function, a free function — has no owner.
//!   - `"enclosing"`: the nearest enclosing class or module, whatever method,
//!     block or call lies between. One outside every class and module has no
//!     owner.
//! - `(#set! symbol.field_order "…")` ([`ORDER_SETTING`]) — which of one
//!   owner's declarations of one name is the Field:
//!   - `"body-first"` (the default): one written directly in the owner's body
//!     ahead of one written in a method or block, then the first in source
//!     order.
//!   - `"source"`: the first in source order.
//! - `(#set! symbol.field_sigil "…")` ([`SIGIL_SETTING`]) — the name is the
//!   captured text, less a symbol literal's leading `:` and then any leading
//!   sigil, written after the sigil: `@x` for `@x`, `:x` and `x`. Without it
//!   the name is the captured text.
//!
//! A value the vocabulary does not know is read as the default; the embedded
//! queries are pinned to known values by this module's tests.
//!
//! In every case a declaration with no owner is dropped, and so is every one
//! of a name its owner already declares as a Field in its own right — a Field
//! no marker took, whose parent is that class-like owner (JavaScript's `x = 0;`
//! beside a constructor's `this.x = …`, S-542).
//!
//! [CR-264]: ../../../docs/requests/CR-264-sprint-109-review-follow-ups.md
//! [FR-EX-20]: ../../../docs/specs/requirements/FR-EX-20.md
//! [FR-EX-26]: ../../../docs/specs/requirements/FR-EX-26.md
//! [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md

use std::collections::{HashMap, HashSet};

use tree_sitter::Query;

use crate::model::NodeKind;

use super::{decl_index, sits_within_any, Decl};

/// The marker capture: the declaration is a field declared at its name.
pub(super) const CAPTURE: &str = "symbol.at_name";

/// The pattern setting naming the owner rule ([`Owner`]).
const OWNER_SETTING: &str = "symbol.field_owner";

/// The pattern setting naming the precedence rule ([`Order`]).
const ORDER_SETTING: &str = "symbol.field_order";

/// The pattern setting naming the sigil the name is written with ([`name`]).
const SIGIL_SETTING: &str = "symbol.field_sigil";

/// The node kinds of a function that binds its own `this` (S-542): a
/// `this.x = …` written inside one (`setTimeout(function () { this.t = 1; })`)
/// is not the instance's. An arrow function keeps the lexical `this`.
const THIS_REBINDING_KINDS: &[&str] = &["function_expression", "generator_function", "generator_function_declaration"];

/// Who owns a field declared at its name ([`OWNER_SETTING`]).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) enum Owner {
    /// The class it is written in, or its method's class (`"scope"`).
    #[default]
    Scope,
    /// The nearest enclosing class or module, through anything (`"enclosing"`).
    Enclosing,
}

/// Which of one owner's declarations of one name is the Field ([`ORDER_SETTING`]).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) enum Order {
    /// Written in the owner's body ahead of a method's, then source order (`"body-first"`).
    #[default]
    BodyFirst,
    /// The first in source order (`"source"`).
    Source,
}

/// The rules one pattern's settings give the fields it declares.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct Rules {
    pub(super) owner: Owner,
    pub(super) order: Order,
}

impl Rules {
    /// The rules `pattern` of `query` sets; the default for each it does not.
    pub(super) fn of_pattern(query: &Query, pattern: usize) -> Self {
        let setting = |key| crate::plugin::queries::pattern_setting(query, pattern, key);
        let owner = match setting(OWNER_SETTING) {
            Some("enclosing") => Owner::Enclosing,
            _ => Owner::Scope,
        };
        let order = match setting(ORDER_SETTING) {
            Some("source") => Order::Source,
            _ => Order::BodyFirst,
        };
        Self { owner, order }
    }
}

/// The name a field declared at its name takes from its `captured` text: as
/// captured, or written with the pattern's sigil ([`SIGIL_SETTING`]).
pub(super) fn name(query: &Query, pattern: usize, captured: &str) -> String {
    match crate::plugin::queries::pattern_setting(query, pattern, SIGIL_SETTING) {
        Some(sigil) if !sigil.is_empty() => {
            format!("{sigil}{}", captured.trim_start_matches(':').trim_start_matches(sigil))
        }
        _ => captured.to_string(),
    }
}

/// Settle the fields declared at their name: give each its owner, keep one per
/// name per owner by the declaration's [`Order`], and drop the rest — one with
/// no owner, and every one of a name its class-like owner already declares as a
/// Field no marker took.
/// Runs on the parents `assign_parents` resolved.
///
/// A dropped declaration is a name node, so no declaration sits inside it: the
/// indices are compacted and every parent is remapped without orphaning one.
pub(super) fn settle(decls: &mut Vec<Decl<'_>>) {
    if !decls.iter().any(|d| d.at_name.is_some()) {
        return;
    }
    let index = decl_index(decls);
    // (written outside its owner's body under body-first, source position,
    // index, owner): sorted, the first of each owner and name is its Field.
    let mut fields: Vec<(bool, usize, usize, usize)> = decls
        .iter()
        .enumerate()
        .filter_map(|(i, d)| {
            let rules = d.at_name?;
            let owner = owner_of(i, rules.owner, decls, &index)?;
            let late = rules.order == Order::BodyFirst && d.parent != Some(owner);
            Some((late, d.start_byte, i, owner))
        })
        .collect();
    fields.sort_unstable();
    let mut keep: Vec<bool> = decls.iter().map(|d| d.at_name.is_none()).collect();
    let mut declared: HashSet<(usize, String)> = decls
        .iter()
        .filter(|d| d.kind == NodeKind::Field && d.at_name.is_none())
        .filter_map(|d| Some((d.parent.filter(|&c| crate::resolve::is_class_like(decls[c].kind))?, d.name.clone())))
        .collect();
    for (_, _, i, owner) in fields {
        if declared.insert((owner, decls[i].name.clone())) {
            keep[i] = true;
            decls[i].parent = Some(owner);
        }
    }
    compact(decls, &keep);
}

/// The owner of declaration `i` under `rule`, `None` when it has none.
fn owner_of(i: usize, rule: Owner, decls: &[Decl<'_>], index: &HashMap<usize, usize>) -> Option<usize> {
    match rule {
        Owner::Scope => scope_owner(&decls[i], decls),
        Owner::Enclosing => enclosing_owner(&decls[i], decls, index),
    }
}

/// [`Owner::Scope`]: the class `decl`'s scope is, or the class of the callable
/// it is, unless a function between them rebinds `this`.
fn scope_owner(decl: &Decl<'_>, decls: &[Decl<'_>]) -> Option<usize> {
    let class_like = |j: usize| crate::resolve::is_class_like(decls[j].kind);
    let scope = decl.parent?;
    if class_like(scope) {
        return Some(scope);
    }
    let callable = matches!(decls[scope].kind, NodeKind::Function | NodeKind::Method);
    let class = decls[scope].parent.filter(|&c| callable && class_like(c))?;
    (!sits_within_any(decl.node, decls[scope].node, THIS_REBINDING_KINDS)).then_some(class)
}

/// [`Owner::Enclosing`]: the nearest declaration enclosing `decl`'s node that
/// is a class or module, whatever lies between.
fn enclosing_owner(decl: &Decl<'_>, decls: &[Decl<'_>], index: &HashMap<usize, usize>) -> Option<usize> {
    std::iter::successors(decl.node.parent(), |node| node.parent())
        .filter_map(|node| index.get(&node.id()).copied())
        .find(|&j| matches!(decls[j].kind, NodeKind::Class | NodeKind::Module))
}

/// Keep the declarations `keep` marks, remapping every parent to the
/// compacted indices.
fn compact(decls: &mut Vec<Decl<'_>>, keep: &[bool]) {
    let mut remap: Vec<Option<usize>> = Vec::with_capacity(decls.len());
    let mut next = 0;
    for &kept in keep {
        remap.push(kept.then_some(next));
        next += usize::from(kept);
    }
    let mut kept = keep.iter();
    decls.retain(|_| *kept.next().unwrap_or(&true));
    for decl in decls.iter_mut() {
        decl.parent = decl.parent.and_then(|p| remap[p]);
    }
}

#[cfg(test)]
#[path = "at_name_tests.rs"]
mod tests;
