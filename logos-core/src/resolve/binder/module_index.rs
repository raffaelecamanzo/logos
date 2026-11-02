//! The module and file lookup of the binder's index (S-828, [CR-258]): the
//! path-derived module tree a path descends, the files and file roots a
//! specifier, an include or a doc path names, the route templates an API
//! operation binds against, each file's import scope, and the path-specifier
//! context and imported scope of the path-grammar languages ([`ModuleIndex`]). A sub-index the [`Index`] owns, with its fields; the
//! [`IndexBuilder`](super::IndexBuilder) fills it.
//!
//! [CR-258]: ../../../../docs/requests/CR-258-the-binders-ctx-and-index-are-split-by-concern.md

use std::collections::{HashMap, HashSet};

use crate::graph_store::UnresolvedRefRow;
use crate::model::{EdgeKind, NodeId, RefForm};

use super::super::go_module::GoModule;
use super::super::load_path::LoadPath;
use super::super::rel_path::{claim_key_of, extension_of, parent_of};
use super::super::ts_config::TsResolution;
use super::{
    exactly_one, global_wildcard_governs, split, FileScope, Index, ModKey, NodeInfo, PackageLayout, Res, Want,
    COMMONJS_EXPORTS, DEFAULT_EXPORT, GLOBAL_WILDCARD_ALIAS,
};

/// The reads one [`ModuleIndex::exported_definitions`](super::module_index::ModuleIndex::exported_definitions) walk has made: per
/// `(module, name)`, the most hops it was read with and what it found.
pub(super) type ExportReads = HashMap<(NodeId, String), (u8, Vec<NodeId>)>;

/// What one TypeScript-family module exports beyond its own declarations
/// (S-534, [FR-RS-19]), from its `Imports` rows marked exported
/// (`extract::ts_export_rows`). The specifiers are the rows' module targets,
/// whose file roots the module's own entry in [`ModuleIndex::imported`](super::module_index::ModuleIndex::imported) holds.
///
/// [FR-RS-19]: ../../../../docs/specs/requirements/FR-RS-19.md
#[derive(Debug, Default)]
pub(super) struct ModuleExports {
    /// The module's file id, which keys its specifiers' bindings.
    pub(super) file_id: i64,
    /// Exported name → the local names it exports (`export default App`,
    /// `export { a as b }`): one for valid code, and two decide nothing.
    pub(super) local: HashMap<String, Vec<String>>,
    /// Exported name → `(specifier, name it is exported from there)` of each
    /// `export { a as b } from 's'`.
    pub(super) named: HashMap<String, Vec<(String, String)>>,
    /// The specifier of each `export * from 's'`, every name but `default`.
    pub(super) star: Vec<String>,
}

/// The module tree, the files and their scopes, the route templates, and the
/// path-specifier context and imported scope, as the builder fills them.
#[derive(Default)]
pub(crate) struct ModuleIndex {
    /// Module tree: `(crate, path)` → module node (file modules from their
    /// paths, inline `mod`s appended beneath them). A key a bodyless `mod x;`
    /// claims is answered by the one file module it declares (S-585,
    /// `hand_declarations_to_their_files`).
    pub(super) by_key: HashMap<ModKey, NodeId>,
    /// The **directory modules** of the import-root languages (S-519,
    /// [FR-RS-14]): every directory between a file's import root and the file
    /// ([`PackageLayout::directory_modules`]). A path descends through one as
    /// through a module, whether or not a package file (`__init__.py`) gives it
    /// a node — a namespace package is a package too — but a directory with no
    /// node is never itself a binding target. Empty for every other language.
    ///
    /// [FR-RS-14]: ../../../../docs/specs/requirements/FR-RS-14.md
    pub(super) dir_modules: HashSet<ModKey>,
    /// The reverse of [`by_key`](ModuleIndex::by_key), for "what module am I
    /// in" walks.
    pub(super) module_key: HashMap<NodeId, ModKey>,
    /// Normalised crate names present in the graph.
    pub(super) crates: HashSet<String>,
    /// file path → every node defined in that file, sorted by `NodeId` — the
    /// universe a doc link/path reference resolves against (S-035).
    pub(super) by_file_path: HashMap<String, Vec<NodeId>>,
    /// file path with its extension removed → the **file-root** module nodes at
    /// that stem, id-sorted — the universe a relative path specifier binds
    /// against (S-439), so `./nav.ts` (recorded `.::nav`) and `./nav` reach
    /// `nav.ts` alike and a `nav.ts` beside a `nav.tsx` is an ambiguity, never a
    /// pick ([NFR-RA-05]).
    ///
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    pub(super) file_roots_by_stem: HashMap<String, Vec<NodeId>>,
    /// directory → the `(file path, file-root module)` pairs directly in it,
    /// id-sorted — the package a Go import path names (S-439), and the files a
    /// Go bare name reads the ones of its own `package` clause among (S-528,
    /// [`ScopeHierarchy::directory_package_name`](super::scope_hierarchy::ScopeHierarchy::directory_package_name)).
    pub(super) file_roots_by_dir: HashMap<String, Vec<(String, NodeId)>>,
    /// Normalized template → the `(METHOD, node)` pairs of every
    /// [`NodeKind::Route`](crate::model::NodeKind::Route) carrying it, sorted by `NodeId` — the universe an
    /// OpenAPI `ApiOperation`→route reference resolves against (S-069,
    /// [FR-CG-09]). A route whose template does not normalize cleanly
    /// (catch-all, regex) is **absent** from this map and so is never a
    /// candidate — honestly unresolved, never approximately matched
    /// ([NFR-RA-05]).
    ///
    /// Keyed on the **template alone**, not on `(method, template)`: a wildcard
    /// (`ANY`) provider serves every verb, and tuple equality cannot express
    /// that, so the method is resolved *inside* the bucket by the shared
    /// [`route_method`](super::super::route_method) rule ([CR-109], [ADR-52]).
    ///
    /// [CR-109]: ../../../../docs/requests/CR-109-wildcard-method-route-matching.md
    /// [FR-CG-09]: ../../../../docs/specs/requirements/FR-CG-09.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    /// [ADR-52]: ../../../../docs/specs/architecture/decisions/ADR-52.md
    pub(super) routes_by_template: HashMap<String, Vec<(String, NodeId)>>,
    /// project-relative path → file id, for the files whose own ledger rows
    /// name a source node of the snapshot — how a node reaches its file's
    /// [`FileScope`] (S-493: whether its file imports its self type's name;
    /// S-588: what a module re-exports). A capture-before-delete row is filed
    /// under another file than its source's, so it is never read here.
    pub(super) file_ids: HashMap<String, i64>,
    /// file id → scope facts from its import rows.
    pub(super) file_scopes: HashMap<i64, FileScope>,
    /// The extensions of the files whose import specifiers are **paths**, each
    /// mapped to the extensions a relative specifier in such a file may resolve
    /// to (S-439; [`LanguageRegistry::specifier_target_extensions`]). Empty
    /// unless the run was given the registry ([`IndexBuilder::with_path_specifiers`](super::IndexBuilder::with_path_specifiers));
    /// empty leaves every non-relative import on the path it took before, and
    /// binds no relative one.
    ///
    /// [`LanguageRegistry::specifier_target_extensions`]: crate::plugin::LanguageRegistry::specifier_target_extensions
    pub(super) specifier_targets: HashMap<String, HashSet<String>>,
    /// The Go modules the tree declares (S-439, [`super::super::go_module`]). Empty unless the run was given the tree root
    /// ([`IndexBuilder::with_path_specifiers`](super::IndexBuilder::with_path_specifiers)).
    pub(super) go_modules: Vec<GoModule>,
    /// The `tsconfig.json`/`jsconfig.json` and workspace packages a bare
    /// TypeScript/JavaScript specifier binds through (S-531,
    /// [`super::super::ts_config`]). Empty unless the run was given the tree root
    /// ([`IndexBuilder::with_ts_resolution`](super::IndexBuilder::with_ts_resolution)).
    pub(super) ts: TsResolution,
    /// The load roots a bare Ruby `require` binds against (S-566,
    /// [`super::super::load_path`]). Empty unless the run was given the tree root
    /// ([`IndexBuilder::with_load_path`](super::IndexBuilder::with_load_path)).
    pub(super) load_path: LoadPath,
    /// file id → specifier target of each of the file's path-grammar `Imports`
    /// rows → the file-root modules that row binds to, id-sorted (empty when it
    /// binds nothing) — the **imported** scope a path-grammar call binds within
    /// (S-440, [`IndexBuilder::with_imported_bindings`](super::IndexBuilder::with_imported_bindings)). Empty until that is called.
    pub(super) imported: HashMap<i64, HashMap<String, Vec<NodeId>>>,
    /// file-root module → what the module exports under another name or
    /// re-exports (S-534, [FR-RS-19]): read off its exported path-grammar
    /// `Imports` rows ([`IndexBuilder::with_imported_bindings`](super::IndexBuilder::with_imported_bindings)), and followed by the
    /// imported rung ([`ModuleIndex::exported_definitions`](super::module_index::ModuleIndex::exported_definitions)). Empty until that is
    /// called, and for a module that only exports its own declarations.
    ///
    /// [FR-RS-19]: ../../../../docs/specs/requirements/FR-RS-19.md
    pub(super) exports: HashMap<NodeId, ModuleExports>,
}

impl ModuleIndex {
    /// Bring each global namespace wildcard ([`GLOBAL_WILDCARD_ALIAS`], S-518)
    /// into view in every package-shaped file of its declaring file's extension
    /// under the declaring file's directory — appended to that file's
    /// [`FileScope::globs`] after its own, in ledger order, so the scope a row
    /// reads is deterministic ([NFR-RA-06]). A file with no ledger row has no
    /// row to bind and is not visited; one with rows but no import of its own
    /// gains a scope here.
    ///
    /// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
    pub(super) fn apply_global_globs(
        &mut self,
        refs: &[UnresolvedRefRow],
        by_symbol: &HashMap<String, NodeId>,
        info: &HashMap<NodeId, NodeInfo>,
        layout: &PackageLayout,
    ) {
        let declared: Vec<(String, String, Vec<String>)> = refs
            .iter()
            .filter(|r| {
                r.kind == EdgeKind::Imports
                    && r.form == RefForm::Glob
                    && r.alias.as_deref() == Some(GLOBAL_WILDCARD_ALIAS)
            })
            .filter_map(|r| {
                let path = by_symbol
                    .get(&r.source_symbol)
                    .and_then(|id| info.get(id))
                    .and_then(|i| i.file_path.as_deref())?;
                let dir = path.rsplit_once('/').map_or("", |(dir, _)| dir).to_string();
                Some((dir, extension_of(path), split(&r.target)))
            })
            .collect();
        if declared.is_empty() {
            return;
        }
        for (path, file_id) in &self.file_ids {
            if !layout.is_package_shaped(path) {
                continue;
            }
            let ext = extension_of(path);
            for (dir, glob_ext, glob) in &declared {
                if !global_wildcard_governs(dir, path) || *glob_ext != ext {
                    continue;
                }
                // A file importing nothing of its own has no scope yet.
                let scope = self.file_scopes.entry(*file_id).or_default();
                if !scope.globs.contains(glob) {
                    scope.globs.push(glob.clone());
                }
            }
        }
    }

    /// Whether the file of `node` imports `name` from outside its crate — a
    /// `use` whose path is not headed by `crate`, `self`, `super` or one of the
    /// crate's own top-level modules (S-493). Such a self type names a foreign
    /// type, whatever type of that name the crate declares. A name brought in
    /// by a glob is not seen; the crate's own declarations still decide it.
    pub(super) fn imports_foreign(&self, node: &NodeInfo, name: &str) -> bool {
        let Some(path) = node
            .file_path
            .as_deref()
            .and_then(|p| self.file_ids.get(p))
            .and_then(|id| self.file_scopes.get(id))
            .and_then(|scope| scope.aliases.get(name))
        else {
            return false;
        };
        let Some(head) = path.first() else { return false };
        let local = matches!(head.as_str(), "crate" | "self" | "super")
            || self
                .by_key
                .contains_key(&(node.crate_name.clone(), vec![head.clone()]));
        !local
    }

    /// The file root of the file at the project-relative `path`, matched whole
    /// — extension included ([`file_roots_by_dir`](super::module_index::ModuleIndex::file_roots_by_dir)):
    /// the file an include names (S-579's `#include`, S-551's PHP `require`).
    pub(super) fn file_root_at(&self, path: &str) -> Res {
        let candidates: Vec<NodeId> = self
            .file_roots_by_dir
            .get(parent_of(path))
            .into_iter()
            .flatten()
            .filter(|(file, _)| file == path)
            .map(|&(_, id)| id)
            .collect();
        exactly_one(&candidates)
    }

    /// Whether a file at `path` writes its import specifiers as paths (S-439) —
    /// the incremental run re-binds every import such a file records
    /// (`resolve::is_affected`).
    pub(crate) fn is_path_specifier_file(&self, path: &str) -> bool {
        self.specifier_targets.contains_key(&claim_key_of(path))
    }

    /// Every definition the module `root` provides under `name` (S-534,
    /// [FR-RS-19]), unsorted and possibly repeated, of any kind — what the
    /// imported rungs read their candidates from ([`ImportedScope::imported_definitions`](super::imported_scope::ImportedScope::imported_definitions)):
    ///
    /// - the module's own top-level declaration of that name — unless the
    ///   module exports something else under it: then the local name it
    ///   exports (`export default App`), or what each `export { a as name }
    ///   from 's'` reaches in `s`, read the same way. The `default` symbol of
    ///   either statement (S-532) is that export, never its rival;
    /// - only for a name the module neither exports nor declares itself, what
    ///   each `export * from 's'` reaches — an explicit export shadows a
    ///   star's (ECMAScript) even when it reaches nothing a call can name (an
    ///   external package's, a class), and a star never re-exports `default`.
    ///
    /// Re-exports are followed `hops` deep at most ([`MAX_ALIAS_DEPTH`](super::MAX_ALIAS_DEPTH)).
    /// `seen` holds, per `(module, name)`, the most hops it was read with and
    /// what that read found: a module reached again is answered from it, and
    /// read again only from a route that can reach further, so the answer
    /// never depends on which route — which file name — came first, and a
    /// cycle ends and adds nothing. Which kinds are candidates is the reader's
    /// to say: a value for a call, a type for a type relation (S-535).
    ///
    /// [FR-RS-19]: ../../../../docs/specs/requirements/FR-RS-19.md
    pub(super) fn exported_definitions(
        &self,
        ix: &Index,
        root: NodeId,
        name: &str,
        hops: u8,
        seen: &mut ExportReads,
    ) -> Vec<NodeId> {
        let key = (root, name.to_string());
        if let Some((read, found)) = seen.get(&key) {
            if *read >= hops {
                return found.clone();
            }
        }
        // A cycle back to this read, at fewer hops, reads nothing.
        seen.insert(key.clone(), (hops, Vec::new()));
        let found = self.module_definitions(ix, root, name, hops, seen);
        seen.insert(key, (hops, found.clone()));
        found
    }

    /// One read of [`exported_definitions`](super::module_index::ModuleIndex::exported_definitions): what
    /// `root` provides under `name`, its re-exports followed `hops` deep.
    fn module_definitions(&self, ix: &Index, root: NodeId, name: &str, hops: u8, seen: &mut ExportReads) -> Vec<NodeId> {
        let exports = self.exports.get(&root);
        // An export entry of `name` is what the module exports under it: the
        // `default` symbol of `export default App` or of `export default b`
        // re-exporting an import is the same export, never a rival.
        let own: Vec<&str> = match (
            exports.and_then(|e| e.local.get(name)),
            exports.is_some_and(|e| e.named.contains_key(name)),
        ) {
            (Some(locals), _) => locals.iter().map(String::as_str).collect(),
            (None, true) => Vec::new(),
            (None, false) => vec![name],
        };
        let mut found: Vec<NodeId> = own
            .into_iter()
            .flat_map(|local| ix.members_named(root, local, Want::Any))
            .collect();
        found.extend(self.export_object_definitions(ix, root, exports, name));
        let (Some(exports), true) = (exports, hops > 0) else {
            return found;
        };
        let reached = |spec: &str| -> &[NodeId] {
            self.imported
                .get(&exports.file_id)
                .and_then(|by_target| by_target.get(spec))
                .map_or(&[], Vec::as_slice)
        };
        for (spec, from) in exports.named.get(name).into_iter().flatten() {
            for &next in reached(spec) {
                found.extend(self.exported_definitions(ix, next, from, hops - 1, seen));
            }
        }
        // A star is read only for a name the module neither exports nor
        // declares itself, whatever that export or declaration turned out to
        // be: an external package's, a class, or one this call already read.
        let explicit = exports.local.contains_key(name)
            || exports.named.contains_key(name)
            || !ix.members_named(root, name, Want::Any).is_empty();
        if !explicit && name != DEFAULT_EXPORT {
            for spec in &exports.star {
                for &next in reached(spec) {
                    found.extend(self.exported_definitions(ix, next, name, hops - 1, seen));
                }
            }
        }
        found
    }

    /// What a CommonJS module's export object provides under `name` (S-540,
    /// [FR-RS-22]): each declaration `root` records as its export object
    /// ([`COMMONJS_EXPORTS`]) for `default`, and its members named `name` for
    /// any other name — `exists` of `const User = module.exports; User.exists
    /// = …` is the module's `exists`. Empty for a module that records none,
    /// every TypeScript one included, and for one that records two: which
    /// object `module.exports` holds when it is required is then not one the
    /// tree names ([NFR-RA-05]).
    ///
    /// [FR-RS-22]: ../../../../docs/specs/requirements/FR-RS-22.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    fn export_object_definitions(
        &self,
        ix: &Index,
        root: NodeId,
        exports: Option<&ModuleExports>,
        name: &str,
    ) -> Vec<NodeId> {
        let Some([object]) = exports.and_then(|e| e.local.get(COMMONJS_EXPORTS)).map(Vec::as_slice) else {
            return Vec::new();
        };
        let objects = ix.members_named(root, object, Want::Any).into_iter();
        match name {
            DEFAULT_EXPORT => objects.collect(),
            _ => objects.flat_map(|object| ix.members_named(object, name, Want::Any)).collect(),
        }
    }
}
