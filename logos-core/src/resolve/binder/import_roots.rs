//! The paths an import-root file's imports name (S-519, S-544, [FR-RS-14],
//! [FR-RS-23]): a relative import read from the file's package, a package an
//! import goes through, the rival declarations two imports give one name, and
//! a call through an imported module or class
//! ([`ImportRootPaths::resolve_module_qualified_call`]).
//!
//! [FR-RS-14]: ../../../../docs/specs/requirements/FR-RS-14.md
//! [FR-RS-23]: ../../../../docs/specs/requirements/FR-RS-23.md

use std::ops::Deref;

use crate::extract::refs::is_relative_head;
use crate::graph_store::UnresolvedRefRow;
use crate::model::{EdgeKind, NodeId, NodeKind};

use super::{exactly_one, split, Ctx, Index, Outcome, Res, Want, MAX_ALIAS_DEPTH};
use super::scope_hierarchy::ScopeHierarchy;

/// The paths an import-root file's imports name.
#[derive(Clone, Copy)]
pub(super) struct ImportRootPaths<'c, 'a> {
    pub(super) cx: &'c Ctx<'a>,
}

impl<'a> Deref for ImportRootPaths<'_, 'a> {
    type Target = Ctx<'a>;

    fn deref(&self) -> &Ctx<'a> {
        self.cx
    }
}

impl ImportRootPaths<'_, '_> {
    /// Every path an import-root file's imports give `head`, when they give it
    /// more than one (S-519): `try: from .fast import parse` / `except
    /// ImportError: from .slow import parse` names two declarations, and the
    /// first-wins alias map would pick one ([NFR-RA-05]). `None` for any other
    /// source, or a name whose imports all give one path — imported once, or
    /// the same import repeated, which `build_file_scopes` records once — the
    /// alias map answers those.
    pub(super) fn rival_expansions(&self, head: &str) -> Option<&[Vec<String>]> {
        let source_file = self.ix.info.get(&self.source)?.file_path.as_deref()?;
        if !self.ix.layout.has_import_roots(source_file) {
            return None;
        }
        let all = self.scope()?.alias_expansions.get(head)?;
        (all.len() > 1).then_some(all.as_slice())
    }

    /// `rest` resolved under each of `expansions`, exactly-one across all of
    /// them (two imports of one declaration agree) — sticky on an ambiguity.
    pub(super) fn resolve_expansions(&self, expansions: &[Vec<String>], rest: &[String], want: Want, depth: u8) -> Res {
        let scope_hierarchy = ScopeHierarchy { cx: self.cx };
        let mut found: Vec<NodeId> = Vec::new();
        for expansion in expansions {
            let mut path = expansion.clone();
            path.extend(rest.iter().cloned());
            match scope_hierarchy.resolve_path(&path, want, depth - 1) {
                Res::Found(id) => found.push(id),
                Res::Ambiguous => return Res::Ambiguous,
                Res::NotFound => {}
            }
        }
        found.sort();
        found.dedup();
        exactly_one(&found)
    }

    /// Whether the source is keyed under an import-root family's crate (S-519):
    /// a closed namespace whose paths name no other crate — `import mylib`
    /// from Python is never a Rust crate `mylib`.
    pub(super) fn in_family_crate(&self) -> bool {
        let ix: &Index = self.ix;
        ix
            .nearest_module(self.source)
            .is_some_and(|(krate, _)| ix.layout.is_family_crate(krate))
    }

    /// A path headed by a relative level (`.`, `..`; [`is_relative_head`]) from
    /// a file keyed under import roots (S-519, [FR-RS-14]) — `from .rules
    /// import Rule` recorded `.::rules::Rule`, `from .._internal import x`
    /// recorded `..::_internal::x`. `.` is the source's package
    /// ([`PackageLayout::package_dir`](crate::resolve::package_key::PackageLayout::package_dir)) and each `..` one package up; the rest
    /// descends from there, and nothing follows it (`from . import *`) names
    /// the package itself. A level must leave at least one package: climbing to
    /// the import root itself (`from .. import x` in a top-level package, `from
    /// . import x` in a top-level script) names nothing, as the language
    /// refuses it ("attempted relative import beyond top-level package").
    ///
    /// `None` for any other path or source — a relative specifier of a
    /// path-grammar language is bound by [`resolve_specifier`](super::specifiers::Specifiers::resolve_specifier),
    /// and no other language records a relative head.
    ///
    /// [FR-RS-14]: ../../../../docs/specs/requirements/FR-RS-14.md
    pub(super) fn resolve_relative(&self, segs: &[String], want: Want) -> Option<Res> {
        let scope_hierarchy = ScopeHierarchy { cx: self.cx };
        if !segs.first().is_some_and(|h| is_relative_head(h)) {
            return None;
        }
        let source_file = self.ix.info.get(&self.source)?.file_path.as_deref()?;
        let (krate, package) = self.ix.layout.package_dir(source_file)?;
        let ups = segs.iter().take_while(|s| s.as_str() == "..").count();
        let heads = segs.iter().take_while(|s| is_relative_head(s)).count();
        let rest = &segs[heads..];
        let Some(depth) = package.len().checked_sub(ups).filter(|d| *d > 0) else {
            return Some(Res::NotFound);
        };
        let base = &package[..depth];
        if rest.is_empty() {
            let admitted = |id: &NodeId| self.ix.info.get(id).is_some_and(|i| want.admits(i.kind));
            let module = self.ix.modules.by_key.get(&(krate, base.to_vec())).filter(|id| admitted(id));
            return Some(module.map_or(Res::NotFound, |&id| Res::Found(id)));
        }
        Some(scope_hierarchy.descend(&krate, base, rest, want))
    }

    /// The package an import-root file's `from pkg import Name` goes **through**
    /// when `Name` is nothing `pkg` declares (S-519, [FR-RS-14]): the import
    /// binds to the package's own module — its `__init__.py` — which is where a
    /// package re-exports a name it imports from a submodule
    /// (`from .map import Map`). The edge says what the file provably depends
    /// on, the package; which declaration the re-export finally names is not
    /// followed ([NFR-RA-05]).
    ///
    /// Only a **package file** answers: a name an ordinary module does not
    /// declare (`from hc.api.models import Missing`) stays unbound, and so does
    /// a namespace package with no `__init__.py`, which re-exports nothing. A
    /// package never imports itself. `None` for any other source.
    ///
    /// [FR-RS-14]: ../../../../docs/specs/requirements/FR-RS-14.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    pub(super) fn package_reexport(&self, segs: &[String]) -> Option<NodeId> {
        let scope_hierarchy = ScopeHierarchy { cx: self.cx };
        let ix: &Index = self.ix;
        let source_file = ix.info.get(&self.source)?.file_path.as_deref()?;
        if !ix.layout.has_import_roots(source_file) {
            return None;
        }
        let (_, package) = segs.split_last()?;
        if package.is_empty() {
            return None;
        }
        let Res::Found(module) = scope_hierarchy.resolve_path(package, Want::Module, MAX_ALIAS_DEPTH) else {
            return None;
        };
        let file = ix.info.get(&module)?.file_path.as_deref()?;
        let own = ix.nearest_module(self.source) == ix.modules.module_key.get(&module);
        (ix.layout.is_package_file(file) && !own).then_some(module)
    }

    /// The **qualified-call** rung of an import-root file (S-544, [FR-RS-23]),
    /// or `None` for any other row or source. A class the call binds is
    /// constructed: it records `Instantiates` (S-521).
    ///
    /// Extraction records a Python call through an imported module or class
    /// (`curl.post()` after `from hc.lib import curl`) as the import's path
    /// plus the name, `hc::lib::curl::post`, aliased by the local name it went
    /// through (`extract::ImportBindings`); a framework handler written
    /// `views.add` arrives as `views::add`, unaliased. Its `@ref.call` captures
    /// are plain identifiers, so every multi-segment call such a file records
    /// is one of these, and each is decided here, `Some`:
    ///
    /// - An aliased row's qualifier (all but the last segment) is an import
    ///   path, read as Python reads one ([`resolve_import_path`](ImportRootPaths::resolve_import_path)):
    ///   never through the file's alias table, whose plain `import a.b` is
    ///   keyed `b` where Python binds `a`, nor through a workspace guess.
    /// - An unaliased row's qualifier is written as the file names it: a single
    ///   name the file imports is read through those imports first — a bare name
    ///   in Python names a module only through an import — and any other path
    ///   as the file's own import of it resolves.
    /// - A **module** qualifier binds the one member of the name the call can
    ///   target — a function, or a class the call constructs.
    /// - A **class** qualifier binds the one method of the name the class
    ///   itself declares. The module tree's `Type::func` collapse
    ///   ([`descend`](super::scope_hierarchy::ScopeHierarchy::descend)) is never taken: it would reach a module
    ///   function beside the class.
    /// - Anything else — a qualifier that names nothing here (an external
    ///   `os`), or names a function — is decided **unbound**, and never
    ///   reaches the workspace suffix match, which would read `os::getcwd` as a
    ///   workspace `os.py`'s.
    ///
    /// [FR-RS-23]: ../../../../docs/specs/requirements/FR-RS-23.md
    pub(super) fn resolve_module_qualified_call(&self, r: &UnresolvedRefRow) -> Option<Outcome> {
        let ix: &Index = self.ix;
        if r.kind != EdgeKind::Calls || !r.target.contains("::") {
            return None;
        }
        let source_file = ix.info.get(&self.source)?.file_path.as_deref()?;
        if !ix.layout.has_import_roots(source_file) {
            return None;
        }
        let segs = split(&r.target);
        let (name, qualifier) = segs.split_last()?;
        let scope = match r.alias {
            Some(_) => self.resolve_import_path(qualifier),
            None => self.resolve_written_qualifier(qualifier),
        };
        let want = self.call_want();
        let candidates = match scope {
            Res::Found(scope) => match ix.info.get(&scope).map(|i| i.kind) {
                Some(NodeKind::Module) => ix.members_named(scope, name, want),
                Some(NodeKind::Class) => ix.members_named(scope, name, Want::Callable),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        let Res::Found(target) = exactly_one(&candidates) else {
            return Some(Outcome::Unbound);
        };
        let kind = if self.constructs(want, target) { EdgeKind::Instantiates } else { r.kind };
        Some(Outcome::Bound {
            source: self.source,
            target,
            kind,
            payload: r.payload.clone(),
        })
    }

    /// The module or class an import `path` names, read as Python reads `import`
    /// (S-544): a relative level from the source's package, any other path from
    /// the root of the source's family crate. The last segment is a submodule,
    /// or a name its parent module declares — and when both exist (`from pkg
    /// import sub` beside a `pkg/__init__.py` declaring `sub` and a
    /// `pkg/sub.py`), which one the import gives depends on what else was
    /// imported first, so it names neither ([NFR-RA-05]).
    ///
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_import_path(&self, path: &[String]) -> Res {
        let ix: &Index = self.ix;
        let Some((last, parent)) = path.split_last() else {
            return Res::NotFound;
        };
        let declared = match self.import_module(parent) {
            Res::Found(module) => ix.members_named(module, last, Want::Any),
            _ => Vec::new(),
        };
        match (self.import_module(path), declared.as_slice()) {
            (Res::Found(module), []) => Res::Found(module),
            (Res::NotFound, [one]) => Res::Found(*one),
            _ => Res::NotFound,
        }
    }

    /// The module an import `path` names ([`resolve_import_path`](ImportRootPaths::resolve_import_path)).
    fn import_module(&self, path: &[String]) -> Res {
        let scope_hierarchy = ScopeHierarchy { cx: self.cx };
        if path.first().is_some_and(|h| is_relative_head(h)) {
            return self.resolve_relative(path, Want::Module).unwrap_or(Res::NotFound);
        }
        match self.source_module() {
            Some((krate, _)) if !path.is_empty() => scope_hierarchy.descend(&krate, &[], path, Want::Module),
            _ => Res::NotFound,
        }
    }

    /// The module or class a written `qualifier` names
    /// ([`resolve_module_qualified_call`](ImportRootPaths::resolve_module_qualified_call)):
    /// a single name the file imports through its imports — every one of them,
    /// exactly-one — and any other path as the file's own import of it resolves.
    pub(super) fn resolve_written_qualifier(&self, qualifier: &[String]) -> Res {
        let scope_hierarchy = ScopeHierarchy { cx: self.cx };
        if let [head] = qualifier {
            if let Some(rivals) = self.rival_expansions(head) {
                return self.resolve_expansions(rivals, &[], Want::Any, MAX_ALIAS_DEPTH);
            }
            if let Some(path) = self.scope().and_then(|s| s.aliases.get(head)) {
                return scope_hierarchy.resolve_path(path, Want::Any, MAX_ALIAS_DEPTH - 1);
            }
        }
        scope_hierarchy.resolve_path(qualifier, Want::Any, MAX_ALIAS_DEPTH)
    }
}
