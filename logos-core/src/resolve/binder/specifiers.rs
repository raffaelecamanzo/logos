//! A path-grammar module specifier bound to the files it names (S-439, S-531,
//! S-534, S-566, [FR-RS-01]): a relative specifier against the importing
//! file's directory, a bare one through `tsconfig`/workspace packages or the
//! load path, a Go import path to its package ([`Specifiers::resolve_specifier`]).
//!
//! [FR-RS-01]: ../../../../docs/specs/requirements/FR-RS-01.md

use std::collections::HashSet;
use std::ops::Deref;

use crate::extract::refs::is_relative_head;
use crate::graph_store::UnresolvedRefRow;
use crate::model::{EdgeKind, NodeId, NodeKind};

use super::module_index::ModuleIndex;
use super::super::go_module::GoModule;
use super::super::rel_path::{claim_key_of, dir_segments, extension_of, fold_path};
use super::{
    exactly_one, split, strip_target_extension, ts_export_of, Ctx, Index, Outcome, Res, TsExport,
    Want,
};

/// A path-grammar module specifier bound to the files it names.
#[derive(Clone, Copy)]
pub(super) struct Specifiers<'c, 'a> {
    pub(super) cx: &'c Ctx<'a>,
}

impl<'a> Deref for Specifiers<'_, 'a> {
    type Target = Ctx<'a>;

    fn deref(&self) -> &Ctx<'a> {
        self.cx
    }
}

impl Specifiers<'_, '_> {
    /// Bind a path-grammar module specifier (S-439, [CR-142] D1, [FR-RS-01]),
    /// or `None` when `target` is not one — the row then takes the member-path
    /// scope hierarchy exactly as before, which is every import of a
    /// name-grammar language (Rust, Python, Java, …).
    ///
    /// - A **relative** specifier (target headed by `.`/`..`, which only
    ///   `extract::refs::specifier_segments` produces) is always decided here:
    ///   [`resolve_relative_specifier`](Specifiers::resolve_relative_specifier) — or,
    ///   from a name-grammar file, whose only relative rows are its file
    ///   includes (S-551, PHP's `require 'lib/a.php'`), the one file the path
    ///   names whole ([`ModuleIndex::file_root_at`](super::module_index::ModuleIndex::file_root_at)).
    /// - Any other specifier from a path-grammar file is decided here too: a
    ///   load-path language's (Ruby's `require`) against its load roots by
    ///   [`resolve_load_path`](Specifiers::resolve_load_path) (S-566), a Go
    ///   import path under a declared module by
    ///   [`resolve_go_package`](Specifiers::resolve_go_package), a TypeScript-family
    ///   one through its `tsconfig`/`jsconfig` aliases and the workspace
    ///   packages by [`resolve_ts_bare`](Specifiers::resolve_ts_bare) ([FR-RS-02],
    ///   S-531), and everything else — an external package specifier (`react`,
    ///   `next/link`) — **unbound**. A bare specifier those rules do not
    ///   rewrite names a package, never a workspace file, so the member-path
    ///   hierarchy (which would read `react` as a name and bind it to a
    ///   workspace `react.ts`) is never consulted for one ([NFR-RA-05]).
    ///
    /// [CR-142]: ../../../../docs/requests/CR-142-cross-file-call-resolution-is-rust-only.md
    /// [FR-RS-01]: ../../../../docs/specs/requirements/FR-RS-01.md
    /// [FR-RS-02]: ../../../../docs/specs/requirements/FR-RS-02.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    pub(super) fn resolve_specifier(&self, target: &str) -> Option<Outcome> {
        let ix: &Index = self.ix;
        let modules: &ModuleIndex = &ix.modules;
        let segs = split(target);
        let source_file = ix.info.get(&self.source)?.file_path.as_deref()?;
        let targets = ix.modules.specifier_targets.get(&claim_key_of(source_file));
        // A relative import of an import-root language names modules, not a
        // path: the member-path hierarchy reads its level (S-519,
        // [`resolve_relative`](super::import_roots::ImportRootPaths::resolve_relative)).
        if targets.is_none() && ix.layout.has_import_roots(source_file) {
            return None;
        }
        let import = |resolved: Res| match resolved {
            Res::Found(target) => Outcome::Bound {
                source: self.source,
                target,
                kind: EdgeKind::Imports,
                payload: None,
            },
            _ => Outcome::Unbound,
        };
        if segs.first().is_some_and(|h| is_relative_head(h)) {
            return Some(import(match targets {
                Some(targets) => self.resolve_relative_specifier(source_file, &segs, targets),
                // A name-grammar file's include (S-551, PHP's `require`): the
                // one file its path names whole beside the includer.
                None => fold_path(&dir_segments(source_file), &segs.join("/"))
                    .map_or(Res::NotFound, |path| modules.file_root_at(&path)),
            }));
        }
        let targets = targets?;
        // A bare specifier of a load-path language binds against its load
        // roots (S-566, [`resolve_load_path`](Specifiers::resolve_load_path)).
        if let Some(roots) = ix.modules.load_path.roots_for(source_file) {
            return Some(import(self.resolve_load_path(roots, &segs, targets)));
        }
        // Only a Go file can name a Go package: a TypeScript `import 'shared'`
        // beside a `go.mod` declaring `module shared` is a package specifier,
        // never a path into the Go tree.
        if extension_of(source_file) == "go" {
            if let Some(package) = self.resolve_go_package(source_file, &segs) {
                return Some(package);
            }
        }
        Some(import(self.resolve_ts_bare(source_file, &segs, targets)))
    }

    /// An `Imports` row of a path-grammar file marked exported (S-534,
    /// [FR-RS-19], [`ts_export_of`]), or `None` for any other row:
    ///
    /// - a re-export (`export * from 's'`, `export { a as b } from 's'`) binds
    ///   exactly as an import of `s` does ([`resolve_specifier`](Specifiers::resolve_specifier)),
    ///   so the barrel's edge reaches the module it re-exports;
    /// - a local export alias (`export default App`) binds to the one
    ///   top-level declaration of its file it names — an `Imports` edge within
    ///   the file, as a Rust `pub use` of an inline module's item has — a value
    ///   before a type of the same name, and stays unbound on none or several.
    ///
    /// [FR-RS-19]: ../../../../docs/specs/requirements/FR-RS-19.md
    pub(super) fn resolve_export_row(&self, r: &UnresolvedRefRow) -> Option<Outcome> {
        let ix: &Index = self.ix;
        let modules: &ModuleIndex = &ix.modules;
        let source_file = ix.info.get(&self.source)?.file_path.as_deref()?;
        if !modules.is_path_specifier_file(source_file) {
            return None;
        }
        match ts_export_of(r)? {
            TsExport::Named { module, .. } | TsExport::Star { module } => {
                Some(self.resolve_specifier(module).unwrap_or(Outcome::Unbound))
            }
            TsExport::Local { local, .. } => {
                let root = ix.file_root_of(self.source);
                // `type Button = …` beside `const Button = …` is TypeScript's
                // one name in two declaration spaces: the value is the export
                // a call reaches, and a type alone is exported only when no
                // value of the name is declared.
                let declared = ix.members_named(root, local, Want::Any);
                let is_type = |id: &NodeId| {
                    ix
                        .info
                        .get(id)
                        .is_some_and(|i| matches!(i.kind, NodeKind::TypeAlias | NodeKind::Interface))
                };
                let values: Vec<NodeId> = declared.iter().copied().filter(|id| !is_type(id)).collect();
                let candidates = if values.is_empty() { declared } else { values };
                Some(match exactly_one(&candidates) {
                    Res::Found(target) => Outcome::Bound {
                        source: self.source,
                        target,
                        kind: EdgeKind::Imports,
                        payload: None,
                    },
                    _ => Outcome::Unbound,
                })
            }
        }
    }

    /// A bare TypeScript-family specifier, bound through the importing file's
    /// nearest `tsconfig.json`/`jsconfig.json` and the workspace packages
    /// (S-531, [FR-RS-02], [`super::super::ts_config`]). Not found when the importing
    /// file is of no opted-in language.
    ///
    /// Each candidate path the config and packages rewrite the specifier into
    /// is tried in order exactly as a relative specifier's path is
    /// ([`resolve_file_path`](Specifiers::resolve_file_path)); the first that names
    /// anything decides, so an ambiguous one (`ui.ts` beside `ui.tsx`) leaves
    /// the import unbound rather than trying the next ([NFR-RA-05]). A
    /// specifier with no candidate — an external package — is not found.
    ///
    /// [FR-RS-02]: ../../../../docs/specs/requirements/FR-RS-02.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_ts_bare(&self, source_file: &str, segs: &[String], targets: &HashSet<String>) -> Res {
        let candidates = self.ix.modules.ts.candidates(source_file, &segs.join("/"));
        for candidate in candidates.into_iter().flatten() {
            let Some(path) = fold_path(&[], &candidate) else { continue };
            // A candidate spells its file whole (`src/index.ts`, an entry's
            // `dist/index.js`); its stem is what the file roots are keyed by.
            let path = strip_target_extension(path, targets);
            match self.resolve_file_path(&path, targets) {
                Res::NotFound => {}
                decided => return decided,
            }
        }
        Res::NotFound
    }

    /// A relative specifier, resolved against the importing file's directory by
    /// the same fold doc links use ([`fold_path`]), then matched to the one
    /// file-root module whose extension-less path it names, else to that
    /// directory's `index` file — the order a TypeScript/JavaScript resolver
    /// tries them. Exactly-one-or-nothing at each step ([NFR-RA-05]): a
    /// `nav.ts` beside a `nav.tsx` stays unbound, and a specifier escaping the
    /// repository root names nothing.
    ///
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_relative_specifier(
        &self,
        source_file: &str,
        segs: &[String],
        targets: &HashSet<String>,
    ) -> Res {
        let Some(path) = fold_path(&dir_segments(source_file), &segs.join("/")) else {
            return Res::NotFound;
        };
        // A load-path language loads the file its relative specifier names,
        // never a directory's `index` file (S-566): `require_relative 'auth'`
        // is `auth.rb` or nothing.
        if self.ix.modules.load_path.roots_for(source_file).is_some() {
            return self.file_at_stem(&path, targets);
        }
        self.resolve_file_path(&path, targets)
    }

    /// A bare specifier of a load-path language (S-566, [FR-RS-34]), bound to
    /// the one file whose extension-less path under one of the load `roots`
    /// ([`super::super::load_path`]) it names — `require 'rack/lint'` reaching
    /// `lib/rack/lint.rb` under the gem's `lib/`; a trailing `.rb` names the
    /// file itself. Exactly one across every root ([NFR-RA-05]): a specifier two
    /// roots hold a file for stays unbound rather than taking the one the
    /// runtime's load order would reach first, which the tree does not state.
    /// One no root holds a file for — `require 'json'`, an external gem — is
    /// not found.
    ///
    /// [FR-RS-34]: ../../../../docs/specs/requirements/FR-RS-34.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_load_path(&self, roots: &[String], segs: &[String], targets: &HashSet<String>) -> Res {
        let spec = strip_target_extension(segs.join("/"), targets);
        let mut found: Vec<NodeId> = Vec::new();
        for root in roots {
            let base: Vec<&str> = root.split('/').filter(|s| !s.is_empty()).collect();
            let Some(path) = fold_path(&base, &spec) else { continue };
            match self.file_at_stem(&path, targets) {
                Res::Found(target) => found.push(target),
                Res::NotFound => {}
                Res::Ambiguous => return Res::Ambiguous,
            }
        }
        found.sort();
        found.dedup();
        exactly_one(&found)
    }

    /// The one file-root module whose extension-less root-relative `path` it
    /// is, else that directory's `index` file — the order a
    /// TypeScript/JavaScript resolver tries them, shared by a relative
    /// specifier and a `tsconfig`/workspace candidate (S-531).
    /// Exactly-one-or-nothing at each step ([NFR-RA-05]).
    ///
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_file_path(&self, path: &str, targets: &HashSet<String>) -> Res {
        match self.file_at_stem(path, targets) {
            Res::NotFound => {}
            decided => return decided,
        }
        if path.is_empty() {
            self.file_at_stem("index", targets)
        } else {
            self.file_at_stem(&format!("{path}/index"), targets)
        }
    }

    /// The one file-root module at the extension-less root-relative `stem`.
    /// Only a file of an extension the importing language resolves to is a
    /// candidate: a `helper.py` beside `helper.ts` neither answers `./helper`
    /// nor makes it ambiguous.
    fn file_at_stem(&self, stem: &str, targets: &HashSet<String>) -> Res {
        let candidates: Vec<NodeId> = self
            .ix
            .modules.file_roots_by_stem
            .get(stem)
            .into_iter()
            .flatten()
            .copied()
            .filter(|id| {
                self.ix
                    .info
                    .get(id)
                    .and_then(|i| i.file_path.as_deref())
                    .is_some_and(|p| targets.contains(&extension_of(p)))
            })
            .collect();
        exactly_one(&candidates)
    }

    /// A Go import path, bound to the **package** it names — every non-test
    /// `.go` file directly in the package directory, the same one-row fan-out a
    /// Terraform module directory takes ([`resolve_module_dir`](super::artifacts::Artifacts::resolve_module_dir)).
    ///
    /// Consulted only for a `.go` importer. Returns `Some` only for an import
    /// path that falls under a module the tree declares ([`super::super::go_module`])
    /// and names a directory holding at least one such file; every other case
    /// returns `None`, which [`resolve_specifier`](Specifiers::resolve_specifier)
    /// decides **unbound**.
    ///
    /// - The path is matched against the declared module paths on a
    ///   whole-segment prefix, so the dotted host is compared as written, never
    ///   split, and `example.com/shopfront` is not under `example.com/shop`.
    /// - The **longest** matching module path wins — a nested module
    ///   (`example.com/shop/tools`) owns its subtree, as it does for the Go
    ///   toolchain.
    /// - Two `go.mod`s declaring the **same** module path (several
    ///   `examples/*/go.mod` saying `module example`) are told apart only by
    ///   the importer: the one whose root holds it — its nearest — is the
    ///   module it builds in. An importer in neither leaves the import unbound
    ///   rather than picking one ([NFR-RA-05]).
    ///
    /// A path under no declared module is a standard-library or third-party
    /// import (`net/http`, `context`, `github.com/lib/pq`): the module
    /// declaration is the evidence that it is external, and no directory that
    /// merely shares its last segments may stand in for it ([NFR-RA-05]).
    /// Without a `go.mod` there is no evidence where the module path ends, so
    /// nothing is bound on a guess. `_test.go` files are not part of the
    /// package another package imports.
    ///
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_go_package(&self, source_file: &str, segs: &[String]) -> Option<Outcome> {
        let spec = segs.join("/");
        let mut under: Vec<(&GoModule, &str)> = self
            .ix
            .modules.go_modules
            .iter()
            .filter_map(|m| {
                let rest = spec.strip_prefix(m.path.as_str())?;
                match rest.strip_prefix('/') {
                    Some(rest) => Some((m, rest)),
                    None if rest.is_empty() => Some((m, rest)),
                    None => None, // a longer segment: `example.com/shopfront`
                }
            })
            .collect();
        // Under no declared module: external — the caller decides it unbound.
        let longest = under.iter().map(|(m, _)| m.path.len()).max()?;
        under.retain(|(m, _)| m.path.len() == longest);
        if under.len() > 1 {
            let owns = |root: &str| root.is_empty() || source_file.starts_with(&format!("{root}/"));
            let nearest = under
                .iter()
                .filter(|(m, _)| owns(&m.root))
                .map(|(m, _)| m.root.len())
                .max()?;
            under.retain(|(m, _)| owns(&m.root) && m.root.len() == nearest);
        }
        let [(module, rest)] = under[..] else {
            return None;
        };
        let dir = match (module.root.is_empty(), rest.is_empty()) {
            (true, _) => rest.to_string(),
            (false, true) => module.root.clone(),
            (false, false) => format!("{}/{rest}", module.root),
        };
        let targets: Vec<NodeId> = self
            .ix
            .modules.file_roots_by_dir
            .get(&dir)
            .into_iter()
            .flatten()
            .filter(|(path, _)| path.ends_with(".go") && !path.ends_with("_test.go"))
            .map(|&(_, id)| id)
            .collect();
        (!targets.is_empty()).then_some(Outcome::BoundMany {
            source: self.source,
            targets,
            kind: EdgeKind::Imports,
            payload: None,
            dispatch: None,
        })
    }
}
