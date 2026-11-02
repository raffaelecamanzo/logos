//! A call or type relation through an import of a path-grammar file, bound
//! within what that import binds to — never through the member-path
//! hierarchy (S-440, S-535, S-536, [FR-RS-03]; [`ImportedScope::resolve_imported_call`]).
//!
//! [FR-RS-03]: ../../../../docs/specs/requirements/FR-RS-03.md

use std::ops::Deref;

use crate::graph_store::UnresolvedRefRow;
use crate::model::{NodeId, NodeKind};

use super::super::rel_path::extension_of;
use super::{
    exactly_one, is_imported_call_target, Ctx, Index, Res, Want, MAX_ALIAS_DEPTH,
};
use super::module_index::{ExportReads, ModuleIndex};

/// A call or type relation bound within what an import binds to.
#[derive(Clone, Copy)]
pub(super) struct ImportedScope<'c, 'a> {
    pub(super) cx: &'c Ctx<'a>,
}

impl<'a> Deref for ImportedScope<'_, 'a> {
    type Target = Ctx<'a>;

    fn deref(&self) -> &Ctx<'a> {
        self.cx
    }
}

impl ImportedScope<'_, '_> {
    /// The **imported** rung for a path-grammar file (S-440, [CR-142] D2,
    /// [FR-RS-03]), or `None` for a single-segment name or a file whose
    /// specifiers are not paths (every Rust call, so Rust binds exactly as
    /// before).
    ///
    /// `target` is `<import target>::<name>`, the form extraction records for a
    /// call through a named import or through an imported module's qualifier,
    /// or `<import target>::<Class>::<method>` for a member call on a named
    /// import (S-536) (`extract::ImportBindings`). It is the **only** multi-segment call a
    /// path-grammar file records — its `@ref.call` captures are plain
    /// identifiers — so every such row is decided here, `Some`, including one
    /// whose prefix names no import of the file: no candidate, unbound. The
    /// exception is a lexical-constant language (Ruby, S-566): its `Foo::bar`
    /// is a call on a constant, decided by the constant rung before this one
    /// ([`resolve_constant_row`](super::lexical_constants::LexicalConstants::resolve_constant_row)), so it never
    /// arrives here. The candidates are what each file-root module the file's
    /// `Imports` row for `<import target>` **bound** to
    /// ([`IndexBuilder::with_imported_bindings`](super::IndexBuilder::with_imported_bindings)) provides under `name`
    /// ([`ModuleIndex::exported_definitions`](super::module_index::ModuleIndex::exported_definitions), S-534): its own top-level function or
    /// value binding of that name or of the local name it exports under it
    /// (`export default App` for a default import's `default`), else what its
    /// re-exports reach, transitively; exactly one binds ([NFR-RA-05]), so a
    /// cycle or a name two modules provide stays unbound.
    ///
    /// Why this is not the Rust alias step, and its own over-binding reasoning:
    ///
    /// - The evidence is the import the call went through, so a same-named
    ///   function in a module the file did **not** import that name from is
    ///   never a candidate — `helper` imported from `./a` never reaches `./b`'s
    ///   `helper`, and never both.
    /// - An import that binds nothing (an external package: `react`,
    ///   `github.com/lib/pq`), or a module that provides no such definition,
    ///   decides the call **unbound** — `Some`, not `None` — so it never falls
    ///   through to the workspace suffix match, which would bind `pq.Open` to a
    ///   workspace `pq/pq.go`.
    /// - Only a top-level function or, in the TypeScript family, value
    ///   binding is a candidate ([`is_imported_call_target`]): what a package
    ///   or module qualifier reaches is a top-level declaration; a Go package
    ///   qualifier reaches a function only. A method is reached through a
    ///   value, whose type the call does not state, and stays under the
    ///   [FR-RS-06] receiver discipline as a bare method name — except
    ///   through an imported class (S-536, [FR-RS-20]): a TypeScript member
    ///   call on a named import, `<import target>::<Class>::<method>`, names
    ///   its receiver as the row's `alias`, and reads **only** as a member of
    ///   the class its import provides
    ///   ([`imported_class_members`](ImportedScope::imported_class_members)). A bare
    ///   call through `<import target>/<Class>` spells the same target and
    ///   names no alias, so each is decided by its own reading, never the
    ///   other's.
    /// - The [FR-RS-07] tie-break and the [FR-RS-08] trait fan-out are not
    ///   consulted: there is no associated method to break a tie with among
    ///   top-level functions, and no trait object in either language.
    ///
    /// [CR-142]: ../../../../docs/requests/CR-142-cross-file-call-resolution-is-rust-only.md
    /// [FR-RS-03]: ../../../../docs/specs/requirements/FR-RS-03.md
    /// [FR-RS-06]: ../../../../docs/specs/requirements/FR-RS-06.md
    /// [FR-RS-07]: ../../../../docs/specs/requirements/FR-RS-07.md
    /// [FR-RS-08]: ../../../../docs/specs/requirements/FR-RS-08.md
    /// [FR-RS-20]: ../../../../docs/specs/requirements/FR-RS-20.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    pub(super) fn resolve_imported_call(&self, r: &UnresolvedRefRow) -> Option<Res> {
        let ix: &Index = self.ix;
        let modules: &ModuleIndex = &ix.modules;
        let (module, name) = r.target.rsplit_once("::")?;
        let source_file = ix.info.get(&self.source)?.file_path.as_deref()?;
        if !modules.is_path_specifier_file(source_file) {
            return None;
        }
        // A member call on a named import names its receiver (S-536).
        if r.alias.is_some() {
            return Some(self.imported_class_members(module, name));
        }
        // A Go package qualifier reaches a function, as it did before the
        // TypeScript family's value bindings joined the candidates (S-534,
        // [FR-RS-19] is the TypeScript family's).
        let function_only = extension_of(source_file) == "go";
        let candidates = self.imported_definitions(module, name, |kind| {
            is_imported_call_target(kind) && (!function_only || kind == NodeKind::Function)
        });
        Some(exactly_one(&candidates))
    }

    /// The callable `name` of the class `qualified` names, read as
    /// `<import target>::<Class>` (S-536, [FR-RS-20]) — the reading of a
    /// member call on a named import, and its only one: the one `Class` the
    /// file's import of `<import target>` provides under `<Class>`
    /// ([`imported_definitions`](ImportedScope::imported_definitions)) — a named, aliased
    /// or default import (`<import target>::default`) alike — and among its own
    /// members exactly one function or method of that name, static or not, as
    /// a Python class qualifier binds
    /// ([`resolve_module_qualified_call`](super::import_roots::ImportRootPaths::resolve_module_qualified_call)).
    /// A name that reaches two classes, or a class declaring the name twice,
    /// is `Ambiguous`; no class — an external package's, a function, a value —
    /// is `NotFound` ([NFR-RA-05]).
    ///
    /// [FR-RS-20]: ../../../../docs/specs/requirements/FR-RS-20.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    fn imported_class_members(&self, qualified: &str, name: &str) -> Res {
        let ix: &Index = self.ix;
        let Some((module, class)) = qualified.rsplit_once("::") else {
            return Res::NotFound;
        };
        match exactly_one(&self.imported_definitions(module, class, |kind| kind == NodeKind::Class)) {
            Res::Found(class) => exactly_one(&ix.members_named(class, name, Want::Callable)),
            other => other,
        }
    }

    /// The definitions the file's import of `module` provides under `name`
    /// whose kind `admits` accepts, id-sorted and deduplicated: what each file
    /// root that file's `Imports` row for `module` bound to
    /// ([`IndexBuilder::with_imported_bindings`](super::IndexBuilder::with_imported_bindings)) exports under it, re-exports
    /// followed ([`ModuleIndex::exported_definitions`](super::module_index::ModuleIndex::exported_definitions)). Empty when the file holds no
    /// bound import of `module` — an external package's, or none at all. The
    /// candidates of both imported rungs: a call's
    /// ([`resolve_imported_call`](ImportedScope::resolve_imported_call)) and a type
    /// relation's ([`resolve_imported_type`](ImportedScope::resolve_imported_type)).
    fn imported_definitions(&self, module: &str, name: &str, admits: impl Fn(NodeKind) -> bool) -> Vec<NodeId> {
        let ix: &Index = self.ix;
        let roots = self
            .file_id
            .and_then(|f| ix.modules.imported.get(&f))
            .and_then(|by_target| by_target.get(module));
        let modules: &ModuleIndex = &ix.modules;
        let mut seen = ExportReads::new();
        let mut candidates: Vec<NodeId> = roots
            .into_iter()
            .flatten()
            .flat_map(|&root| modules.exported_definitions(ix, root, name, MAX_ALIAS_DEPTH, &mut seen))
            .filter(|id| ix.info.get(id).is_some_and(|i| admits(i.kind)))
            .collect();
        candidates.sort_unstable();
        candidates.dedup();
        candidates
    }

    /// The **imported** rung of a type relation in a language of structural
    /// types (S-535, [FR-EX-17]; TypeScript), or `None` for a single-segment
    /// target or a file of any other language, whose row takes the scope it
    /// always took.
    ///
    /// Extraction records a type named through a named or default import as
    /// `<import target>::<name>`, as it records a call through one
    /// (`extract::ImportBindings`), and every other multi-segment type it
    /// records — `ns.Dto` through a namespace import, `React.Component` — is
    /// a member of a value, never a path of modules. So every such row is
    /// decided here, `Some`: it binds the one definition of a kind `want`
    /// admits that its import provides under `name`
    /// ([`imported_definitions`](ImportedScope::imported_definitions)) — `interface
    /// User` and not a `const User` beside it — and a prefix that is no bound
    /// import of the file (an external package, an unread namespace) binds
    /// nothing, rather than reaching the member-path hierarchy, which would
    /// read the specifier as names ([NFR-RA-05]).
    ///
    /// [FR-EX-17]: ../../../../docs/specs/requirements/FR-EX-17.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    pub(super) fn resolve_imported_type(&self, target: &str, want: Want) -> Option<Res> {
        let ix: &Index = self.ix;
        let modules: &ModuleIndex = &ix.modules;
        let (module, name) = target.rsplit_once("::")?;
        let source_file = ix.info.get(&self.source)?.file_path.as_deref()?;
        if !modules.is_path_specifier_file(source_file) || !ix.layout.has_structural_types(source_file) {
            return None;
        }
        Some(exactly_one(&self.imported_definitions(module, name, |kind| want.admits(kind))))
    }
}
