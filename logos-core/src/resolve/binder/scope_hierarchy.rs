//! The default binding hierarchy (S-011, [FR-RS-03]): a bare name through the
//! lexical chain, its module, its imports and the workspace fallbacks
//! ([`ScopeHierarchy::resolve_name`]); a path down the module tree
//! ([`ScopeHierarchy::resolve_path`], [`ScopeHierarchy::descend`]), where a
//! Rust path's type segment is decided by the one associated-item lookup
//! (S-607, [FR-RS-47]); and the crate → workspace acceptance step every
//! policy-gated fallback shares ([`ScopeHierarchy::prefer_crate`]).
//!
//! [FR-RS-03]: ../../../../docs/specs/requirements/FR-RS-03.md
//! [FR-RS-47]: ../../../../docs/specs/requirements/FR-RS-47.md

use std::ops::Deref;

use crate::config::BindingPolicy;
use crate::model::{NodeId, NodeKind};

use super::super::package_key::normalize_crate;
use super::{
    exactly_one, is_type_like, Ctx, Index, ModKey, Res, Residue, Syntax, Want, MAX_ALIAS_DEPTH,
};
use super::call_reach::CallReach;
use super::companions::Companions;
use super::import_roots::ImportRootPaths;
use super::package_names::PackageNames;
use super::package_paths::PackagePaths;
use super::rust_globs::RustGlobs;
#[cfg(test)]
use super::PATH_VISITS;

/// The default binding hierarchy: a bare name, a path down the module tree, the workspace fallbacks.
#[derive(Clone, Copy)]
pub(super) struct ScopeHierarchy<'c, 'a> {
    pub(super) cx: &'c Ctx<'a>,
}

impl<'a> Deref for ScopeHierarchy<'_, 'a> {
    type Target = Ctx<'a>;

    fn deref(&self) -> &Ctx<'a> {
        self.cx
    }
}

impl ScopeHierarchy<'_, '_> {
    /// Resolve a multi-or-single segment path by the scope hierarchy.
    pub(super) fn resolve_path(&self, segs: &[String], want: Want, depth: u8) -> Res {
        let import_roots = ImportRootPaths { cx: self.cx };
        let rust_globs = RustGlobs { cx: self.cx };
        let package_paths = PackagePaths { cx: self.cx };
        #[cfg(test)]
        PATH_VISITS.with(|v| v.set(v.get() + 1));
        if segs.is_empty() || depth == 0 {
            return Res::NotFound;
        }
        // 0) A relative import of an import-root language (S-519, [FR-RS-14]):
        //    `.`/`..` heads are its level, read from the source's package.
        if let Some(resolved) = import_roots.resolve_relative(segs, want) {
            return resolved;
        }
        if segs.len() == 1 {
            return self.resolve_name(&segs[0], want, depth);
        }

        let head = segs[0].as_str();
        let rest = &segs[1..];
        let source_mod = self.source_module();

        // 1) `crate::…` — the source's crate root.
        if head == "crate" {
            if let Some((krate, _)) = &source_mod {
                return self.descend(krate, &[], rest, want);
            }
            return Res::NotFound;
        }
        // 2) `self::…` — the source's module.
        if head == "self" {
            if let Some((krate, mods)) = &source_mod {
                return self.descend(krate, mods, rest, want);
            }
            return Res::NotFound;
        }
        // 3) `super::…` (possibly chained) — ancestors of the source module.
        if head == "super" {
            let Some((krate, mods)) = &source_mod else {
                return Res::NotFound;
            };
            let supers = segs.iter().take_while(|s| s.as_str() == "super").count();
            if supers > mods.len() {
                return Res::NotFound; // more supers than module depth
            }
            let base = &mods[..mods.len() - supers];
            return self.descend(krate, base, &segs[supers..], want);
        }
        // 4) A package-shaped source (CR-149): the rest of the path is decided
        //    by the package rungs alone, in the language's own order — its
        //    lexical, imported, same-package, enclosing-namespace (S-595) and
        //    on-demand heads, then the fully-qualified name. The module tree below names no package
        //    directory, and the workspace suffix match would read a
        //    fully-qualified name as a guess.
        if let Some(package) = &self.source_package {
            return package_paths.resolve_package_path(package, segs, want, depth);
        }
        // 4b) A `use`-alias head: substitute and resolve the expansion
        //     (depth-limited — an import cycle terminates as NotFound). An
        //     import-root file importing the head twice reads every import.
        if let Some(rivals) = import_roots.rival_expansions(head) {
            match import_roots.resolve_expansions(rivals, rest, want, depth) {
                Res::NotFound if !self.type_reached.get() => {}
                decided => return decided,
            }
        } else if let Some(alias_path) = self.scope().and_then(|s| s.aliases.get(head)) {
            let mut expanded = alias_path.clone();
            expanded.extend(rest.iter().cloned());
            match self.resolve_path(&expanded, want, depth - 1) {
                Res::NotFound if !self.type_reached.get() => {} // fall through to wider scopes
                decided => return decided,
            }
        }
        // 5) A crate-name head (`logos_core::…`) — never from an import-root
        //    family's crate, which names no other crate (S-519).
        let norm = normalize_crate(head);
        if self.ix.modules.crates.contains(&norm) && !import_roots.in_family_crate() {
            match self.descend(&norm, &[], rest, want) {
                Res::NotFound if !self.type_reached.get() => {} // a same-named module may still match below
                decided => return decided,
            }
        }
        // 6) Relative to the source module (`child_mod::item`), then to the
        //    crate root.
        if let Some((krate, mods)) = &source_mod {
            match self.descend(krate, mods, segs, want) {
                Res::NotFound if !self.type_reached.get() => {}
                decided => return decided,
            }
            match self.descend(krate, &[], segs, want) {
                Res::NotFound if !self.type_reached.get() => {}
                decided => return decided,
            }
        }
        // 7) Through the file's glob imports: candidates across every glob
        //    module, exactly-one overall.
        match rust_globs.through_globs(segs, want, depth) {
            Res::NotFound if !self.type_reached.get() => {}
            decided => return decided,
        }
        // 8) Policy-gated workspace fallback: unique module-path-suffix match —
        //    never for a type relation ([`scope_only`](Ctx::scope_only)). A
        //    receiver-method call never reaches here (S-514, CR-066).
        if self.policy != BindingPolicy::Strict && !self.scope_only.get() {
            return self.suffix_match(segs, want);
        }
        Res::NotFound
    }

    /// Resolve a bare name by the scope hierarchy (function-local outward).
    ///
    /// Reached only for a **single-segment** name (multi-segment paths route
    /// through [`descend`](ScopeHierarchy::descend)). A receiver-method call never arrives
    /// here (it binds by its receiver's shape, S-514); the CR-068 Part B
    /// free-function tie-break ([`prefer_free_functions`](super::call_reach::CallReach::prefer_free_functions)) fires only for a
    /// bare-path call, gated on [`bare_path_call`](Ctx::bare_path_call).
    fn resolve_name(&self, name: &str, want: Want, depth: u8) -> Res {
        let import_roots = ImportRootPaths { cx: self.cx };
        let rust_globs = RustGlobs { cx: self.cx };
        let package_names = PackageNames { cx: self.cx };
        let companions = Companions { cx: self.cx };
        // 1) Lexical Contains chain, innermost first: nested decls of the
        //    source itself (or of its enclosing scope, for a declaration's
        //    header — `lexical_start`), then each enclosing scope up to the
        //    file module.
        //    A scope whose callables of the name admit none of the call's
        //    arguments is passed over (S-592, [`applicable`](Ctx::applicable)).
        //    A class's companion is in scope right after the class (I4-INT,
        //    [`companion_of`](super::companions::Companions::companion_of)). Only a type of the written
        //    arity (S-602, [`admits_arity`](super::package_names::PackageNames::admits_arity)): a `Result<T>`
        //    beside the source in its own file is not the `Result` it writes.
        let mut cursor = Some(self.lexical_start.get());
        while let Some(scope) = cursor {
            let mut members = self.scope_members(scope, name, want);
            members.retain(|&id| !self.is_own_header(id) && package_names.admits_arity(id));
            let members = companions.narrow_pair(members, want);
            match exactly_one(&members) {
                Res::NotFound => {}
                decided => return decided, // found — or a *known* ambiguity
            }
            cursor = self.ix.parent.get(&scope).copied();
        }
        // A package-shaped source (CR-149) continues on its own rungs: its
        // imports, its package, its wildcards — never the module tree.
        if let Some(package) = &self.source_package {
            return package_names.resolve_package_name(package, name, want, depth);
        }
        // 2) A child module of the source module, a crate-root module
        //    (sibling files are linked via the path-derived module tree, not
        //    via Contains), or an extern crate's root (`use other::*` /
        //    `use other;` name the crate itself) — for a lookup that admits a
        //    module: never a call's, nor a type relation's (S-522).
        if want.admits(NodeKind::Module) {
            if let Some((krate, mods)) = self.source_module() {
                let mut child = mods.clone();
                child.push(name.to_string());
                if let Some(&id) = self.ix.modules.by_key.get(&(krate.clone(), child)) {
                    return Res::Found(id);
                }
                if let Some(&id) = self.ix.modules.by_key.get(&(krate, vec![name.to_string()])) {
                    return Res::Found(id);
                }
            }
            let norm = normalize_crate(name);
            if let Some(&id) = self
                .ix
                .modules.by_key
                .get(&(norm, Vec::new()))
                .filter(|_| !import_roots.in_family_crate())
            {
                return Res::Found(id);
            }
        }
        // 3) The file's `use` aliases — every one of them, exactly-one, when an
        //    import-root file imports the name twice.
        if let Some(rivals) = import_roots.rival_expansions(name) {
            match import_roots.resolve_expansions(rivals, &[], want, depth) {
                Res::NotFound => {}
                decided => return decided,
            }
        } else if let Some(alias_path) = self.scope().and_then(|s| s.aliases.get(name)) {
            match self.resolve_path(alias_path, want, depth - 1) {
                Res::NotFound => {}
                decided => return decided,
            }
        }
        // 4) The file's glob imports.
        let single = [name.to_string()];
        match rust_globs.through_globs(&single, want, depth) {
            Res::NotFound => {}
            decided => return decided,
        }
        // 5) The other files of the source's directory package (S-528, Go).
        match self.directory_package_name(name, want) {
            Res::NotFound => {}
            decided => return decided,
        }
        // 6) Workspace unique-name fallback — aggressive only for bare names,
        //    never for a type relation ([`scope_only`](Ctx::scope_only)). A
        //    receiver-method call never reaches here (S-514, CR-066).
        if self.policy == BindingPolicy::Aggressive && !self.scope_only.get() {
            return self.unique_by_name(name, want);
        }
        Res::NotFound
    }

    /// The members of `scope` named `name` that one level of
    /// [`resolve_name`](ScopeHierarchy::resolve_name)'s lexical chain offers: those the
    /// call's arguments admit ([`applicable`](Ctx::applicable)), free functions
    /// preferred ([`prefer_free_functions`](super::call_reach::CallReach::prefer_free_functions)) — else,
    /// for a class with a companion, the companion's (I4-INT,
    /// [`companion_of`](super::companions::Companions::companion_of)), the scope Kotlin reads next.
    fn scope_members(&self, scope: NodeId, name: &str, want: Want) -> Vec<NodeId> {
        let call_reach = CallReach { cx: self.cx };
        let companions = Companions { cx: self.cx };
        let ix: &Index = self.ix;
        let read = |at: NodeId| call_reach.prefer_free_functions(self.applicable(ix.members_named(at, name, want)));
        let members = read(scope);
        if !members.is_empty() {
            return members;
        }
        companions.companion_of(scope).map(read).unwrap_or(members)
    }

    /// A bare name from a directory-package source (S-528, [FR-RS-17]: Go)
    /// that its own file, its imports and its dot-imports do not supply: the
    /// exactly-one top-level declaration of the name among the **other** files
    /// of the source's package — the files of its directory that declare its
    /// `package` clause ([`PackageLayout::directory_package`](crate::resolve::package_key::PackageLayout::directory_package), read off
    /// [`ModuleIndex::file_roots_by_dir`](super::module_index::ModuleIndex::file_roots_by_dir)). Their
    /// declarations are read as the lexical rung reads the source's own file —
    /// the argument filter ([`applicable`](Ctx::applicable)) and the
    /// free-function tie-break ([`prefer_free_functions`](super::call_reach::CallReach::prefer_free_functions)),
    /// so a method that only shares the name is never one.
    ///
    /// Two are a known ambiguity that stops the walk ([NFR-RA-05]): a name a
    /// build-tag variant declares again (`f_linux.go` and `f_windows.go`) binds
    /// neither, nor reaches the workspace fallback. None passes on. A `package
    /// foo_test` file declares another clause, so it is a package of its own
    /// and never reaches `foo`'s names, nor they its. A directory holding a
    /// file of the language whose clause is not recorded — a store not yet
    /// re-extracted since clauses were, file by file — is no known package:
    /// the rung reads nothing there, so a build-tag variant it cannot see is
    /// never outvoted by the one it can. Open to a type relation: the package
    /// is the scope the name is written in.
    ///
    /// [FR-RS-17]: ../../../../docs/specs/requirements/FR-RS-17.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    fn directory_package_name(&self, name: &str, want: Want) -> Res {
        let call_reach = CallReach { cx: self.cx };
        let ix: &Index = self.ix;
        let layout = &ix.layout;
        let Some(path) = ix.file_of(self.source) else {
            return Res::NotFound;
        };
        let Some((dir, clause)) = layout.directory_package(path) else {
            return Res::NotFound;
        };
        let mut found: Vec<NodeId> = Vec::new();
        for (sibling, root) in ix.modules.file_roots_by_dir.get(dir).into_iter().flatten() {
            if sibling == path || !layout.declares_directory_packages(sibling) {
                continue;
            }
            match layout.directory_package(sibling) {
                Some((_, c)) if c == clause => found.extend(ix.members_named(*root, name, want)),
                Some(_) => {}
                // A file whose clause is not recorded (not re-extracted since
                // clauses were, or a damaged parse) may be of the package and
                // declare the name: the package is not known in full.
                None => return Res::NotFound,
            }
        }
        exactly_one(&call_reach.prefer_free_functions(self.applicable(found)))
    }

    /// Walk `segs` down the module tree from `(krate, base)`.
    pub(super) fn descend(&self, krate: &str, base: &[String], segs: &[String], want: Want) -> Res {
        let call_reach = CallReach { cx: self.cx };
        let ix: &Index = self.ix;
        let mut key: ModKey = (krate.to_string(), base.to_vec());
        for (i, seg) in segs.iter().enumerate() {
            let is_last = i == segs.len() - 1;

            if !is_last {
                // Try the segment as a child module in place (push, check,
                // pop on miss) — no per-step key clone. An import-root
                // language's directory is a module whether or not a package
                // file gives it a node (S-519).
                key.1.push(seg.clone());
                if ix.modules.by_key.contains_key(&key) || ix.modules.dir_modules.contains(&key) {
                    continue;
                }
                key.1.pop();
                return self.type_segment(&key, segs, i, want);
            }

            // Final segment: a child module (preferred for imports) or a
            // member item of the current module. Only a lookup that admits a
            // module takes the child — never a call's, nor a type relation's.
            if want.admits(NodeKind::Module) {
                let mut child = key.1.clone();
                child.push(seg.clone());
                if let Some(&m) = ix.modules.by_key.get(&(key.0.clone(), child)) {
                    return Res::Found(m);
                }
            }
            if want == Want::Module {
                return Res::NotFound;
            }
            let Some(&scope_node) = ix.modules.by_key.get(&key) else {
                return Res::NotFound;
            };
            let mut members = self.applicable(ix.members_named(scope_node, seg, want));
            // A bare call's `use` or glob import reaches the module's free
            // function, never an associated one collapsed beside it (S-590).
            if self.bare_path_call.get() {
                members.retain(|&id| !call_reach.is_unreachable_member(id));
            }
            return exactly_one(&members);
        }
        Res::NotFound
    }

    /// [`descend`](ScopeHierarchy::descend)'s step at `segs[i]`, a segment naming
    /// no child module of `key`: the type of a written `T::m()`, whose call the
    /// one lookup decides in an impl-block language, else the `Type::func`
    /// collapse — and for any other segment, nothing.
    fn type_segment(&self, key: &ModKey, segs: &[String], i: usize, want: Want) -> Res {
        let rust_globs = RustGlobs { cx: self.cx };
        let ix: &Index = self.ix;
        let seg = &segs[i];
        // A written `T::m()` in an impl-block language (S-607,
        // [FR-RS-47]): the next-to-last segment names a type the
        // module declares or re-exports, and the call binds among
        // that type's items alone, by the one lookup with path syntax
        // — decided here, whatever it found ([`Ctx::type_reached`]).
        // Nothing else (an import of `T::m`) names an associated item.
        //
        // [FR-RS-47]: ../../../../docs/specs/requirements/FR-RS-47.md
        if self.impl_blocks && i == segs.len() - 2 {
            if !want.is_call() {
                return Res::NotFound;
            }
            return match rust_globs.type_in_module(key, seg, MAX_ALIAS_DEPTH) {
                Res::Found(ty) => {
                    self.type_reached.set(true);
                    self.lookup(ty, &segs[i + 1], Syntax::Path, None)
                }
                Res::Ambiguous => {
                    self.type_reached.set(true);
                    self.note(want, || Residue::TypeAmbiguous);
                    Res::Ambiguous
                }
                Res::NotFound => Res::NotFound,
            };
        }
        // The `Type::func` collapse of every other language:
        // associated items live at module scope, so when the
        // next-to-last segment names a type in the current module,
        // the final segment is looked up among the module's
        // functions.
        if i == segs.len() - 2 {
            if let Some(&scope_node) = ix.modules.by_key.get(key) {
                let type_here = ix
                    .members_named(scope_node, seg, Want::Any)
                    .into_iter()
                    .any(|id| ix.info.get(&id).is_some_and(|n| is_type_like(n.kind)));
                if type_here {
                    return exactly_one(&self.applicable(ix.members_named(
                        scope_node,
                        &segs[i + 1],
                        Want::Callable,
                    )));
                }
            }
        }
        Res::NotFound
    }

    /// Workspace fallback for a multi-segment path (balanced+): the final
    /// segment's name matches and the module the node sits in ends with the
    /// leading segments — crate-first, then workspace, exactly-one at each
    /// step ([FR-RS-03] crate → workspace levels).
    ///
    /// A **module** candidate sits in its parent: its own key ends in its own
    /// name, so `pkg::mod` reaches the module `[…, pkg, mod]` through its parent
    /// key `[…, pkg]` (S-519, [FR-RS-14]). Compared against its own key, a
    /// module could only ever match a path that spelled its name twice.
    ///
    /// [FR-RS-14]: ../../../../docs/specs/requirements/FR-RS-14.md
    fn suffix_match(&self, segs: &[String], want: Want) -> Res {
        let ix: &Index = self.ix;
        let (prefix, last) = segs.split_at(segs.len() - 1);
        let Some(all) = ix.by_name.get(&last[0]) else {
            return Res::NotFound;
        };
        let matches_suffix = |id: &NodeId| -> bool {
            let Some(info) = ix.info.get(id) else {
                return false;
            };
            if !want.admits(info.kind) {
                return false;
            }
            match ix.modules.module_key.get(id) {
                // A module: its parent's key must end with the prefix; a crate
                // root has no parent to match.
                Some((_, own)) => own
                    .split_last()
                    .is_some_and(|(_, parent)| parent.ends_with(prefix)),
                // Any other node: its enclosing module's key must.
                None => ix
                    .nearest_module(*id)
                    .is_some_and(|(_, mods)| mods.ends_with(prefix)),
            }
        };
        let candidates: Vec<NodeId> = all.iter().copied().filter(matches_suffix).collect();
        self.prefer_crate(&self.applicable(candidates))
    }

    /// Workspace unique-name fallback (method calls at balanced+, bare names
    /// at aggressive): crate-first, then workspace, exactly-one at each step.
    pub(super) fn unique_by_name(&self, name: &str, want: Want) -> Res {
        let Some(all) = self.ix.by_name.get(name) else {
            return Res::NotFound;
        };
        let candidates: Vec<NodeId> = all
            .iter()
            .copied()
            .filter(|id| self.ix.info.get(id).is_some_and(|i| want.admits(i.kind)))
            .collect();
        self.prefer_crate(&self.applicable(candidates))
    }

    /// The crate → workspace acceptance step shared by every workspace
    /// fallback ([FR-RS-03] hierarchy order): exactly-one among the source
    /// crate's candidates wins; a crate-level ambiguity is final; only an
    /// empty crate set escalates to the workspace-wide exactly-one test.
    ///
    /// [FR-RS-03]: ../../../../docs/specs/requirements/FR-RS-03.md
    fn prefer_crate(&self, candidates: &[NodeId]) -> Res {
        let source_crate = self
            .ix
            .info
            .get(&self.source)
            .map(|i| i.crate_name.as_str())
            .unwrap_or_default();
        let in_crate: Vec<NodeId> = candidates
            .iter()
            .copied()
            .filter(|id| {
                self.ix
                    .info
                    .get(id)
                    .is_some_and(|i| i.crate_name == source_crate)
            })
            .collect();
        match exactly_one(&in_crate) {
            // An import-root family's crate is a closed namespace (S-519): its
            // fallback never escalates to another language's modules.
            Res::NotFound if self.ix.layout.is_family_crate(source_crate) => Res::NotFound,
            Res::NotFound => exactly_one(candidates),
            decided => decided,
        }
    }
}
