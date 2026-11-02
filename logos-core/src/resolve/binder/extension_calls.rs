//! A receiver call bound to the one extension function of its name in scope
//! whose receiver type the call's receiver can be (S-560, S-833,
//! [FR-RS-31]), and the top-level callables the scopes a source's language
//! reads offer ([`ExtensionCalls::top_level_callables`]).
//!
//! [FR-RS-31]: ../../../../docs/specs/requirements/FR-RS-31.md

use std::collections::HashSet;
use std::ops::Deref;

use crate::model::{NodeId, NodeKind};

use super::{exactly_one, is_class_like, Ctx, Res, Residue, Want, MAX_SUPERTYPE_DEPTH};
use super::package_names::PackageNames;
use super::receiver_shapes::ReceiverShapes;
use super::wildcards::Wildcards;

/// The receiver an extension call is written on (S-833, [FR-RS-31]), which
/// decides the extensions it can reach
/// ([`resolve_extension_call`](ExtensionCalls::resolve_extension_call)).
///
/// [FR-RS-31]: ../../../../docs/specs/requirements/FR-RS-31.md
#[derive(Clone, Copy)]
pub(super) enum ExtensionReceiver<'p> {
    /// A receiver whose type the file proves ([FR-RS-32]): the type path its
    /// row spells (`LocalDate`, from `LocalDate::format`; empty for a class's
    /// implicit `this`), and the repository type it names, when it names
    /// exactly one.
    ///
    /// [FR-RS-32]: ../../../../docs/specs/requirements/FR-RS-32.md
    Proven { path: &'p [String], ty: Option<NodeId> },
    /// A receiver whose type nothing proves.
    Unproven,
}

/// A receiver call bound to the one extension function in scope.
#[derive(Clone, Copy)]
pub(super) struct ExtensionCalls<'c, 'a> {
    pub(super) cx: &'c Ctx<'a>,
}

impl<'a> Deref for ExtensionCalls<'_, 'a> {
    type Target = Ctx<'a>;

    fn deref(&self) -> &Ctx<'a> {
        self.cx
    }
}

impl ExtensionCalls<'_, '_> {
    /// Bind a receiver call `x.name()` to the one **extension function**
    /// `name` in scope that `x` can be the receiver of (S-560, S-833,
    /// [FR-RS-31]): a top-level function with a recorded self type, its
    /// receiver type ([`DeclarationFacts::self_types`](super::declaration_facts::DeclarationFacts::self_types)) —
    /// Kotlin's `fun String.toSlug()`. Which receiver types apply is the
    /// `receiver`'s to say ([`ExtensionReceiver`]):
    /// - **proven** ([FR-RS-32]): the name of the repository type it names or
    ///   of an in-repository supertype of it
    ///   ([`ancestry_names`](ExtensionCalls::ancestry_names)), or, for a type
    ///   the repository does not declare, the name written —
    ///   `d.format("x")` on a `LocalDate` never reaches `fun
    ///   LocalDateTime.format`, and `s.toSlug()` on a `String` reaches `fun
    ///   String.toSlug()`;
    /// - **unproven**: a class-like type of the repository in the source's
    ///   family ([`family_classes_named`](ExtensionCalls::family_classes_named)),
    ///   the only receiver type whose members the graph sees, so the only one
    ///   for which "no member answers" can be checked — mixit's
    ///   `Base64.getDecoder().decode(s)` never reaches `fun ServerRequest.decode`.
    ///
    /// A nested type never matches by name ([`is_top_level`](ExtensionCalls::is_top_level)):
    /// the receiver an extension records is its type's last segment.
    ///
    /// `None`, so the row stays `no-receiver-evidence` (or the proven type's
    /// miss), when the source's language declares no extension functions
    /// ([`extension_calls`](Ctx::extension_calls)), when no applicable one is
    /// in scope, and when a member of the name could be the one called: a
    /// member wins over an extension, so a callable member `name` of any class
    /// of the source's family ([`member_named`](ExtensionCalls::member_named))
    /// rules the extension out.
    ///
    /// The scopes are read in the language's order, and the first holding an
    /// extension of the name decides:
    /// 1. the file's imports of `name` — final: an import of a library's
    ///    extension (`kotlinx.coroutines.flow.map`) is the one in force, so a
    ///    same-package extension is never reached past it;
    /// 2. the source's own package ([`package_callables`](super::package_names::PackageNames::package_callables));
    /// 3. the file's wildcards ([`glob_members`](super::wildcards::Wildcards::glob_members)).
    ///
    /// Each keeps the extensions the call's written argument count admits
    /// ([`call_args`](Ctx::call_args)) and whose receiver type applies; a scope
    /// holding none that applies is passed over. Two at one scope bind neither
    /// ([`Residue::OverloadAmbiguous`]): two receiver types that both apply, or
    /// one written twice, which the graph cannot tell apart ([NFR-RA-05]).
    ///
    /// [FR-RS-31]: ../../../../docs/specs/requirements/FR-RS-31.md
    /// [FR-RS-32]: ../../../../docs/specs/requirements/FR-RS-32.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    pub(super) fn resolve_extension_call(&self, name: &str, receiver: ExtensionReceiver<'_>) -> Option<Res> {
        if !self.extension_calls || self.member_named(name) {
            return None;
        }
        // A repository type is matched through its ancestry; only a type the
        // repository does not declare (`String`) is matched by its written name.
        let proven: Option<HashSet<&str>> = match receiver {
            ExtensionReceiver::Proven { ty: Some(ty), .. } => Some(self.ancestry_names(ty)),
            ExtensionReceiver::Proven { path, ty: None } => Some(path.last().map(String::as_str).into_iter().collect()),
            ExtensionReceiver::Unproven => None,
        };
        let applies = |ty: &str| match &proven {
            Some(names) => names.contains(ty),
            None => self.family_classes_named(ty).into_iter().any(|c| self.is_top_level(c)),
        };
        // A receiver call carries no filtered count ([`filtered_count`]): its
        // written arguments are read here, as an extension's receiver is not
        // one of its parameters.
        let found = self.top_level_callables(name, |id| {
            self.ix.facts.self_types.get(&id).is_some_and(|ty| applies(ty))
                && self.call_args.is_none_or(|count| self.admits_count(id, count))
        })?;
        match exactly_one(&found) {
            Res::NotFound => None,
            Res::Ambiguous => {
                self.note(Want::Callable, || Residue::OverloadAmbiguous);
                Some(Res::Ambiguous)
            }
            found => Some(found),
        }
    }

    /// Bind a call on a class's own instance — `f()` or `this.f()` inside
    /// `class` — that none of its members, inherited ones included, answers, to
    /// the one extension `name` in scope whose receiver type is `class` or an
    /// in-repository supertype of it (S-833, [FR-RS-31]): the implicit `this`
    /// is a proven `class` ([`resolve_extension_call`](ExtensionCalls::resolve_extension_call)).
    ///
    /// [FR-RS-31]: ../../../../docs/specs/requirements/FR-RS-31.md
    pub(super) fn resolve_implicit_extension_call(&self, class: NodeId, name: &str) -> Option<Res> {
        self.resolve_extension_call(name, ExtensionReceiver::Proven { path: &[], ty: Some(class) })
    }

    /// The top-level callables named `name` that `keep` admits, from the first
    /// scope the source's language reads that holds one (S-560, [FR-RS-31]):
    /// the file's imports of `name` — final, so nothing past an import is
    /// reached — else the source's own package, else its wildcards. Sorted and
    /// deduplicated; `None` for a source of no package.
    ///
    /// [FR-RS-31]: ../../../../docs/specs/requirements/FR-RS-31.md
    fn top_level_callables(&self, name: &str, keep: impl Fn(NodeId) -> bool) -> Option<Vec<NodeId>> {
        let package_names = PackageNames { cx: self.cx };
        let wildcards = Wildcards { cx: self.cx };
        let kept = |found: Vec<NodeId>| {
            let mut found: Vec<NodeId> = found.into_iter().filter(|&id| keep(id)).collect();
            found.sort_unstable();
            found.dedup();
            found
        };
        Some(match self.scope().and_then(|s| s.alias_expansions.get(name)) {
            Some(imports) => kept(
                imports
                    .iter()
                    .flat_map(|path| package_names.package_callables(path, Want::Callable))
                    .collect(),
            ),
            None => {
                let mut fqn = self.source_package.clone()?;
                fqn.push(name.to_string());
                let own = kept(package_names.package_callables(&fqn, Want::Callable));
                if own.is_empty() {
                    kept(wildcards.glob_members(name, Want::Callable, false, None).unwrap_or_default())
                } else {
                    own
                }
            }
        })
    }

    /// Whether a top-level function named `ty` — no extension — is in the
    /// source's scope (S-563): `fun Repo(): Other`, which a call `Repo()`
    /// reaches before the class's constructor, so a receiver `val r = Repo()`
    /// is not proven a `Repo`. A row does not say which proof typed it, so a
    /// receiver declared `Repo` refuses too.
    pub(super) fn factory_named(&self, ty: &str) -> bool {
        self.top_level_callables(ty, |id| {
            !self.ix.facts.self_types.contains_key(&id)
                && self.ix.info.get(&id).is_some_and(|i| matches!(i.kind, NodeKind::Function | NodeKind::Method))
        })
        .is_some_and(|found| !found.is_empty())
    }

    /// Whether a callable named `name` is a member of some class-like
    /// declaration of the source's interop family (S-560) — a member a receiver
    /// call of that name may reach, which an extension of the name never
    /// outranks ([`resolve_extension_call`](ExtensionCalls::resolve_extension_call)).
    pub(super) fn member_named(&self, name: &str) -> bool {
        let family = self.source_family.as_deref();
        self.ix.by_name.get(name).into_iter().flatten().any(|id| {
            let Some(info) = self.ix.info.get(id) else {
                return false;
            };
            Want::Callable.admits(info.kind)
                && self
                    .ix
                    .parent
                    .get(id)
                    .and_then(|p| self.ix.info.get(p))
                    .is_some_and(|p| is_class_like(p.kind))
                && info.file_path.as_deref().and_then(|p| self.ix.layout.family(p)).as_deref() == family
        })
    }

    /// The class-like declarations named `name` in the source's interop family
    /// (S-560, S-833): the repository types an extension's receiver type `name`
    /// may be, whose members the graph sees.
    pub(super) fn family_classes_named(&self, name: &str) -> Vec<NodeId> {
        let family = self.source_family.as_deref();
        self.ix
            .by_name
            .get(name)
            .into_iter()
            .flatten()
            .copied()
            .filter(|ty| {
                self.ix.info.get(ty).is_some_and(|i| {
                    is_class_like(i.kind) && i.file_path.as_deref().and_then(|p| self.ix.layout.family(p)).as_deref() == family
                })
            })
            .collect()
    }

    /// Whether the class the source sits in ([`caller_class`](ReceiverShapes::caller_class))
    /// is a `receiver`, by name, or has an in-repository supertype of that
    /// name (S-833, [FR-RS-31]): a bare call inside class `C` has an implicit
    /// `this` of type `C`, so it may reach `fun C.f()`.
    ///
    /// [FR-RS-31]: ../../../../docs/specs/requirements/FR-RS-31.md
    pub(super) fn caller_class_is(&self, receiver: &str) -> bool {
        let receiver_shapes = ReceiverShapes { cx: self.cx };
        receiver_shapes.caller_class().is_some_and(|class| self.ancestry_names(class).contains(receiver))
    }

    /// The names of `ty` and of every type of its [`ancestry`](ExtensionCalls::ancestry)
    /// that is no member of another type ([`is_top_level`](ExtensionCalls::is_top_level)).
    fn ancestry_names(&self, ty: NodeId) -> HashSet<&str> {
        self.ancestry(ty)
            .into_iter()
            .filter(|&t| self.is_top_level(t))
            .filter_map(|t| self.ix.info.get(&t).map(|i| i.name.as_str()))
            .collect()
    }

    /// Whether type `ty` is declared outside every class-like type (S-833
    /// review). An extension of a nested type records only the type's last
    /// segment — `fun Box.Companion.make()` records `Companion`, `fun
    /// Bar.Builder.build()` records `Builder` — so a nested type's simple name
    /// says nothing of the type around it, and it never matches one: every
    /// companion is named `Companion`.
    fn is_top_level(&self, ty: NodeId) -> bool {
        !self.ix.parent.get(&ty).and_then(|p| self.ix.info.get(p)).is_some_and(|p| is_class_like(p.kind))
    }

    /// `ty` and every in-repository supertype it reaches (S-833): up its
    /// proven `Extends` rows and the interfaces it implements
    /// ([`Hierarchy::interfaces`](super::hierarchy::Hierarchy::interfaces)), each type once,
    /// bounded by [`MAX_SUPERTYPE_DEPTH`] levels. A base the graph does not
    /// hold ends its branch.
    fn ancestry(&self, ty: NodeId) -> Vec<NodeId> {
        let mut seen = HashSet::from([ty]);
        let mut all = vec![ty];
        let mut level = vec![ty];
        for _ in 0..MAX_SUPERTYPE_DEPTH {
            level = level
                .iter()
                .flat_map(|&t| self.ix.hierarchy.supertypes_of(t).iter().chain(self.ix.hierarchy.interfaces_of(t)))
                .copied()
                .filter(|t| seen.insert(*t))
                .collect();
            if level.is_empty() {
                break;
            }
            all.extend(&level);
        }
        all
    }
}
