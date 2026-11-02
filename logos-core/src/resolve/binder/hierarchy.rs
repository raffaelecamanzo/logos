//! The type hierarchy of the binder's index (S-828, [CR-258]): each type's
//! in-repository supertypes and interfaces, the types whose walk is open, the
//! Ruby mixins, and the tokens and families a sync reads to know a walk moved
//! ([`Hierarchy`]). What [`build_supertypes`](super::index_builder) reads off
//! the supertype rows is installed whole into the one the [`Index`](super::Index)
//! owns ([`Hierarchy::install`]).
//!
//! [CR-258]: ../../../../docs/requests/CR-258-the-binders-ctx-and-index-are-split-by-concern.md

use std::collections::{HashMap, HashSet};

use crate::model::NodeId;

/// The type hierarchy (S-468, [CR-150] §3.2 B; S-522): each type's
/// in-repository supertypes, the name tokens a sync must watch to keep a walk
/// over it fresh, and the families whose rows it holds
/// ([`Hierarchy::supertypes`], [`Hierarchy::tokens`], [`Hierarchy::families`]).
/// A built one holds what its rows read; the index's holds every type's.
///
/// [CR-150]: ../../../../docs/requests/CR-150-java-receiver-typing-for-method-calls.md
#[derive(Debug, Default)]
pub(super) struct Hierarchy {
    /// A type → the in-repository types its `Extends` rows bind to as
    /// `Extends`, id-sorted (S-468, [CR-150] §3.2 B; S-522 for every language
    /// whose types record one): one superclass for a class (each base of a
    /// Python class), the super-interfaces for an interface. A supertype bound
    /// as `Implements` — a C# or Kotlin class's interface — is not one, as a
    /// Java class's `implements` is not. Bound from the `Extends` ledger rows
    /// by S-466's own rule ([`bind`](super::bind)) while the index is built, so it is
    /// available on the very first index pass, before any `Extends` edge is
    /// committed — the [`impls_by_trait_method`](super::impl_index::ImplIndex::impls_by_trait_method)
    /// precedent. An unbound `Extends` (a JDK, library or other-member
    /// superclass) contributes nothing: the walk ends there.
    ///
    /// [CR-150]: ../../../../docs/requests/CR-150-java-receiver-typing-for-method-calls.md
    pub(super) supertypes: HashMap<NodeId, Vec<NodeId>>,
    /// A type → the in-repository interfaces it implements, id-sorted (S-609,
    /// [FR-RS-48]) — only for a type whose language inherits interface member
    /// bodies ([`PackageLayout::inherits_interface_bodies`](crate::resolve::package_key::PackageLayout::inherits_interface_bodies)): a supertype row
    /// bound as `Implements`. What a supertype walk visits once the type's
    /// `Extends` chain holds no applicable candidate. Empty for every other
    /// language, whose walk ends with the chain.
    ///
    /// [FR-RS-48]: ../../../../docs/specs/requirements/FR-RS-48.md
    pub(super) interfaces: HashMap<NodeId, Vec<NodeId>>,
    /// The classes whose base class the graph does not hold (S-609): of a
    /// language that inherits interface member bodies, one with a supertype
    /// row that binds nothing and may name its base; of any language, one with
    /// several bases. That base may declare the method, and a class's member
    /// beats an interface's, so a walk whose `Extends` chain crosses one never
    /// goes on to the interfaces.
    pub(super) unseen_bases: HashSet<NodeId>,
    /// The name tokens of the type hierarchy: every hierarchy `Extends` row's
    /// target, bound or not (S-468), and every `Implements` row's of a
    /// language that inherits interface member bodies (S-609). A sync
    /// dirtying one of them may move a supertype walk whose row spells none of
    /// them ([`Invalidation::hierarchy_touched`](super::Invalidation::hierarchy_touched)) — an interface gaining or losing a
    /// default body among them. A type's own name is not needed: a walk that
    /// crosses a type other than its start crosses it as some supertype row's
    /// target, and a row starting at a type spells that type's name.
    pub(super) tokens: HashSet<String>,
    /// The interop families of those rows' sources (S-522,
    /// [`PackageLayout::family`](crate::resolve::package_key::PackageLayout::family)): the languages whose calls a supertype walk
    /// can bind, so whose calls a moved hierarchy re-selects
    /// ([`Invalidation::walks_hierarchy`](super::Invalidation::walks_hierarchy)).
    pub(super) families: HashSet<String>,
    /// The classes that use a trait (S-522, a PHP `use`, [`TRAIT_USE_ALIAS`](super::TRAIT_USE_ALIAS)).
    /// A used trait's method outranks every inherited one, and an unbound
    /// trait's methods are not in the graph, so a supertype walk reads such a
    /// class's own members and never climbs **through** it
    /// ([`SupertypeClimb::supertype_member`](super::supertype_climb::SupertypeClimb::supertype_member)). A `parent::m()` written in it starts at its
    /// parent, which its traits do not touch.
    pub(super) trait_users: HashSet<NodeId>,
    /// The types whose supertypes the walk cannot see in full (S-592): one with
    /// a supertype row that binds no walked `Extends` — an external base, an
    /// interface (whose abstract members the walk never binds, and whose
    /// default bodies it reaches only in a language declaring so, S-609),
    /// several bases — or that uses a trait ([`Hierarchy::trait_users`](super::hierarchy::Hierarchy::trait_users)). A callable of the name
    /// may sit there, so an implicit-receiver call whose class walk crossed one
    /// never goes on to a free function ([`ReceiverShapes::resolve_self_receiver`](super::receiver_shapes::ReceiverShapes::resolve_self_receiver)).
    pub(super) open: HashSet<NodeId>,
    /// A Ruby class or module → the modules it mixes into its instances'
    /// ancestors (S-568, [FR-RS-35]): its bound `include`s and `prepend`s, and
    /// whether one of either names no single module the graph holds. What
    /// [`SupertypeClimb::ancestor_walk`](super::supertype_climb::SupertypeClimb::ancestor_walk) reads beside the class's superclass
    /// ([`Hierarchy::supertypes`](super::hierarchy::Hierarchy::supertypes)). Empty for every other language.
    ///
    /// [FR-RS-35]: ../../../../docs/specs/requirements/FR-RS-35.md
    pub(super) mixins: HashMap<NodeId, Mixins>,
    /// Whether a structural-types type (S-535) names a supertype through an
    /// import (`<import target>::<name>`, [`IndexBuilder::with_structural_supertypes`](super::IndexBuilder::with_structural_supertypes)):
    /// its place in the hierarchy then moves with that import's binding, which
    /// a module descriptor (`tsconfig.json`, `package.json`) can move while
    /// spelling no token ([`moved_by_descriptor`](Hierarchy::moved_by_descriptor)).
    /// The structural step marks the hierarchy it builds, and
    /// [`install`](Hierarchy::install) keeps the mark of every hierarchy it
    /// takes: its one writer, so the mark is never cleared.
    pub(super) imported: bool,
}

/// The modules one Ruby class or module mixes into its instances' ancestors
/// (S-568, [FR-RS-35]; [`Hierarchy::mixins`](super::hierarchy::Hierarchy::mixins)), each list id-sorted.
///
/// [FR-RS-35]: ../../../../docs/specs/requirements/FR-RS-35.md
#[derive(Debug, Default)]
pub(super) struct Mixins {
    /// The modules its `include`s bind to: read after the class's own methods.
    pub(super) includes: Vec<NodeId>,
    /// The modules its `prepend`s bind to: read before them.
    pub(super) prepends: Vec<NodeId>,
    /// An `include` binds no single module the graph holds — an external one
    /// (`include Comparable`), or a constant of several declarations.
    pub(super) unseen_includes: bool,
    /// A `prepend` binds none.
    pub(super) unseen_prepends: bool,
}

impl Hierarchy {
    /// Put `hierarchy` into this one — the one place a [`Hierarchy`] is
    /// taken apart, so a field added to it cannot be kept by one installer and
    /// dropped by the other. The per-type entries of every type in `replacing`
    /// go first, so its entries are `hierarchy`'s alone
    /// ([`with_structural_supertypes`](super::IndexBuilder::with_structural_supertypes));
    /// [`IndexBuilder::new`](super::IndexBuilder::new) installs into an empty
    /// one and replaces nothing.
    pub(super) fn install(&mut self, hierarchy: Hierarchy, replacing: &HashSet<NodeId>) {
        let Hierarchy {
            supertypes,
            interfaces,
            unseen_bases,
            tokens,
            families,
            trait_users,
            open,
            mixins,
            imported,
        } = hierarchy;
        for source in replacing {
            self.supertypes.remove(source);
            self.interfaces.remove(source);
            self.unseen_bases.remove(source);
            self.trait_users.remove(source);
            self.open.remove(source);
            self.mixins.remove(source);
        }
        self.supertypes.extend(supertypes);
        self.interfaces.extend(interfaces);
        self.unseen_bases.extend(unseen_bases);
        self.tokens.extend(tokens);
        self.families.extend(families);
        self.trait_users.extend(trait_users);
        self.open.extend(open);
        self.mixins.extend(mixins);
        self.imported |= imported;
    }

    /// The in-repository supertypes of `ty` its `Extends` rows bind to.
    pub(super) fn supertypes_of(&self, ty: NodeId) -> &[NodeId] {
        self.supertypes.get(&ty).map_or(&[], Vec::as_slice)
    }

    /// The in-repository interfaces `ty` implements, when its language
    /// inherits their member bodies (S-609, [`Hierarchy::interfaces`]).
    pub(super) fn interfaces_of(&self, ty: NodeId) -> &[NodeId] {
        self.interfaces.get(&ty).map_or(&[], Vec::as_slice)
    }

    /// Whether `dirty` — the tokens a sync added or removed — names a type a
    /// supertype row of the hierarchy names ([`Hierarchy::tokens`]).
    pub(super) fn touched(&self, dirty: &HashSet<String>) -> bool {
        self.tokens.iter().any(|t| dirty.contains(t))
    }

    /// Whether a sync that changed `changed` can move the type hierarchy while
    /// dirtying none of its tokens (S-535): a structural-types type names its
    /// supertype through an import ([`Hierarchy::imported`]), and a
    /// module descriptor changed — a `tsconfig.json` `paths` retarget rebinds
    /// that import, and with it the base a `super.m()` climbs, though the
    /// config spells nothing the `Extends` row names.
    pub(super) fn moved_by_descriptor(&self, changed: &HashSet<String>) -> bool {
        self.imported && changed.iter().any(|p| super::super::is_module_descriptor(p))
    }
}
