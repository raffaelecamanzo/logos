//! The facts the store records per declaration beside its nodes and ledger
//! rows (S-828, [CR-258]): each callable's arity, receiver and linkage facts,
//! each node's recorded self type, an enum's variants and a type's required
//! signatures — what a rung's filters read and no ledger row says
//! ([`DeclarationFacts`]). A sub-index the [`Index`](super::Index) owns, with its
//! fields; the [`IndexBuilder`](super::IndexBuilder)'s store-fact steps fill
//! it, and an index given none reads every fact as unknown.
//!
//! [CR-258]: ../../../../docs/requests/CR-258-the-binders-ctx-and-index-are-split-by-concern.md

use std::collections::{HashMap, HashSet};

use crate::model::{NodeId, ParamRange, ReceiverMode};

/// A link-unit file's local bindings (S-584): `(file, name)` → each binding's
/// `(line, scope first line, scope last line)` ([`IndexBuilder::with_link_unit`](super::IndexBuilder::with_link_unit)).
pub(super) type LocalScopes = HashMap<(String, String), Vec<(u32, u32, u32)>>;

/// What the store records per declaration beyond its node and ledger rows,
/// each map empty — and the linkage facts `None` — until the builder is given
/// them.
#[derive(Debug, Default)]
pub(super) struct DeclarationFacts {
    /// The parameter range each callable records (S-591, [FR-EX-32]; the
    /// `nodes.param_min`/`param_max` columns) — the arguments a call may pass
    /// it, which the `self`, `super`, typed and overloading bare-call arms
    /// filter their candidates by (S-592, [`Ctx::applicable`](super::Ctx::applicable)). A callable
    /// whose range is unknown is not here, so it is never dropped. Empty
    /// unless the run was given the store's facts ([`IndexBuilder::with_arities`](super::IndexBuilder::with_arities)).
    ///
    /// [FR-EX-32]: ../../../../docs/specs/requirements/FR-EX-32.md
    pub(super) arities: HashMap<NodeId, ParamRange>,
    /// The callables recorded as taking `self` (S-604; the `nodes.takes_self`
    /// column, true) — the ones a path call `T::m(x, …)` passes the receiver
    /// to as its first argument (S-607). Empty unless the run was given the
    /// store's facts ([`IndexBuilder::with_arities`](super::IndexBuilder::with_arities)).
    pub(super) with_self: HashSet<NodeId>,
    /// The callables recorded as **not** taking `self` (S-604, [CR-200]; the
    /// `nodes.takes_self` column, false) — a Rust associated function such as
    /// `fn new() -> Self`, which a method call `x.m()` can never invoke, and a
    /// Ruby callable of the class or module object, the only ones a call on a
    /// constant binds (S-565, [`LexicalConstants::resolve_constant_row`](super::lexical_constants::LexicalConstants::resolve_constant_row)), and a Scala
    /// `object`'s callable, the only ones a call on a companion pair binds
    /// (S-573, [`Companions::companion_members`](super::companions::Companions::companion_members)). A
    /// method-syntax lookup ([`Ctx::lookup`](super::Ctx::lookup), S-607) drops them before any
    /// rank; a callable whose fact is unknown is not here, so it is never
    /// dropped. Empty unless the run was given the store's facts
    /// ([`IndexBuilder::with_arities`](super::IndexBuilder::with_arities)).
    ///
    /// [CR-200]: ../../../../docs/requests/CR-200-a-rust-method-call-binds-only-a-callable-that-takes-self.md
    pub(super) without_self: HashSet<NodeId>,
    /// How each callable writes its receiver (S-606; `nodes.receiver_mode`) —
    /// what tells an inherent `&self` method from a trait's by-value one
    /// ([`Ctx::lookup`](super::Ctx::lookup)).
    pub(super) receiver_modes: HashMap<NodeId, ReceiverMode>,
    /// An enum node → its variant names (S-606; `nodes.variants`) — what makes
    /// `E::A(1)` a call of no callable ([`Residue::NotACallable`](super::Residue::NotACallable)).
    pub(super) variants: HashMap<NodeId, Vec<String>>,
    /// node → the self type its plugin query recorded for it (S-493,
    /// [FR-RS-11]; the `nodes.self_type` column). Empty unless the run was given
    /// the store's self types ([`IndexBuilder::with_self_types`](super::IndexBuilder::with_self_types)). What a Go `Self::m`
    /// row binds through, and what marks a method no bare call reaches; an
    /// impl-block language's call reads its impl header instead (S-607).
    ///
    /// [FR-RS-11]: ../../../../docs/specs/requirements/FR-RS-11.md
    pub(super) self_types: HashMap<NodeId, String>,
    /// `(crate, self type, name)` → the callables recorded with that self type,
    /// id-sorted — the universe a `Self::m` call binds within in a language
    /// that records a method's receiver type name but no `impl` block (Go,
    /// S-509; [`SelfTypes::resolve_recorded_self_type_call`](super::self_types::SelfTypes::resolve_recorded_self_type_call)). Across every file of
    /// the crate; a same-named type of another crate is another key, never a
    /// candidate. Same-named types of two modules of one crate share a key,
    /// and the call's own module tells them apart. A file of an
    /// impl-block language ([`PackageLayout::looks_up_impl_blocks`](crate::resolve::package_key::PackageLayout::looks_up_impl_blocks): Rust)
    /// contributes nothing: its calls bind through the one associated-item
    /// lookup ([`Ctx::lookup`](super::Ctx::lookup), S-607).
    pub(super) methods_by_self_type: HashMap<(String, String, String), Vec<NodeId>>,
    /// `(crate, name)` → how many type-like nodes ([`is_type_like`](super::is_type_like)) the crate
    /// declares under that name — whether a self type's base name denotes one
    /// type of the crate, which is what lets a candidate outside the caller's
    /// module be that type's method.
    pub(super) type_names: HashMap<(String, String), usize>,
    /// `(crate, name)` → the class-like nodes ([`is_class_like`](super::is_class_like)) of that name
    /// the crate declares, id-sorted, for each name a recorded self type spells
    /// — the type whose fields a receiver read binds among in a language whose
    /// methods sit at module level (Go, S-529; [`ReceiverShapes::self_type_class`](super::receiver_shapes::ReceiverShapes::self_type_class)).
    pub(super) self_type_classes: HashMap<(String, String), Vec<NodeId>>,
    /// `(type node, name)` → the required signatures of that name the type
    /// declares, id-sorted (S-537, [FR-RS-21]): an abstract class's bodyless
    /// method and an interface's, of a language of structural types
    /// (TypeScript). Kept out of every other lookup ([`bindable`](super::bindable)); only a call
    /// on a proven receiver of the type reads them
    /// ([`ProvenReceivers::resolve_structural_receiver_call`](super::proven_receivers::ProvenReceivers::resolve_structural_receiver_call)), which binds the declared
    /// method, never an implementation. Empty unless the run was given them
    /// ([`IndexBuilder::with_signatures`](super::IndexBuilder::with_signatures)).
    ///
    /// [FR-RS-21]: ../../../../docs/specs/requirements/FR-RS-21.md
    pub(super) declared_signatures: HashMap<(NodeId, String), Vec<NodeId>>,
    /// The callables a type never inherits from an interface it implements
    /// (S-609, [FR-RS-48]): an abstract one (no body) and one its plugin marks
    /// uninherited — a `static` or `private` interface member. Never a
    /// candidate at an interface level of a walk. Empty unless the run was
    /// given the store's facts ([`IndexBuilder::with_uninherited`](super::IndexBuilder::with_uninherited)).
    ///
    /// [FR-RS-48]: ../../../../docs/specs/requirements/FR-RS-48.md
    pub(super) uninherited: HashSet<NodeId>,
    /// Every unexported function (S-584, [FR-RS-40]); the link-unit rung reads
    /// only a link-unit file's — a C `static`, its own file's alone. `None`
    /// until the run is given the store's linkage facts
    /// ([`IndexBuilder::with_link_unit`](super::IndexBuilder::with_link_unit)) — and with none known, the rung binds
    /// nothing rather than offer a `static` to another file. A C/C++
    /// prototype pairs with no unexported definition, and with none known,
    /// with none at all ([`prototype_pairs`](super::prototype_pairs), S-820).
    ///
    /// [FR-RS-40]: ../../../../docs/specs/requirements/FR-RS-40.md
    pub(super) file_local: Option<HashSet<NodeId>>,
    /// A link-unit file's local bindings (S-584, S-593 facts): `(file, name)`
    /// → each binding's `(line, scope first line, scope last line)`. A call
    /// whose name one binds at the call's line — a function-pointer parameter
    /// or local — calls through it, never a function the rung could bind.
    /// Empty until the run is given them ([`IndexBuilder::with_link_unit`](super::IndexBuilder::with_link_unit)).
    pub(super) link_unit_locals: LocalScopes,
}
