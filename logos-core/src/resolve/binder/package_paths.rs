//! A path from a package-shaped or declared-namespace source (CR-149, S-560,
//! S-595, [FR-RS-13]): its head read in the language's order for a simple type
//! name, then the rest walked from it ([`PackagePaths::resolve_package_path`]);
//! a fully-qualified name ([`PackagePaths::resolve_fqn`]); and the files a
//! namespace wildcard names.
//!
//! [FR-RS-13]: ../../../../docs/specs/requirements/FR-RS-13.md

use std::ops::Deref;

use crate::model::{EdgeKind, NodeId};

use super::supertype_climb::SupertypeClimb;
use super::{exactly_one, is_top_level_term, Ctx, Index, Outcome, Res, Residue, Want, FULLY_QUALIFIED_HEAD};
use super::companions::Companions;
use super::import_roots::ImportRootPaths;
use super::package_names::PackageNames;
use super::wildcards::Wildcards;

/// A path from a package-shaped or declared-namespace source, and a fully-qualified name.
#[derive(Clone, Copy)]
pub(super) struct PackagePaths<'c, 'a> {
    pub(super) cx: &'c Ctx<'a>,
}

impl<'a> Deref for PackagePaths<'_, 'a> {
    type Target = Ctx<'a>;

    fn deref(&self) -> &Ctx<'a> {
        self.cx
    }
}

impl PackagePaths<'_, '_> {
    /// A multi-segment path from a package-shaped source ([CR-149]), its head
    /// read in the language's order for a simple type name (JLS §6.5.5): a
    /// member type in lexical scope (`Inner.Deep` inside `Outer`); else the
    /// file's single-type imports of it — **final**, as in
    /// [`resolve_package_name`](super::package_names::PackageNames::resolve_package_name) (and, as there, a
    /// wildcard before the package where the language reads it first, S-573),
    /// so `Map.Entry`
    /// under `import java.util.Map` never reaches a same-package `Map`. Every
    /// distinct import of the head's name is read
    /// ([`resolve_expansions`](super::import_roots::ImportRootPaths::resolve_expansions), S-599): they bind where
    /// they reach one declaration and are [`Res::Ambiguous`] where they reach
    /// two, never the first import's alone (a rival head met while another's
    /// rivals are being read binds nothing, [`in_rival_expansion`]); else a type
    /// of the source's own package; else, for a language that declares them
    /// (S-595, [FR-RS-45]), the path read under the source's own namespace and
    /// then each namespace enclosing it, nearest first
    /// ([`resolve_fqn_under`](PackagePaths::resolve_fqn_under)) — two types at one level
    /// are [`Res::Ambiguous`], never a pick of an outer level, and a level
    /// whose type has no such member ends that walk but not the rungs below it;
    /// a level that names no type reads its namespace block's wildcards
    /// before the next level out (S-603, as
    /// [`resolve_package_name`](super::package_names::PackageNames::resolve_package_name) does);
    /// else one a file-level
    /// wildcard brings into view; else the whole path read as a fully-qualified
    /// name ([`resolve_fqn`](PackagePaths::resolve_fqn)). The first rung whose head names a
    /// type decides — a simple type name obscures a package of the same
    /// spelling, as the language rules it. A path headed [`FULLY_QUALIFIED_HEAD`]
    /// (a PHP `\App\Foo::bar()`, S-547) takes the last rung alone.
    ///
    /// [`in_rival_expansion`]: Ctx::in_rival_expansion
    /// [CR-149]: ../../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    /// [FR-RS-45]: ../../../../docs/specs/requirements/FR-RS-45.md
    pub(super) fn resolve_package_path(&self, package: &[String], segs: &[String], want: Want, depth: u8) -> Res {
        let import_roots = ImportRootPaths { cx: self.cx };
        let package_names = PackageNames { cx: self.cx };
        let wildcards = Wildcards { cx: self.cx };
        let companions = Companions { cx: self.cx };
        let supertype_climb = SupertypeClimb { cx: self.cx };
        let Some((head, rest)) = segs.split_first() else {
            return Res::NotFound;
        };
        // A path written fully qualified (PHP's `\App\Foo::bar()`, S-547) is read
        // from the global namespace alone, as a type relation's is.
        if head == FULLY_QUALIFIED_HEAD {
            return self.resolve_fqn(rest, want);
        }
        let mut cursor = Some(self.lexical_start.get());
        while let Some(scope) = cursor {
            let mut heads = supertype_climb.member_types(scope, head);
            heads.retain(|&id| !self.is_own_header(id));
            if let Some(decided) = companions.walk_from(&heads, rest, want) {
                return decided;
            }
            cursor = self.ix.parent.get(&scope).copied();
        }
        if let Some(expansions) = self.scope().and_then(|s| s.alias_expansions.get(head)) {
            // A rival head met while another's rivals are being read binds
            // nothing — outcome-preserving for every shape that terminates, as
            // for `through_globs`: such a path never converged under the depth
            // budget before S-599 either. A lone import still reads freely.
            let rival = expansions.len() > 1;
            if rival && self.in_rival_expansion.replace(true) {
                return Res::NotFound;
            }
            let decided = import_roots.resolve_expansions(expansions, rest, want, depth);
            if rival {
                self.in_rival_expansion.set(false);
            }
            if decided == Res::Ambiguous {
                // Two imports of the head's name reach two declarations; a
                // reason an inner walk already recorded is kept.
                self.note(want, || Residue::TypeAmbiguous);
            }
            return decided;
        }
        if self.wildcards_first {
            if let Some(decided) = wildcards.wildcard_head(head, rest, want, None) {
                return decided;
            }
        }
        if let Some(decided) = companions.walk_from(package_names.package_type(package, head), rest, want) {
            return decided;
        }
        // The source's own namespace is the first prefix (`B.Thing` in `A.X` is
        // `A.X.B.Thing`), as in `type_candidates`, then each enclosing one.
        let own = self.enclosing_namespaces.then_some(package);
        for level in own.into_iter().chain(package_names.enclosing_levels(package)) {
            match self.resolve_fqn_under(level, segs, want) {
                None => {}
                // A nearer level that reached a type hides the outer levels,
                // but one whose walk then found no such member supplies
                // nothing: the wildcards and the fully-qualified read decide,
                // as they did before this rung existed — never a guess past
                // the level (NFR-RA-05).
                Some(Res::NotFound) => break,
                Some(decided) => return decided,
            }
            // Then the level's own block's `using`s, before the next level out
            // (S-603).
            if let Some(decided) = wildcards.wildcard_head(head, rest, want, Some(level)) {
                return decided;
            }
        }
        if !self.wildcards_first {
            if let Some(decided) = wildcards.wildcard_head(head, rest, want, None) {
                return decided;
            }
        }
        let resolved = self.resolve_fqn(segs, want);
        if resolved == Res::NotFound {
            // No rung reached a type for the head (a type that was reached
            // recorded its own reason first): the receiver's type is declared
            // by no file here.
            self.note(want, || Residue::ExternalType {
                candidates: self.type_candidates(package, segs),
            });
        }
        resolved
    }

    /// The fully-qualified names the type of a call `segs` (the receiver type
    /// segments, then the member) could be, in the order the source's scope
    /// reads them: the source's own package, each enclosing namespace of it for
    /// a language that declares them (S-595) — each level followed by the
    /// wildcards of its namespace block (S-603) — each non-static wildcard of
    /// the file, then the path as written when it is qualified. Its single-type import never
    /// appears here — an imported head is expanded and resolved as the path
    /// written. A simple name as written would name a type of the default
    /// package, which a file in a named package cannot see (JLS §7.5); a
    /// default-package source reaches it as its own package, first.
    fn type_candidates(&self, package: &[String], segs: &[String]) -> Vec<Vec<String>> {
        let package_names = PackageNames { cx: self.cx };
        let Some((_, ty)) = segs.split_last() else {
            return Vec::new();
        };
        let mut candidates: Vec<Vec<String>> = Vec::new();
        let mut push = |prefix: &[String]| {
            let mut fqn = prefix.to_vec();
            fqn.extend(ty.iter().cloned());
            if !candidates.contains(&fqn) {
                candidates.push(fqn);
            }
        };
        let scope = self.scope();
        for level in std::iter::once(package).chain(package_names.enclosing_levels(package)) {
            push(level);
            for glob in scope.and_then(|s| s.block_globs.get(level)).into_iter().flatten() {
                push(glob);
            }
        }
        for glob in scope.into_iter().flat_map(|s| &s.globs) {
            push(glob);
        }
        if ty.len() > 1 {
            push(&[]);
        }
        candidates
    }

    /// `segs` read as a fully-qualified name ([CR-149]): the **longest** prefix
    /// that names a top-level package-shaped type, then the rest as that type's
    /// nested members — so `a::b::C::m` is member `m` of `a.b.C`, and `a::b::C`
    /// is the class itself, never its file module. Two types under the prefix (a
    /// `src/main` and a `src/test` declaration of one name) are
    /// [`Res::Ambiguous`] ([NFR-RA-05]); a name no in-repository type carries —
    /// the JDK, Spring, Lombok, a generated class — is [`Res::NotFound`].
    ///
    /// [CR-149]: ../../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    pub(super) fn resolve_fqn(&self, segs: &[String], want: Want) -> Res {
        self.resolve_fqn_under(&[], segs, want)
            .unwrap_or(Res::NotFound)
    }

    /// A wildcard's path read as a name ([`resolve_fqn`](PackagePaths::resolve_fqn)
    /// for [`Want::Any`]) that names a type or a namespace (S-831): a top-level
    /// property is neither, so one spelled like the path — `val util` in package
    /// `app`, beside the package `app.util` — is not found, and `import app.util.*`
    /// keeps reading the package's members and files.
    pub(super) fn resolve_glob_path(&self, segs: &[String]) -> Res {
        match self.resolve_fqn(segs, Want::Any) {
            Res::Found(id) if self.ix.info.get(&id).is_some_and(|i| is_top_level_term(i.kind)) => Res::NotFound,
            res => res,
        }
    }

    /// `segs` read as a name relative to the namespace `prefix` (S-595): the
    /// longest `prefix` + leading-`segs` that names a top-level type of the
    /// source's family — at least the head of `segs` — then the rest as that
    /// type's nested members, the walk [`resolve_fqn`](PackagePaths::resolve_fqn) takes
    /// (which is this with an empty `prefix`). `None` when no such type exists,
    /// so the caller's next rung decides; else the walk's result, where two
    /// types under the prefix are [`Res::Ambiguous`].
    fn resolve_fqn_under(&self, prefix: &[String], segs: &[String], want: Want) -> Option<Res> {
        let package_names = PackageNames { cx: self.cx };
        let companions = Companions { cx: self.cx };
        let family_types = package_names.family_types();
        let mut fqn = prefix.to_vec();
        fqn.extend_from_slice(segs);
        for split_at in (prefix.len() + 1..=fqn.len()).rev() {
            if let Some(types) = family_types.and_then(|types| types.get(&fqn[..split_at])) {
                return companions.walk_from(types, &fqn[split_at..], want);
            }
            // The whole name may be a top-level function (S-573) — read after
            // a type of the name, which an import of it names first.
            let callables = self.functions_before_properties(
                self.applicable(package_names.package_callables(&fqn[..split_at], want)),
            );
            if split_at == fqn.len() && !callables.is_empty() {
                return Some(exactly_one(&callables));
            }
            // Or another language's file facade (S-560): Java's `a.b.UtilsKt`.
            if let Some(decided) = self.walk_facade(&fqn[..split_at], &fqn[split_at..], want) {
                return Some(decided);
            }
        }
        None
    }

    /// The functions among `found`, or all of it when none is a function
    /// (S-831): a top-level property and a function may share a name, and an
    /// import of the name has always reached the function — the property is read
    /// only where no function of the name is declared, as a type is read before
    /// either.
    fn functions_before_properties(&self, found: Vec<NodeId>) -> Vec<NodeId> {
        let callable = |id: &NodeId| self.ix.info.get(id).is_some_and(|i| Want::Callable.admits(i.kind));
        if found.iter().any(callable) {
            found.into_iter().filter(callable).collect()
        } else {
            found
        }
    }

    /// `rest` read from the file facade `facade` names (S-560, [FR-RS-31];
    /// [`PackageIndex::facades_by_fqn`](super::package_index::PackageIndex::facades_by_fqn)) — `None` when it names none of the source's
    /// family, or the source's language declares facades itself
    /// ([`reads_facades`](Ctx::reads_facades)), so the caller's next rung
    /// decides. The facade alone is its file's module (`import a.b.UtilsKt`);
    /// one segment more is among the file's top-level functions, exactly one
    /// of which the call's arguments admit — an extension's receiver is its
    /// first argument there, so it takes one more than the extension declares.
    /// Functions of the name that admit none are [`Residue::NoApplicableOverload`],
    /// as on every arity-filtered arm. A facade holds no type: a longer path
    /// reaches nothing. Two files of one facade class are [`Res::Ambiguous`].
    ///
    /// [FR-RS-31]: ../../../../docs/specs/requirements/FR-RS-31.md
    fn walk_facade(&self, facade: &[String], rest: &[String], want: Want) -> Option<Res> {
        let ix: &Index = self.ix;
        if !self.reads_facades {
            return None;
        }
        let files = ix.packages.facades_by_fqn.get(self.source_family.as_deref()?)?.get(facade)?;
        let file = match exactly_one(files) {
            Res::Found(file) => file,
            decided => {
                self.note(want, || Residue::TypeAmbiguous);
                return Some(decided);
            }
        };
        let mut inapplicable = false;
        let decided = match rest {
            [] if ix.info.get(&file).is_some_and(|i| want.admits(i.kind)) => Res::Found(file),
            [name] => {
                let mut found = ix.members_named(file, name, Want::Callable);
                let named = !found.is_empty();
                if let Some(count) = self.arg_count {
                    found.retain(|&id| {
                        let declared = if ix.facts.self_types.contains_key(&id) { count.checked_sub(1) } else { Some(count) };
                        declared.is_some_and(|c| self.admits_count(id, c))
                    });
                }
                inapplicable = named && found.is_empty();
                exactly_one(&found)
            }
            _ => Res::NotFound,
        };
        match decided {
            Res::Found(_) => {}
            Res::Ambiguous => self.note(want, || Residue::OverloadAmbiguous),
            Res::NotFound if inapplicable => self.note(want, || Residue::NoApplicableOverload),
            Res::NotFound => self.note(want, || Residue::SupertypeUnreached),
        }
        Some(decided)
    }

    /// The files a declared-namespace source's namespace wildcard names (S-518,
    /// [FR-RS-13]): every **other** file declaring the namespace `segs`
    /// ([`PackageIndex::files_by_namespace`](super::package_index::PackageIndex::files_by_namespace)), as one [`Outcome::BoundMany`] — `None`
    /// when the source is not a declared-namespace file, or no other file
    /// declares it (a framework namespace, `System`, stays unbound,
    /// [NFR-RA-05]).
    ///
    /// [FR-RS-13]: ../../../../docs/specs/requirements/FR-RS-13.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    pub(super) fn namespace_files(&self, segs: &[String]) -> Option<Outcome> {
        let source_file = self
            .ix
            .info
            .get(&self.source)
            .and_then(|i| i.file_path.as_deref())?;
        if !self.ix.layout.declares_namespaces(source_file) {
            return None;
        }
        let own_file = self.ix.modules.by_file_path.get(source_file);
        let targets: Vec<NodeId> = self
            .ix
            .packages.files_by_namespace
            .get(self.source_family.as_deref()?)?
            .get(segs)?
            .iter()
            .copied()
            .filter(|id| !own_file.is_some_and(|own| own.contains(id)))
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
