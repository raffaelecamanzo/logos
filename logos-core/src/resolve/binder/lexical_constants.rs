//! A row of a lexical-constant language (S-565, S-567, S-568, [FR-RS-33]),
//! bound through the constant index alone: a call on a constant, the class a
//! `.new` instantiates, a superclass or mixin, a route's `controller#action`
//! ([`LexicalConstants::resolve_constant_row`]).
//!
//! [FR-RS-33]: ../../../../docs/specs/requirements/FR-RS-33.md

use std::collections::HashSet;
use std::ops::Deref;

use crate::graph_store::UnresolvedRefRow;
use crate::model::{EdgeKind, NodeId, NodeKind};

use super::super::package_key::constant_fqn;
use super::{exactly_one, split, Ctx, Index, Res, Residue, Want, FULLY_QUALIFIED_HEAD};

/// A row of a language that names its classes and modules by lexical constants.
#[derive(Clone, Copy)]
pub(super) struct LexicalConstants<'c, 'a> {
    pub(super) cx: &'c Ctx<'a>,
}

impl<'a> Deref for LexicalConstants<'_, 'a> {
    type Target = Ctx<'a>;

    fn deref(&self) -> &Ctx<'a> {
        self.cx
    }
}

impl LexicalConstants<'_, '_> {
    /// A row of a lexical-constant language (S-565, [FR-RS-33]), bound through
    /// the constant index ([`PackageIndex::constants`](super::package_index::PackageIndex::constants)) alone:
    ///
    /// - a **call** `Foo::bar` (Ruby's `Foo.bar`) — `Foo` read as a constant
    ///   ([`constant`](LexicalConstants::constant)), then `bar` among the callables every
    ///   declaration of it holds that are called on the class or module object
    ///   itself: recorded as not taking `self` (`def self.bar`, a
    ///   `class << self` method, a `module_function`; [`DeclarationFacts::without_self`](super::declaration_facts::DeclarationFacts::without_self)).
    ///   Its instance methods are no candidate, and neither is an inherited
    ///   class method, which the graph does not walk to. Exactly one, after the
    ///   argument count where the language filters by one
    ///   ([`applicable`](Ctx::applicable); Ruby, which overloads nothing, does
    ///   not);
    /// - an **instantiation** `Foo` — the one class the constant declares;
    /// - a compact declaration's **scope** `A` (a `TypeUses` from the
    ///   declaration's header) — the one class or module it names, read from
    ///   the scope enclosing the declaration.
    ///
    /// A reopened class or module is one constant: its declarations pool
    /// their methods. An edge to the constant itself needs one node, so a
    /// constant declared more than once binds no instantiation or scope. A
    /// constant some file declares a class and another a module is two
    /// declarations and binds nothing ([NFR-RA-05]).
    ///
    /// [FR-RS-33]: ../../../../docs/specs/requirements/FR-RS-33.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    pub(super) fn resolve_constant_row(&self, r: &UnresolvedRefRow) -> Res {
        let ix: &Index = self.ix;
        let segs = split(&r.target);
        let header = r.kind == EdgeKind::TypeUses && ix.packages.constant_of.contains_key(&self.source);
        let start = if header {
            ix.parent.get(&self.source).copied().unwrap_or(self.source)
        } else {
            self.source
        };
        if r.kind != EdgeKind::Calls {
            return self.constant_node(&segs, start, |kind| match r.kind {
                EdgeKind::Instantiates => kind == NodeKind::Class,
                _ => matches!(kind, NodeKind::Class | NodeKind::Module),
            });
        }
        let want = Want::Callable;
        let Some((name, path)) = segs.split_last() else {
            return Res::NotFound;
        };
        let declarations = match self.constant(path, start) {
            Ok(declarations) => declarations,
            Err(res) => {
                self.note(want, || match res {
                    Res::Ambiguous => Residue::TypeAmbiguous,
                    _ => Residue::ExternalType {
                        candidates: self.constant_candidates(path, start),
                    },
                });
                return res;
            }
        };
        let mut candidates: Vec<NodeId> = declarations
            .iter()
            .flat_map(|&d| ix.members_named(d, name, want))
            .filter(|id| ix.facts.without_self.contains(id))
            .collect();
        candidates.sort();
        candidates.dedup();
        if candidates.is_empty() {
            self.note(want, || Residue::SupertypeUnreached);
            return Res::NotFound;
        }
        let candidates = self.applicable(candidates);
        let decided = exactly_one(&candidates);
        match decided {
            Res::NotFound => self.note(want, || Residue::NoApplicableOverload),
            Res::Ambiguous => self.note(want, || Residue::OverloadAmbiguous),
            Res::Found(_) => {}
        }
        decided
    }

    /// The one class or module the constant `segs` names from the scope
    /// `start`, of a kind `admits` (S-565): an edge to a constant needs one
    /// node, so a constant declared more than once is [`Res::Ambiguous`], as
    /// is one some file declares a class and another a module.
    fn constant_node(&self, segs: &[String], start: NodeId, admits: impl Fn(NodeKind) -> bool) -> Res {
        match self.constant(segs, start) {
            Ok([one]) if self.ix.info.get(one).is_some_and(|i| admits(i.kind)) => Res::Found(*one),
            Ok([_]) | Err(Res::NotFound) => Res::NotFound,
            Ok(_) | Err(_) => Res::Ambiguous,
        }
    }

    /// The one instance method `action` of the class `constant` names from the
    /// top level (S-567, [`bind_constant_action`](super::bind_constant_action)): the constant's declarations
    /// must all be classes — a reopened class pools its methods — and among
    /// their callables of that name, those recorded as not taking `self` (`def
    /// self.index`, [`DeclarationFacts::without_self`](super::declaration_facts::DeclarationFacts::without_self)) are no action. An action the class
    /// inherits is not walked to.
    pub(super) fn constant_action(&self, constant: &[String], action: &str) -> Option<NodeId> {
        let ix: &Index = self.ix;
        let fqn: Vec<String> = std::iter::once(FULLY_QUALIFIED_HEAD.to_string())
            .chain(constant.iter().cloned())
            .collect();
        let declarations = self.constant(&fqn, self.source).ok()?;
        let classes = declarations
            .iter()
            .all(|id| ix.info.get(id).is_some_and(|i| i.kind == NodeKind::Class));
        if !classes {
            return None;
        }
        let mut candidates: Vec<NodeId> = declarations
            .iter()
            .flat_map(|&d| ix.members_named(d, action, Want::Callable))
            .filter(|id| !ix.facts.without_self.contains(id))
            .collect();
        candidates.sort();
        candidates.dedup();
        match exactly_one(&candidates) {
            Res::Found(id) => Some(id),
            _ => None,
        }
    }

    /// A Ruby superclass or mixin row (S-568, [FR-RS-35]), bound through the
    /// constant index to the one node of the constant it names that `want`
    /// admits ([`ancestry_want`](super::ancestry_want)). A superclass is read where Ruby evaluates
    /// it, from the scope enclosing the class — `class A < B` inside `module M`
    /// reads `M::B`, then `B` — and a mixin from inside the class or module
    /// body, its own constant first. A class is never its own superclass.
    ///
    /// [FR-RS-35]: ../../../../docs/specs/requirements/FR-RS-35.md
    pub(super) fn constant_relation(&self, r: &UnresolvedRefRow, want: Want) -> Res {
        let start = match r.kind {
            EdgeKind::Extends => self.ix.parent.get(&self.source).copied().unwrap_or(self.source),
            _ => self.source,
        };
        match self.constant_node(&split(&r.target), start, |kind| want.admits(kind)) {
            Res::Found(target) if target == self.source => Res::NotFound,
            res => res,
        }
    }

    /// The declarations of the constant `segs` names from the scope `start`
    /// (S-565, [FR-RS-33]), read as Ruby reads it: the head is looked up in
    /// the constant of each class or module enclosing `start` (itself
    /// included), innermost first, then at the top level — the first level
    /// declaring it decides, and the rest of the path is read under it alone. A
    /// path headed [`FULLY_QUALIFIED_HEAD`] (`::Foo`) is read from the top level
    /// only. `Err(NotFound)` when no level declares the head, or the decided
    /// level does not declare the whole path; `Err(Ambiguous)` when the
    /// constant's declarations are a class and a module.
    ///
    /// [FR-RS-33]: ../../../../docs/specs/requirements/FR-RS-33.md
    fn constant(&self, segs: &[String], start: NodeId) -> Result<&[NodeId], Res> {
        let constants = self
            .source_family
            .as_deref()
            .and_then(|family| self.ix.packages.constants.get(family))
            .ok_or(Res::NotFound)?;
        let Some((head, _)) = segs.split_first() else {
            return Err(Res::NotFound);
        };
        let fqn = if head == FULLY_QUALIFIED_HEAD {
            constant_fqn(&[], segs)
        } else {
            let level = self
                .constant_levels(start)
                .into_iter()
                .find(|level| constants.contains_key(&constant_fqn(level, &segs[..1])))
                .ok_or(Res::NotFound)?;
            constant_fqn(level, segs)
        };
        let declarations = constants.get(&fqn).ok_or(Res::NotFound)?;
        let kinds: HashSet<NodeKind> = declarations
            .iter()
            .filter_map(|id| self.ix.info.get(id).map(|i| i.kind))
            .collect();
        if kinds.len() > 1 {
            return Err(Res::Ambiguous);
        }
        Ok(declarations)
    }

    /// The constants enclosing `start`, innermost first, then the top level
    /// (empty) — the lexical levels a constant is looked up at (S-565).
    fn constant_levels(&self, start: NodeId) -> Vec<&[String]> {
        let mut levels: Vec<&[String]> = Vec::new();
        let mut at = Some(start);
        while let Some(id) = at {
            if let Some(fqn) = self.ix.packages.constant_of.get(&id) {
                levels.push(fqn);
            }
            at = self.ix.parent.get(&id).copied();
        }
        levels.push(&[]);
        levels
    }

    /// The fully-qualified names the constant `segs` could have been from
    /// `start`, in lookup order — the residue of a call on a constant no level
    /// declares (S-565).
    fn constant_candidates(&self, segs: &[String], start: NodeId) -> Vec<Vec<String>> {
        let mut candidates: Vec<Vec<String>> = self
            .constant_levels(start)
            .into_iter()
            .map(|level| constant_fqn(level, segs))
            .collect();
        // A fully-qualified path names one constant from every level.
        candidates.dedup();
        candidates
    }
}
