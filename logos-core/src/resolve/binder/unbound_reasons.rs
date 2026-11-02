//! Why a call stays unbound, and the factory function that rivals a class
//! (S-607, S-521).
//!
//! [`UnboundReasons::unbound_reason`] reads the reason an impl-block-language
//! call records when no rung its bind took recorded one ([`super::residue`]),
//! off the path's own resolution, by scope and module model only.
//! [`UnboundReasons::rival_function`] keeps a class a declared call
//! constructs from binding when a top-level function of its own name sits in
//! its package ([NFR-RA-05]).
//!
//! [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md

use std::ops::Deref;

use crate::graph_store::UnresolvedRefRow;
use crate::model::{NodeId, NodeKind, RefForm};

use super::{is_type_like, split, Ctx, Index, Res, Residue, Want, MAX_ALIAS_DEPTH, SELF_TYPE_HEAD};
use super::scope_hierarchy::ScopeHierarchy;

/// Why a call stays unbound, and the factory function that rivals a class.
#[derive(Clone, Copy)]
pub(super) struct UnboundReasons<'c, 'a> {
    pub(super) cx: &'c Ctx<'a>,
}

impl<'a> Deref for UnboundReasons<'_, 'a> {
    type Target = Ctx<'a>;

    fn deref(&self) -> &Ctx<'a> {
        self.cx
    }
}

impl UnboundReasons<'_, '_> {
    /// `true` when a top-level `Function` of `class`'s own name sits in the
    /// class's package, within its family (S-521): Kotlin's factory function
    /// `fun Foo(s: String): Foo` beside `class Foo`. The package rungs read
    /// types only, so the function is no candidate there and the class alone
    /// would answer — but the call names both, and stays unbound ([NFR-RA-05]).
    /// A class with no package (a path-model file) is never rivalled here: its
    /// language's rungs already see functions and classes together.
    ///
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    pub(super) fn rival_function(&self, class: NodeId) -> bool {
        let Some(info) = self.ix.info.get(&class) else {
            return false;
        };
        let Some(path) = info.file_path.as_deref() else {
            return false;
        };
        let Some(package) = self.ix.layout.package_of(path) else {
            return false;
        };
        let family = self.ix.layout.family(path);
        let top_level = |id: &NodeId| {
            self.ix
                .parent
                .get(id)
                .and_then(|p| self.ix.info.get(p))
                .is_some_and(|p| p.kind == NodeKind::Module)
        };
        self.ix.by_name.get(&info.name).into_iter().flatten().any(|id| {
            self.ix.info.get(id).is_some_and(|f| {
                f.kind == NodeKind::Function
                    && top_level(id)
                    && f.file_path.as_deref().is_some_and(|fp| {
                        self.ix.layout.package_of(fp).as_ref() == Some(&package)
                            && self.ix.layout.family(fp) == family
                    })
            })
        })
    }

    /// Why the impl-block-language call `r` stays unbound, when no rung its
    /// bind took recorded a reason (S-607, [FR-RS-47], [`residue`](super::residue)) — read
    /// off the path's own resolution, by scope and module model only:
    ///
    /// - a bare name — [`Residue::OverloadAmbiguous`] when it reaches two
    ///   callables; [`Residue::NotACallable`] when it reaches a type (`W(1)`,
    ///   `Self(1)`) or an enum variant a `use` or glob brings in; else
    ///   [`Residue::NameNotInScope`];
    /// - a path — its prefix resolved: a repository module that declares no
    ///   callable of the name is [`Residue::NameNotInScope`] (or
    ///   [`Residue::NotACallable`] for a type of the name), a repository type
    ///   [`Residue::NotACallable`] for one of its variants, else
    ///   [`Residue::SupertypeUnreached`]; two of them
    ///   [`Residue::TypeAmbiguous`]; none — `std::…`, an external crate — is
    ///   [`Residue::ExternalType`];
    /// - a receiver call — [`Residue::NoReceiverEvidence`].
    ///
    /// [FR-RS-47]: ../../../../docs/specs/requirements/FR-RS-47.md
    pub(super) fn unbound_reason(&self, r: &UnresolvedRefRow) -> Residue {
        let scope_hierarchy = ScopeHierarchy { cx: self.cx };
        let ix: &Index = self.ix;
        let external = || Residue::ExternalType {
            candidates: Vec::new(),
        };
        if r.form != RefForm::Path {
            return Residue::NoReceiverEvidence;
        }
        self.scope_only.set(true);
        let segs = split(&r.target);
        let Some((name, prefix)) = segs.split_last() else {
            return Residue::NameNotInScope;
        };
        if prefix.is_empty() {
            return self.bare_reason(name);
        }
        match scope_hierarchy.resolve_path(prefix, Want::Any, MAX_ALIAS_DEPTH) {
            Res::Found(id) => match ix.info.get(&id).map(|i| i.kind) {
                Some(NodeKind::Module) => {
                    let typed = ix.members_named(id, name, Want::Any).into_iter().any(|m| {
                        ix.info.get(&m).is_some_and(|i| is_type_like(i.kind))
                    });
                    if typed {
                        Residue::NotACallable
                    } else {
                        Residue::NameNotInScope
                    }
                }
                _ if self.is_variant(id, name) => Residue::NotACallable,
                _ => Residue::SupertypeUnreached,
            },
            Res::Ambiguous => Residue::TypeAmbiguous,
            Res::NotFound => external(),
        }
    }

    /// [`unbound_reason`](UnboundReasons::unbound_reason) for a bare name.
    fn bare_reason(&self, name: &str) -> Residue {
        let scope_hierarchy = ScopeHierarchy { cx: self.cx };
        if name == SELF_TYPE_HEAD {
            return Residue::NotACallable;
        }
        let single = [name.to_string()];
        if scope_hierarchy.resolve_path(&single, Want::Callable, MAX_ALIAS_DEPTH) == Res::Ambiguous {
            return Residue::OverloadAmbiguous;
        }
        if matches!(scope_hierarchy.resolve_path(&single, Want::Type, MAX_ALIAS_DEPTH), Res::Found(_)) {
            return Residue::NotACallable;
        }
        // A variant a `use E::A` or a `use E::*` brings into scope.
        let scope = self.scope();
        let imported = scope
            .and_then(|s| s.aliases.get(name))
            .and_then(|path| path.split_last())
            .is_some_and(|(_, enum_path)| !enum_path.is_empty() && self.names_variant(enum_path, name));
        let globbed = scope.is_some_and(|s| s.globs.iter().any(|g| self.names_variant(g, name)));
        if imported || globbed {
            Residue::NotACallable
        } else {
            Residue::NameNotInScope
        }
    }

    /// Whether `path` names a repository enum with a variant `name`.
    fn names_variant(&self, path: &[String], name: &str) -> bool {
        let scope_hierarchy = ScopeHierarchy { cx: self.cx };
        matches!(scope_hierarchy.resolve_path(path, Want::Type, MAX_ALIAS_DEPTH), Res::Found(ty) if self.is_variant(ty, name))
    }
}
