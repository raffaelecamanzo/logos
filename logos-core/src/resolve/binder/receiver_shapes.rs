//! A receiver call bound by its receiver's shape (S-514, [FR-RS-12]): `self`
//! among the caller's own class and up its proven `Extends`, `super` only up
//! that chain, `other` nowhere unless an extension function answers; and a
//! member access bound to a field of the caller's own container (CR-005,
//! [FR-EX-08]).
//!
//! [FR-RS-12]: ../../../../docs/specs/requirements/FR-RS-12.md
//! [FR-EX-08]: ../../../../docs/specs/requirements/FR-EX-08.md

use std::collections::HashSet;
use std::ops::Deref;

use crate::model::{NodeId, NodeKind};

use super::hierarchy::Hierarchy;
use super::supertype_climb::SupertypeClimb;
use super::{
    exactly_one, is_class_like, Climb, Ctx, Index, NodeInfo, PackageLayout, Res, Residue, Syntax, Want,
    MAX_ALIAS_DEPTH, MAX_CONTAINS_DEPTH, MAX_SUPERTYPE_DEPTH, SELF_TYPE_HEAD,
};
use super::companions::Companions;
use super::extension_calls::{ExtensionCalls, ExtensionReceiver};
use super::scope_hierarchy::ScopeHierarchy;
use super::self_types::SelfTypes;

/// A receiver call bound by its receiver's shape, and a member access.
#[derive(Clone, Copy)]
pub(super) struct ReceiverShapes<'c, 'a> {
    pub(super) cx: &'c Ctx<'a>,
}

impl<'a> Deref for ReceiverShapes<'_, 'a> {
    type Target = Ctx<'a>;

    fn deref(&self) -> &Ctx<'a> {
        self.cx
    }
}

impl ReceiverShapes<'_, '_> {
    /// Resolve a member-access fact to the one `Field` of the source method's
    /// own class-like container named `field` (CR-005, [FR-EX-08]).
    ///
    /// The container is the caller's own class ([`caller_class`](ReceiverShapes::caller_class)),
    /// else, for a method not lexically nested under its type (Go), the type
    /// its recorded self type names ([`self_type_class`](ReceiverShapes::self_type_class),
    /// S-529); the field binds iff that container directly contains **exactly
    /// one** `Field` of that name — the same single acceptance rule every
    /// binder level shares ([NFR-RA-05]). A caller with neither, or a language
    /// whose fields are not extracted as nodes, yields no candidate, so the
    /// access stays honestly unresolved and retries on sync — never fabricated.
    ///
    /// [FR-EX-08]: ../../../../docs/specs/requirements/FR-EX-08.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    pub(super) fn resolve_member_access(&self, field: &str) -> Res {
        let ix: &Index = self.ix;
        if self.module_before_class() {
            return Res::NotFound;
        }
        match self.caller_class().or_else(|| self.self_type_class()) {
            Some(class) => exactly_one(&ix.members_named(class, field, Want::Field)),
            None => Res::NotFound,
        }
    }

    /// The class-like type the caller's recorded self type names (S-529,
    /// [FR-RS-18]) — the container of a receiver read in a language whose
    /// methods sit at module level, where the `Contains` ancestry holds no
    /// class (Go's `func (c *ioCore) Write()` beside `type ioCore struct`): the
    /// one class-like node of that name declared in the caller's own package
    /// ([`package_of`]), whichever of its files declares the type. A method's
    /// receiver type is its own package's, so a same-named type of another
    /// package — another directory, or the external test package `foo_test`
    /// beside `foo` (S-830) — is never a candidate, however few the crate
    /// holds. `None` when the caller records no self type, when its package is
    /// unknown, or when its package does not hold exactly one ([NFR-RA-05]).
    ///
    /// [FR-RS-18]: ../../../../docs/specs/requirements/FR-RS-18.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    fn self_type_class(&self) -> Option<NodeId> {
        let self_type = self.ix.facts.self_types.get(&self.source)?;
        let caller = self.ix.info.get(&self.source)?;
        let own = package_of(&self.ix.layout, caller)?;
        let local: Vec<NodeId> = self
            .ix
            .facts.self_type_classes
            .get(&(caller.crate_name.clone(), self_type.clone()))?
            .iter()
            .copied()
            .filter(|c| self.ix.info.get(c).and_then(|info| package_of(&self.ix.layout, info)) == Some(own))
            .collect();
        match local[..] {
            [one] => Some(one),
            _ => None,
        }
    }

    /// Whether a `Module` lies between the source and the class
    /// [`caller_class`](ReceiverShapes::caller_class) would name (S-569): a Ruby mixin
    /// nested in a class (a concern's `ClassMethods`) has `self` of whatever
    /// includes it, never the class around it, so its member access is no
    /// access of that class's Fields. `false` when no class bears the module
    /// at all — a method under its file's module (Go's receiver methods), which
    /// [`resolve_member_access`](ReceiverShapes::resolve_member_access) leaves to its own
    /// fallback.
    fn module_before_class(&self) -> bool {
        let mut cursor = Some(self.source);
        let mut inside_module = false;
        for _ in 0..MAX_CONTAINS_DEPTH {
            let Some(id) = cursor else { return false };
            match self.ix.info.get(&id).map(|i| i.kind) {
                Some(kind) if is_class_like(kind) => return inside_module,
                Some(NodeKind::Module) => inside_module = true,
                _ => {}
            }
            cursor = self.ix.parent.get(&id).copied();
        }
        false
    }

    /// The caller's own class: the nearest class-bearing container
    /// ([`is_class_like`]) on the source's `Contains` ancestry, the source
    /// itself included — `None` when there is none (a free function, a method
    /// of a language whose methods sit at module level). Bounded by
    /// [`MAX_CONTAINS_DEPTH`] against a malformed `Contains` cycle.
    pub(super) fn caller_class(&self) -> Option<NodeId> {
        let mut cursor = Some(self.source);
        for _ in 0..MAX_CONTAINS_DEPTH {
            let id = cursor?;
            if self.ix.info.get(&id).is_some_and(|i| is_class_like(i.kind)) {
                return Some(id);
            }
            cursor = self.ix.parent.get(&id).copied();
        }
        None
    }

    /// Bind a receiver call `self.name()` — shape `self` (S-514, [FR-RS-12]):
    /// exactly one callable `name` among the caller's own class's members, else
    /// the nearest level of its proven `Extends` chain holding exactly one
    /// ([`type_member`](super::supertype_climb::SupertypeClimb::type_member), the [FR-RS-10] walk). A free function
    /// or a module-level sibling is never a candidate: only the class's own
    /// `Contains` children are — and, once the class and every supertype it
    /// has are seen to hold no applicable `name`, the members of its companion
    /// and then of its superclasses' companions (I4-INT,
    /// [`then_companion`](ReceiverShapes::then_companion)), the implicit receivers Kotlin
    /// reads next.
    ///
    /// A caller that is a function of a recorded `impl` block (Rust) binds
    /// through its block's self type, by the one lookup with method syntax —
    /// and one that is a trait's default body fans out through its trait
    /// ([`self_type_call`](super::self_types::SelfTypes::self_type_call), S-607, S-608) — the shape a
    /// `self.m()` keeps when its header recorded no self type (`impl a::X`,
    /// `impl Tr for std::io::Error`). A caller with a recorded self type in a
    /// language without impl blocks (Go: a method at module level) binds
    /// through it ([`resolve_recorded_self_type_call`](super::self_types::SelfTypes::resolve_recorded_self_type_call)).
    /// Extraction records such a call as `Self::name` already, so this reaches
    /// it only for a row extracted without that rewrite.
    ///
    /// A caller in no class records [`Residue::NoReceiverEvidence`]: the
    /// receiver is its own instance, but nothing says of what.
    ///
    /// Each level keeps only the callables whose parameter range admits the
    /// call (S-592, [FR-RS-43]), so a member the call cannot invoke is passed
    /// over for an inherited overload. When the class and its supertypes hold
    /// the name but none applicable, a language whose unqualified in-class call
    /// is on the instance, and which records its classes' supertypes
    /// ([`falls_through`](Ctx::falls_through): Kotlin), goes on to the free
    /// functions and imports in scope, as its own lookup does — Kotlin's
    /// `module { }` inside a class whose `module(a, b)` cannot take it calls
    /// the imported `module`. The ledger does not tell `m()` from `this.m()`,
    /// so this reads every such `self` row as the unqualified call; it is taken
    /// only when the walk saw every supertype ([`Climb::closed`]), since an
    /// external base or an interface's default body may hold the overload the
    /// call reaches. Nothing applicable anywhere is
    /// [`Residue::NoApplicableOverload`].
    ///
    /// A closed walk that finds nothing, in a language that declares extension
    /// functions, binds the one extension in scope whose receiver type is the
    /// class or an in-repository supertype of it (S-833, [FR-RS-31],
    /// [`resolve_implicit_extension_call`](super::extension_calls::ExtensionCalls::resolve_implicit_extension_call)):
    /// `f()` and `this.f()` inside `class Sub : Base()` both reach `fun
    /// Base.f()`.
    ///
    /// A class of a lexical-constant language (Ruby, S-568) binds along its
    /// ancestors instead ([`ancestor_walk`](super::supertype_climb::SupertypeClimb::ancestor_walk)).
    ///
    /// [FR-RS-10]: ../../../../docs/specs/requirements/FR-RS-10.md
    /// [FR-RS-12]: ../../../../docs/specs/requirements/FR-RS-12.md
    /// [FR-RS-31]: ../../../../docs/specs/requirements/FR-RS-31.md
    /// [FR-RS-43]: ../../../../docs/specs/requirements/FR-RS-43.md
    pub(super) fn resolve_self_receiver(&self, name: &str) -> Res {
        let self_types = SelfTypes { cx: self.cx };
        let supertype_climb = SupertypeClimb { cx: self.cx };
        if self.impl_blocks && (self_types.own_header().is_some() || self_types.own_trait().is_some()) {
            return self_types.self_type_call(name, Syntax::Method);
        }
        if self.ix.facts.self_types.contains_key(&self.source) {
            if let Some(res) = self_types.resolve_recorded_self_type_call(&format!("{SELF_TYPE_HEAD}::{name}")) {
                return res;
            }
        }
        let Some(class) = self.caller_class() else {
            self.note(Want::Callable, || Residue::NoReceiverEvidence);
            return Res::NotFound;
        };
        if self.lexical_constants {
            let climb = supertype_climb.ancestor_walk(class, name, false);
            supertype_climb.note_climb(&climb);
            return climb.res;
        }
        let climb = self.then_companion(supertype_climb.supertype_walk(vec![class], HashSet::from([class]), name), class, name);
        if climb.res == Res::NotFound && climb.closed {
            let extensions = ExtensionCalls { cx: self.cx };
            if let Some(res) = extensions.resolve_implicit_extension_call(class, name) {
                return res;
            }
        }
        if climb.res == Res::NotFound && climb.inapplicable && climb.closed && self.falls_through {
            match self.free_call(name) {
                Res::NotFound => {}
                Res::Ambiguous => {
                    self.note(Want::Callable, || Residue::OverloadAmbiguous);
                    return Res::Ambiguous;
                }
                found => return found,
            }
        }
        supertype_climb.note_climb(&climb);
        climb.res
    }

    /// `climb`, the walk of `name` from `class` up, continued into the
    /// companions of `class` and of each superclass above it, nearest first
    /// (I4-INT, [`companion_climb`](super::companions::Companions::companion_climb)) — the implicit
    /// receivers Kotlin reads after the instance, a superclass's companion
    /// scope being linked into its subclasses — when it decided nothing and saw
    /// every supertype: a base the graph does not hold may declare the name,
    /// which would come first. The first companion that decides ends the walk.
    /// Some level holding the name with no applicable callable stays recorded
    /// across them all.
    fn then_companion(&self, climb: Climb, class: NodeId, name: &str) -> Climb {
        let companions = Companions { cx: self.cx };
        let ix: &Index = self.ix;
        let hierarchy: &Hierarchy = &ix.hierarchy;
        if climb.res != Res::NotFound || !climb.closed {
            return climb;
        }
        let mut inapplicable = climb.inapplicable;
        let mut seen = HashSet::from([class]);
        let mut level = vec![class];
        for _ in 0..MAX_SUPERTYPE_DEPTH {
            for up in level.iter().filter_map(|&ty| companions.companion_climb(ty, name)) {
                inapplicable |= up.inapplicable;
                if up.res != Res::NotFound {
                    return Climb { inapplicable, ..up };
                }
            }
            level = level
                .iter()
                .flat_map(|&ty| hierarchy.supertypes_of(ty).iter().copied())
                .filter(|ty| seen.insert(*ty))
                .collect();
            if level.is_empty() {
                break;
            }
        }
        Climb { inapplicable, ..climb }
    }

    /// `name` resolved as the unqualified free call the language makes once no
    /// member of the caller's class admits it (S-592,
    /// [`resolve_self_receiver`](ReceiverShapes::resolve_self_receiver)): the scope walk a
    /// bare Path call takes, from the caller outward — the class's own members
    /// are passed over again by their ranges — then the file's imports. A
    /// callable only: a class of that name is not what `name()` on the instance
    /// fell through to. By scope and imports only ([`scope_only`](Ctx::scope_only)):
    /// the aggressive policy's workspace name match would bind a same-named
    /// method of an unrelated class once the caller's own one is filtered out.
    fn free_call(&self, name: &str) -> Res {
        let scope_hierarchy = ScopeHierarchy { cx: self.cx };
        self.bare_path_call.set(true);
        let scoped = self.scope_only.replace(true);
        let res = scope_hierarchy.resolve_path(&[name.to_string()], Want::Callable, MAX_ALIAS_DEPTH);
        self.scope_only.set(scoped);
        self.bare_path_call.set(false);
        res
    }

    /// Bind a receiver call of shape `other`, or of none: nowhere
    /// ([`Residue::NoReceiverEvidence`]), unless it is an extension call
    /// ([`resolve_extension_call`](super::extension_calls::ExtensionCalls::resolve_extension_call), S-560).
    pub(super) fn resolve_other_receiver(&self, name: &str) -> Res {
        let extensions = ExtensionCalls { cx: self.cx };
        extensions.resolve_extension_call(name, ExtensionReceiver::Unproven).unwrap_or_else(|| {
            self.note(Want::Callable, || Residue::NoReceiverEvidence);
            Res::NotFound
        })
    }

    /// Bind a receiver call `super.name()` — shape `super` (S-514,
    /// [FR-RS-12]): only through a proven `Extends` of the caller's own class,
    /// to exactly one callable at the nearest supertype level holding one. The
    /// caller's own class is never a level, so it never binds the caller's own
    /// method. No proven `Extends` — an external base, or a language whose
    /// plugin records none — is [`Residue::SupertypeUnreached`]. In Ruby
    /// (S-568) the levels are the class's ancestors past the class itself —
    /// its included modules, then its superclass's ([`ancestor_walk`](super::supertype_climb::SupertypeClimb::ancestor_walk)).
    ///
    /// [FR-RS-12]: ../../../../docs/specs/requirements/FR-RS-12.md
    pub(super) fn resolve_super_receiver(&self, name: &str) -> Res {
        let supertype_climb = SupertypeClimb { cx: self.cx };
        let ix: &Index = self.ix;
        let hierarchy: &Hierarchy = &ix.hierarchy;
        let Some(class) = self.caller_class() else {
            self.note(Want::Callable, || Residue::NoReceiverEvidence);
            return Res::NotFound;
        };
        // Ruby's `super` goes on along the class's ancestors (S-568).
        if self.lexical_constants {
            let climb = supertype_climb.ancestor_walk(class, name, true);
            supertype_climb.note_climb(&climb);
            return climb.res;
        }
        // A class whose own `Extends` binds to itself (`interface I extends I`,
        // which parses) is never its own base level.
        let bases: Vec<NodeId> = hierarchy
            .supertypes_of(class)
            .iter()
            .copied()
            .filter(|&base| base != class)
            .collect();
        supertype_climb.supertype_member(bases, HashSet::from([class]), name)
    }
}

/// The package a node's file sits in, as [`ReceiverShapes::self_type_class`]
/// reads it: for a directory-package language (Go) its directory and recorded
/// `package` clause (S-830, [`PackageLayout::directory_package`]) — `None`
/// while the clause is unrecorded — and for any other its directory, as
/// before (S-529).
///
/// [`PackageLayout::directory_package`]: crate::resolve::package_key::PackageLayout::directory_package
fn package_of<'i>(layout: &'i PackageLayout, info: &'i NodeInfo) -> Option<(&'i str, &'i str)> {
    let path = info.file_path.as_deref()?;
    if layout.declares_directory_packages(path) {
        layout.directory_package(path)
    } else {
        Some((super::super::dir_of(path), ""))
    }
}
