//! A call on a receiver whose type the file proves, retyped `T::m` by
//! extraction (S-537, S-546, S-563, S-576, S-587/S-588, [FR-RS-42]): bound
//! among `T`'s own methods by the language's own reading of `T` — the one
//! lookup for Rust, `T`'s members up its proven `Extends` for a
//! structural-types language, never a companion's
//! ([`ProvenReceivers::resolve_proven_receiver_call`]).
//!
//! [FR-RS-42]: ../../../../docs/specs/requirements/FR-RS-42.md

use std::collections::HashSet;
use std::ops::Deref;

use crate::graph_store::UnresolvedRefRow;
use crate::model::{NodeId, NodeKind};

use super::hierarchy::Hierarchy;
use super::supertype_climb::SupertypeClimb;
use super::{
    exactly_one, split, Ctx, Index, ModKey, Res, Residue, Syntax, Want, MAX_ALIAS_DEPTH,
    MAX_SUPERTYPE_DEPTH,
};
use super::extension_calls::{ExtensionCalls, ExtensionReceiver};
use super::import_roots::ImportRootPaths;
use super::imported_scope::ImportedScope;
use super::rust_globs::RustGlobs;
use super::scope_hierarchy::ScopeHierarchy;

/// A call on a receiver whose type the file proves.
#[derive(Clone, Copy)]
pub(super) struct ProvenReceivers<'c, 'a> {
    pub(super) cx: &'c Ctx<'a>,
}

impl<'a> Deref for ProvenReceivers<'_, 'a> {
    type Target = Ctx<'a>;

    fn deref(&self) -> &Ctx<'a> {
        self.cx
    }
}

impl ProvenReceivers<'_, '_> {
    /// Resolve a Rust call retyped from its proven receiver (S-587) — `x.m()`
    /// recorded as `T::m` of shape `other`, with the wrappers peeled to reach
    /// `T` — among `T`'s own methods (S-588, [FR-RS-42]). A call from a file of
    /// structural types (TypeScript, S-537) is bound among its type's members
    /// instead ([`resolve_structural_receiver_call`](ProvenReceivers::resolve_structural_receiver_call)),
    /// and so is one from a file whose types lend their name to a companion
    /// (Kotlin, S-563,
    /// [`resolve_companion_language_receiver_call`](ProvenReceivers::resolve_companion_language_receiver_call))
    /// and one from a file whose types pair with a same-named singleton
    /// (Scala, S-576,
    /// [`resolve_companion_pair_receiver_call`](ProvenReceivers::resolve_companion_pair_receiver_call)),
    /// and one from an import-root file (Python, S-546,
    /// [`resolve_import_root_receiver_call`](ProvenReceivers::resolve_import_root_receiver_call)).
    ///
    /// 1. **A wrapper's own method binds nothing.** When a peeled wrapper
    ///    provides `m` itself ([`PackageLayout::wrapper_provides`], the list
    ///    the plugin declares), the call is the wrapper's: `x.clone()` on an
    ///    `Arc<T>` is `Arc::clone`, whatever `T` defines —
    ///    [`Residue::ExternalType`], as its target is outside the repository.
    /// 2. **`T` is the one type the file names.** Read through the file's
    ///    `use` declarations ([`resolve_receiver_type`](ProvenReceivers::resolve_receiver_type)),
    ///    in the caller's crate or another in-repository one, and never by a
    ///    workspace guess ([`scope_only`](Ctx::scope_only)). A `T` the
    ///    repository does not declare — `String`, `Vec`, `std::io::Error`, an
    ///    external crate's — binds nothing: [`Residue::ExternalType`]; one the
    ///    file's scope names twice is [`Residue::TypeAmbiguous`].
    /// 3. **`m` is `T`'s one method of the name**, by the one associated-item
    ///    lookup with method syntax ([`lookup`](Ctx::lookup), S-607): an
    ///    associated function never, an inherent method before a trait impl's.
    ///
    /// [FR-RS-42]: ../../../../docs/specs/requirements/FR-RS-42.md
    /// [`PackageLayout::wrapper_provides`]: crate::resolve::package_key::PackageLayout::wrapper_provides
    pub(super) fn resolve_proven_receiver_call(&self, r: &UnresolvedRefRow) -> Res {
        let external = || Residue::ExternalType {
            candidates: Vec::new(),
        };
        let segs = split(&r.target);
        let Some((name, type_path)) = segs.split_last().filter(|(_, ty)| !ty.is_empty()) else {
            self.note(Want::Callable, || Residue::NoReceiverEvidence);
            return Res::NotFound;
        };
        let source_file = self
            .ix
            .info
            .get(&self.source)
            .and_then(|i| i.file_path.as_deref())
            .unwrap_or_default();
        if self.ix.layout.has_structural_types(source_file) {
            return self.resolve_structural_receiver_call(type_path, name);
        }
        if self.ix.layout.companion_name(source_file).is_some() {
            return self.resolve_companion_language_receiver_call(type_path, name);
        }
        if self.ix.layout.pairs_companion_objects(source_file) {
            return self.resolve_companion_pair_receiver_call(&segs);
        }
        if self.ix.layout.has_import_roots(source_file) {
            return self.resolve_import_root_receiver_call(type_path, name);
        }
        let through_wrapper = r.peeled.as_deref().unwrap_or_default().split(' ').any(|wrapper| {
            self.ix.layout.wrapper_provides(source_file, wrapper, name)
        });
        if through_wrapper {
            self.note(Want::Callable, external);
            return Res::NotFound;
        }
        match self.resolve_receiver_type(type_path) {
            Res::Found(ty) => self.lookup(ty, name, Syntax::Method, None),
            Res::Ambiguous => {
                self.note(Want::Callable, || Residue::TypeAmbiguous);
                Res::Ambiguous
            }
            Res::NotFound => {
                self.note(Want::Callable, external);
                Res::NotFound
            }
        }
    }

    /// Resolve a call on a proven receiver of a structural-types language
    /// (S-537, [FR-RS-21]; TypeScript) — `this.users.find()` recorded as
    /// `<import target>::UsersService::find`, or `Mailer::send` for a type no
    /// named import brings in — among its type's members.
    ///
    /// 1. **`T` is the one type the file names**: through the named import the
    ///    path's head records, the one definition that import provides
    ///    ([`resolve_imported_type`](super::imported_scope::ImportedScope::resolve_imported_type), S-535); a bare
    ///    `T`, in the file's own scope, never by a workspace guess
    ///    ([`scope_only`](Ctx::scope_only)). An import that reaches no
    ///    repository type (`@nestjs/common`'s `Logger`) is
    ///    [`Residue::ExternalType`]; two candidates are
    ///    [`Residue::TypeAmbiguous`].
    /// 2. **`m` is exactly one callable of `T`, or of the nearest level of its
    ///    proven `Extends` chain holding one** ([`type_member`](super::supertype_climb::SupertypeClimb::type_member)),
    ///    a required signature pooled with the members at each level
    ///    ([`reads_signatures`](Ctx::reads_signatures)): a call on an abstract
    ///    class or an interface binds its declared method, never an
    ///    implementation.
    ///
    /// [FR-RS-21]: ../../../../docs/specs/requirements/FR-RS-21.md
    fn resolve_structural_receiver_call(&self, type_path: &[String], name: &str) -> Res {
        let supertype_climb = SupertypeClimb { cx: self.cx };
        match self.structural_receiver_type(type_path) {
            Res::Found(ty) => {
                self.reads_signatures.set(true);
                let found = supertype_climb.type_member(ty, name);
                self.reads_signatures.set(false);
                found
            }
            Res::Ambiguous => {
                self.note(Want::Callable, || Residue::TypeAmbiguous);
                Res::Ambiguous
            }
            Res::NotFound => {
                self.note(Want::Callable, || Residue::ExternalType { candidates: Vec::new() });
                Res::NotFound
            }
        }
    }

    /// The repository type a structural-types or companion-language file's
    /// proven receiver path names (S-537, S-563): through the named import its
    /// head records ([`resolve_imported_type`](super::imported_scope::ImportedScope::resolve_imported_type)),
    /// else in the file's own scope. A type lookup records no call residue.
    pub(super) fn structural_receiver_type(&self, type_path: &[String]) -> Res {
        let scope_hierarchy = ScopeHierarchy { cx: self.cx };
        let imported_scope = ImportedScope { cx: self.cx };
        imported_scope
            .resolve_imported_type(&type_path.join("::"), Want::Type)
            .unwrap_or_else(|| scope_hierarchy.resolve_path(type_path, Want::Type, MAX_ALIAS_DEPTH))
    }

    /// Resolve a call on a proven receiver of a file whose types lend their
    /// name to a companion (S-563, [FR-RS-32]; Kotlin) — `repo.find()` after
    /// `repo: Repo`, recorded as `Repo::find` — among `T`'s own members, never
    /// its companion's, which only a call on the class reaches (I4-INT).
    ///
    /// `T` and `m` are read as a structural-types file reads them
    /// ([`resolve_structural_receiver_call`](ProvenReceivers::resolve_structural_receiver_call)):
    /// the one type the file's scope, imports, package and wildcards name,
    /// then exactly one callable of `T`, or of the nearest level of its proven
    /// supertypes holding one. A call no level answers — `T` is no repository
    /// type (`String`), or none of its levels declares `m` — binds an extension
    /// `m` whose receiver type is `T`, or an in-repository supertype of the
    /// repository type `T` names
    /// ([`resolve_extension_call`](super::extension_calls::ExtensionCalls::resolve_extension_call), S-560, S-833):
    /// a proven `LocalDate` never reaches `fun LocalDateTime.format`. Where
    /// that rung decides — two extensions in scope — its reason is the row's,
    /// not the type's miss before it.
    ///
    /// A one-segment `T` that a top-level function of its name also spells
    /// ([`factory_named`](super::extension_calls::ExtensionCalls::factory_named)) proves nothing:
    /// [`Residue::TypeAmbiguous`].
    ///
    /// [FR-RS-32]: ../../../../docs/specs/requirements/FR-RS-32.md
    fn resolve_companion_language_receiver_call(&self, type_path: &[String], name: &str) -> Res {
        let extensions = ExtensionCalls { cx: self.cx };
        if let [ty] = type_path {
            if extensions.factory_named(ty) {
                self.note(Want::Callable, || Residue::TypeAmbiguous);
                return Res::Ambiguous;
            }
        }
        match self.resolve_structural_receiver_call(type_path, name) {
            Res::NotFound => {
                let typed_miss = self.miss.take();
                let ty = match self.structural_receiver_type(type_path) {
                    Res::Found(ty) => Some(ty),
                    _ => None,
                };
                let receiver = ExtensionReceiver::Proven { path: type_path, ty };
                extensions.resolve_extension_call(name, receiver).unwrap_or_else(|| {
                    *self.miss.borrow_mut() = typed_miss;
                    Res::NotFound
                })
            }
            decided => decided,
        }
    }

    /// Resolve a call on a proven receiver of a file whose types pair with a
    /// same-named singleton (S-576, [FR-RS-37]; Scala) — `order.ship()` after
    /// `order: Order`, recorded as `Order::ship` — among `T`'s own members,
    /// never its companion `object`'s, which only a call on the name reaches
    /// (S-573).
    ///
    /// The path is read as the written `Order.ship()` is, through the file's
    /// scope, imports, package and wildcards, and a `T` with no companion binds
    /// its callable, or its nearest proven supertype's, as that call does
    /// ([`walk_from`](super::companions::Companions::walk_from)). Where `T` names a companion pair, the
    /// pair's callables that take an instance receiver are read in place of
    /// the singleton's ([`instance_receiver`](Ctx::instance_receiver)): exactly
    /// one decides, and the pair's supertypes are not climbed. No extension
    /// rung follows: the language declares none.
    ///
    /// [FR-RS-37]: ../../../../docs/specs/requirements/FR-RS-37.md
    fn resolve_companion_pair_receiver_call(&self, segs: &[String]) -> Res {
        let scope_hierarchy = ScopeHierarchy { cx: self.cx };
        let want = self.call_want();
        self.instance_receiver.set(true);
        let found = scope_hierarchy.resolve_path(segs, want, MAX_ALIAS_DEPTH);
        self.instance_receiver.set(false);
        if found == Res::NotFound && self.passed_over.get() {
            self.note(want, || Residue::NoApplicableOverload);
        }
        found
    }

    /// Resolve a call on a proven receiver of an import-root file (S-546,
    /// [FR-RS-24]; Python) — `svc.run()` after `svc: Service`, recorded as
    /// `Service::run` — among its type's methods.
    ///
    /// 1. **`T` is the one class the file names**, read as a written qualifier
    ///    is (S-544, [`resolve_written_qualifier`](super::import_roots::ImportRootPaths::resolve_written_qualifier)):
    ///    through every import of the name, else the file's own scope, never
    ///    by a workspace guess ([`scope_only`](Ctx::scope_only)). A name that
    ///    reaches no repository declaration (`str`, `requests`' `Session`) is
    ///    [`Residue::ExternalType`]; two are [`Residue::TypeAmbiguous`], and so
    ///    is a name the file both imports and declares at its top level
    ///    (`class Service(Service)`), which the statements' order decides; a
    ///    declaration that is no class — `x = Factory()` where `Factory` is a
    ///    function — proved nothing: [`Residue::NoReceiverEvidence`].
    /// 2. **`m` is exactly one callable of `T`, or of the nearest level of its
    ///    proven `Extends` chain holding one** ([`type_member`](super::supertype_climb::SupertypeClimb::type_member)).
    ///    An attribute of the name at or below that level — `run = other`,
    ///    `self.run = callback` — is what the call reaches instead, whatever
    ///    it holds: [`Residue::NoReceiverEvidence`] ([`attribute_hides`](ProvenReceivers::attribute_hides)).
    ///
    /// [FR-RS-24]: ../../../../docs/specs/requirements/FR-RS-24.md
    fn resolve_import_root_receiver_call(&self, type_path: &[String], name: &str) -> Res {
        let import_roots = ImportRootPaths { cx: self.cx };
        let supertype_climb = SupertypeClimb { cx: self.cx };
        if let [head] = type_path {
            if self.imports_and_declares(head) {
                self.note(Want::Callable, || Residue::TypeAmbiguous);
                return Res::Ambiguous;
            }
        }
        match import_roots.resolve_written_qualifier(type_path) {
            Res::Found(ty) if self.ix.info.get(&ty).is_some_and(|i| i.kind == NodeKind::Class) => {
                if self.attribute_hides(ty, name) {
                    self.note(Want::Callable, || Residue::NoReceiverEvidence);
                    return Res::NotFound;
                }
                supertype_climb.type_member(ty, name)
            }
            Res::Found(_) => {
                self.note(Want::Callable, || Residue::NoReceiverEvidence);
                Res::NotFound
            }
            Res::Ambiguous => {
                self.note(Want::Callable, || Residue::TypeAmbiguous);
                Res::Ambiguous
            }
            Res::NotFound => {
                self.note(Want::Callable, || Residue::ExternalType { candidates: Vec::new() });
                Res::NotFound
            }
        }
    }

    /// Whether the source's file imports `name` and also declares it among its
    /// own top-level members (S-546): `from m import Service` beside `class
    /// Service(Service)`, where which one a use names is the statements'
    /// order, not the file's scope.
    fn imports_and_declares(&self, name: &str) -> bool {
        let ix: &Index = self.ix;
        let imported = self.scope().is_some_and(|s| s.aliases.contains_key(name) || s.alias_expansions.contains_key(name));
        let mut cursor = Some(self.source);
        while let Some(id) = cursor {
            if ix.info.get(&id).is_some_and(|i| i.kind == NodeKind::Module) {
                return imported && !ix.members_named(id, name, Want::Any).is_empty();
            }
            cursor = ix.parent.get(&id).copied();
        }
        false
    }

    /// Whether a Field named `name` stands on the `Extends` chain of `ty` at
    /// or below the first level declaring a callable of that name — the
    /// levels [`type_member`](super::supertype_climb::SupertypeClimb::type_member) climbs (S-546). In Python one
    /// namespace holds attributes and methods, so such an attribute hides the
    /// method above it.
    fn attribute_hides(&self, ty: NodeId, name: &str) -> bool {
        let ix: &Index = self.ix;
        let hierarchy: &Hierarchy = &ix.hierarchy;
        let mut level = vec![ty];
        let mut seen = HashSet::from([ty]);
        for _ in 0..MAX_SUPERTYPE_DEPTH {
            if level.iter().any(|&t| !ix.members_named(t, name, Want::Field).is_empty()) {
                return true;
            }
            if level.is_empty() || level.iter().any(|&t| !ix.members_named(t, name, Want::Callable).is_empty()) {
                return false;
            }
            level = level
                .iter()
                .flat_map(|&t| hierarchy.supertypes_of(t).iter().copied())
                .filter(|s| seen.insert(*s))
                .collect();
        }
        false
    }

    /// The one in-repository type a proven receiver's type path names (S-588):
    /// the module tree's rungs ([`resolve_path`](super::scope_hierarchy::ScopeHierarchy::resolve_path)) for a type
    /// ([`Want::Type`]), except that a type the caller's own module declares
    /// is read first ([`own_module_type`](ProvenReceivers::own_module_type)), a name the
    /// file imports twice (a top-level and an inline module's `use`) is
    /// [`Res::Ambiguous`], and a head the file imports by `use` is read
    /// through that import **alone**. Rust's explicit `use` shadows a glob and
    /// the prelude, so `use std::fmt::Error` names std's `Error` even where a
    /// glob or the crate root brings a crate `Error` into view — and an import
    /// that reaches no in-repository declaration names an external type.
    ///
    /// A path whose module re-exports the name (`use logos_core::Engine`, with
    /// `pub use engine::Engine;` in that crate's root; `crate::Runtime`) is
    /// followed through the re-export ([`reexported_type`](super::rust_globs::RustGlobs::reexported_type)):
    /// it names the declaration the re-export names, which the module walk
    /// alone cannot see. So is a name a glob of an **ancestor** module brings
    /// in from that module's own imports (`use super::*` in a test module whose
    /// parent imports `Runtime`), exactly one across the globs; a glob of any
    /// other module brings in only its `pub` imports, which the graph cannot
    /// tell apart, so none is read.
    pub(super) fn resolve_receiver_type(&self, segs: &[String]) -> Res {
        let scope_hierarchy = ScopeHierarchy { cx: self.cx };
        let rust_globs = RustGlobs { cx: self.cx };
        let head = segs[0].as_str();
        if let [name] = segs {
            match self.own_module_type(name) {
                Res::NotFound => {}
                decided => return decided,
            }
        }
        // One file holds one import scope, its inline modules' `use`s
        // included: two imports of the name may belong to two modules, and
        // which is the caller's is unknown.
        if self
            .scope()
            .and_then(|s| s.alias_expansions.get(head))
            .is_some_and(|all| all.len() > 1)
        {
            return Res::Ambiguous;
        }
        let imported = (!matches!(head, "crate" | "self" | "super"))
            .then(|| self.scope().and_then(|s| s.aliases.get(head)))
            .flatten();
        let path: Vec<String> = match imported {
            Some(alias_path) => alias_path.iter().chain(&segs[1..]).cloned().collect(),
            None => segs.to_vec(),
        };
        let depth = if imported.is_some() { MAX_ALIAS_DEPTH - 1 } else { MAX_ALIAS_DEPTH };
        match scope_hierarchy.resolve_path(&path, Want::Type, depth) {
            Res::NotFound => {}
            decided => return decided,
        }
        let Some((name, prefix)) = path.split_last() else {
            return Res::NotFound;
        };
        if !prefix.is_empty() {
            return match self.module_key_of(prefix, depth) {
                Some(key) => rust_globs.reexported_type(&key, name, MAX_ALIAS_DEPTH),
                None => Res::NotFound,
            };
        }
        // Only an ancestor's glob brings its private imports in: a module's
        // `use` is private to it and its descendants, and the graph does not
        // record which imports are `pub` (a sibling's private `use` of a crate
        // `String` must never shadow the prelude's).
        let globs = self.scope().map_or(&[][..], |s| s.globs.as_slice());
        let own = self.source_module();
        let mut found: Vec<NodeId> = Vec::new();
        for glob in globs {
            let Some(key) = self.module_key_of(glob, depth) else { continue };
            let ancestor = own
                .as_ref()
                .is_some_and(|(krate, mods)| *krate == key.0 && mods.starts_with(&key.1));
            if !ancestor {
                continue;
            }
            match rust_globs.reexported_type(&key, name, MAX_ALIAS_DEPTH) {
                Res::Found(id) => found.push(id),
                Res::Ambiguous => return Res::Ambiguous,
                Res::NotFound => {}
            }
        }
        found.sort();
        found.dedup();
        exactly_one(&found)
    }

    /// A type named `name` declared in the caller's own module (S-588): the
    /// caller's `Contains` chain up to and including its nearest module, never
    /// beyond it. Rust's `use` in an inline module cannot shadow a declaration
    /// of the module enclosing it, and the file's single import scope cannot
    /// say which module a `use` sits in — so a declaration in the caller's own
    /// module is read first, and an enclosing module's is not read here at all.
    fn own_module_type(&self, name: &str) -> Res {
        let ix: &Index = self.ix;
        let mut cursor = Some(self.source);
        while let Some(scope) = cursor {
            match exactly_one(&ix.members_named(scope, name, Want::Type)) {
                Res::NotFound => {}
                decided => return decided,
            }
            if ix.info.get(&scope).is_some_and(|i| i.kind == NodeKind::Module) {
                break;
            }
            cursor = ix.parent.get(&scope).copied();
        }
        Res::NotFound
    }

    /// The module a path names, read from the source (S-588): `crate`, `self`
    /// and `super` alone name the source's crate root, own module and parent,
    /// which a path lookup for a member does not; any longer path is resolved
    /// as a [`Want::Module`].
    pub(super) fn module_key_of(&self, segs: &[String], depth: u8) -> Option<ModKey> {
        let scope_hierarchy = ScopeHierarchy { cx: self.cx };
        let (krate, mods) = self.source_module()?;
        let supers = segs.iter().take_while(|s| s.as_str() == "super").count();
        match segs {
            [head] if head == "crate" => Some((krate, Vec::new())),
            [head] if head == "self" => Some((krate, mods)),
            _ if supers == segs.len() && supers <= mods.len() => {
                Some((krate, mods[..mods.len() - supers].to_vec()))
            }
            _ => match scope_hierarchy.resolve_path(segs, Want::Module, depth) {
                Res::Found(module) => self.ix.modules.module_key.get(&module).cloned(),
                _ => None,
            },
        }
    }
}
