//! What a package-shaped file's wildcard imports bring into view (CR-149,
//! S-573): a type's members or a package's types under one name
//! ([`Wildcards::glob_members`]), read by the bare-name and the path rungs.

use std::ops::Deref;

use crate::model::NodeId;

use super::{exactly_one, is_type_like, Ctx, Index, Res, Residue, Want};
use super::companions::Companions;
use super::package_names::PackageNames;
use super::package_paths::PackagePaths;

/// What a package-shaped file's wildcard imports bring into view.
#[derive(Clone, Copy)]
pub(super) struct Wildcards<'c, 'a> {
    pub(super) cx: &'c Ctx<'a>,
}

impl<'a> Deref for Wildcards<'_, 'a> {
    type Target = Ctx<'a>;

    fn deref(&self) -> &Ctx<'a> {
        self.cx
    }
}

impl Wildcards<'_, '_> {
    /// [`resolve_package_name`](super::package_names::PackageNames::resolve_package_name)'s on-demand rung:
    /// the one member the file's wildcards bring into view under `name`
    /// ([`glob_members`](Wildcards::glob_members)) of the written arity
    /// ([`admits_arity`](super::package_names::PackageNames::admits_arity), S-602) — `None` when they bring
    /// none, so the next rung decides. `block` names the namespace whose block's
    /// wildcards are read (S-603) — the file's own when `None`. Two types the
    /// `using`s of one block supply are [`Residue::TypeAmbiguous`], as two types
    /// of one enclosing level are: the walk stops there.
    pub(super) fn wildcard_name(&self, name: &str, want: Want, block: Option<&[String]>) -> Option<Res> {
        let package_names = PackageNames { cx: self.cx };
        let Some(mut found) = self.glob_members(name, want, false, block) else {
            // A wildcard's type is declared twice, and one declaration
            // could supply the name.
            self.note(want, || Residue::TypeAmbiguous);
            return Some(Res::Ambiguous);
        };
        found.retain(|&id| package_names.admits_arity(id));
        match exactly_one(&found) {
            Res::NotFound => None,
            Res::Ambiguous if block.is_some() => {
                self.note(want, || Residue::TypeAmbiguous);
                Some(Res::Ambiguous)
            }
            Res::Ambiguous => {
                // Two wildcards each supply a callable of this name.
                self.note(want, || Residue::OverloadAmbiguous);
                Some(Res::Ambiguous)
            }
            decided => Some(decided),
        }
    }

    /// [`resolve_package_path`](super::package_paths::PackagePaths::resolve_package_path)'s on-demand rung:
    /// `rest` walked from the one type the file's wildcards bring into view
    /// under `head` — `None` when they bring none, so the next rung decides.
    /// `block` names the namespace whose block's wildcards are read (S-603) —
    /// the file's own when `None`.
    pub(super) fn wildcard_head(&self, head: &str, rest: &[String], want: Want, block: Option<&[String]>) -> Option<Res> {
        let companions = Companions { cx: self.cx };
        match self.glob_members(head, Want::Any, true, block) {
            Some(found) => companions.walk_from(&found, rest, want),
            None => {
                self.note(want, || Residue::TypeAmbiguous);
                Some(Res::Ambiguous)
            }
        }
    }

    /// What the file's wildcards bring into view under `name` ([CR-149]):
    /// for a glob naming a type, `C`'s members named `name` — every static
    /// member for `import static a.b.C.*`, member types only for
    /// `import a.b.C.*`; for one naming a package (`import a.b.*`), the
    /// package's top-level types named `name` — and, for a source whose
    /// language declares them, its top-level functions (S-573,
    /// [`package_callables`](super::package_names::PackageNames::package_callables)); for one naming a companion pair
    /// (S-573), the pair's members ([`companion_members`](super::companions::Companions::companion_members)).
    /// `types_only` keeps type-like
    /// candidates alone — a path head must be a type. `block` reads the
    /// wildcards of that namespace's block instead of the file's (S-603,
    /// [`FileScope::block_globs`](super::FileScope::block_globs)). Deduplicated and id-sorted across
    /// every glob, so the caller's exactly-one test is over the union; `None`
    /// when a glob's own type name is ambiguous and one of its declarations
    /// has a member named `name` — an ambiguity no wider rung may overrule
    /// ([NFR-RA-05]). A name none of them declares is not blocked.
    ///
    /// No recursion through the file's globs: a glob's own target is read as a
    /// fully-qualified name only ([`resolve_fqn`](super::package_paths::PackagePaths::resolve_fqn)), so the
    /// CR-016 fan-out [`through_globs`](super::rust_globs::RustGlobs::through_globs) guards against
    /// cannot arise here.
    ///
    /// [CR-149]: ../../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    pub(super) fn glob_members(&self, name: &str, want: Want, types_only: bool, block: Option<&[String]>) -> Option<Vec<NodeId>> {
        let package_names = PackageNames { cx: self.cx };
        let package_paths = PackagePaths { cx: self.cx };
        let companions = Companions { cx: self.cx };
        let ix: &Index = self.ix;
        let Some(scope) = self.scope() else {
            return Some(Vec::new());
        };
        // The file's wildcards, or the non-static ones of one namespace block
        // (S-603, `block`), which a block records no other kind of.
        let (globs, static_globs) = match block {
            None => (scope.globs.as_slice(), scope.static_globs.as_slice()),
            Some(level) => (scope.block_globs.get(level).map_or(&[][..], Vec::as_slice), &[][..]),
        };
        // A non-static wildcard imports types only — a static one, every static
        // member — so a method is only ever reached through `import static`.
        let globs = globs
            .iter()
            .map(|g| (g, true))
            .chain(static_globs.iter().map(|g| (g, types_only)));
        // A package's top-level functions (S-573, S-560) are its members as
        // its types are: any wildcard of the package brings them into view,
        // unless the lookup wants a type (a path head).
        let callables_wanted = !types_only;
        let mut found: Vec<NodeId> = Vec::new();
        for (glob, types_only) in globs {
            let admits = |id: &NodeId| {
                ix.info.get(id).is_some_and(|i| {
                    want.admits(i.kind) && (!types_only || is_type_like(i.kind))
                })
            };
            match package_paths.resolve_glob_path(glob) {
                Res::Found(ty) if ix.info.get(&ty).is_some_and(|i| is_type_like(i.kind)) => {
                    let members = ix.members_named(ty, name, want).into_iter().filter(admits);
                    found.extend(self.applicable(companions.narrow_pair(members.collect(), want)));
                }
                Res::Found(_) => {}
                // Two declarations of the wildcard's type: `name` is ambiguous
                // only if one of them could supply it. A name neither declares
                // — an unrelated import's head, say — never came from this
                // wildcard, so it must not be blocked by it.
                Res::Ambiguous => {
                    // A wildcard of a companion pair (S-573) imports the pair's
                    // members, as one of a type imports that type's.
                    if let Some(pair) = self.glob_pair(glob) {
                        found.extend(companions.companion_members(pair, name, want).into_iter().filter(admits));
                        continue;
                    }
                    let could_supply = package_names
                        .family_types()
                        .and_then(|types| types.get(glob.as_slice()))
                        // `None`: ambiguous below a nested segment — stay conservative.
                        .is_none_or(|types| {
                            types.iter().any(|&ty| {
                                ix.members_named(ty, name, want).iter().any(admits)
                            })
                        });
                    if could_supply {
                        return None;
                    }
                }
                Res::NotFound => {
                    // A companion pair stays whole, whatever kind is wanted
                    // (S-573) — but a supertype names its trait (S-575):
                    // [`admitted_types`](super::companions::Companions::admitted_types) owns that rule.
                    let declared = package_names.package_type(glob, name);
                    found.extend(companions.admitted_types(declared, want).into_iter().filter(|id| {
                        !types_only || ix.info.get(id).is_some_and(|i| is_type_like(i.kind))
                    }));
                    let fqn = [glob.as_slice(), &[name.to_string()]].concat();
                    if callables_wanted {
                        found.extend(self.applicable(package_names.package_callables(&fqn, want)));
                    }
                }
            }
        }
        found.sort();
        found.dedup();
        Some(found)
    }

    /// The companion pair a wildcard's own path names, read as a
    /// fully-qualified name (S-573, [`companion_pair`](super::companions::Companions::companion_pair)).
    fn glob_pair(&self, glob: &[String]) -> Option<[NodeId; 2]> {
        let package_names = PackageNames { cx: self.cx };
        let companions = Companions { cx: self.cx };
        package_names.family_types()
            .and_then(|types| types.get(glob))
            .and_then(|types| companions.companion_pair(types))
    }
}
