//! A path walked down from a type (S-559, S-573, S-575, [FR-RS-36]), and the
//! companions and companion pairs the walk reads: a type's anonymous companion,
//! a type and its same-named singleton read as one name
//! ([`Companions::walk_from`]).
//!
//! [FR-RS-36]: ../../../../docs/specs/requirements/FR-RS-36.md

use std::collections::HashSet;
use std::ops::Deref;

use crate::model::{NodeId, NodeKind};

use super::supertype_climb::SupertypeClimb;
use super::{exactly_one, is_class_like, is_type_like, Climb, Ctx, Index, Res, Residue, Want};

/// A path walked down from a type, and the companions and companion pairs it reads.
#[derive(Clone, Copy)]
pub(super) struct Companions<'c, 'a> {
    pub(super) cx: &'c Ctx<'a>,
}

impl<'a> Deref for Companions<'_, 'a> {
    type Target = Ctx<'a>;

    fn deref(&self) -> &Ctx<'a> {
        self.cx
    }
}

impl Companions<'_, '_> {
    /// Walk `rest` down from the one type in `candidates`: `None` when there is
    /// none (the caller's next rung decides), else the walk's result — sticky
    /// [`Res::Ambiguous`] for two candidates — unless the two are one companion
    /// pair and `rest` goes on past them (S-573,
    /// [`walk_from_companions`](Companions::walk_from_companions)). Every segment but
    /// the last must name exactly one nested type; the last names a member
    /// admitted by `want`, and an empty `rest` is the type itself.
    ///
    /// A **call** ([`Want::is_call`]) takes its last segment through
    /// [`type_member`](super::supertype_climb::SupertypeClimb::type_member): the type's own callable, else its
    /// in-repository supertypes' (S-468) — unless the type has a companion
    /// (I4-INT, [`companion_climb`](Companions::companion_climb)), which then decides
    /// alone. Every other lookup reads the type's own members only, as before.
    pub(super) fn walk_from(&self, candidates: &[NodeId], rest: &[String], want: Want) -> Option<Res> {
        let supertype_climb = SupertypeClimb { cx: self.cx };
        let ix: &Index = self.ix;
        let mut cursor = match exactly_one(candidates) {
            Res::NotFound => return None,
            Res::Ambiguous => {
                if let Some(pair) = self.companion_pair(candidates).filter(|_| !rest.is_empty()) {
                    return Some(self.walk_from_companions(pair, rest, want));
                }
                // `rest` is empty here: a supertype names the pair's trait.
                if let Some(supertype) = self.companion_pair(candidates).and_then(|pair| self.pair_supertype(pair, want)) {
                    return Some(Res::Found(supertype));
                }
                self.note(want, || Residue::TypeAmbiguous);
                return Some(Res::Ambiguous);
            }
            Res::Found(ty) => ty,
        };
        let Some((last, inner)) = rest.split_last() else {
            let admitted = ix.info.get(&cursor).is_some_and(|i| want.admits(i.kind));
            return Some(if admitted { Res::Found(cursor) } else { Res::NotFound });
        };
        for seg in inner {
            match exactly_one(&supertype_climb.member_types(cursor, seg)) {
                Res::Found(nested) => cursor = nested,
                other => {
                    self.note(want, || match other {
                        Res::Ambiguous => Residue::TypeAmbiguous,
                        // A nested type its in-graph outer type does not
                        // declare: generated (a Lombok builder) or inherited.
                        _ => Residue::ExternalType {
                            candidates: Vec::new(),
                        },
                    });
                    return Some(other);
                }
            }
        }
        if want.is_call() {
            // A type with a companion lends it its name (I4-INT): `C.make()`
            // calls the companion's `make`, never an instance member of `C`,
            // which a call on the class cannot reach.
            if let Some(climb) = self.companion_climb(cursor, last) {
                supertype_climb.note_climb(&climb);
                return Some(climb.res);
            }
            return Some(supertype_climb.type_member(cursor, last));
        }
        Some(exactly_one(&self.narrow_pair(ix.members_named(cursor, last, want), want)))
    }

    /// The **companion** of `ty` (I4-INT, [FR-RS-31]): its one nested type
    /// named what the language of `ty`'s file calls an anonymous companion
    /// object ([`PackageLayout::companion_name`], Kotlin's `Companion`, the
    /// node S-559 nests its members under). `None` when `ty` is no class-like
    /// type ([`is_class_like`]) — a file, a function or a namespace holding a
    /// top-level type of that name lends it no companion — when its language
    /// declares none, or when `ty` has none, or two, which no lookup picks
    /// between.
    ///
    /// [FR-RS-31]: ../../../../docs/specs/requirements/FR-RS-31.md
    /// [`PackageLayout::companion_name`]: crate::resolve::package_key::PackageLayout::companion_name
    pub(super) fn companion_of(&self, ty: NodeId) -> Option<NodeId> {
        let supertype_climb = SupertypeClimb { cx: self.cx };
        let info = self.ix.info.get(&ty).filter(|i| is_class_like(i.kind))?;
        let file = info.file_path.as_deref()?;
        let companion_name = self.ix.layout.companion_name(file)?;
        match exactly_one(&supertype_climb.member_types(ty, companion_name)) {
            Res::Found(companion) => Some(companion),
            _ => None,
        }
    }

    /// The [`supertype_walk`](super::supertype_climb::SupertypeClimb::supertype_walk) for `name` from the
    /// companion of `ty` ([`companion_of`](Companions::companion_of)); `None` when
    /// `ty` has none.
    pub(super) fn companion_climb(&self, ty: NodeId, name: &str) -> Option<Climb> {
        let supertype_climb = SupertypeClimb { cx: self.cx };
        let companion = self.companion_of(ty)?;
        Some(supertype_climb.supertype_walk(vec![companion], HashSet::from([companion]), name))
    }

    /// The two declarations of `candidates` when they are one **companion
    /// pair** (S-573, [FR-RS-36]): a type and the same-named singleton
    /// declared beside it — the same file, the same parent — in a language
    /// that pairs them ([`PackageLayout::pairs_companion_objects`], Scala's
    /// `class Chain` and `object Chain`). Two such declarations of one name are
    /// legal only as a pair. `None` for any other set.
    ///
    /// [FR-RS-36]: ../../../../docs/specs/requirements/FR-RS-36.md
    /// [`PackageLayout::pairs_companion_objects`]: crate::resolve::package_key::PackageLayout::pairs_companion_objects
    pub(super) fn companion_pair(&self, candidates: &[NodeId]) -> Option<[NodeId; 2]> {
        let &[a, b] = candidates else {
            return None;
        };
        let (x, y) = (self.ix.info.get(&a)?, self.ix.info.get(&b)?);
        let file = x.file_path.as_deref()?;
        let paired = is_type_like(x.kind)
            && is_type_like(y.kind)
            && x.name == y.name
            && y.file_path.as_deref() == Some(file)
            && self.ix.parent.get(&a) == self.ix.parent.get(&b)
            && self.ix.layout.pairs_companion_objects(file);
        paired.then_some([a, b])
    }

    /// The types of `types` — one name's declarations in one package — that
    /// `want` admits; or, when they are one companion pair (S-573), both of
    /// them, whatever `want` admits: the name names the pair, and a lookup by
    /// kind must not pick the object out of it (`new T {}` instantiates the
    /// trait, never `object T`). Two candidates are the exactly-one test's
    /// [`Res::Ambiguous`].
    pub(super) fn admitted_types(&self, types: &[NodeId], want: Want) -> Vec<NodeId> {
        if let Some(pair) = self.companion_pair(types) {
            return match self.pair_supertype(pair, want) {
                Some(supertype) => vec![supertype],
                None => types.to_vec(),
            };
        }
        types
            .iter()
            .copied()
            .filter(|id| self.ix.info.get(id).is_some_and(|i| want.admits(i.kind)))
            .collect()
    }

    /// The trait of a companion `pair` that a supertype lookup (`want`
    /// [`Want::Supertype`], Scala's `extends A with B`) names (S-575,
    /// [FR-EX-28]): an `object` is never a supertype, and it is never a trait,
    /// so of a `trait AccountService` and its `object AccountService` the trait
    /// is the one the clause names. `None` for any other lookup, and for a
    /// `class` and its `object`, which are both classes here: nothing in the
    /// graph tells the object out, so that name stays ambiguous ([NFR-RA-05]).
    ///
    /// [FR-EX-28]: ../../../../docs/specs/requirements/FR-EX-28.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    fn pair_supertype(&self, pair: [NodeId; 2], want: Want) -> Option<NodeId> {
        if want != Want::Supertype {
            return None;
        }
        let is_trait = |id: &NodeId| self.ix.info.get(id).is_some_and(|i| i.kind == NodeKind::Trait);
        match pair.iter().filter(|id| is_trait(id)).collect::<Vec<_>>().as_slice() {
            [only] => Some(**only),
            _ => None,
        }
    }

    /// `ids` as a lookup by `want` reads them: a companion pair narrowed to the
    /// trait a supertype names ([`pair_supertype`](Companions::pair_supertype)), any
    /// other set as it is — the one narrowing every rung a supertype row
    /// climbs applies: the lexical scopes, a type's members and a wildcard's.
    pub(super) fn narrow_pair(&self, ids: Vec<NodeId>, want: Want) -> Vec<NodeId> {
        match self.companion_pair(&ids).and_then(|pair| self.pair_supertype(pair, want)) {
            Some(supertype) => vec![supertype],
            None => ids,
        }
    }

    /// The members of a companion pair named `name` that `want` admits (S-573):
    /// what the name reaches through a path. A callable is the singleton's
    /// only — one recorded as taking no instance receiver
    /// ([`DeclarationFacts::without_self`](super::declaration_facts::DeclarationFacts::without_self)): `Chain.empty()` calls the object's `empty`,
    /// never an instance method of the class. On a proven receiver
    /// ([`instance_receiver`](Ctx::instance_receiver), S-576) it is the
    /// class's only — every other one: `chain.size()` on a `Chain` never calls
    /// the object's. Any other member is read from both. Narrowed to the
    /// callables the call's arguments admit.
    pub(super) fn companion_members(&self, pair: [NodeId; 2], name: &str, want: Want) -> Vec<NodeId> {
        let ix: &Index = self.ix;
        let on_instance = self.instance_receiver.get();
        let mut found: Vec<NodeId> = pair
            .iter()
            .flat_map(|&ty| ix.members_named(ty, name, want))
            .filter(|id| {
                ix.info.get(id).is_some_and(|i| {
                    !Want::Callable.admits(i.kind) || ix.facts.without_self.contains(id) != on_instance
                })
            })
            .collect();
        found.sort_unstable();
        found.dedup();
        self.applicable(found)
    }

    /// [`walk_from`](Companions::walk_from) from a companion pair (S-573): the next
    /// segment is read among both declarations' members — a nested type to go
    /// on from, or the last segment's member ([`companion_members`](Companions::companion_members)).
    /// Exactly one decides; the pair's supertypes are not climbed. `rest` is
    /// not empty: the name alone still names two declarations.
    fn walk_from_companions(&self, pair: [NodeId; 2], rest: &[String], want: Want) -> Res {
        let supertype_climb = SupertypeClimb { cx: self.cx };
        let Some((next, after)) = rest.split_first() else {
            return Res::Ambiguous;
        };
        if after.is_empty() {
            let decided = exactly_one(&self.companion_members(pair, next, want));
            // Why it gave up, as `note_climb` records a supertype walk's: the
            // object held the name and admitted no call of it, or held none.
            match decided {
                Res::Found(_) => {}
                Res::Ambiguous => self.note(want, || Residue::OverloadAmbiguous),
                Res::NotFound if self.passed_over.get() => self.note(want, || Residue::NoApplicableOverload),
                Res::NotFound => self.note(want, || Residue::SupertypeUnreached),
            }
            return decided;
        }
        let nested: Vec<NodeId> = pair.iter().flat_map(|&ty| supertype_climb.member_types(ty, next)).collect();
        match self.walk_from(&nested, after, want) {
            Some(decided) => decided,
            None => {
                self.note(want, || Residue::ExternalType {
                    candidates: Vec::new(),
                });
                Res::NotFound
            }
        }
    }
}
