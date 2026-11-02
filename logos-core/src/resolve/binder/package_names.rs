//! A bare name from a package-shaped or declared-namespace source, after the
//! lexical chain (CR-149, S-518, S-551, S-595, [FR-RS-13]): its imports, its
//! package's types, each enclosing namespace's, its wildcards and the global
//! function fallback ([`PackageNames::resolve_package_name`]).
//!
//! [FR-RS-13]: ../../../../docs/specs/requirements/FR-RS-13.md

use std::collections::HashMap;
use std::ops::Deref;

use crate::config::BindingPolicy;
use crate::model::NodeId;

use super::{exactly_one, Ctx, Res, Residue, Want};
use super::companions::Companions;
use super::scope_hierarchy::ScopeHierarchy;
use super::wildcards::Wildcards;

/// A bare name from a package-shaped or declared-namespace source.
#[derive(Clone, Copy)]
pub(super) struct PackageNames<'c, 'a> {
    pub(super) cx: &'c Ctx<'a>,
}

impl<'a> Deref for PackageNames<'_, 'a> {
    type Target = Ctx<'a>;

    fn deref(&self) -> &Ctx<'a> {
        self.cx
    }
}

impl PackageNames<'_, '_> {
    /// A bare name from a package-shaped source, after the lexical chain
    /// ([CR-149], [FR-RS-03]) — Java's scope order for a simple name:
    ///
    /// 1. **imported** — every single-type or single-static import naming it
    ///    (the file's alias expansions), each resolved through
    ///    [`resolve_path`](super::scope_hierarchy::ScopeHierarchy::resolve_path), exactly-one across all of them.
    ///    **Final** when the file imports the name at all: a single-type or
    ///    single-static import shadows the package and the wildcards (JLS
    ///    §6.4.1), so an import of the JDK's `Map` or a library's `Message`
    ///    that binds nothing here leaves the name unresolved rather than
    ///    reaching a same-package or wildcard type of that name (S-466 review);
    /// 2. **same package** — a top-level type of the source's own package,
    ///    visible without an import, or a top-level function of it in a
    ///    language that declares them namespace members (S-573; PHP's global
    ///    functions, S-551, are the empty package's);
    /// 3. **enclosing namespaces** — only for a language that declares them
    ///    (S-595, [FR-RS-45]): a top-level type of each namespace enclosing the
    ///    source's own, nearest first ([`enclosing_levels`](PackageNames::enclosing_levels)),
    ///    exactly one per level — two at one level are
    ///    [`Residue::TypeAmbiguous`] and stop the walk, none passes outward.
    ///    Each level's types — the source's own namespace's in rung 2 too — are
    ///    followed by the wildcards written inside that namespace's block
    ///    (S-603, [CR-199]: C#'s `namespace A { using X; … }`,
    ///    [`FileScope::block_globs`](super::FileScope::block_globs)), exactly one again, two
    ///    [`Residue::TypeAmbiguous`] and stopping the walk, before the next
    ///    level out;
    /// 4. **on-demand** — a member of a type, or a type of a package, the file
    ///    imports with a wildcard written outside every namespace block
    ///    ([`glob_members`](super::wildcards::Wildcards::glob_members)) — read
    ///    before rungs 2 and 3 in a language whose wildcards outrank the
    ///    package's other files ([`wildcards_first`](Ctx::wildcards_first),
    ///    S-573: Scala; the file's own declarations came first, lexically);
    /// 5. **the global function** — only for a call, in a language whose
    ///    unqualified function names fall back to the global namespace (S-551,
    ///    [FR-RS-26]: PHP's), from a namespaced file
    ///    ([`global_function`](PackageNames::global_function)). A file of the global
    ///    namespace reads the global functions as its rung 2: its package is
    ///    the empty one;
    /// 6. the policy-gated workspace name fallback, as for every other
    ///    language — except for a type relation, which binds by scope and
    ///    package key only ([`scope_only`](Ctx::scope_only)).
    ///
    /// A **receiver**-method call ([`RefForm::Method`](crate::model::RefForm::Method)) takes none of these
    /// rungs: its target is its receiver's type's member ([CR-150]), which no
    /// import names; it binds by its receiver's shape and never reaches this
    /// walk (S-514, [CR-066]).
    ///
    /// Each rung is exactly-one; a known ambiguity stops the walk ([NFR-RA-05]).
    ///
    /// For a language that tells types apart by their arity (S-602, [CR-199]:
    /// C#'s `Result` and `Result<T>`), rungs 2, 3 and 4 — and the lexical
    /// chain before them ([`resolve_name`](super::scope_hierarchy::ScopeHierarchy::resolve_name)), which holds the
    /// source's own file — admit only a type of the arity the row writes
    /// ([`admits_arity`](PackageNames::admits_arity)) before their exactly-one test: a level holding only the other arity passes the
    /// name on, and ambiguity is judged among the types of the written arity
    /// ([FR-RS-46]). A row of unknown arity — a call through a type name —
    /// admits any, as before.
    ///
    /// [CR-149]: ../../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    /// [CR-199]: ../../../../docs/requests/CR-199-csharp-resolution-honours-type-arity-and-using-scope.md
    /// [FR-RS-46]: ../../../../docs/specs/requirements/FR-RS-46.md
    /// [CR-066]: ../../../../docs/requests/CR-066-receiver-method-overbinding.md
    /// [CR-150]: ../../../../docs/requests/CR-150-java-receiver-typing-for-method-calls.md
    /// [FR-RS-03]: ../../../../docs/specs/requirements/FR-RS-03.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    /// [FR-RS-26]: ../../../../docs/specs/requirements/FR-RS-26.md
    pub(super) fn resolve_package_name(&self, package: &[String], name: &str, want: Want, depth: u8) -> Res {
        let scope_hierarchy = ScopeHierarchy { cx: self.cx };
        let wildcards = Wildcards { cx: self.cx };
        let companions = Companions { cx: self.cx };
        if let Some(expansions) = self.scope().and_then(|s| s.alias_expansions.get(name)) {
            let mut imported: Vec<NodeId> = Vec::new();
            for alias_path in expansions {
                match scope_hierarchy.resolve_path(alias_path, want, depth - 1) {
                    Res::Found(id) => imported.push(id),
                    Res::Ambiguous => return Res::Ambiguous,
                    Res::NotFound => {}
                }
            }
            imported.sort();
            imported.dedup();
            let decided = exactly_one(&imported);
            if decided == Res::Ambiguous {
                // Two imports each supply a callable of this name.
                self.note(want, || Residue::OverloadAmbiguous);
            }
            return decided;
        }
        // Scala reads its wildcards before the package's other files (S-573).
        if self.wildcards_first {
            if let Some(decided) = wildcards.wildcard_name(name, want, None) {
                return decided;
            }
        }
        let mut same_package = companions.admitted_types(self.package_type(package, name), want);
        // Only a type of the written arity (S-602).
        same_package.retain(|&id| self.admits_arity(id));
        // Only a function the call's arguments admit (S-592), as the lexical
        // rungs require (S-560).
        same_package.extend(self.applicable(self.namespace_callables(package, name, want)));
        match exactly_one(&same_package) {
            Res::NotFound => {}
            decided => return decided,
        }
        // The `using`s of the source's own namespace block (S-603), read at its
        // level as the path rung reads them.
        if self.enclosing_namespaces {
            if let Some(decided) = wildcards.wildcard_name(name, want, Some(package)) {
                return decided;
            }
        }
        for level in self.enclosing_levels(package) {
            let found: Vec<NodeId> = self
                .package_type(level, name)
                .iter()
                .copied()
                .filter(|id| self.ix.info.get(id).is_some_and(|i| want.admits(i.kind)))
                .filter(|&id| self.admits_arity(id))
                .collect();
            match exactly_one(&found) {
                Res::NotFound => {}
                Res::Ambiguous => {
                    // Two types of one name at the nearest level that has any:
                    // the walk stops, never reaching a level further out.
                    self.note(want, || Residue::TypeAmbiguous);
                    return Res::Ambiguous;
                }
                decided => return decided,
            }
            // Then that level's own block's `using`s, before the next level out.
            if let Some(decided) = wildcards.wildcard_name(name, want, Some(level)) {
                return decided;
            }
        }
        if !self.wildcards_first {
            if let Some(decided) = wildcards.wildcard_name(name, want, None) {
                return decided;
            }
        }
        if let Some(decided) = self.global_function(package, name, want) {
            return decided;
        }
        if self.policy == BindingPolicy::Aggressive && !self.scope_only.get() {
            return scope_hierarchy.unique_by_name(name, want);
        }
        Res::NotFound
    }

    /// [`resolve_package_name`](PackageNames::resolve_package_name)'s **global function**
    /// rung (S-551, [FR-RS-26]): a call from a namespaced file of a language
    /// whose unqualified function names fall back to the global namespace
    /// (PHP's, [`global_function_fallback`](Ctx::global_function_fallback)) —
    /// the one top-level function of the name the files declaring no namespace
    /// declare ([`namespace_callables`](PackageNames::namespace_callables) under the
    /// empty package, a file of unknown namespace a rival), once its own
    /// namespace declared none. Exactly one, as the same-package rung reads it
    /// (through [`applicable`](Ctx::applicable), which narrows nothing for a
    /// bare PHP call: it carries no filtered argument count): two are a known
    /// ambiguity and stop the walk ([NFR-RA-05]).
    ///
    /// `None` — the next rung decides — for any lookup but a call (a class name
    /// never falls back), for a file declaring no namespace (its same-package
    /// rung read the global namespace already), and when no global function
    /// has the name. Policy-free, as every scope rung is.
    ///
    /// [FR-RS-26]: ../../../../docs/specs/requirements/FR-RS-26.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    fn global_function(&self, package: &[String], name: &str, want: Want) -> Option<Res> {
        if !self.global_function_fallback || package.is_empty() || !want.is_call() {
            return None;
        }
        match exactly_one(&self.applicable(self.namespace_callables(&[], name, want))) {
            Res::NotFound => None,
            decided => Some(decided),
        }
    }

    /// The top-level functions named `name` that `package` holds and `want`
    /// admits ([`package_callables`](PackageNames::package_callables)) — with, for the
    /// global namespace of a language whose unqualified calls fall back to it
    /// (S-551, [`global_function_fallback`](Ctx::global_function_fallback)),
    /// every function of the name a file of unknown namespace declares
    /// ([`PackageIndex::unplaced_callables`](super::package_index::PackageIndex::unplaced_callables)), once one is found. That file may
    /// declare it in the global namespace, so a name it shares is not known
    /// there in full and binds neither ([NFR-RA-05]). Alone, such a function
    /// is no candidate: its namespace is not known.
    ///
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    fn namespace_callables(&self, package: &[String], name: &str, want: Want) -> Vec<NodeId> {
        let mut found = self.package_callables(&[package, &[name.to_string()]].concat(), want);
        if found.is_empty() || !package.is_empty() || !self.global_function_fallback {
            return found;
        }
        let unplaced = self
            .ix
            .packages.unplaced_callables
            .get(self.source_family.as_deref().unwrap_or_default())
            .and_then(|by_name| by_name.get(&[name.to_string()][..]));
        found.extend(
            unplaced
                .into_iter()
                .flatten()
                .copied()
                .filter(|id| self.ix.info.get(id).is_some_and(|i| want.admits(i.kind))),
        );
        found
    }

    /// Whether `id` may be what the written type names (S-602, [CR-199]): a
    /// type of the row's written arity ([`type_arity`](Ctx::type_arity)), or
    /// any node when that arity, or `id`'s own ([`PackageIndex::type_arities`](super::package_index::PackageIndex::type_arities)), is
    /// unknown — a non-type node records none. The filter of the lexical chain
    /// of [`resolve_name`](super::scope_hierarchy::ScopeHierarchy::resolve_name) and of the own-namespace,
    /// enclosing-namespace and namespace-wildcard rungs of
    /// [`resolve_package_name`](PackageNames::resolve_package_name), applied before
    /// each rung's exactly-one test: `Result` and `Result<T>` in one namespace
    /// are no ambiguity ([FR-RS-46]), and a level holding only the other
    /// arity passes the name outward.
    ///
    /// [CR-199]: ../../../../docs/requests/CR-199-csharp-resolution-honours-type-arity-and-using-scope.md
    /// [FR-RS-46]: ../../../../docs/specs/requirements/FR-RS-46.md
    pub(super) fn admits_arity(&self, id: NodeId) -> bool {
        match (self.type_arity, self.ix.packages.type_arities.get(&id)) {
            (Some(written), Some(&declared)) => written == declared,
            _ => true,
        }
    }

    /// The namespaces enclosing the source's own `package`, nearest first
    /// (S-595, [FR-RS-45]): `A.B.C` → `A.B`, `A`. Empty unless the source's
    /// language declares the key, and never the global namespace — a package of
    /// one segment, or none, has no enclosing level. A rung reads each level
    /// for exactly one type; two stop the walk unbound, none passes outward
    /// ([NFR-RA-05]).
    ///
    /// [FR-RS-45]: ../../../../docs/specs/requirements/FR-RS-45.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    pub(super) fn enclosing_levels<'p>(&self, package: &'p [String]) -> impl Iterator<Item = &'p [String]> {
        let depth = if self.enclosing_namespaces { package.len() } else { 0 };
        (1..depth).rev().map(move |len| &package[..len])
    }

    /// The top-level types named `name` in `package`, id-sorted — of the
    /// source's own interop family only (S-519).
    pub(super) fn package_type(&self, package: &[String], name: &str) -> &[NodeId] {
        let mut fqn = package.to_vec();
        fqn.push(name.to_string());
        self.family_types()
            .and_then(|types| types.get(&fqn))
            .map_or(&[], Vec::as_slice)
    }

    /// The top-level functions declared under the fully-qualified name `fqn`
    /// that `want` admits, id-sorted (S-573, [`PackageIndex::callables_by_fqn`](super::package_index::PackageIndex::callables_by_fqn)) — of
    /// the source's own interop family and its own language, and empty unless
    /// the source's language declares top-level functions members of their
    /// namespace. Two languages of one family that both declare them do not
    /// name each other's by their package (S-560): Kotlin reaches a Scala 3
    /// top-level `def` through its `package` object, and Scala a Kotlin `fun`
    /// through its file facade.
    pub(super) fn package_callables(&self, fqn: &[String], want: Want) -> Vec<NodeId> {
        if !self.top_level_callables {
            return Vec::new();
        }
        let language = |id: &NodeId| {
            let path = self.ix.info.get(id).and_then(|i| i.file_path.as_deref());
            path.and_then(|p| self.ix.layout.language(p))
        };
        let own = language(&self.source);
        self.ix
            .packages.callables_by_fqn
            .get(self.source_family.as_deref().unwrap_or_default())
            .and_then(|callables| callables.get(fqn))
            .into_iter()
            .flatten()
            .copied()
            .filter(|id| self.ix.info.get(id).is_some_and(|i| want.admits(i.kind)) && language(id) == own)
            .collect()
    }

    /// The fully-qualified type index of the source's interop family (S-519,
    /// [`PackageIndex::types_by_fqn`](super::package_index::PackageIndex::types_by_fqn)); `None` when no file of that family declares a
    /// type, or the source has no file.
    pub(super) fn family_types(&self) -> Option<&HashMap<Vec<String>, Vec<NodeId>>> {
        self.ix.packages.types_by_fqn.get(self.source_family.as_deref()?)
    }
}
