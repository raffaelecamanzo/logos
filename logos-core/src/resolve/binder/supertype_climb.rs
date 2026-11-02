//! A member found up a type's supertypes (S-468, S-537, S-609, [FR-RS-10]):
//! the `Extends` chain climbed nearest first, then the interfaces it
//! implements ([`SupertypeClimb::type_member`]) — or, for a Ruby instance, up
//! the class's ancestors in Ruby's order, mixins included (S-568, [FR-RS-35];
//! [`SupertypeClimb::ancestor_walk`]).
//!
//! [FR-RS-10]: ../../../../docs/specs/requirements/FR-RS-10.md
//! [FR-RS-35]: ../../../../docs/specs/requirements/FR-RS-35.md

use std::collections::HashSet;
use std::ops::Deref;

use crate::model::NodeId;

use super::hierarchy::Hierarchy;
use super::{
    exactly_one, is_type_like, Climb, Ctx, Index, Levels, Res, Residue, Want,
    MAX_SUPERTYPE_DEPTH,
};
use super::hierarchy::Mixins;

/// A member found up a type's supertypes, or a class's ancestors.
#[derive(Clone, Copy)]
pub(super) struct SupertypeClimb<'c, 'a> {
    pub(super) cx: &'c Ctx<'a>,
}

impl<'a> Deref for SupertypeClimb<'_, 'a> {
    type Target = Ctx<'a>;

    fn deref(&self) -> &Ctx<'a> {
        self.cx
    }
}

impl SupertypeClimb<'_, '_> {
    /// The one callable `name` of type `ty` (S-468, [CR-150] §3.2 B, [FR-RS-10]):
    /// exactly one among `ty`'s own `Contains` children; failing that, the
    /// in-repository `Extends` chain ([`Hierarchy::supertypes`](super::hierarchy::Hierarchy::supertypes)) climbed nearest
    /// first, binding on the first level that holds exactly one.
    ///
    /// Two or more at the deciding level is an ambiguity, never a pick
    /// ([NFR-RA-05]): an overload stays unbound, and a same-named method
    /// further up is never reached past it. A level is every supertype of the
    /// level below — one superclass, or an interface's super-interfaces, whose
    /// candidates are pooled. The walk is cycle-guarded (a type is visited
    /// once) and bounded by [`MAX_SUPERTYPE_DEPTH`]; it ends where the chain
    /// leaves the graph, which is the honest miss [`Residue::SupertypeUnreached`]
    /// names. An interface's implementations are never reached: the walk goes
    /// up, not across ([CR-150] §3.3).
    ///
    /// [CR-150]: ../../../../docs/requests/CR-150-java-receiver-typing-for-method-calls.md
    /// [FR-RS-10]: ../../../../docs/specs/requirements/FR-RS-10.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    pub(super) fn type_member(&self, ty: NodeId, name: &str) -> Res {
        self.supertype_member(vec![ty], HashSet::from([ty]), name)
    }

    /// [`type_member`](SupertypeClimb::type_member)'s walk, from the level `level` up
    /// ([`supertype_walk`](SupertypeClimb::supertype_walk)), recording why it gave up.
    pub(super) fn supertype_member(&self, level: Vec<NodeId>, seen: HashSet<NodeId>, name: &str) -> Res {
        let climb = self.supertype_walk(level, seen, name);
        self.note_climb(&climb);
        climb.res
    }

    /// Record why a [`Climb`] gave up: two callables at its deciding level are
    /// [`Residue::OverloadAmbiguous`]; none applicable at any level holding the
    /// name is [`Residue::NoApplicableOverload`] (S-592); no level holding it
    /// at all is [`Residue::SupertypeUnreached`].
    pub(super) fn note_climb(&self, climb: &Climb) {
        match climb.res {
            Res::Found(_) => {}
            Res::Ambiguous => self.note(Want::Callable, || Residue::OverloadAmbiguous),
            Res::NotFound if climb.inapplicable => {
                self.note(Want::Callable, || Residue::NoApplicableOverload);
            }
            Res::NotFound => self.note(Want::Callable, || Residue::SupertypeUnreached),
        }
    }

    /// [`type_member`](SupertypeClimb::type_member)'s walk, from the level `level` up —
    /// `seen` holds the types already visited, which the walk never revisits.
    /// An empty first level is a chain that never entered the graph. A class
    /// that uses a trait ([`Hierarchy::trait_users`](super::hierarchy::Hierarchy::trait_users), S-522) is read for its own
    /// members and never climbed through, so the walk ends there too.
    ///
    /// Each level's callables of the name are first narrowed to those whose
    /// parameter range admits the call ([`applicable`](Ctx::applicable),
    /// S-592, [FR-RS-43]): a level holding the name but no applicable callable
    /// is passed over, as the language passes over an inapplicable overload to
    /// an inherited one, and exactly-one decides at the first level with any.
    ///
    /// A language that inherits interface member bodies (S-609, [FR-RS-48])
    /// goes on once the whole `Extends` chain is passed with nothing decided:
    /// to the interfaces the chain's types implement ([`Hierarchy::interfaces`](super::hierarchy::Hierarchy::interfaces)),
    /// then their super-interfaces, nearest level first — class before
    /// interface, so a superclass's `m(int)` beats a default `m(int)`. At an
    /// interface level only an inheritable member is a candidate: one with a
    /// body that the plugin does not mark uninherited ([`DeclarationFacts::uninherited`](super::declaration_facts::DeclarationFacts::uninherited)).
    /// A level whose applicable callables are all uninheritable — an abstract
    /// re-declaration hides the default above it — decides nothing and ends the
    /// walk. A chain that crosses a class whose base the graph does not hold
    /// ([`Hierarchy::unseen_bases`](super::hierarchy::Hierarchy::unseen_bases)) reaches no interface: that base may declare the
    /// method.
    ///
    /// [FR-RS-43]: ../../../../docs/specs/requirements/FR-RS-43.md
    /// [FR-RS-48]: ../../../../docs/specs/requirements/FR-RS-48.md
    pub(super) fn supertype_walk(&self, level: Vec<NodeId>, mut seen: HashSet<NodeId>, name: &str) -> Climb {
        let ix: &Index = self.ix;
        let hierarchy: &Hierarchy = &ix.hierarchy;
        let mut climb = Climb {
            res: Res::NotFound,
            inapplicable: false,
            closed: true,
        };
        let mut chain = Vec::new();
        if !self.climb_levels(level, &mut seen, name, Levels::Extends(&mut chain), &mut climb) {
            return climb;
        }
        if chain.iter().any(|t| ix.hierarchy.unseen_bases.contains(t)) {
            return climb;
        }
        let interfaces: Vec<NodeId> = chain
            .iter()
            .flat_map(|&t| hierarchy.interfaces_of(t).iter().copied())
            .filter(|i| seen.insert(*i))
            .collect();
        self.climb_levels(interfaces, &mut seen, name, Levels::Interfaces, &mut climb);
        climb
    }

    /// One phase of a [`supertype_walk`](SupertypeClimb::supertype_walk), from `level`
    /// up its `Extends` rows, recording what it found in `climb`: `true` when
    /// it ran off the top of the chain with nothing decided, `false` when a
    /// level decided, or the depth bound cut it. [`Levels::Extends`] collects
    /// every type it crosses.
    fn climb_levels(
        &self,
        mut level: Vec<NodeId>,
        seen: &mut HashSet<NodeId>,
        name: &str,
        mut levels: Levels<'_>,
        climb: &mut Climb,
    ) -> bool {
        let ix: &Index = self.ix;
        let hierarchy: &Hierarchy = &ix.hierarchy;
        level.sort_unstable();
        level.dedup();
        seen.extend(level.iter().copied());
        for _ in 0..MAX_SUPERTYPE_DEPTH {
            if level.is_empty() {
                return true;
            }
            if let Levels::Extends(chain) = &mut levels {
                chain.extend(level.iter().copied());
            }
            climb.closed &= !level.iter().any(|t| ix.hierarchy.open.contains(t));
            let mut named: Vec<NodeId> = level
                .iter()
                .flat_map(|&t| ix.members_named(t, name, Want::Callable))
                .chain(level.iter().flat_map(|&t| self.declared_signatures(t, name).iter().copied()))
                .collect();
            named.sort_unstable();
            named.dedup();
            let held = !named.is_empty();
            let mut found = self.applicable(named);
            climb.inapplicable |= held && found.is_empty();
            if matches!(levels, Levels::Interfaces) && !found.is_empty() {
                found.retain(|id| !ix.facts.uninherited.contains(id));
                if found.is_empty() {
                    return false;
                }
            }
            match exactly_one(&found) {
                Res::NotFound => {}
                decided => {
                    climb.res = decided;
                    return false;
                }
            }
            // A class that uses a trait is never climbed through (S-522).
            let mut next: Vec<NodeId> = level
                .iter()
                .filter(|t| !ix.hierarchy.trait_users.contains(t))
                .flat_map(|&t| hierarchy.supertypes_of(t).iter().copied())
                .filter(|s| seen.insert(*s))
                .collect();
            if next.is_empty() {
                return true;
            }
            next.sort_unstable();
            level = next;
        }
        // The depth bound cut the chain: what lies beyond it is unseen.
        climb.closed = false;
        false
    }

    /// The walk of a call on a Ruby instance (S-568, [FR-RS-35]) — `name()`,
    /// `self.name()`, or a bare `name` no local binds — from the caller's
    /// class `class` along its ancestors, in Ruby's order. For each class, from
    /// the caller's up its superclasses ([`Hierarchy::supertypes`](super::hierarchy::Hierarchy::supertypes)):
    ///
    /// 1. the modules it prepends;
    /// 2. the class itself — every declaration of its constant, since a
    ///    reopened class is one class ([`PackageIndex::constants`](super::package_index::PackageIndex::constants));
    /// 3. the modules it includes — less those its superclasses already mix
    ///    in, which Ruby leaves where the superclass put them
    ///    ([`inherited_mixins`](SupertypeClimb::inherited_mixins)).
    ///
    /// Exactly one callable at the first level holding the name decides
    /// ([NFR-RA-05]). The modules mixed in at one place are one level, with
    /// every module those mix in in turn: Ruby orders them by when each was
    /// mixed in, which a reopening in another file makes unreadable, so two of
    /// them defining the name decide nothing rather than pick. A callable of
    /// the class or module object itself (`def self.x`, a `module_function`;
    /// [`DeclarationFacts::without_self`](super::declaration_facts::DeclarationFacts::without_self)) is no candidate: an instance does not answer
    /// it. The walk stops, deciding nothing, at a level it cannot see —
    /// a prepended or included module the graph does not hold, which may
    /// define the name — and where the superclass leaves the graph or is not
    /// one class. `past_own` starts past the caller's class itself, its
    /// prepends included — where a `super` goes on from.
    ///
    /// [FR-RS-35]: ../../../../docs/specs/requirements/FR-RS-35.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    pub(super) fn ancestor_walk(&self, class: NodeId, name: &str, past_own: bool) -> Climb {
        let mut climb = Climb {
            res: Res::NotFound,
            inapplicable: false,
            closed: true,
        };
        let mixins = |of: &[NodeId]| of.iter().filter_map(|t| self.ix.hierarchy.mixins.get(t)).collect::<Vec<_>>();
        let mut seen: HashSet<NodeId> = HashSet::new();
        let mut class = self.declarations_of(class);
        let mut skip_own = past_own;
        let none = HashSet::new();
        for _ in 0..MAX_SUPERTYPE_DEPTH {
            if !class.iter().all(|&c| seen.insert(c)) {
                break;
            }
            let of_class = mixins(&class);
            // This class's levels in order; `None` is one the graph cannot see.
            let mut levels = Vec::with_capacity(3);
            if !skip_own {
                let prepends = of_class.iter().flat_map(|m| m.prepends.iter().copied()).collect();
                let unseen = of_class.iter().any(|m| m.unseen_prepends);
                levels.push(self.mixin_closure(prepends, unseen, &mut seen, &none));
                levels.push(Some(class.clone()));
            }
            skip_own = false;
            let includes: Vec<NodeId> = of_class.iter().flat_map(|m| m.includes.iter().copied()).collect();
            let unseen = of_class.iter().any(|m| m.unseen_includes);
            let inherited = if includes.is_empty() { HashSet::new() } else { self.inherited_mixins(&class) };
            levels.push(self.mixin_closure(includes, unseen, &mut seen, &inherited));
            for level in levels {
                let Some(level) = level else {
                    climb.closed = false;
                    return climb;
                };
                if self.ancestor_level(&level, name, &mut climb) {
                    return climb;
                }
            }
            match self.superclasses(&class)[..] {
                [] => return climb,
                [one] => class = self.declarations_of(one),
                _ => break,
            }
        }
        // A cycle, several superclasses, or the depth bound: unseen beyond.
        climb.closed = false;
        climb
    }

    /// The superclasses the declarations `class` of one Ruby class name,
    /// id-sorted: none, the one, or — a contradiction the walk reads as an
    /// unseen level — several.
    fn superclasses(&self, class: &[NodeId]) -> Vec<NodeId> {
        let ix: &Index = self.ix;
        let hierarchy: &Hierarchy = &ix.hierarchy;
        let mut supers: Vec<NodeId> = class.iter().flat_map(|&c| hierarchy.supertypes_of(c).iter().copied()).collect();
        supers.sort_unstable();
        supers.dedup();
        supers
    }

    /// Every module the superclasses above `class` mix in, transitively —
    /// which an `include` in `class` itself skips, as Ruby skips including a
    /// module the superclass's ancestors already hold: the module stays where
    /// the superclass put it, after the superclass's own methods.
    fn inherited_mixins(&self, class: &[NodeId]) -> HashSet<NodeId> {
        let (mut inherited, mut crossed, none) = (HashSet::new(), HashSet::new(), HashSet::new());
        let mut level = self.superclasses(class);
        for _ in 0..MAX_SUPERTYPE_DEPTH {
            let [one] = level[..] else {
                break;
            };
            let decls = self.declarations_of(one);
            if !decls.iter().all(|&c| crossed.insert(c)) {
                break;
            }
            let roots = decls
                .iter()
                .filter_map(|t| self.ix.hierarchy.mixins.get(t))
                .flat_map(|m| m.prepends.iter().chain(&m.includes).copied())
                .collect();
            let mut closure = HashSet::new();
            if let Some(modules) = self.mixin_closure(roots, false, &mut closure, &none) {
                inherited.extend(modules);
            }
            level = self.superclasses(&decls);
        }
        inherited
    }

    /// One level of an [`ancestor_walk`](SupertypeClimb::ancestor_walk): `true` when it
    /// holds an instance callable named `name`, which decides the walk — the
    /// exactly-one of those the call's argument count admits, recorded in
    /// `climb`.
    fn ancestor_level(&self, level: &[NodeId], name: &str, climb: &mut Climb) -> bool {
        let ix: &Index = self.ix;
        let mut named: Vec<NodeId> = level
            .iter()
            .flat_map(|&t| ix.members_named(t, name, Want::Callable))
            .filter(|id| !ix.facts.without_self.contains(id))
            .collect();
        if named.is_empty() {
            return false;
        }
        named.sort_unstable();
        named.dedup();
        let found = self.applicable(named);
        climb.inapplicable |= found.is_empty();
        climb.res = exactly_one(&found);
        true
    }

    /// The modules `roots` with every module they mix in, transitively, not in
    /// `seen` nor in `skip` — one level of an [`ancestor_walk`](SupertypeClimb::ancestor_walk),
    /// id-sorted — or `None` when the graph cannot see all of it: `unseen` says
    /// one of the roots binds no module, or one of the modules mixes in one that
    /// binds none. A module in `skip` is left out with everything it mixes in.
    fn mixin_closure(
        &self,
        roots: Vec<NodeId>,
        unseen: bool,
        seen: &mut HashSet<NodeId>,
        skip: &HashSet<NodeId>,
    ) -> Option<Vec<NodeId>> {
        let mut level = Vec::new();
        let mut unseen = unseen;
        let mut next = roots;
        while !next.is_empty() {
            next.retain(|&m| !skip.contains(&m) && seen.insert(m));
            let modules: Vec<&Mixins> = next.iter().filter_map(|m| self.ix.hierarchy.mixins.get(m)).collect();
            unseen |= modules.iter().any(|m| m.unseen_includes || m.unseen_prepends);
            level.append(&mut next);
            next = modules
                .iter()
                .flat_map(|m| m.prepends.iter().chain(&m.includes).copied())
                .collect();
        }
        if unseen {
            return None;
        }
        level.sort_unstable();
        Some(level)
    }

    /// Every declaration of the constant the Ruby class or module `ty`
    /// declares, of `ty`'s kind, id-sorted — `ty` alone when it has no
    /// constant (S-568).
    fn declarations_of(&self, ty: NodeId) -> Vec<NodeId> {
        let kind = self.ix.info.get(&ty).map(|i| i.kind);
        let declarations = self
            .ix
            .packages.constant_of
            .get(&ty)
            .and_then(|fqn| self.ix.packages.constants.get(self.source_family.as_deref()?)?.get(fqn));
        match declarations {
            Some(ids) => ids
                .iter()
                .copied()
                .filter(|id| self.ix.info.get(id).map(|i| i.kind) == kind)
                .collect(),
            None => vec![ty],
        }
    }

    /// The required signatures named `name` the type `ty` declares, while a
    /// walk [`reads_signatures`](Ctx::reads_signatures) (S-537); none otherwise.
    fn declared_signatures(&self, ty: NodeId, name: &str) -> &[NodeId] {
        if !self.reads_signatures.get() {
            return &[];
        }
        self.ix
            .facts.declared_signatures
            .get(&(ty, name.to_string()))
            .map_or(&[], Vec::as_slice)
    }

    /// The type-like members of `scope` named `name`.
    pub(super) fn member_types(&self, scope: NodeId, name: &str) -> Vec<NodeId> {
        let ix: &Index = self.ix;
        ix
            .members_named(scope, name, Want::Any)
            .into_iter()
            .filter(|id| ix.info.get(id).is_some_and(|i| is_type_like(i.kind)))
            .collect()
    }
}
