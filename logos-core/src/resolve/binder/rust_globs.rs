//! What a module's glob imports and re-exports bring into view ([FR-RS-02];
//! S-588, S-607): a path resolved through each of the file's globs, exactly
//! one across all of them ([`RustGlobs::through_globs`]), and the type a
//! module declares or re-exports by a `use` or a `pub use` glob
//! ([`RustGlobs::type_in_module`]).
//!
//! [FR-RS-02]: ../../../../docs/specs/requirements/FR-RS-02.md

use std::ops::Deref;

use crate::model::NodeId;

use super::super::package_key::normalize_crate;
use super::supertype_climb::SupertypeClimb;
use super::{exactly_one, Ctx, FileScope, ModKey, Res, Want};
use super::scope_hierarchy::ScopeHierarchy;

/// What a module's glob imports and re-exports bring into view.
#[derive(Clone, Copy)]
pub(super) struct RustGlobs<'c, 'a> {
    pub(super) cx: &'c Ctx<'a>,
}

impl<'a> Deref for RustGlobs<'_, 'a> {
    type Target = Ctx<'a>;

    fn deref(&self) -> &Ctx<'a> {
        self.cx
    }
}

impl RustGlobs<'_, '_> {
    /// The one type `name` the module `key` holds (S-607): a type-like member
    /// it declares, else one it re-exports — by a named `use`, or a `pub use`
    /// glob ([`reexported_type`](RustGlobs::reexported_type)), `depth` hops at most.
    pub(super) fn type_in_module(&self, key: &ModKey, name: &str, depth: u8) -> Res {
        let supertype_climb = SupertypeClimb { cx: self.cx };
        let Some(&module) = self.ix.modules.by_key.get(key) else {
            return Res::NotFound;
        };
        match exactly_one(&supertype_climb.member_types(module, name)) {
            Res::NotFound => self.reexported_type(key, name, depth),
            decided => decided,
        }
    }

    /// The import scope of the file that holds the module `key` — one file
    /// holds one scope, its inline modules' `use`s included.
    pub(super) fn module_scope(&self, key: &ModKey) -> Option<&FileScope> {
        self.ix
            .modules.by_key
            .get(key)
            .and_then(|m| self.ix.info.get(m))
            .and_then(|i| i.file_path.as_deref())
            .and_then(|p| self.ix.modules.file_ids.get(p))
            .and_then(|id| self.ix.modules.file_scopes.get(id))
    }

    /// Where a `use` path written in the module `key` may start (S-588):
    /// `crate::`, `self::` and `super::` from the module, a bare head from the
    /// module itself and — when it names another repository crate — from that
    /// crate's root. Each anchor is `(crate, module path, the rest)`.
    fn anchors<'p>(&self, key: &ModKey, path: &'p [String]) -> Vec<(String, Vec<String>, &'p [String])> {
        let (krate, mods) = key;
        let supers = path.iter().take_while(|s| s.as_str() == "super").count();
        let mut anchors: Vec<(String, Vec<String>, &[String])> = Vec::new();
        match path.first().map(String::as_str) {
            Some("crate") => anchors.push((krate.clone(), Vec::new(), &path[1..])),
            Some("self") => anchors.push((krate.clone(), mods.clone(), &path[1..])),
            Some("super") if supers <= mods.len() => {
                anchors.push((krate.clone(), mods[..mods.len() - supers].to_vec(), &path[supers..]));
            }
            Some("super") | None => {}
            Some(head) => {
                anchors.push((krate.clone(), mods.clone(), path));
                let norm = normalize_crate(head);
                if self.ix.modules.crates.contains(&norm) && norm != *krate {
                    anchors.push((norm, Vec::new(), &path[1..]));
                }
            }
        }
        anchors
    }

    /// The type `name` the module `key` brings in through its `pub use` globs
    /// (S-607, [FR-RS-47]): each glob's module, read from `key` as a `use`
    /// path is ([`anchors`](RustGlobs::anchors)), declares it or re-exports it in
    /// turn ([`reexported_type`](RustGlobs::reexported_type)), `depth` hops at most.
    /// Exactly one across the globs; two are [`Res::Ambiguous`].
    ///
    /// [FR-RS-47]: ../../../../docs/specs/requirements/FR-RS-47.md
    pub(super) fn through_reexported_globs(&self, key: &ModKey, globs: &[Vec<String>], name: &str, depth: u8) -> Res {
        let mut found: Vec<NodeId> = Vec::new();
        for glob in globs {
            for (krate, mut base, rest) in self.anchors(key, glob) {
                base.extend(rest.iter().cloned());
                match self.type_in_module(&(krate, base), name, depth - 1) {
                    Res::Found(id) => {
                        found.push(id);
                        break;
                    }
                    Res::Ambiguous => return Res::Ambiguous,
                    Res::NotFound => {}
                }
            }
        }
        found.sort_unstable();
        found.dedup();
        exactly_one(&found)
    }

    /// The type the module `key` re-exports as `name` (S-588): its file's
    /// `use` of that name, read from the module itself — `crate::`, `self::`,
    /// `super::`, a path relative to the module, or another crate's name — to
    /// a type the target module declares, or, failing that, re-exports in
    /// turn, `depth` hops at most (an import cycle ends as not found). With no
    /// `use` of the name, its `pub use` globs are followed (S-607,
    /// [`through_reexported_globs`](RustGlobs::through_reexported_globs)). A
    /// module's private `use` reads alike: a written path through one does not
    /// compile, so no call site spells one — and only an ancestor's glob is
    /// read through it ([`resolve_receiver_type`](super::proven_receivers::ProvenReceivers::resolve_receiver_type)).
    pub(super) fn reexported_type(&self, key: &ModKey, name: &str, depth: u8) -> Res {
        let scope_hierarchy = ScopeHierarchy { cx: self.cx };
        if depth == 0 {
            return Res::NotFound;
        }
        let Some(scope) = self.module_scope(key) else {
            return Res::NotFound;
        };
        // One file holds one import scope, its inline modules' `use`s
        // included: a file importing the name twice (a top-level `use` and an
        // inline `mod`'s `pub use`) does not say which one `key`'s module
        // holds — as for the caller's own file
        // ([`resolve_receiver_type`](super::proven_receivers::ProvenReceivers::resolve_receiver_type)).
        if scope.alias_expansions.get(name).is_some_and(|all| all.len() > 1) {
            return Res::Ambiguous;
        }
        let Some(alias) = scope.aliases.get(name) else {
            return self.through_reexported_globs(key, &scope.exported_globs, name, depth);
        };
        for (krate, base, rest) in self.anchors(key, alias) {
            let Some((last, prefix)) = rest.split_last() else { continue };
            match scope_hierarchy.descend(&krate, &base, rest, Want::Type) {
                Res::NotFound => {}
                decided => return decided,
            }
            let mut next = (krate, base);
            next.1.extend(prefix.iter().cloned());
            if self.ix.modules.by_key.contains_key(&next) {
                match self.reexported_type(&next, last, depth - 1) {
                    Res::NotFound => {}
                    decided => return decided,
                }
            }
        }
        Res::NotFound
    }

    /// Resolve `segs` as members reached through each of the file's glob
    /// imports; accept iff exactly one distinct target across all globs.
    ///
    /// Each glob's *own* module path is resolved via [`resolve_path`](super::scope_hierarchy::ScopeHierarchy::resolve_path),
    /// whose import step lands back here — so on a file carrying `G` glob
    /// imports a single lookup fans out `G`-wide at every recursion level, i.e.
    /// `O(G^MAX_ALIAS_DEPTH)` work per reference. A real trigger (CR-016): a file
    /// with 14 `use super::*` / `use super::sub::*` imports drove a single bind
    /// to ~1.5e9 operations (~150 s on one core); across the parallel bind pass
    /// many such refs detonate at once and peg every core — the dominant cost of
    /// a cold full index (measured on the self-graph: a cold index climbed to a
    /// load of 27 within 27 s before this guard, 3.8 s at flat load after).
    ///
    /// The `in_glob_resolution` guard breaks the recursion: while a glob's
    /// module is being resolved, a re-entry here returns `NotFound` at once.
    /// This is **outcome-preserving** — a glob module reachable *only* through
    /// the same in-scope glob set never converged under the depth budget anyway
    /// (it bottomed out at `NotFound`), so we return that result without the
    /// exponential. The sync≡reindex equivalence net and the real self-graph
    /// (edge set unchanged) gate the equivalence.
    pub(super) fn through_globs(&self, segs: &[String], want: Want, depth: u8) -> Res {
        let scope_hierarchy = ScopeHierarchy { cx: self.cx };
        let Some(scope) = self.scope() else {
            return Res::NotFound;
        };
        // Re-entrant glob-module resolution contributes nothing (see above).
        if self.in_glob_resolution.get() {
            return Res::NotFound;
        }
        let mut found: Vec<NodeId> = Vec::new();
        for glob in &scope.globs {
            // Resolve the glob's module itself (depth-limited like an alias),
            // with glob fan-out suppressed for the duration so the lookup cannot
            // recurse back through the file's glob set.
            self.in_glob_resolution.set(true);
            let resolved = scope_hierarchy.resolve_path(glob, Want::Module, depth.saturating_sub(1));
            self.in_glob_resolution.set(false);
            let module = match resolved {
                Res::Found(m) => m,
                Res::Ambiguous => return Res::Ambiguous,
                Res::NotFound => continue,
            };
            let Some(key) = self.ix.modules.module_key.get(&module) else {
                continue;
            };
            match scope_hierarchy.descend(&key.0, &key.1, segs, want) {
                Res::Found(id) => found.push(id),
                Res::Ambiguous => return Res::Ambiguous,
                Res::NotFound if self.type_reached.get() => return Res::NotFound,
                Res::NotFound => {}
            }
        }
        found.sort();
        found.dedup();
        exactly_one(&found)
    }
}
