//! A type relation (S-466, S-522, [FR-RS-15]) bound to the one in-repository
//! type of a kind it admits, through the source's scope and its module model —
//! never a workspace guess ([`TypeRelations::bind_type_relation`]).
//!
//! [FR-RS-15]: ../../../../docs/specs/requirements/FR-RS-15.md

use std::ops::Deref;

use crate::graph_store::UnresolvedRefRow;
use crate::model::{EdgeKind, RefForm};

use super::{
    is_type_like, relation_edge_kind, split, Ctx, Outcome, Res, Want, FULLY_QUALIFIED_HEAD,
    MAX_ALIAS_DEPTH, PARTIAL_TYPE_ALIAS,
};
use super::imported_scope::ImportedScope;
use super::includes::IncludeLookup;
use super::lexical_constants::LexicalConstants;
use super::package_paths::PackagePaths;
use super::scope_hierarchy::ScopeHierarchy;

/// A type relation bound to the one type of a kind it admits.
#[derive(Clone, Copy)]
pub(super) struct TypeRelations<'c, 'a> {
    pub(super) cx: &'c Ctx<'a>,
}

impl<'a> Deref for TypeRelations<'_, 'a> {
    type Target = Ctx<'a>;

    fn deref(&self) -> &Ctx<'a> {
        self.cx
    }
}

impl TypeRelations<'_, '_> {
    /// Resolve a type relation (S-466, [CR-149] §3.2 B, [FR-EX-10]; S-522,
    /// [FR-RS-15]) to the one type of a kind `want` admits.
    ///
    /// A `Path` row takes the source's own scope order — the lexical chain (a
    /// nested or same-file type), then its module model's rungs. A
    /// package-shaped source takes S-465's package rungs: single-type imports
    /// (final for the name they import), the source's package or namespace, its
    /// wildcards, and for a qualified name the fully-qualified index
    /// ([`resolve_package_path`](super::package_paths::PackagePaths::resolve_package_path)). Any other source
    /// — a Python class, a namespace file whose namespace is unknown — takes the
    /// module tree: its imports, its modules, its globs. Neither ever takes a
    /// policy-gated workspace guess ([`scope_only`](Ctx::scope_only)). A target
    /// headed [`FULLY_QUALIFIED_HEAD`] is read by the fully-qualified index
    /// alone, and a header never names its own declaration
    /// ([`is_own_header`](Ctx::is_own_header)). `want` filters the candidates
    /// of every rung, so a same-named type of the wrong kind is
    /// no candidate at all (on the fully-qualified rung it is applied after
    /// the exactly-one test, so a `src/main`/`src/test` pair stays ambiguous).
    /// A type with no source here — the JDK, a library, a generated class —
    /// resolves to nothing and stays in `unresolved_refs` ([NFR-RA-05]).
    ///
    /// **Header scope.** An `Extends` or `Implements` names its type in the
    /// declaration's header, where the declaration's own member types are not
    /// in scope (JLS §6.3): it is read from the enclosing scope
    /// ([`lexical_start`](Ctx::lexical_start)). A `TypeUses` sourced at a type
    /// may come from either place — a superclass's type argument (header) or a
    /// constructor parameter (body; constructors are not nodes) — and one row
    /// cannot say which, so it binds only where both readings agree, or where
    /// the header reading finds nothing (a header name that resolves only to a
    /// member type does not compile). Two different types is a refusal, never
    /// a pick ([NFR-RA-05]).
    ///
    /// A capture-before-delete `Symbol` row ([ADR-10]) is still a pure lookup,
    /// but of a target whose kind the relation also admits: an edit that turns
    /// the superclass into an interface leaves the relation unbound on sync,
    /// exactly as a cold index would ([NFR-RA-06]).
    ///
    /// [CR-149]: ../../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    /// [FR-EX-10]: ../../../../docs/specs/requirements/FR-EX-10.md
    /// [FR-RS-15]: ../../../../docs/specs/requirements/FR-RS-15.md
    /// [ADR-10]: ../../../../docs/specs/architecture/decisions/ADR-10.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    /// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
    /// Bind the type-relation row `r`, looked up with `want` ([`RowArms::bind`](super::row_arms::RowArms::bind)'s
    /// type-relation arm): to the one type [`resolve_type_relation`](TypeRelations::resolve_type_relation)
    /// reaches, its edge kind following the target ([`relation_edge_kind`]).
    ///
    /// A partial type part's own row (S-557, [FR-RS-29], [`PARTIAL_TYPE_ALIAS`])
    /// binds by its type instead: a later part extends its type's first-declared
    /// part, the canonical node, and the edge says it is that link. The first
    /// part, and a declaration marked partial that unites with nothing, bind
    /// nothing — never a supertype read off the name the row spells.
    ///
    /// [FR-RS-29]: ../../../../docs/specs/requirements/FR-RS-29.md
    pub(super) fn bind_type_relation(&self, r: &UnresolvedRefRow, want: Want) -> Outcome {
        let (source, ix) = (self.source, self.ix);
        if r.kind == EdgeKind::Extends && r.alias.as_deref() == Some(PARTIAL_TYPE_ALIAS) {
            return match ix.partials.first_of(source) {
                first if first == source => Outcome::Unbound,
                first => Outcome::Bound {
                    source,
                    target: first,
                    kind: EdgeKind::Extends,
                    payload: Some(PARTIAL_TYPE_ALIAS.to_string()),
                },
            };
        }
        match self.resolve_type_relation(r, want) {
            // A later part's `Extends` to its own first part is the partial link
            // whichever row binds it — a capture-before-delete row too (ADR-10),
            // which carries no alias and no payload — so the edge says so either
            // way, and a sync's insert order cannot cost it its payload.
            Res::Found(target) if target != source && ix.partials.first_of(source) == target => Outcome::Bound {
                source,
                target,
                kind: EdgeKind::Extends,
                payload: Some(PARTIAL_TYPE_ALIAS.to_string()),
            },
            Res::Found(target) => Outcome::Bound {
                source,
                target,
                kind: relation_edge_kind(r.kind, want, ix.info.get(&target).map(|i| i.kind)),
                payload: r.payload.clone(),
            },
            _ => Outcome::Unbound,
        }
    }

    fn resolve_type_relation(&self, r: &UnresolvedRefRow, want: Want) -> Res {
        let scope_hierarchy = ScopeHierarchy { cx: self.cx };
        let imported_scope = ImportedScope { cx: self.cx };
        let package_paths = PackagePaths { cx: self.cx };
        let constants = LexicalConstants { cx: self.cx };
        let includes = IncludeLookup { cx: self.cx };
        if r.form == RefForm::Symbol {
            return match self.ix.by_symbol.get(&r.target) {
                Some(&t) if self.ix.info.get(&t).is_some_and(|i| want.admits(i.kind)) => {
                    Res::Found(t)
                }
                _ => Res::NotFound,
            };
        }
        if self.lexical_constants {
            return constants.constant_relation(r, want);
        }
        if let Some(imported) = imported_scope.resolve_imported_type(&r.target, want) {
            return imported;
        }
        let segs = split(&r.target);
        // An include-roots language's base reads its includes (S-579).
        if let Some(res) = includes.resolve_included_type(&segs, want) {
            return res;
        }
        if segs.first().map(String::as_str) == Some(FULLY_QUALIFIED_HEAD) {
            return package_paths.resolve_fqn(&segs[1..], want);
        }
        let header = || -> Res {
            let Some(&enclosing) = self.ix.parent.get(&self.source) else {
                return Res::NotFound;
            };
            self.lexical_start.set(enclosing);
            let resolved = scope_hierarchy.resolve_path(&segs, want, MAX_ALIAS_DEPTH);
            self.lexical_start.set(self.source);
            resolved
        };
        match r.kind {
            EdgeKind::Extends | EdgeKind::Implements => header(),
            EdgeKind::TypeUses
                if self.ix.info.get(&self.source).is_some_and(|i| is_type_like(i.kind)) =>
            {
                match (scope_hierarchy.resolve_path(&segs, want, MAX_ALIAS_DEPTH), header()) {
                    (Res::Found(body), Res::Found(head)) if body != head => Res::Ambiguous,
                    (body, _) => body,
                }
            }
            _ => scope_hierarchy.resolve_path(&segs, want, MAX_ALIAS_DEPTH),
        }
    }
}
