//! The scope-hierarchy binder — the pure core of the resolution engine
//! (S-011, [FR-RS-01], [FR-RS-02], [FR-RS-03], [NFR-RA-05]).
//!
//! An [`IndexBuilder`] digests an immutable graph snapshot (nodes, `Contains`
//! edges, the reference ledger) into the lookup structures binding needs, the
//! [`Index`]; [`bind`] then resolves one ledger row against it. Nothing here touches the
//! store — the caller reads the snapshot and commits the outcomes — so the
//! whole algorithm is deterministic, `Sync`, and unit-testable in memory, and
//! the component's *parallel compute, serial commit* contract holds by
//! construction ([resolution-engine]).
//!
//! # The binding hierarchy ([FR-RS-03])
//!
//! A reference is tried against ever-wider scopes, in order:
//!
//! 1. **function-local / lexical** — the `Contains` ancestor chain of the
//!    referencing declaration, innermost first (a nested `fn`, the enclosing
//!    item, … up to the file module);
//! 2. **module** — the file-module scope is the last step of (1); sibling and
//!    child *file* modules resolve through the path-derived module tree;
//! 3. **imports** — the file's `use`-alias map (including `as` renames), then
//!    its glob imports ([FR-RS-01], [FR-RS-02]). A language whose import
//!    specifiers are *paths* (TypeScript, JavaScript, Go) reaches this rung
//!    differently: extraction records a call through an import qualified by
//!    the module it names, and [`ImportedScope::resolve_imported_call`](imported_scope::ImportedScope::resolve_imported_call) binds it within
//!    that import's **bound** targets — the `Imports` binding itself, never
//!    the name hierarchy, which cannot read a path (S-440);
//! 4. **crate** — explicit `crate::`/`self::`/`super::` paths and
//!    crate-name-headed paths through the module tree;
//! 5. **workspace** — policy-gated unique-candidate fallbacks
//!    ([`BindingPolicy`]).
//!
//! A file of a **package-shaped** language (Java, [CR-149]) is keyed by its
//! package, and one of a **declared-namespace** language (PHP, C#, Kotlin,
//! Scala; S-518, [FR-RS-13]) by the namespace it declares ([`PackageLayout`]).
//! Either, after the lexical chain, takes its own rungs instead of 2–5: its
//! single-type/static imports, the top-level types of its own package, those of
//! each namespace enclosing it when its plugin declares them (S-595,
//! [FR-RS-45]), what its wildcards bring into view, then a path read as a
//! fully-qualified name
//! ([`PackageNames::resolve_package_name`](package_names::PackageNames::resolve_package_name), [`PackagePaths::resolve_package_path`](package_paths::PackagePaths::resolve_package_path)). An import
//! binds to the type or member it names — a declared-namespace wildcard, which
//! names no type, to the files declaring the namespace
//! ([`PackagePaths::namespace_files`](package_paths::PackagePaths::namespace_files)) — and the workspace suffix match is never
//! consulted; the aggressive bare-name fallback alone stays policy-gated as
//! everywhere else.
//!
//! [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
//! [FR-RS-13]: ../../../docs/specs/requirements/FR-RS-13.md
//! [FR-RS-45]: ../../../docs/specs/requirements/FR-RS-45.md
//!
//! A file of an **import-root** language (Python; S-519, [FR-RS-14]) keeps
//! rungs 1–5, keyed under its family's own crate: a relative import reads its
//! `.`/`..` level from the file's package ([`ImportRootPaths::resolve_relative`](import_roots::ImportRootPaths::resolve_relative)), every
//! directory under its import root descends as a module, a `from pkg import
//! Name` that names nothing `pkg/__init__.py` declares binds to the package
//! ([`ImportRootPaths::package_reexport`](import_roots::ImportRootPaths::package_reexport)), and its workspace fallbacks never leave the
//! crate. The fully-qualified type and namespace indexes the package rungs read
//! are partitioned by interop family, so no source reaches another family's
//! types.
//!
//! [FR-RS-14]: ../../../docs/specs/requirements/FR-RS-14.md
//!
//! A call binds a `Function` or `Method`. A language whose plugin declares more
//! (S-521, [FR-RS-16]) looks its calls up with [`Want::DeclaredCall`]: a `Class`
//! it constructs, bound as `Instantiates` (Python, Kotlin, Scala), or a `Macro`
//! it expands, bound as `Calls` (C) — on the same rungs, under the same rule.
//!
//! [FR-RS-16]: ../../../docs/specs/requirements/FR-RS-16.md
//!
//! A file of an **impl-block** language (Rust; S-607, [FR-RS-47]) decides a
//! call to a type's method — `Self::m()`, `self.m()`, a proven `x.m()`, a
//! written `T::m()`, a qualified `<T as Tr>::m()` — by one associated-item
//! lookup ([`Ctx::lookup`]) among the functions of every `impl` block whose
//! header resolves to `T` ([`ImplIndex::assoc_items`](impl_index::ImplIndex::assoc_items)) and the trait defaults those
//! blocks lend it, retrying a method call on `T`'s `Deref` target (S-608).
//! Once a path's type segment is reached the lookup decides the call, whatever
//! it found: no wider rung reads the path again ([`Ctx::type_reached`]). A
//! trait-typed call — a trait body's `self.m()`, a `dyn`/`impl`/bound-typed
//! receiver, a written `Tr::m(x)` — fans out to every impl of the method and
//! the trait's default body ([`TraitDispatch::fan_out`](trait_dispatch::TraitDispatch::fan_out)).
//!
//! [FR-RS-47]: ../../../docs/specs/requirements/FR-RS-47.md
//!
//! # Never fabricate ([NFR-RA-05])
//!
//! Every level ends in the same acceptance rule: bind **iff the candidate set
//! has exactly one element**. Zero candidates falls through to the next level
//! (except where a path's type was reached, above);
//! two or more is [`Res::Ambiguous`] and aborts the whole attempt — escalating
//! past a *known* ambiguity is how mis-binds happen ([AR-05]). The policy knob
//! widens the search, never the acceptance rule.
//!
//! [resolution-engine]: ../../../docs/specs/architecture/components/resolution-engine.md
//! [FR-RS-01]: ../../../docs/specs/requirements/FR-RS-01.md
//! [FR-RS-02]: ../../../docs/specs/requirements/FR-RS-02.md
//! [FR-RS-03]: ../../../docs/specs/requirements/FR-RS-03.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [AR-05]: ../../../docs/specs/architecture.md#13-risk-register
//! [`BindingPolicy`]: crate::config::BindingPolicy

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use crate::config::BindingPolicy;
use crate::graph_store::{EdgeRow, NodeRow, UnresolvedRefRow};
use crate::model::{EdgeKind, NodeId, NodeKind, ReceiverShape, RefForm};
use crate::plugin::CallTargets;
use includes::IncludeScope;
use lexical_constants::LexicalConstants;
use row_arms::RowArms;
use unbound_reasons::UnboundReasons;
use impl_index::ImplHeader;
use super::package_key::{ModuleKey, PackageLayout};
use crate::model::dispatch::dispatch_payload;

// The include scope of an include-roots language — C++'s `#include`s, and
// the calls, `using`s and base classes read through them (S-579). See its
// module docs.
mod includes;

// The link-unit rung of a language whose files make one link unit — a C call
// across translation units (S-584). See its module docs.
mod link_unit;

// A bare call a local binding shadows — through a local import, or a call
// through a local value (S-594). See its module docs.
mod local_binding;

// The one associated-item lookup of an impl-block language (S-607, S-608) —
// a kernel method of `Ctx`, beside the rungs its recursion runs through.
mod lookup;

// The arm a row takes, by its kind and form — the body of `bind`.
mod row_arms;

// Why a call stays unbound, and the factory function that rivals a class (`UnboundReasons`).
mod unbound_reasons;

// The default binding hierarchy: a bare name, a path down the module tree, the workspace fallbacks (`ScopeHierarchy`).
mod scope_hierarchy;

// The paths an import-root file's imports name (`ImportRootPaths`).
mod import_roots;

// What a module's glob imports and re-exports bring into view (`RustGlobs`).
mod rust_globs;

// A call through the caller's own type, and the traits whose items it may call (`SelfTypes`).
mod self_types;

// A type relation bound to the one type of a kind it admits (`TypeRelations`).
mod type_relations;

// A call on a receiver whose type the file proves (`ProvenReceivers`).
mod proven_receivers;

// A receiver call bound by its receiver's shape, and a member access (`ReceiverShapes`).
mod receiver_shapes;

// A trait-typed call fanned out to every target it may dispatch to (`TraitDispatch`).
mod trait_dispatch;

// A receiver call bound to the one extension function in scope (`ExtensionCalls`).
mod extension_calls;

// Which declarations a bare call can reach (`CallReach`).
mod call_reach;

// A path-grammar module specifier bound to the files it names (`Specifiers`).
mod specifiers;

// A call or type relation bound within what an import binds to (`ImportedScope`).
mod imported_scope;

// A cross-artifact reference bound to the one artifact or code node it names (`Artifacts`).
mod artifacts;

// A bare name from a package-shaped or declared-namespace source (`PackageNames`).
mod package_names;

// A path from a package-shaped or declared-namespace source, and a fully-qualified name (`PackagePaths`).
mod package_paths;

// What a package-shaped file's wildcard imports bring into view (`Wildcards`).
mod wildcards;

// A row of a language that names its classes and modules by lexical constants (`LexicalConstants`).
mod lexical_constants;

// A path walked down from a type, and the companions and companion pairs it reads (`Companions`).
mod companions;

// A member found up a type's supertypes, or a class's ancestors (`SupertypeClimb`).
mod supertype_climb;

// A documentation link or code-name token bound to its one target (`DocReferences`).
mod doc_references;

// A C/C++ prototype paired with its definition in another file (S-820). See its
// module docs.
mod prototype_pairs;
pub(crate) use prototype_pairs::PairLinks;

// A bare Kotlin call inside a lambda with a receiver (S-834). See its module
// docs. Its mark, `LambdaReceiver`, and the proven twin are the kernel's, as
// the other aliases are: the rung and the sync rule
// (`Invalidation::reads_lambda_receiver`) read them from here, and neither the
// kernel nor the sync rule references anything in the rung.
mod receiver_lambdas;

// The lookup index's builder: the snapshot digest and every `with_*` step
// (S-828). See its module docs.
mod index_builder;
pub(crate) use index_builder::IndexBuilder;

// The lookup index's sub-indexes, each a concern with its own fields (S-828):
// the module tree and files, the layout-keyed declarations, the type
// hierarchy, the traits and impl blocks, and the store's per-declaration facts.
mod module_index;
pub(crate) use module_index::ModuleIndex;
mod package_index;
pub(crate) use package_index::PackageIndex;
mod hierarchy;
mod impl_index;
mod declaration_facts;

// Which rows a sync re-binds (`Invalidation`, S-828). See its module docs.
mod invalidation;
pub(crate) use invalidation::Invalidation;

/// A module's identity: `(crate name, module path segments)`.
type ModKey = ModuleKey;

/// `Contains` membership: scope → name → member nodes, each list id-sorted for
/// a deterministic candidate order ([NFR-RA-06]).
///
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
type Members = HashMap<NodeId, HashMap<String, Vec<NodeId>>>;

/// Which phase of a supertype walk [`SupertypeClimb::climb_levels`](supertype_climb::SupertypeClimb::climb_levels) climbs (S-609).
enum Levels<'a> {
    /// The `Extends` chain, collecting every type it crosses.
    Extends(&'a mut Vec<NodeId>),
    /// The interfaces the chain implements, where only an inheritable member
    /// is a candidate ([`DeclarationFacts::uninherited`](declaration_facts::DeclarationFacts::uninherited)).
    Interfaces,
}

/// What a supertype walk found ([`SupertypeClimb::supertype_walk`](supertype_climb::SupertypeClimb::supertype_walk), S-592).
#[derive(Debug, Clone, Copy)]
struct Climb {
    /// The deciding level's exactly-one, or [`Res::NotFound`] when no level
    /// held an applicable callable of the name.
    res: Res,
    /// Some level held callables of the name, none of which admits the call's
    /// argument count.
    inapplicable: bool,
    /// Every type the walk crossed has its supertypes in the graph
    /// ([`Hierarchy::open`](hierarchy::Hierarchy::open)): no level the walk could not see may hold the
    /// name.
    closed: bool,
}

/// The result of one binding attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Exactly one candidate was found: create `source --kind--> target`.
    Bound {
        source: NodeId,
        target: NodeId,
        kind: EdgeKind,
        /// The cross-artifact relation class to stamp on the edge
        /// ([`ArtifactRelation`](crate::model::ArtifactRelation) wire token) — set
        /// for an `ArtifactRef`/`ArtifactBinding` bind (CR-011), `None` for every
        /// code/doc/access bind.
        payload: Option<String>,
    },
    /// One reference that fans out to **several** targets — a Terraform local
    /// module call binding to every admitted `.tf` [`NodeKind::ConfigFile`] in its
    /// source directory (CR-011, [FR-CG-08]). The one cross-artifact relation whose
    /// single ledger row legitimately yields more than one edge: a module `source`
    /// names a *directory*, not a file, and pulls in every `.tf` under it. Each
    /// target becomes one edge sharing the relation payload; the row is resolved
    /// because at least one bound. `targets` is non-empty and `NodeId`-sorted, so
    /// the produced edge set is deterministic ([NFR-RA-06]). Still never-fabricate:
    /// every target is a real indexed `ConfigFile` ([NFR-RA-05]).
    ///
    /// Two code relations take the same shape for the same reason — one written
    /// reference that names a *set*: a provable `dyn T` call fanning out to the
    /// trait method's impls (S-281), and a Go import path naming a package
    /// directory, which binds to every non-test `.go` file in it (S-439). Both
    /// carry no payload. The trait fan-out alone carries a [`Dispatch`]
    /// (S-712): its implementor edges are marked, the trait method's is not.
    ///
    /// [FR-CG-08]: ../../../docs/specs/requirements/FR-CG-08.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    BoundMany {
        source: NodeId,
        targets: Vec<NodeId>,
        kind: EdgeKind,
        payload: Option<String>,
        /// The dispatch marker of a trait fan-out ([`TraitDispatch::fan_out`](trait_dispatch::TraitDispatch::fan_out)); `None`
        /// for every other set — a Terraform module call, a Go import path, a
        /// namespace wildcard — and for a fan-out that reaches no
        /// implementation.
        dispatch: Option<Dispatch>,
    },
    /// Zero candidates anywhere, or an ambiguity — the ref stays in the
    /// ledger and is retried on the next sync ([FR-RS-03]).
    ///
    /// [FR-RS-03]: ../../../docs/specs/requirements/FR-RS-03.md
    Unbound,
}

/// Which edges of a trait fan-out are dispatch (S-712, [CR-226] BR-76): the
/// ones to an implementation of the trait method, each stamped
/// [`dispatch_payload`]`(method)`. The edge to the trait's own method — its
/// default body — is the caller's dependency and stays unmarked.
///
/// [CR-226]: ../../../docs/requests/CR-226-dispatch-is-not-a-dependency.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Dispatch {
    /// The symbol of the trait method the call went through: its default
    /// body's, or its required signature's ([`ImplIndex::trait_method_symbol`](impl_index::ImplIndex::trait_method_symbol)).
    pub(crate) method: String,
    /// The targets reached as implementations, id-sorted: every target but the
    /// trait's own method. Never empty.
    pub(crate) implementors: Vec<NodeId>,
}

impl Dispatch {
    /// The payload of the edge to `target`: the marker for an implementor,
    /// `None` for the trait's own method.
    pub(crate) fn payload_of(&self, target: NodeId) -> Option<String> {
        self.implementors
            .binary_search(&target)
            .is_ok()
            .then(|| dispatch_payload(&self.method))
    }
}

/// A trait-typed call's fan-out ([`TraitDispatch::fan_out`](trait_dispatch::TraitDispatch::fan_out)): the set it binds, and
/// which of it is dispatch.
#[derive(Debug)]
struct Fan {
    targets: Vec<NodeId>,
    dispatch: Option<Dispatch>,
}

impl Outcome {
    /// The nodes a binding reached: its one target, its set, or none.
    pub(crate) fn into_targets(self) -> Vec<NodeId> {
        match self {
            Outcome::Bound { target, .. } => vec![target],
            Outcome::BoundMany { targets, .. } => targets,
            Outcome::Unbound => Vec::new(),
        }
    }
}

/// An intermediate lookup result. `Ambiguous` is sticky: once a scope level
/// *knows* there are two candidates, no wider level may overrule it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Res {
    Found(NodeId),
    NotFound,
    Ambiguous,
}

/// How a call to a type's method is written (S-607, [FR-RS-47]) — what decides
/// the one lookup's `takes_self` and arity filters ([`Ctx::lookup`]).
///
/// [FR-RS-47]: ../../../docs/specs/requirements/FR-RS-47.md
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Syntax {
    /// `self.m()`, a proven `x.m()`: the receiver is passed outside the
    /// argument list, and only a callable taking `self` is reached.
    Method,
    /// `Self::m()`, `T::m()`: every associated callable is reached, a
    /// self-taking one with its receiver as the first argument.
    Path,
}

/// One candidate of the associated-item lookup (S-608, [`Ctx::probe`]): a
/// function of an `impl` block, or a trait's default body that block lends its
/// type — `block` indexes [`ImplIndex::impl_headers`](impl_index::ImplIndex::impl_headers), whose header ranks it
/// (inherent or trait, and which trait).
#[derive(Debug, Clone, Copy)]
struct Candidate {
    id: NodeId,
    block: usize,
}

/// The trait a qualified `<T as Tr>::m` names (S-608, [FR-RS-47] rule 5): the
/// repository trait the call's scope resolves `Tr` to, or — a trait the graph
/// cannot place — its simple name, matched against the impl headers that name
/// an unplaced trait of that name ([`SelfTypes::resolve_qualified_call`](self_types::SelfTypes::resolve_qualified_call)).
///
/// [FR-RS-47]: ../../../docs/specs/requirements/FR-RS-47.md
#[derive(Debug, Clone, Copy)]
enum Named<'a> {
    Placed(NodeId),
    Unplaced(&'a str),
}

impl Named<'_> {
    /// Whether the `impl` block `header` is an impl of this trait.
    fn names(self, header: &ImplHeader) -> bool {
        match self {
            Named::Placed(tr) => header.trait_node == Some(tr),
            Named::Unplaced(simple) => {
                header.trait_node.is_none()
                    && header
                        .trait_path
                        .as_deref()
                        .is_some_and(|path| path.rsplit("::").next() == Some(simple))
            }
        }
    }
}

/// Why a package-shaped `Calls` row stays unbound (S-468, [CR-150] §3.2 C,
/// [FR-RS-10]) — the reason the per-language readout counts it under
/// ([FR-RS-09]). Recorded by the lookup that gave up, on the same walk that
/// binds, so the reason can never describe a different path than the bind took.
///
/// `type-in-another-member` is not here: one member's graph cannot tell another
/// member's type from a library's. An [`ExternalType`](Residue::ExternalType)
/// carries the names it tried, and a workspace sorts them against what the other
/// members declare.
///
/// [CR-150]: ../../../docs/requests/CR-150-java-receiver-typing-for-method-calls.md
/// [FR-RS-09]: ../../../docs/specs/requirements/FR-RS-09.md
/// [FR-RS-10]: ../../../docs/specs/requirements/FR-RS-10.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Residue {
    /// The file proves no receiver type: a bare Method-form row, or a bare call
    /// that names no import. In every language, a receiver call whose shape is
    /// `other` or absent (S-514), and a `self` / `super` call made from no class.
    NoReceiverEvidence,
    /// The receiver's type is declared by no file of this graph — the JDK, a
    /// library, a generated type, or another member. `candidates` are the
    /// fully-qualified names the type could be, in the order its scope reads
    /// them; a type nested under an in-graph type that does not declare it has
    /// none.
    ///
    /// Also a recorded-self-type `Self::m` call (S-493) whose self type the
    /// crate declares no type of, or whose caller's file imports it from
    /// outside the crate; and in an impl-block language (S-607) a call whose
    /// `T` — an impl header's, a proven receiver's — is no repository type
    /// (`String`, `Vec`, `std::io::Error`, an `impl` for a primitive or a
    /// generic parameter), a method a peeled wrapper provides (`Arc::clone`,
    /// S-588), a dyn call's external trait, and a path whose head leaves the
    /// repository (`Vec::new()`, `serde_json::to_string`); all with no
    /// candidates.
    ExternalType { candidates: Vec<Vec<String>> },
    /// The type (or the nearest supertype level holding the name) declares two
    /// or more callables of that name whose parameter range admits the call
    /// (S-592: no argument type is read), or two static imports each supply
    /// one.
    /// Also a recorded-self-type `Self::m` call whose self type — a name the
    /// crate declares for one type — records several `m` that the caller's
    /// module does not narrow to one (S-493, Go); and in an impl-block language
    /// two functions `m` of the deciding rank of the lookup's `T`, or an
    /// inherent `&self` method beside a trait's by-value one (S-607), and a
    /// bare call reaching two callables.
    OverloadAmbiguous,
    /// The type's name reaches two in-graph declarations (a `src/main` and a
    /// `src/test` class of one fully-qualified name). Also a recorded-self-type
    /// `Self::m` call whose self type's base name the crate declares for
    /// several types, when the caller's module does not decide the candidate
    /// (S-493, Go). Also a Rust `T` — a proven receiver's, an impl header's, a
    /// written path's — the scope names twice (two globs, two imports; S-588,
    /// S-607).
    TypeAmbiguous,
    /// The type is in the graph, and neither it nor any in-graph supertype
    /// declares the name: the chain leaves the graph (a JDK, library or
    /// other-member superclass, the implicit `Object`), stops at an interface,
    /// or cycles. Also a recorded-self-type `Self::m` call whose self type
    /// records no `m` in its crate (S-493, Go), and in an impl-block language
    /// a `T` whose impls hold no function `m` the call can reach — a derive
    /// supplies it, method syntax met only an associated function, or the
    /// trait is not in scope (S-607) — nor any trait default its impls lend
    /// it, nor its `Deref` chain (S-608); and a trait-typed call whose trait
    /// has neither an impl nor a default of `m` (a supertrait's is not read).
    SupertypeUnreached,
    /// Callables of the name were found, and none admits the call's argument
    /// count (S-592, [FR-RS-43]): every level of the walk holding the name
    /// held only callables whose parameter range excludes it, and no free
    /// function or import the language goes on to admits it either.
    ///
    /// [FR-RS-43]: ../../../docs/specs/requirements/FR-RS-43.md
    NoApplicableOverload,
    /// The call names a repository declaration that is not a callable (S-607,
    /// [FR-RS-47]): an enum variant (`E::A(1)`, `Self::A`), or a tuple-struct
    /// constructor (`W(1)`, `Self(1)`).
    ///
    /// [FR-RS-47]: ../../../docs/specs/requirements/FR-RS-47.md
    NotACallable,
    /// No rung reaches a repository callable of the name (S-607, [FR-RS-47]): a
    /// prelude or std function reached by a bare name (`drop`, `Some`), an
    /// item the file does not import, or a repository module that declares no
    /// such item. A closure or a local is [`Residue::LocalBinding`] (S-594).
    ///
    /// [FR-RS-47]: ../../../docs/specs/requirements/FR-RS-47.md
    NameNotInScope,
    /// A bare call names a parameter or a variable the caller's body binds
    /// where the call is (S-594, [FR-RS-44]): a call through a value, which
    /// this graph does not follow ([`LOCAL_VALUE_ALIAS`]).
    ///
    /// [FR-RS-44]: ../../../docs/specs/requirements/FR-RS-44.md
    LocalBinding,
}

/// Which node kinds satisfy a lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    /// A call target: `Function` or `Method`.
    Callable,
    /// A call target in a language whose plugin declares more than callables
    /// (S-521, [FR-RS-16]): a `Function` or `Method`, and every kind its
    /// [`CallTargets`] admit — a `Class` the call constructs, a `Macro` it
    /// expands. One candidate set under the one exactly-one rule, so a function
    /// and a class of one name in one scope are two candidates ([NFR-RA-05]).
    /// A language that declares neither looks up [`Want::Callable`], exactly as
    /// before ([`Ctx::call_want`]).
    ///
    /// [FR-RS-16]: ../../../docs/specs/requirements/FR-RS-16.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    DeclaredCall(CallTargets),
    /// An import target: any declared node (module preferred over a same-named
    /// item) — never a framework-promoted `route` or `component`
    /// ([`is_framework_promoted`]), which is derived from a declaration and is
    /// not what an import spells.
    Any,
    /// A glob's target: a module only.
    Module,
    /// A member-access target: a `Field` only (CR-005, [FR-EX-08]).
    ///
    /// [FR-EX-08]: ../../../docs/specs/requirements/FR-EX-08.md
    Field,
    /// An `Extends` from a class, or a Java `Instantiates`: a `Class` only
    /// (S-466, [CR-149]; S-522 for every language) — except where the language
    /// leaves a supertype's kind unsaid ([`Want::Supertype`]).
    ///
    /// [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    Class,
    /// An `Extends` from an interface: an `Interface` only (S-466, [CR-149]).
    ///
    /// [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    Interface,
    /// An `Implements` from a type: an `Interface` or a `Trait` (S-522,
    /// [FR-RS-15]) — a Java or PHP class's interface, a PHP class's `use`d
    /// trait.
    ///
    /// [FR-RS-15]: ../../../docs/specs/requirements/FR-RS-15.md
    Implemented,
    /// An `Extends` from a class whose language leaves the supertype's kind
    /// unsaid (S-522, [FR-RS-15]; C#'s `: B, IC`, Kotlin's `: Base(), Iface`):
    /// a `Class`, an `Interface` or a `Trait`, under one exactly-one rule. The
    /// edge kind then follows the target ([`relation_edge_kind`]).
    ///
    /// [FR-RS-15]: ../../../docs/specs/requirements/FR-RS-15.md
    Supertype,
    /// A Java `TypeUses`: any type-like node ([`is_type_like`]) (S-466,
    /// [CR-149]).
    ///
    /// [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    Type,
}

impl Want {
    fn admits(self, kind: NodeKind) -> bool {
        match self {
            Want::Callable => matches!(kind, NodeKind::Function | NodeKind::Method),
            Want::DeclaredCall(targets) => {
                Want::Callable.admits(kind)
                    || (targets.classes && kind == NodeKind::Class)
                    || (targets.macros && kind == NodeKind::Macro)
            }
            Want::Any => !is_framework_promoted(kind),
            Want::Module => kind == NodeKind::Module,
            Want::Field => kind == NodeKind::Field,
            Want::Class => kind == NodeKind::Class,
            Want::Interface => kind == NodeKind::Interface,
            Want::Implemented => matches!(kind, NodeKind::Interface | NodeKind::Trait),
            Want::Supertype => {
                matches!(kind, NodeKind::Class | NodeKind::Interface | NodeKind::Trait)
            }
            Want::Type => is_type_like(kind),
        }
    }

    /// `true` for a call's lookup, [`Want::Callable`] or [`Want::DeclaredCall`]:
    /// the rungs that never offer a module to a call, record a call's
    /// [`Residue`], and take a type's member through its supertypes treat the
    /// two alike — they differ only in what [`admits`](Want::admits).
    fn is_call(self) -> bool {
        matches!(self, Want::Callable | Want::DeclaredCall(_))
    }
}

/// `true` for the nodes the framework pass promotes beside the declaration they
/// are derived from (S-012): a `route` and a `component`. A component shares
/// its declaration's name and file — a Django model `Check` is a class and a
/// component — so a lookup that admitted both would read every import of the
/// class as ambiguous (S-519 measured 541 of healthchecks' 585 unbound internal
/// imports so). The promoted node is reached through its `References` edge to
/// the declaration, never by name.
fn is_framework_promoted(kind: NodeKind) -> bool {
    matches!(kind, NodeKind::Route | NodeKind::Component)
}

/// `true` for a class-bearing container whose lexically-enclosed `Field` members
/// a member access can resolve against (CR-005, [FR-EX-08]): a `Class`/`Struct`/
/// `Interface`/`Enum`/`Trait`. The scope a `self.x`/`this.x` access is bound
/// within — never wider, so an own-field access can never bind to a field of a
/// different type ([NFR-RA-05]).
///
/// [FR-EX-08]: ../../../docs/specs/requirements/FR-EX-08.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
///
/// Shared with extraction's receiver typing (S-467), whose `this` is the same
/// class-bearing container.
pub(crate) fn is_class_like(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Class | NodeKind::Struct | NodeKind::Interface | NodeKind::Enum | NodeKind::Trait
    )
}

/// `true` for kinds whose associated items collapse to module scope (the
/// `Type::func` rule — `impl` blocks are not captured scopes, S-007).
///
/// Shared with extraction, which records a generic arity on exactly these
/// kinds (S-602, `NodeFact::type_arity`) — the kinds the namespace rungs read
/// as types.
pub(crate) fn is_type_like(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Struct
            | NodeKind::Enum
            | NodeKind::Trait
            | NodeKind::Class
            | NodeKind::Interface
            | NodeKind::TypeAlias
    )
}

/// A top-level value a namespace declares beside its functions (S-831): the
/// kinds `build_top_level_callables` indexes that no call names.
fn is_top_level_term(kind: NodeKind) -> bool {
    matches!(kind, NodeKind::Variable | NodeKind::Constant)
}

/// Maximum alias-chain hops before a lookup gives up — terminates resolution
/// on a self-referential import cycle (the S-011 sprint test) instead of
/// recursing forever.
const MAX_ALIAS_DEPTH: u8 = 8;

/// Maximum `Deref` hops a method-syntax lookup retries through (S-608,
/// [`Ctx::lookup`]) — each type is visited once besides, so a cycle ends
/// sooner.
const MAX_DEREF_DEPTH: usize = 8;

/// Hard cap on the `Contains`-hierarchy walk in [`DocReferences::typed_owner`](doc_references::DocReferences::typed_owner) (S-039) — a
/// defensive bound against a malformed cycle, far above any real doc/code
/// nesting depth (markdown headings reach 6; module nesting follows directory
/// depth).
const MAX_CONTAINS_DEPTH: u32 = 64;

/// Hard cap on the levels [`SupertypeClimb::type_member`](supertype_climb::SupertypeClimb::type_member) climbs a Java type's in-repository
/// `Extends` chain (S-468, [CR-150] §3.2 B) — a bound beside the cycle guard,
/// far above any real hierarchy depth, so no malformed hierarchy can make the
/// walk unbounded.
///
/// [CR-150]: ../../../docs/requests/CR-150-java-receiver-typing-for-method-calls.md
const MAX_SUPERTYPE_DEPTH: usize = 64;

/// The per-file scope facts derived from the ledger's import rows.
///
/// A re-extracted file's import rows are replaced wholesale by `persist_file`
/// (delete + reinsert) *before* the resolution pass runs, and untouched
/// files' rows persist (flagged, not deleted, when bound) — so at bind time
/// every file's current `use` aliases are present here. A deferred call ref
/// in an untouched file therefore still sees its file's aliases on every
/// retry.
#[derive(Debug, Default)]
struct FileScope {
    /// In-scope name → the `::`-split path it abbreviates.
    aliases: HashMap<String, Vec<String>>,
    /// In-scope name → **every** distinct path the file's imports give it, in
    /// ledger order of first appearance — an import repeated verbatim is one
    /// path (S-519). The package rung reads this, not the first-wins `aliases`: two
    /// static imports may name one method overloaded across two types
    /// (`import static a.A.m; import static b.B.m;`, CR-149), and that name is
    /// then ambiguous rather than the first import's ([NFR-RA-05]).
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    alias_expansions: HashMap<String, Vec<Vec<String>>>,
    /// Glob-imported module paths (`use m::*` → `["m"]`), unresolved form. For a
    /// package-shaped file (CR-149) these are the non-static wildcards
    /// (`import a.b.*`, `import a.b.C.*`, C#'s `using A.B;` outside every
    /// namespace block), which bring
    /// **types** into scope only — the file's own, then the global ones in force
    /// over it ([`ModuleIndex::apply_global_globs`](module_index::ModuleIndex::apply_global_globs), S-518).
    globs: Vec<Vec<String>>,
    /// The globs among [`globs`](FileScope::globs) the file re-exports — a Rust
    /// `pub use inner::*;` (S-606's `exported` mark) — which a path through
    /// the file's module follows to a type the glob's module holds (S-607,
    /// [`RustGlobs::reexported_type`](rust_globs::RustGlobs::reexported_type)).
    exported_globs: Vec<Vec<String>>,
    /// A package-shaped file's **static** wildcards (`import static a.b.C.*`,
    /// recorded with the alias `*`, [`STATIC_WILDCARD_ALIAS`]): every static
    /// member of the named type comes into scope, methods and fields included.
    static_globs: Vec<Vec<String>>,
    /// A declared-namespace file's namespace wildcards written inside a
    /// namespace block (S-603, [`block_wildcard_alias`]), by the block's
    /// namespace, each list in ledger order. Never among
    /// [`globs`](FileScope::globs): a block's `using`s are read at its
    /// namespace's level of the walk, after that namespace's types and before
    /// the next level out ([`PackageNames::resolve_package_name`](package_names::PackageNames::resolve_package_name)).
    block_globs: HashMap<Vec<String>, Vec<Vec<String>>>,
}

/// The alias a static wildcard import's `Glob` row carries — "every static
/// member name of the type" (CR-149; C#'s `using static`, S-518). A non-static
/// wildcard carries none, a global one [`GLOBAL_WILDCARD_ALIAS`], and one
/// written inside a namespace block its block ([`block_wildcard_alias`]).
pub(crate) const STATIC_WILDCARD_ALIAS: &str = "*";

/// The alias a **global** namespace wildcard's `Glob` row carries — C#'s
/// `global using N;` (S-518, [FR-RS-13]): the namespace comes into view in every
/// file of the declaring file's language under the declaring file's directory,
/// not in that file alone. The directory stands in for the project, which the
/// `.csproj` would name and is not read; by .NET convention global usings sit
/// at the project root, and one declared deeper reaches fewer files, never more
/// ([NFR-RA-05]).
///
/// [FR-RS-13]: ../../../docs/specs/requirements/FR-RS-13.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
pub(crate) const GLOBAL_WILDCARD_ALIAS: &str = "global";

/// `true` when a global wildcard declared in directory `dir` (`""` at the
/// root) reaches the file at `path`: the file sits beneath it. The one
/// definition of a global `using`'s reach — the binder's scope and the
/// framework pass's candidacy ([`resolve::framework`](super::framework),
/// S-554) both ask it — whose extension half each caller adds.
pub(crate) fn global_wildcard_governs(dir: &str, path: &str) -> bool {
    dir.is_empty() || path.starts_with(&format!("{dir}/"))
}

/// The alias a namespace wildcard written **inside a namespace block** carries
/// (S-603, [CR-199], [FR-RS-45]): the block's namespace, braced — `{Acme.Sub}`
/// for the `using X;` of `namespace Acme.Sub { … }` or of a `namespace Acme.Sub;`
/// it follows, `{A}` for one of `namespace A { … }` around a nested
/// `namespace B`. Recorded only for a language that declares
/// `enclosing_namespaces`, where it is read at that namespace's level of the
/// walk ([`FileScope::block_globs`]) — recorded only for a block enclosing every
/// declaration of its file, whose `using`s apply to each. One in a block
/// enclosing none of them carries the empty block, `{}`, and is read nowhere;
/// one in a block enclosing only some, and a file-level wildcard, carry none.
/// No namespace spells a brace, so it is never [`STATIC_WILDCARD_ALIAS`] or
/// [`GLOBAL_WILDCARD_ALIAS`] (a global `using` is never inside a block).
///
/// [CR-199]: ../../../docs/requests/CR-199-csharp-resolution-honours-type-arity-and-using-scope.md
/// [FR-RS-45]: ../../../docs/specs/requirements/FR-RS-45.md
pub(crate) fn block_wildcard_alias(block: &[String]) -> String {
    format!("{{{}}}", super::package_key::namespace_text(block))
}

/// The head of a call through the caller's own type, `Self::m` (S-493,
/// [FR-RS-11]): what a written `Self::m()` records, and what extraction records
/// for a `self.m()` inside a method with a recorded self type.
///
/// [FR-RS-11]: ../../../docs/specs/requirements/FR-RS-11.md
pub(crate) const SELF_TYPE_HEAD: &str = "Self";

/// The head of a **fully-qualified** type-relation target (S-522,
/// [FR-RS-15]): PHP's `extends \Exception`, C#'s `: global::System.Exception`.
/// Such a name is read from the global namespace and nowhere else, so it is
/// bound by the fully-qualified index alone — never by the source's own
/// namespace first, where `namespace Foo; class Exception extends \Exception`
/// would name the class itself. No language spells a name `\`, so the head can
/// never be a type's or a package's.
///
/// [FR-RS-15]: ../../../docs/specs/requirements/FR-RS-15.md
pub(crate) const FULLY_QUALIFIED_HEAD: &str = "\\";

/// The alias a PHP trait `use`'s `Implements` row carries (S-522, [FR-RS-15]):
/// the class uses the trait, whose method outranks every inherited one — what
/// an `implements` never says. Its class's hierarchy ends there
/// (`build_supertypes`). A relation row has no alias otherwise.
///
/// [FR-RS-15]: ../../../docs/specs/requirements/FR-RS-15.md
pub(crate) const TRAIT_USE_ALIAS: &str = "use";

/// The alias a Ruby `prepend M`'s `Implements` row carries (S-568,
/// [FR-RS-35]): the module precedes the class among its ancestors, so a call
/// on the instance reads it before the class's own methods
/// ([`SupertypeClimb::ancestor_walk`](supertype_climb::SupertypeClimb::ancestor_walk)). An `include M` row carries no alias.
///
/// [FR-RS-35]: ../../../docs/specs/requirements/FR-RS-35.md
pub(crate) const MIXIN_PREPEND_ALIAS: &str = "prepend";

/// The alias a Ruby `extend M`'s `Implements` row carries (S-568,
/// [FR-RS-35]): the module's methods become the object's own singleton
/// methods, so they are no level of its instances' ancestor chain.
///
/// [FR-RS-35]: ../../../docs/specs/requirements/FR-RS-35.md
pub(crate) const MIXIN_EXTEND_ALIAS: &str = "extend";

/// The name a TypeScript-family module's default export is imported and
/// re-exported under (S-534, [FR-RS-19]): a default import `import App from
/// './app'` is a call through `<module>::default`, and `export default App`
/// records `App` as the module's `default`. `export *` never re-exports it.
///
/// [FR-RS-19]: ../../../docs/specs/requirements/FR-RS-19.md
pub(crate) const DEFAULT_EXPORT: &str = "default";

/// The name a CommonJS module's export object is recorded under (S-540,
/// [FR-RS-22]): `module.exports = X` and `const X = module.exports` record `X`
/// aliased `module.exports`, which no identifier can spell. The object's
/// members are the module's names, and the object is its `default`
/// ([`ModuleIndex::exported_definitions`](module_index::ModuleIndex::exported_definitions)).
///
/// [FR-RS-22]: ../../../docs/specs/requirements/FR-RS-22.md
pub(crate) const COMMONJS_EXPORTS: &str = "module.exports";

/// The alias a C# `partial` type declaration's own `Extends` row carries (S-557,
/// [FR-RS-29]): the declaration is one part of a type, and the parts sharing
/// one fully-qualified name are one logical type (`build_partial_types`). The
/// row names the declaration itself; a later part's binds to the first-declared
/// part, by file path order, and the first part's own stays unbound. Its edge
/// carries the payload [`PARTIAL_TYPE_ALIAS`] too, which is how the metrics
/// tell the link from a supertype ([`GraphStore::partial_type_links`]).
///
/// [FR-RS-29]: ../../../docs/specs/requirements/FR-RS-29.md
/// [`GraphStore::partial_type_links`]: crate::graph_store::GraphStore::partial_type_links
pub(crate) const PARTIAL_TYPE_ALIAS: &str = "partial";

/// The alias a bare call to a **local value** carries (S-594, [FR-RS-44]): a
/// parameter or a variable the caller's body binds is in force at the call
/// (`extract::local_shadow`), so it calls whatever value the name holds — data
/// flow this graph does not follow. It binds nothing and is counted
/// [`Residue::LocalBinding`]; it never falls through to a member or a
/// same-named module function. No identifier spells it.
///
/// [FR-RS-44]: ../../../docs/specs/requirements/FR-RS-44.md
pub(crate) const LOCAL_VALUE_ALIAS: &str = "local-value";

/// The alias a bare call to a **local import** carries (S-594, [FR-RS-44]): a
/// local `from m import f`, a block-scope `using ns::f` or a block `use` binds
/// the name, so the call resolves through that import alone
/// ([`LocalBindings::resolve_local_import_call`](local_binding::LocalBindings::resolve_local_import_call)), never the lexical scope that reaches
/// the caller itself or a same-named function the import hides. No identifier
/// spells it.
///
/// [FR-RS-44]: ../../../docs/specs/requirements/FR-RS-44.md
pub(crate) const LOCAL_IMPORT_ALIAS: &str = "local-import";

/// The alias of a bare call inside a lambda with a receiver (S-834,
/// [FR-RS-31]), alone when nothing proves the receiver ([`LambdaReceiver`]).
///
/// [FR-RS-31]: ../../../docs/specs/requirements/FR-RS-31.md
const RECEIVER_LAMBDA_ALIAS: &str = "receiver-lambda";

/// The suffix of a proven receiver's alias when another receiver lambda
/// encloses the lambda: past `T`, the enclosing one's receiver comes first.
const ENCLOSED: &str = ":enclosed";

/// What extraction proved of the receiver of the lambda a bare call sits in
/// (S-834, [FR-RS-31]), spelled as the row's alias
/// ([`alias`](LambdaReceiver::alias)): no identifier spells `receiver-lambda`,
/// `receiver-lambda:<T>` or `receiver-lambda:<T>:enclosed`. The
/// `receiver_lambdas` rung decides the row it marks.
///
/// [FR-RS-31]: ../../../docs/specs/requirements/FR-RS-31.md
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LambdaReceiver<'a> {
    /// The receiver's proven type, as the proven row `T::m` spells it.
    pub(crate) ty: Option<&'a str>,
    /// Whether another receiver lambda encloses this one.
    pub(crate) enclosed: bool,
}

impl<'a> LambdaReceiver<'a> {
    /// The alias the row carries.
    pub(crate) fn alias(self) -> String {
        match self.ty {
            None => RECEIVER_LAMBDA_ALIAS.to_string(),
            Some(ty) if self.enclosed => format!("{RECEIVER_LAMBDA_ALIAS}:{ty}{ENCLOSED}"),
            Some(ty) => format!("{RECEIVER_LAMBDA_ALIAS}:{ty}"),
        }
    }

    /// The mark `alias` spells; `None` for every other alias.
    pub(crate) fn of(alias: &'a str) -> Option<Self> {
        let rest = alias.strip_prefix(RECEIVER_LAMBDA_ALIAS)?;
        if rest.is_empty() {
            return Some(Self { ty: None, enclosed: false });
        }
        let ty = rest.strip_prefix(':').filter(|t| !t.is_empty())?;
        Some(match ty.strip_suffix(ENCLOSED) {
            Some(ty) => Self { ty: Some(ty), enclosed: true },
            None => Self { ty: Some(ty), enclosed: false },
        })
    }
}

/// `r`'s proven twin — the `T::m` row of shape `other` a call `x.m()` on a
/// receiver of the proven type `ty` records (S-834): what a bare call inside a
/// lambda whose receiver extraction proved is bound as first, and selected by
/// a sync as ([`Invalidation::reads_lambda_receiver`]).
fn proven_twin(r: &UnresolvedRefRow, ty: &str) -> UnresolvedRefRow {
    UnresolvedRefRow {
        target: format!("{ty}::{}", r.target),
        alias: None,
        form: RefForm::Path,
        receiver: Some(ReceiverShape::Other),
        ..r.clone()
    }
}

/// One node's binding-relevant facts.
#[derive(Debug)]
struct NodeInfo {
    kind: NodeKind,
    crate_name: String,
    /// The human-facing name — the heading text of a `DocSection`, the symbol
    /// name of a code node. Carried here for doc resolution (S-035), which
    /// matches a link's `#anchor` against a section's slugified name.
    name: String,
    /// The defining file's project-relative path, when bound to one — the key
    /// doc-link/path references resolve against (S-035).
    file_path: Option<String>,
}

/// The nodes and edges a binding run resolves against (S-606, [CR-202] F3):
/// every node but a **required signature** — a trait's bodyless
/// `fn m(&self);`, or a C-family prototype its own file defines (S-577) — and
/// every edge but those touching one (its `Contains` from its container). A signature is recorded so the lookup of [FR-RS-47] can read it,
/// and until that lookup says how a call reaches one it enters no candidate
/// set — save a call on a proven receiver of a structural-types language
/// (S-537), which reads an abstract class's or an interface's bodyless method
/// through [`DeclarationFacts::declared_signatures`](declaration_facts::DeclarationFacts::declared_signatures) alone: a `self.m()` in a trait's default body, a name-unique fallback and a
/// documentation name bind exactly what they bound before the signature was a
/// node. Borrowed unchanged when the graph holds no signature.
///
/// [CR-202]: ../../../docs/requests/CR-202-one-rust-associated-item-lookup.md
/// [FR-RS-47]: ../../../docs/specs/requirements/FR-RS-47.md
pub(crate) fn bindable<'a>(
    nodes: &'a [NodeRow],
    edges: &'a [EdgeRow],
    signatures: &HashSet<NodeId>,
) -> (Cow<'a, [NodeRow]>, Cow<'a, [EdgeRow]>) {
    if signatures.is_empty() {
        return (Cow::Borrowed(nodes), Cow::Borrowed(edges));
    }
    let nodes = nodes.iter().filter(|n| !signatures.contains(&n.id)).cloned().collect();
    let edges = edges
        .iter()
        .filter(|e| !signatures.contains(&e.source) && !signatures.contains(&e.target))
        .cloned()
        .collect();
    (Cow::Owned(nodes), Cow::Owned(edges))
}

/// The immutable lookup index one resolution run binds against, as its
/// builder yields it ([`IndexBuilder`]).
///
/// It holds the graph's node facts and `Contains` topology — the kernel every
/// concern reads — and owns one sub-index per query concern, each with its own
/// fields: the module tree and files ([`ModuleIndex`](module_index::ModuleIndex)),
/// the layout-keyed declarations ([`PackageIndex`](package_index::PackageIndex)),
/// the type hierarchy ([`Hierarchy`](hierarchy::Hierarchy)), the traits and
/// impl blocks ([`ImplIndex`](impl_index::ImplIndex)), the store's
/// per-declaration facts ([`DeclarationFacts`](declaration_facts::DeclarationFacts)), the
/// include scope ([`IncludeScope`]) and the prototype pairs. Which rows a sync
/// re-binds is read through [`Index::invalidation`].
pub(crate) struct Index {
    /// Canonical symbol string → node.
    by_symbol: HashMap<String, NodeId>,
    /// Per-node facts.
    info: HashMap<NodeId, NodeInfo>,
    /// `Contains`: child → parent.
    parent: HashMap<NodeId, NodeId>,
    /// `Contains`: scope → name → members, each list sorted by `NodeId`.
    members: Members,
    /// name → every node carrying it, sorted by `NodeId` (the unique-match
    /// fallback universe) — the parts of a partial type as its first-declared
    /// part alone (S-557, [`Index::partials`]).
    by_name: HashMap<String, Vec<NodeId>>,
    /// The parts of every partial type (S-557, [FR-RS-29];
    /// [`build_partial_types`](index_builder)): the declarations of one fully-qualified name
    /// its plugin marks partial ([`PARTIAL_TYPE_ALIAS`]), which are one logical
    /// type. Member lookup reads the union of the parts' members
    /// ([`Index::members_named`]), the type's name names its first-declared
    /// part ([`types_by_fqn`](package_index::PackageIndex::types_by_fqn) and
    /// [`by_name`](Index::by_name) hold that part alone), and every part has
    /// the union of the parts' supertypes ([`PartialTypes::unite_hierarchy`]).
    /// Each part keeps its own node. Empty for every language without partial
    /// types.
    ///
    /// [FR-RS-29]: ../../../docs/specs/requirements/FR-RS-29.md
    partials: PartialTypes,
    /// How every file is keyed: by its package ([CR-149]), by the namespace it
    /// declares (S-518), or by its path — under its plugin's package-file stems
    /// and import roots — plus each language's interop family (S-519). The
    /// registry's layout in production, with the import roots detected from
    /// this graph's files ([`IndexBuilder::new`]); Rust's stems alone
    /// for a synthetic test graph (`Index::build`, the tests' shorthand).
    ///
    /// [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    layout: PackageLayout,
    /// What the include-roots languages' files include and see through it
    /// (S-579, [`includes`]): read off the graph's files and `#include` rows
    /// when the index is built, before the supertype hierarchy, whose C++
    /// bases are declared in the headers a class's file includes.
    includes: IncludeScope,
    /// The include-roots languages' parameter shapes and the cross-file
    /// prototype/definition pairs they make (S-820, [`prototype_pairs`]).
    /// Empty unless the run was given the store's shapes
    /// ([`IndexBuilder::with_prototype_pairs`]).
    pairs: prototype_pairs::PrototypePairs,
    /// The module tree, the files and their scopes, the path-specifier context
    /// and the imported scope (S-828).
    modules: module_index::ModuleIndex,
    /// The fully-qualified declarations a package-shaped, declared-namespace
    /// or lexical-constant file names, and the types' arities (S-828).
    packages: package_index::PackageIndex,
    /// Each type's supertypes, interfaces and mixins, and what a sync reads of
    /// them (S-828).
    hierarchy: hierarchy::Hierarchy,
    /// The traits' fan-out universe and the recorded `impl` blocks (S-828).
    impls: impl_index::ImplIndex,
    /// What the store records per declaration beside its rows (S-828).
    facts: declaration_facts::DeclarationFacts,
}

impl Index {
    /// The file-root module of `node`: its outermost ancestor in the same file
    /// (itself for a file's module).
    fn file_root_of(&self, node: NodeId) -> NodeId {
        let file = self.info.get(&node).and_then(|i| i.file_path.as_deref());
        let mut root = node;
        while let Some(&parent) = self.parent.get(&root) {
            if self.info.get(&parent).and_then(|i| i.file_path.as_deref()) != file {
                break;
            }
            root = parent;
        }
        root
    }

    /// The project-relative file of node `id`, when it has one — how the
    /// framework pass reads a
    /// [`package_types`](package_index::PackageIndex::package_types) answer's
    /// file (S-470) without a second node → file map.
    pub(crate) fn file_of(&self, id: NodeId) -> Option<&str> {
        self.info.get(&id).and_then(|info| info.file_path.as_deref())
    }

    /// The one workspace [`NodeKind::Trait`] node named `name`, or `None` when
    /// zero or several carry the name — the never-fabricate acceptance rule
    /// ([NFR-RA-05]) applied to trait resolution: a `dyn T` call whose trait is
    /// ambiguous or external stays an honest miss rather than guessing one.
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn trait_by_name(&self, name: &str) -> Option<NodeId> {
        unique_trait(&self.by_name, &self.info, name)
    }

    /// Members of `scope` named `name`, filtered by `want`, id-sorted.
    ///
    /// The members of a partial type's part are those of every part (S-557,
    /// [FR-RS-29]), and a member that is itself a part of a partial type is
    /// read as its first-declared part ([`Index::partials`]).
    ///
    /// [FR-RS-29]: ../../../docs/specs/requirements/FR-RS-29.md
    fn members_named(&self, scope: NodeId, name: &str, want: Want) -> Vec<NodeId> {
        let admitted = |id: &NodeId| self.info.get(id).is_some_and(|i| want.admits(i.kind));
        let named = |scope: &NodeId| self.members.get(scope).and_then(|m| m.get(name));
        if self.partials.first.is_empty() {
            return named(&scope).map(|ids| ids.iter().copied().filter(admitted).collect()).unwrap_or_default();
        }
        let mut ids: Vec<NodeId> = self
            .partials
            .parts_of(&scope)
            .iter()
            .filter_map(named)
            .flatten()
            .map(|&id| self.partials.first_of(id))
            .filter(admitted)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// The module key of the nearest enclosing module of `node` (itself
    /// included), if any.
    fn nearest_module(&self, node: NodeId) -> Option<&ModKey> {
        let mut cursor = Some(node);
        while let Some(id) = cursor {
            if let Some(key) = self.modules.module_key.get(&id) {
                return Some(key);
            }
            cursor = self.parent.get(&id).copied();
        }
        None
    }

    /// How every file is keyed ([`PackageLayout`]) — the layout predicates a
    /// pass outside the binder reads (`resolve::is_affected`).
    pub(crate) fn layout(&self) -> &PackageLayout {
        &self.layout
    }

    /// The module tree and files, for a pass outside the binder.
    pub(crate) fn modules(&self) -> &module_index::ModuleIndex {
        &self.modules
    }

    /// The layout-keyed declarations, for a pass outside the binder (the
    /// framework pass, S-470; a workspace's member comparison, S-468).
    pub(crate) fn packages(&self) -> &package_index::PackageIndex {
        &self.packages
    }

    /// Which rows a sync re-binds, read off this index ([`Invalidation`]).
    pub(crate) fn invalidation(&self) -> Invalidation<'_> {
        Invalidation { ix: self }
    }
}


/// The parts of the partial types (S-557, [FR-RS-29], [`Index::partials`]).
///
/// [FR-RS-29]: ../../../docs/specs/requirements/FR-RS-29.md
#[derive(Debug, Default)]
struct PartialTypes {
    /// Each part of a partial type → its first-declared part, the type's
    /// canonical node; the first part maps to itself.
    first: HashMap<NodeId, NodeId>,
    /// Each first-declared part → every part of its type, in declaration order
    /// (file path, then line).
    parts: HashMap<NodeId, Vec<NodeId>>,
    /// Every type declaration its plugin marks partial, a part of a type or
    /// not ([`PARTIAL_TYPE_ALIAS`]) → its arity, the type-parameter count its
    /// row records — what a sync re-selects the rows inside of
    /// ([`Invalidation::ref_affected`](Invalidation::ref_affected)).
    marked: HashMap<NodeId, u32>,
}

impl PartialTypes {
    /// The first-declared part of `id`'s type, or `id` itself when it is no
    /// part.
    fn first_of(&self, id: NodeId) -> NodeId {
        self.first.get(&id).copied().unwrap_or(id)
    }

    /// Every part of `id`'s type, or `id` alone when it is no part.
    fn parts_of<'a>(&'a self, id: &'a NodeId) -> &'a [NodeId] {
        self.first
            .get(id)
            .and_then(|first| self.parts.get(first))
            .map_or(std::slice::from_ref(id), Vec::as_slice)
    }
}

/// The one workspace [`NodeKind::Trait`] node named `name`, or `None` when zero
/// or several carry it — the single source of truth for the never-fabricate
/// trait-resolution rule ([NFR-RA-05]) shared by [`Index::trait_by_name`] (query
/// time) and `build_impls_by_trait_method` (index-build time), so the two can
/// never drift.
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn unique_trait(
    by_name: &HashMap<String, Vec<NodeId>>,
    info: &HashMap<NodeId, NodeInfo>,
    name: &str,
) -> Option<NodeId> {
    let traits: Vec<NodeId> = by_name
        .get(name)
        .into_iter()
        .flatten()
        .copied()
        .filter(|id| info.get(id).is_some_and(|i| i.kind == NodeKind::Trait))
        .collect();
    match traits.as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}

/// The kind of type a type-relation row may bind to (S-466, [CR-149] §3.2 B;
/// S-522, [FR-RS-15]), or `None` when `r` is not one. Only a `Path` (or its
/// capture-before-delete `Symbol`) row is, and of two shapes:
///
/// - an `Extends`, `Implements`, `Instantiates` or `TypeUses` from a
///   package-shaped source — Java, and a declared-namespace file whose
///   namespace is known (S-518);
/// - in any language, an `Extends` or `Implements` whose source is a
///   class-like declaration ([`is_class_like`]): a Python, PHP, C# or Kotlin
///   type's supertype (S-522), bound through whichever module model its
///   language declares — the package rungs, a declared namespace, or the path
///   modules of [FR-RS-14].
///
/// - in a language of structural types (`structural`, TypeScript, S-535,
///   [FR-EX-17]), every relation, from any declaration: a function's parameter
///   type, a module-level variable's, a `new` in a method.
///
/// `Extends` relates like to like, so it reads the source's own kind: an
/// interface's binds an interface, a class's a class — or, where its language
/// leaves the kind unsaid (`kind_follows_target`, C# and Kotlin), a class, an
/// interface or a trait, its edge kind following the target
/// ([`relation_edge_kind`]). `Implements` binds an interface or a trait. In a
/// structural language an interface's `Extends` and every `Implements` bind any
/// type-like declaration ([`Want::Type`]): TypeScript implements an abstract
/// class and extends a class into an interface; a class's `Extends` still
/// binds a class.
///
/// Every other row gets `None` and keeps the arm it had. That includes Rust's
/// `Implements`, which is sourced at an impl **method**, so the S-281 trait bind
/// and its `dyn` fan-out are untouched.
///
/// [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
/// [FR-EX-17]: ../../../docs/specs/requirements/FR-EX-17.md
/// [FR-RS-14]: ../../../docs/specs/requirements/FR-RS-14.md
/// [FR-RS-15]: ../../../docs/specs/requirements/FR-RS-15.md
fn type_relation_want(
    r: &UnresolvedRefRow,
    source_kind: Option<NodeKind>,
    package_shaped: bool,
    kind_follows_target: bool,
    structural: bool,
) -> Option<Want> {
    if !matches!(r.form, RefForm::Path | RefForm::Symbol) {
        return None;
    }
    let from_type = source_kind.is_some_and(is_class_like);
    let supertype = from_type && matches!(r.kind, EdgeKind::Extends | EdgeKind::Implements);
    if !package_shaped && !supertype && !structural {
        return None;
    }
    match r.kind {
        EdgeKind::Extends | EdgeKind::Implements
            if structural && (r.kind == EdgeKind::Implements || source_kind == Some(NodeKind::Interface)) =>
        {
            Some(Want::Type)
        }
        EdgeKind::Extends if source_kind == Some(NodeKind::Interface) => Some(Want::Interface),
        EdgeKind::Extends if from_type && kind_follows_target => Some(Want::Supertype),
        EdgeKind::Extends | EdgeKind::Instantiates => Some(Want::Class),
        EdgeKind::Implements => Some(Want::Implemented),
        EdgeKind::TypeUses => Some(Want::Type),
        _ => None,
    }
}

/// [`type_relation_want`] for `r`, read off its source node `source` and its
/// file's module model in `layout` — the one reading every caller shares: the
/// bind itself, the hierarchy build and the trait fan-out's exclusion.
fn relation_want_of(
    r: &UnresolvedRefRow,
    source: Option<&NodeInfo>,
    layout: &PackageLayout,
) -> Option<Want> {
    let path = source.and_then(|i| i.file_path.as_deref());
    if path.is_some_and(|p| layout.resolves_lexical_constants(p)) {
        if let Some(want) = ancestry_want(r, source.map(|i| i.kind)) {
            return Some(want);
        }
    }
    type_relation_want(
        r,
        source.map(|i| i.kind),
        path.is_some_and(|p| layout.is_package_shaped(p)),
        path.is_some_and(|p| layout.supertype_kind_follows_target(p)),
        path.is_some_and(|p| layout.has_structural_types(p)),
    )
}

/// The kind a Ruby ancestry row binds (S-568, [FR-RS-35]), or `None` when `r`
/// is none: a class's superclass (`Extends`) binds a class, and a class's or
/// module's mixin (`Implements`: `include`, `prepend`, `extend`) a module —
/// Ruby mixes in nothing else. Read through the constant index
/// ([`LexicalConstants::constant_relation`](lexical_constants::LexicalConstants::constant_relation)).
///
/// [FR-RS-35]: ../../../docs/specs/requirements/FR-RS-35.md
fn ancestry_want(r: &UnresolvedRefRow, source_kind: Option<NodeKind>) -> Option<Want> {
    if !matches!(r.form, RefForm::Path | RefForm::Symbol) {
        return None;
    }
    match (r.kind, source_kind?) {
        (EdgeKind::Extends, NodeKind::Class) => Some(Want::Class),
        (EdgeKind::Implements, NodeKind::Class | NodeKind::Module) => Some(Want::Module),
        _ => None,
    }
}

/// The edge a type relation looked up with `want` records once it binds a node
/// of `target` kind (S-522, [FR-RS-15]): the row's own kind, except that a
/// supertype whose syntax was silent ([`Want::Supertype`]) records
/// `Implements` to an interface or a trait — as a call that binds a class
/// records `Instantiates` ([`Ctx::constructs`]).
///
/// [FR-RS-15]: ../../../docs/specs/requirements/FR-RS-15.md
fn relation_edge_kind(row_kind: EdgeKind, want: Want, target: Option<NodeKind>) -> EdgeKind {
    match (want, target) {
        (Want::Supertype, Some(NodeKind::Interface | NodeKind::Trait)) => EdgeKind::Implements,
        _ => row_kind,
    }
}

/// What one exported `Imports` row of a path-grammar file says (S-534,
/// [FR-RS-19]; `extract::ts_export_rows`).
///
/// [FR-RS-19]: ../../../docs/specs/requirements/FR-RS-19.md
enum TsExport<'r> {
    /// `export { local as exported }`, `export default local`: a
    /// single-segment target, the file's own declaration.
    Local { local: &'r str, exported: &'r str },
    /// `export { name as exported } from 'module'`: `<module>::<name>`.
    Named { module: &'r str, name: &'r str, exported: &'r str },
    /// `export * from 'module'`: a `Glob` row.
    Star { module: &'r str },
}

/// The export `r` records, or `None` for a row not marked exported — every
/// import, and every row of a language that marks none. Read only for a
/// path-grammar file's row: a Rust `pub use` is followed by its own rungs.
fn ts_export_of(r: &UnresolvedRefRow) -> Option<TsExport<'_>> {
    if r.kind != EdgeKind::Imports || r.exported != Some(true) {
        return None;
    }
    match r.form {
        RefForm::Glob => Some(TsExport::Star { module: &r.target }),
        RefForm::Path => Some(match r.target.rsplit_once("::") {
            Some((module, name)) => TsExport::Named {
                module,
                name,
                exported: r.alias.as_deref().unwrap_or(name),
            },
            None => TsExport::Local {
                local: &r.target,
                exported: r.alias.as_deref()?,
            },
        }),
        _ => None,
    }
}

/// Whether a node of `kind` is what a call through an import can name
/// (S-440, S-534): a top-level function — an arrow-bound `const` and a wrapped
/// component included — or a top-level value binding, such as the `default`
/// symbol of `export default registerAs(…)` or `const Dialog =
/// Primitive.Root`. A class is reached through `new` or its members, never
/// called.
fn is_imported_call_target(kind: NodeKind) -> bool {
    matches!(kind, NodeKind::Function | NodeKind::Constant | NodeKind::Variable)
}

/// `path` with its trailing extension removed when it is one of `targets` —
/// the extensions that name the imported file itself (`src/index.ts`, a Ruby
/// `require 'rack/lint.rb'`); any other extension is kept.
fn strip_target_extension(path: String, targets: &HashSet<String>) -> String {
    match path.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.contains('/') && targets.contains(&ext.to_ascii_lowercase()) => {
            stem.to_string()
        }
        _ => path,
    }
}

/// Reduce a candidate list to a [`Res`] — the single acceptance rule every
/// scope level shares ([NFR-RA-05]: exactly one, or nothing).
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn exactly_one(candidates: &[NodeId]) -> Res {
    match candidates {
        [one] => Res::Found(*one),
        [] => Res::NotFound,
        _ => Res::Ambiguous,
    }
}

/// Bind one ledger row against the index under `policy`.
pub(crate) fn bind(r: &UnresolvedRefRow, ix: &Index, policy: BindingPolicy) -> Outcome {
    bind_traced(r, ix, policy).0
}

/// The instance method `action` of the class the constant `constant` names,
/// read from the top level, as seen from `source` — a route's `controller#action`
/// handler (S-567, [FR-FW-03]; [`LexicalConstants::constant_action`](lexical_constants::LexicalConstants::constant_action)). `None` outside a
/// lexical-constant language, and wherever exactly one method is not proven
/// ([NFR-RA-05]).
///
/// [FR-FW-03]: ../../../docs/specs/requirements/FR-FW-03.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
pub(crate) fn bind_constant_action(
    ix: &Index,
    source: NodeId,
    constant: &[String],
    action: &str,
    policy: BindingPolicy,
) -> Option<NodeId> {
    let ctx = Ctx::at(ix, source, None, policy, action);
    let constants = LexicalConstants { cx: &ctx };
    ctx.lexical_constants.then(|| constants.constant_action(constant, action)).flatten()
}

#[cfg(test)]
thread_local! {
    /// [`ScopeHierarchy::resolve_path`](scope_hierarchy::ScopeHierarchy::resolve_path) entries on this thread, which the binder's own
    /// tests read to bound the work one bind does (S-519) instead of timing it.
    static PATH_VISITS: Cell<u64> = const { Cell::new(0) };
}

/// [`bind`], and how many times it entered [`ScopeHierarchy::resolve_path`](scope_hierarchy::ScopeHierarchy::resolve_path) — a work
/// count a test can compare across inputs deterministically.
#[cfg(test)]
pub(crate) fn bind_counting_path_visits(r: &UnresolvedRefRow, ix: &Index, policy: BindingPolicy) -> (Outcome, u64) {
    PATH_VISITS.with(|v| v.set(0));
    let outcome = bind(r, ix, policy);
    (outcome, PATH_VISITS.with(Cell::get))
}

/// Why `r` — a `Calls` row from a package-shaped source or an impl-block
/// language (every one of its rows, S-607), or in any other language a
/// `Self::m` call through the caller's recorded self type (S-493), a
/// receiver call bound by its shape (S-514) or a call through a local binding
/// (S-594) — stays unbound (S-468, [CR-150]
/// §3.2 C), or `None` when it is no such row, or when it binds
/// now: a capture-before-delete `Symbol` row, or a row the ledger holds unbound
/// that this binder binds (a graph bound by an older binary, or a sync that did
/// not re-select it). The readout counts those apart as unclassified.
///
/// [CR-150]: ../../../docs/requests/CR-150-java-receiver-typing-for-method-calls.md
pub(crate) fn residue(r: &UnresolvedRefRow, ix: &Index, policy: BindingPolicy) -> Option<Residue> {
    if r.kind != EdgeKind::Calls || r.form == RefForm::Symbol {
        return None;
    }
    let (outcome, miss) = bind_traced(r, ix, policy);
    if outcome != Outcome::Unbound {
        return None;
    }
    let source_file = ix
        .by_symbol
        .get(&r.source_symbol)
        .and_then(|id| ix.info.get(id))
        .and_then(|i| i.file_path.as_deref())?;
    if ix.layout.looks_up_impl_blocks(source_file) {
        // Every unbound call of an impl-block language carries a reason
        // (S-607, [FR-RS-47]): the lookup's own, else the one its path's
        // resolution gave up with ([`UnboundReasons::unbound_reason`](unbound_reasons::UnboundReasons::unbound_reason)).
        //
        // [FR-RS-47]: ../../../docs/specs/requirements/FR-RS-47.md
        return Some(miss.unwrap_or_else(|| {
            let source = ix.by_symbol[&r.source_symbol];
            let ctx = Ctx::for_row(r, ix, source, policy);
            let unbound_reasons = UnboundReasons { cx: &ctx };
            unbound_reasons.unbound_reason(r)
        }));
    }
    if !ix.layout.is_package_shaped(source_file) {
        // Outside a package-shaped language only a call through the caller's
        // recorded self type (S-493, Go's S-509,
        // [`SelfTypes::resolve_recorded_self_type_call`](self_types::SelfTypes::resolve_recorded_self_type_call)), a receiver call (S-514,
        // the `RefForm::Method` arm) and a call through a local binding
        // (S-594, [`LocalBindings::resolve_local_binding_call`](local_binding::LocalBindings::resolve_local_binding_call)) record a reason.
        return miss;
    }
    Some(miss.unwrap_or(match r.form {
        // A type-qualified row the walk gave up on without naming why: its
        // head was never reached as a type (an import chain past the alias
        // depth). Reported where the type is sought, never as a guess at a
        // member.
        RefForm::Path if r.target.contains("::") => Residue::ExternalType {
            candidates: Vec::new(),
        },
        _ => Residue::NoReceiverEvidence,
    }))
}

/// [`bind`], and the [`Residue`] the first package-shaped call lookup that gave
/// up recorded — `None` when none did (outside a package-shaped language, every
/// row but a `Self::m` call through the caller's own type, S-493, a receiver
/// call, S-514, and a call the associated-item lookup decided, S-607).
/// Meaningful only for an
/// [`Outcome::Unbound`]: a row that binds may still carry the miss of a rung it
/// tried first (one of two static imports naming an external type).
fn bind_traced(
    r: &UnresolvedRefRow,
    ix: &Index,
    policy: BindingPolicy,
) -> (Outcome, Option<Residue>) {
    // No source node, no edge — a captured ref whose source file was removed
    // stays unbound (and harmless) until its file returns.
    let Some(&source) = ix.by_symbol.get(&r.source_symbol) else {
        return (Outcome::Unbound, None);
    };
    let source_info = ix.info.get(&source);
    let relation = relation_want_of(r, source_info, &ix.layout);
    let ctx = Ctx::for_row(r, ix, source, policy);
    ctx.scope_only.set(relation.is_some() || is_proven_receiver_call(r));
    let row_arms = RowArms { cx: &ctx };
    let outcome = match (row_arms.bind(r, relation), ctx.fanned.take()) {
        (Outcome::Unbound, Some(Fan { targets, dispatch })) => Outcome::BoundMany {
            source,
            targets,
            kind: EdgeKind::Calls,
            payload: None,
            dispatch,
        },
        (outcome, _) => outcome,
    };
    (outcome, ctx.miss.into_inner())
}

/// The argument count `r`'s candidates are filtered by (S-592, [FR-RS-43]), or
/// `None` when the row is filtered by none: its count, or its file's language's
/// arity, is unknown — a JavaScript file enforces
/// none ([`PackageLayout::checks_arity`]) — or it takes no arm that chooses
/// among same-named callables by their arguments.
///
/// The arms that do: a receiver call of shape `self` or `super` ([FR-RS-12]),
/// in every language, and every Path-form call — bare, or typed `T::m` — of a
/// language that overloads callables ([`PackageLayout::overloads_calls`]:
/// Java, Kotlin, Scala, C#, C++). None of those counts an explicit receiver
/// among its arguments. A path a language without overloading writes —
/// Python's `Base.__init__(self, x)` — and a Go `Self::m` call (S-493) keep
/// binding by name, as before. An impl-block language's call to a type's
/// method (Rust's `Self::m`, a proven `x.m()`, `S::c(&self, 6)`) is filtered
/// by its own syntax-aware count instead ([`Ctx::call_args`],
/// [`Ctx::admitted_by_count`], S-607).
///
/// [FR-RS-12]: ../../../docs/specs/requirements/FR-RS-12.md
/// [FR-RS-43]: ../../../docs/specs/requirements/FR-RS-43.md
/// [`PackageLayout::checks_arity`]: crate::resolve::package_key::PackageLayout::checks_arity
/// [`PackageLayout::overloads_calls`]: crate::resolve::package_key::PackageLayout::overloads_calls
fn filtered_count(r: &UnresolvedRefRow, source_file: Option<&str>, layout: &PackageLayout) -> Option<u32> {
    let file = source_file?;
    if r.kind != EdgeKind::Calls || !layout.checks_arity(file) {
        return None;
    }
    let receiver_shaped = r.form == RefForm::Method
        && matches!(r.receiver, Some(ReceiverShape::SelfInstance | ReceiverShape::Super));
    let overloaded_path = r.form == RefForm::Path && layout.overloads_calls(file);
    (receiver_shaped || overloaded_path)
        .then_some(r.arg_count)
        .flatten()
}

/// `true` for a row naming a type as written — a supertype, a `new`, a type
/// use — whose `arg_count` records the written type's arity where its
/// language tells types apart by it (S-602, [`Ctx::type_arity`]).
fn is_type_relation(kind: EdgeKind) -> bool {
    matches!(kind, EdgeKind::Extends | EdgeKind::Implements | EdgeKind::Instantiates | EdgeKind::TypeUses)
}

/// `true` for a call retyped from its proven receiver — a Rust one (S-587),
/// a TypeScript one (S-537), a Python one (S-546) or a Kotlin one (S-563):
/// the Path-form `T::m` of shape `other`, which nothing else records — a
/// written `T::m()` has no receiver shape. A Rust call is bound among `T`'s
/// methods alone (S-588, [`ProvenReceivers::resolve_proven_receiver_call`](proven_receivers::ProvenReceivers::resolve_proven_receiver_call)), a TypeScript
/// one among `T`'s members up its proven `Extends`
/// ([`ProvenReceivers::resolve_structural_receiver_call`](proven_receivers::ProvenReceivers::resolve_structural_receiver_call)), a Python one among its
/// class's methods up that chain ([`ProvenReceivers::resolve_import_root_receiver_call`](proven_receivers::ProvenReceivers::resolve_import_root_receiver_call)),
/// and a Kotlin one among `T`'s own members, never its companion's
/// ([`ProvenReceivers::resolve_companion_language_receiver_call`](proven_receivers::ProvenReceivers::resolve_companion_language_receiver_call));
/// `T` is read by scope and module model only ([`Ctx::scope_only`]): a
/// same-named type the file does not import is not the type it proved.
pub(crate) fn is_proven_receiver_call(r: &UnresolvedRefRow) -> bool {
    r.kind == EdgeKind::Calls && r.form == RefForm::Path && r.receiver == Some(ReceiverShape::Other)
}

/// `true` for the documentation node kinds — the doc→code matcher excludes
/// these so a code-name token binds only to *code* ([FR-DG-04]). Thin alias
/// over the canonical [`NodeKind::is_doc`] so the rule has one source of truth;
/// it equals the file-level ∪ section-level families defined below.
fn is_doc_kind(kind: NodeKind) -> bool {
    kind.is_doc()
}

/// `true` for the swe-skills *typed* doc node kinds (S-039, [FR-DG-07]) — the
/// promoted `Requirement`/`Adr`/`Story`. A resolved doc→doc reference between
/// two of these is a typed trace ([`EdgeKind::TracesTo`]) rather than a generic
/// [`EdgeKind::DocReference`] (see [`Ctx::doc_reference_kind`]).
///
/// [FR-DG-07]: ../../../docs/specs/requirements/FR-DG-07.md
fn is_typed_doc_kind(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Requirement | NodeKind::Adr | NodeKind::Story
    )
}

/// `true` for a *file-level* doc node: the generic [`NodeKind::DocFile`] or a
/// typed file artifact promoted from one (`Requirement`/`Adr`, S-039). A
/// no-anchor doc link resolves to exactly one of these, so promotion leaves
/// S-035 doc→doc resolution intact ([FR-DG-03], [FR-DG-07]).
fn is_doc_file_kind(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::DocFile | NodeKind::Requirement | NodeKind::Adr
    )
}

/// `true` for a *section-level* doc node: the generic [`NodeKind::DocSection`]
/// or a typed `Story` promoted from one (S-039). An anchored doc link resolves
/// to exactly one of these ([FR-DG-03], [FR-DG-07]).
fn is_doc_section_kind(kind: NodeKind) -> bool {
    matches!(kind, NodeKind::DocSection | NodeKind::Story)
}

/// Split a documentation link `target` into its path part and the lower-cased
/// `#anchor` (an empty anchor is treated as absent).
fn split_anchor(target: &str) -> (&str, Option<String>) {
    match target.split_once('#') {
        Some((p, a)) if !a.is_empty() => (p, Some(a.to_string())),
        Some((p, _)) => (p, None),
        None => (target, None),
    }
}

fn split(target: &str) -> Vec<String> {
    target
        .split("::")
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// One binding attempt's context: the referencing node and its file's scope.
///
/// Single-threaded: one `Ctx` per [`bind`] call, never shared across threads
/// (the parallel pass gives each ref its own), so the `Cell` guard below needs
/// no synchronization.
struct Ctx<'a> {
    source: NodeId,
    file_id: Option<i64>,
    ix: &'a Index,
    /// The package the source's file declares, when its language is
    /// package-shaped ([`PackageLayout::package_of`], [CR-149]) — the switch
    /// between the package rung and the default module-tree hierarchy. `None`
    /// for every file of every other language, so their binding is untouched.
    ///
    /// [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    source_package: Option<Vec<String>>,
    /// The interop family of the source's file ([`PackageLayout::family`],
    /// S-519) — the partition of the type and namespace indexes its package
    /// rungs read. `None` for a source with no file.
    source_family: Option<String>,
    /// Whether the source's language declares that a namespace sees the types
    /// of its enclosing namespaces ([`PackageLayout::sees_enclosing_namespaces`],
    /// S-595, [FR-RS-45]) — what turns on the
    /// [`enclosing_levels`](package_names::PackageNames::enclosing_levels) rung.
    ///
    /// [FR-RS-45]: ../../../docs/specs/requirements/FR-RS-45.md
    enclosing_namespaces: bool,
    /// Whether the source's language names its classes and modules by lexical
    /// constants ([`PackageLayout::resolves_lexical_constants`], S-565,
    /// [FR-RS-33]) — what routes its calls, instantiations and type uses to
    /// the constant rung ([`resolve_constant_row`](lexical_constants::LexicalConstants::resolve_constant_row)).
    ///
    /// [FR-RS-33]: ../../../docs/specs/requirements/FR-RS-33.md
    lexical_constants: bool,
    /// Whether the source's language declares its files' top-level functions
    /// members of their namespace ([`PackageLayout::declares_top_level_callables`],
    /// S-573) — what lets its rungs read [`PackageIndex::callables_by_fqn`](package_index::PackageIndex::callables_by_fqn).
    top_level_callables: bool,
    /// Whether a bare call of the source's language that its namespace
    /// declares no function for reaches the global namespace's function
    /// ([`PackageLayout::falls_back_to_global_functions`], S-551: PHP;
    /// [`global_function`](package_names::PackageNames::global_function)).
    global_function_fallback: bool,
    /// Whether a wildcard import of the source's file outranks its namespace's
    /// other files ([`PackageLayout::reads_wildcards_first`], S-573: Scala) —
    /// the order [`resolve_package_name`](package_names::PackageNames::resolve_package_name) and
    /// [`resolve_package_path`](package_paths::PackagePaths::resolve_package_path) read their rungs in.
    wildcards_first: bool,
    /// Whether a receiver call of the source's language no receiver type
    /// proves may bind an extension function
    /// ([`PackageLayout::binds_extension_calls`], S-560: Kotlin;
    /// [`resolve_extension_call`](extension_calls::ExtensionCalls::resolve_extension_call)).
    extension_calls: bool,
    /// Whether the source's language names other languages' file facades
    /// (S-560, [`PackageIndex::facades_by_fqn`](package_index::PackageIndex::facades_by_fqn)): a source of a language that declares
    /// none itself ([`PackageLayout::declares_file_facades`]) — Java naming
    /// Kotlin's `UtilsKt`.
    reads_facades: bool,
    /// Whether the source's language declares `implicit_receiver = "none"`
    /// explicitly ([`PackageLayout::bare_calls_free_only`], S-590, [FR-RS-07]):
    /// its bare call reaches no instance member, so
    /// [`is_unreachable_member`](call_reach::CallReach::is_unreachable_member) holds for one.
    ///
    /// [FR-RS-07]: ../../../docs/specs/requirements/FR-RS-07.md
    bare_calls_free_only: bool,
    /// Whether the source's language makes a `self` call no member of its
    /// class admits go on to the free functions and imports in scope
    /// ([`PackageLayout::falls_through_to_free_calls`], S-592;
    /// [`resolve_self_receiver`](receiver_shapes::ReceiverShapes::resolve_self_receiver)).
    falls_through: bool,
    /// The argument count the arity-choosing arms filter candidates by
    /// ([`filtered_count`], S-592, [FR-RS-43]) — `None` when this row is
    /// filtered by none, so [`applicable`](Ctx::applicable) keeps every
    /// candidate.
    ///
    /// [FR-RS-43]: ../../../docs/specs/requirements/FR-RS-43.md
    arg_count: Option<u32>,
    /// The call's argument count as written, for the associated-item lookup's
    /// syntax-aware arity filter ([`lookup`](Ctx::lookup), S-607) — `None`
    /// when unknown, or when the source's language enforces no arity.
    call_args: Option<u32>,
    /// The generic arity of the type a type-relation row names as written
    /// (S-602, [CR-199]; the row's `arg_count`), when the source's language
    /// tells types apart by it ([`PackageLayout::matches_type_arity`]) — what
    /// [`admits_arity`](package_names::PackageNames::admits_arity) filters the namespace rungs'
    /// candidates by. `None` for every other row, and for a written type of
    /// unknown arity, which admits any.
    ///
    /// [CR-199]: ../../../docs/requests/CR-199-csharp-resolution-honours-type-arity-and-using-scope.md
    type_arity: Option<u32>,
    /// Whether the source's language decides a call to a type's method by the
    /// one associated-item lookup over recorded `impl` blocks
    /// ([`PackageLayout::looks_up_impl_blocks`], S-607, [FR-RS-47]).
    ///
    /// [FR-RS-47]: ../../../docs/specs/requirements/FR-RS-47.md
    impl_blocks: bool,
    /// Set once a path's type segment was reached and the lookup decided the
    /// call ([`descend`](scope_hierarchy::ScopeHierarchy::descend), S-607): Rust resolves `T` in the
    /// path's own scope and looks `m` up among `T`'s items alone, so no wider
    /// rung may read the path again — a same-named `T` at the crate root, a
    /// glob's, a workspace suffix match is another type.
    type_reached: Cell<bool>,
    /// Set when [`applicable`](Ctx::applicable) dropped every candidate of a
    /// non-empty set: some rung found callables of the name, and none admitted
    /// the call — the [`Residue::NoApplicableOverload`] a row that stays
    /// unbound records.
    passed_over: Cell<bool>,
    policy: BindingPolicy,
    /// Re-entrancy guard for [`through_globs`](rust_globs::RustGlobs::through_globs): set while a
    /// glob's own module path is being resolved, so that resolution cannot fan
    /// back out through the file's glob set. Without it, a file with `G` glob
    /// imports drives `O(G^MAX_ALIAS_DEPTH)` work per reference (CR-016).
    in_glob_resolution: Cell<bool>,
    /// Re-entrancy guard for the qualified-head rung's rival imports (S-599,
    /// [`resolve_package_path`](package_paths::PackagePaths::resolve_package_path)): set while a head's
    /// rival expansions are being resolved, so a rival head met inside them
    /// reads nothing instead of fanning out again. A path whose segments are
    /// themselves heads with rival imports (`import a.a; import b.a; import
    /// a.b; import b.b;` and `a.X.m()`) would otherwise drive
    /// `O(N^MAX_ALIAS_DEPTH)` work per reference — the shape CR-016 closed for
    /// globs.
    in_rival_expansion: Cell<bool>,
    /// Enable the CR-068 Part B free-function/associated-method tie-break in
    /// [`prefer_free_functions`](call_reach::CallReach::prefer_free_functions) for the duration of
    /// one resolution. Set **only** while resolving a single-segment bare-**path**
    /// call ([`RefForm::Path`] + [`EdgeKind::Calls`]) — precisely the shape that
    /// must bind the one free function over same-named associated methods
    /// ([FR-RS-07]). A receiver **method** call ([`RefForm::Method`]) never
    /// reaches [`resolve_name`](scope_hierarchy::ScopeHierarchy::resolve_name) at all (S-514): it binds by
    /// its receiver's shape.
    ///
    /// [FR-RS-07]: ../../../docs/specs/requirements/FR-RS-07.md
    bare_path_call: Cell<bool>,
    /// Pool a type's required signatures with its members at each level of a
    /// supertype walk ([`DeclarationFacts::declared_signatures`](declaration_facts::DeclarationFacts::declared_signatures)). Set **only** while a
    /// call on a proven receiver of a structural-types language binds among its
    /// type's members ([`resolve_structural_receiver_call`](proven_receivers::ProvenReceivers::resolve_structural_receiver_call),
    /// S-537): every other lookup binds no signature. A Kotlin proven receiver
    /// (S-563) takes the same lookup, and its types hold none.
    reads_signatures: Cell<bool>,
    /// Read a companion pair's instance callables in place of its singleton's
    /// ([`companion_members`](companions::Companions::companion_members)). Set **only** while a
    /// call on a proven receiver of a companion-pair language binds
    /// ([`resolve_companion_pair_receiver_call`](proven_receivers::ProvenReceivers::resolve_companion_pair_receiver_call),
    /// S-576): an instance of `T` answers `T`'s methods, never its `object`'s.
    instance_receiver: Cell<bool>,
    /// Bind by scope and module model only: set for a type relation (S-466,
    /// [CR-149]; S-522 for every language), whose target must be the one
    /// in-repository type its source's lexical scope, imports, package,
    /// namespace, modules or wildcards name. The policy-gated workspace
    /// fallbacks are off for it — the aggressive policy's **name** match and
    /// the balanced one's module-path **suffix** match — because a same-named
    /// type in a module the file never imports is not the type it wrote
    /// ([NFR-RA-05]): an import of a JDK `List` must not bind an in-house one,
    /// nor a Django `models.Model` base a repository's own `Model`. The
    /// hierarchy is built under the strict policy (`build_supertypes`), so the
    /// edge and the supertype walk agree under every policy.
    ///
    /// Also set while a receiver call falls through to the free functions in
    /// scope ([`free_call`](receiver_shapes::ReceiverShapes::free_call), S-592): a receiver call never
    /// reaches a workspace guess, so a same-named method of an unrelated class
    /// is never what `this.m(1)` meant.
    ///
    /// [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    scope_only: Cell<bool>,
    /// Where [`resolve_name`](scope_hierarchy::ScopeHierarchy::resolve_name)'s lexical chain — and a
    /// package-shaped path's lexical head — starts: the source itself, except
    /// while a type relation is read in its declaration's **header** (S-466;
    /// S-522 for every language), where it is the declaration's enclosing
    /// scope. A class's member types are in scope in its body, not in its
    /// `extends`/`implements` clause (JLS §6.3, §8.1.4), so `class Svc
    /// implements Callback { interface Callback {} }` never names its own
    /// nested type.
    lexical_start: Cell<NodeId>,
    /// Why a package-shaped call lookup gave up, when one did (S-468,
    /// [`Residue`]) — the first reason recorded wins: it is the rung that
    /// decided the row, and a later one only widens a search that rung already
    /// lost. Read by [`residue`] alone, and only for a row that stays unbound;
    /// binding never consults it.
    miss: RefCell<Option<Residue>>,
    /// The targets a trait-typed call fans out to ([FR-RS-08]'s set,
    /// [`fan_out`](trait_dispatch::TraitDispatch::fan_out)), when one did: a `dyn`, `impl`- or
    /// bound-typed receiver, a trait body's `self` call, a written `Tr::m(x)`
    /// (S-608). Its rung answers [`Res::NotFound`], since a [`Res`] holds one
    /// node, and [`bind_traced`] turns the set into the row's
    /// [`Outcome::BoundMany`] — the one place a fan-out becomes edges.
    ///
    /// [FR-RS-08]: ../../../docs/specs/requirements/FR-RS-08.md
    fanned: RefCell<Option<Fan>>,
}

impl<'a> Ctx<'a> {
    /// A context reading names from `source`'s scope, in file `file_id`, under
    /// `policy` — the file-derived facts read once here, every per-row knob
    /// at its default: no argument count, every workspace rung open.
    fn at(ix: &'a Index, source: NodeId, file_id: Option<i64>, policy: BindingPolicy, target: &str) -> Ctx<'a> {
        let source_file = ix.info.get(&source).and_then(|i| i.file_path.as_deref());
        Ctx {
            source,
            file_id,
            ix,
            source_package: source_file.and_then(|p| ix.layout.package_of(p)),
            source_family: source_file.and_then(|p| ix.layout.family(p)),
            enclosing_namespaces: source_file.is_some_and(|p| ix.layout.sees_enclosing_namespaces(p)),
            lexical_constants: source_file.is_some_and(|p| ix.layout.resolves_lexical_constants(p)),
            top_level_callables: source_file.is_some_and(|p| ix.layout.declares_top_level_callables(p)),
            global_function_fallback: source_file.is_some_and(|p| ix.layout.falls_back_to_global_functions(p)),
            wildcards_first: source_file.is_some_and(|p| ix.layout.reads_wildcards_first(p)),
            extension_calls: source_file.is_some_and(|p| ix.layout.binds_extension_calls(p)),
            reads_facades: source_file.is_some_and(|p| !ix.layout.declares_file_facades(p)),
            bare_calls_free_only: source_file.is_some_and(|p| ix.layout.bare_calls_free_only(p)),
            falls_through: source_file.is_some_and(|p| ix.layout.falls_through_to_free_calls(p, target)),
            arg_count: None,
            call_args: None,
            type_arity: None,
            impl_blocks: source_file.is_some_and(|p| ix.layout.looks_up_impl_blocks(p)),
            type_reached: Cell::new(false),
            passed_over: Cell::new(false),
            policy,
            in_glob_resolution: Cell::new(false),
            in_rival_expansion: Cell::new(false),
            bare_path_call: Cell::new(false),
            reads_signatures: Cell::new(false),
            instance_receiver: Cell::new(false),
            scope_only: Cell::new(false),
            lexical_start: Cell::new(source),
            miss: RefCell::new(None),
            fanned: RefCell::new(None),
        }
    }

    /// The context `r` binds in, from its source node `source`: its file's
    /// facts, and the argument counts its arms filter by.
    fn for_row(r: &UnresolvedRefRow, ix: &'a Index, source: NodeId, policy: BindingPolicy) -> Ctx<'a> {
        let mut ctx = Ctx::at(ix, source, r.file_id, policy, &r.target);
        let source_file = ix.info.get(&source).and_then(|i| i.file_path.as_deref());
        ctx.arg_count = filtered_count(r, source_file, &ix.layout);
        ctx.call_args = r
            .arg_count
            .filter(|_| r.kind == EdgeKind::Calls && source_file.is_some_and(|p| ix.layout.checks_arity(p)));
        ctx.type_arity = r
            .arg_count
            .filter(|_| is_type_relation(r.kind) && source_file.is_some_and(|p| ix.layout.matches_type_arity(p)));
        ctx
    }

    /// The context an `impl` header is read in ([`IndexBuilder::with_associated_items`](IndexBuilder::with_associated_items)):
    /// `scope`, by scope and module model only — never a workspace guess.
    fn for_scope(ix: &'a Index, scope: NodeId, file_id: Option<i64>) -> Ctx<'a> {
        let ctx = Ctx::at(ix, scope, file_id, BindingPolicy::Strict, "");
        ctx.scope_only.set(true);
        ctx
    }
}

impl Ctx<'_> {
    fn scope(&self) -> Option<&FileScope> {
        self.file_id.and_then(|id| self.ix.modules.file_scopes.get(&id))
    }

    /// Record why a call lookup ([`Want::is_call`]) gave up, unless an
    /// earlier step already did. A lookup for any other kind records nothing:
    /// its miss is a sub-step (a glob's own type, an import's target), not the
    /// call's.
    fn note(&self, want: Want, why: impl FnOnce() -> Residue) {
        if want.is_call() {
            let mut miss = self.miss.borrow_mut();
            if miss.is_none() {
                *miss = Some(why());
            }
        }
    }

    /// `candidates` less every callable whose parameter range excludes the
    /// call's argument count (S-592, [FR-RS-43]) — applied **before**
    /// exactly-one by every arm that chooses among same-named callables, so a
    /// callable the call cannot invoke never binds, nor makes an applicable one
    /// ambiguous. Only a recorded range drops a candidate: an unknown range,
    /// any node that is not a callable (a class a call constructs, a type), and
    /// every candidate of a row with no filtered count
    /// ([`arg_count`](Ctx::arg_count)) are kept. A default or a variadic
    /// parameter widens the range itself, so it is admitted here. When every
    /// candidate of a non-empty set is dropped, the rung is passed over and
    /// [`passed_over`](Ctx::passed_over) remembers it.
    ///
    /// [FR-RS-43]: ../../../docs/specs/requirements/FR-RS-43.md
    fn applicable(&self, mut candidates: Vec<NodeId>) -> Vec<NodeId> {
        let Some(count) = self.arg_count else {
            return candidates;
        };
        let named = !candidates.is_empty();
        candidates.retain(|id| self.admits_count(*id, count));
        if named && candidates.is_empty() {
            self.passed_over.set(true);
        }
        candidates
    }

    /// Whether `id`'s recorded parameter range admits `count` arguments — an
    /// unknown range admits any ([`applicable`](Ctx::applicable)'s one test).
    fn admits_count(&self, id: NodeId, count: u32) -> bool {
        self.ix.facts.arities.get(&id).is_none_or(|range| range.admits(count))
    }

    /// `true` for the declaration whose header is being read (S-522): a
    /// supertype's lexical lookup starts at the declaration's enclosing scope
    /// ([`lexical_start`](Ctx::lexical_start)), which holds the declaration
    /// itself — and a class never names itself as its base. Python's `from
    /// unittest import TestCase` then `class TestCase(TestCase)` names the
    /// import, which the scope's own `TestCase` would otherwise shadow.
    fn is_own_header(&self, id: NodeId) -> bool {
        id == self.source && self.lexical_start.get() != self.source
    }

    /// The source's crate and module path (the `self`/`super`/relative base).
    fn source_module(&self) -> Option<ModKey> {
        self.ix.nearest_module(self.source).cloned()
    }

    /// The lookup a bare or path-qualified `Calls` row takes (S-521,
    /// [FR-RS-16]): [`Want::DeclaredCall`] when the source file's plugin
    /// declares a call target beyond a callable, else [`Want::Callable`] — so a
    /// language that declares neither key resolves exactly as before.
    ///
    /// [FR-RS-16]: ../../../docs/specs/requirements/FR-RS-16.md
    fn call_want(&self) -> Want {
        let targets = self
            .ix
            .info
            .get(&self.source)
            .and_then(|i| i.file_path.as_deref())
            .map(|path| self.ix.layout.call_targets(path))
            .unwrap_or_default();
        if targets.any() {
            Want::DeclaredCall(targets)
        } else {
            Want::Callable
        }
    }

    /// `true` when a call looked up with `want` bound `target` by constructing
    /// it: a `Class`, admitted only because the caller's plugin declares that
    /// calling a class instantiates it (S-521, [FR-RS-16]). Such a bind records
    /// `Instantiates`, never `Calls`.
    ///
    /// [FR-RS-16]: ../../../docs/specs/requirements/FR-RS-16.md
    fn constructs(&self, want: Want, target: NodeId) -> bool {
        matches!(want, Want::DeclaredCall(t) if t.classes)
            && self.ix.info.get(&target).is_some_and(|i| i.kind == NodeKind::Class)
    }

    /// The one node named `name` whose kind `keep` admits, workspace-wide, under
    /// the exactly-one rule ([`exactly_one`]): a schema type name's code type
    /// ([`Artifacts::resolve_code_type_name`](artifacts::Artifacts::resolve_code_type_name)),
    /// a documentation code-name token's code symbol
    /// ([`DocReferences::resolve_doc_code_name`](doc_references::DocReferences::resolve_doc_code_name)).
    fn one_named(&self, name: &str, keep: impl Fn(NodeKind) -> bool) -> Res {
        let Some(all) = self.ix.by_name.get(name) else {
            return Res::NotFound;
        };
        let candidates: Vec<NodeId> = all
            .iter()
            .copied()
            .filter(|id| self.ix.info.get(id).is_some_and(|i| keep(i.kind)))
            .collect();
        exactly_one(&candidates)
    }

    /// Whether the node `ty` is an enum recording a variant `name` (S-606).
    fn is_variant(&self, ty: NodeId, name: &str) -> bool {
        self.ix.facts.variants.get(&ty).is_some_and(|v| v.iter().any(|x| x == name))
    }

}

/// The index's test-only surface: the shorthand builds the binder's tests
/// start from, and the sync and layout predicates they read off an index, each
/// forwarding to the concern that answers it — the tests predate the split
/// (S-828) and assert on `Index` as they always did. A test module, so it is
/// test code to the graph as it is to the compiler.
#[cfg(test)]
mod test_api {
    use std::collections::HashSet;

    use crate::graph_store::{EdgeRow, NodeRow, UnresolvedRefRow};
    use crate::model::NodeId;

    use super::{Index, IndexBuilder, PackageLayout};

    impl Index {
        /// The index of a snapshot given no store fact ([`IndexBuilder::new`]):
        /// the path model for every file, with the package-file stems the rust
        /// plugin declares for `.rs` ([`PackageLayout::rust_stems_for_tests`],
        /// S-519), so a synthetic `src/lib.rs` still names its crate. Test-only
        /// since S-470: both production builders — the resolution pass and the
        /// framework-promotion pass — build with the registry's layout
        /// ([`IndexBuilder::new`]).
        pub(crate) fn build(nodes: &[NodeRow], edges: &[EdgeRow], refs: &[UnresolvedRefRow]) -> Index {
            IndexBuilder::for_tests(nodes, edges, refs).build()
        }

        /// [`IndexBuilder::new`] given no store fact, for the binder's tests.
        pub(crate) fn build_with_layout(
            nodes: &[NodeRow],
            edges: &[EdgeRow],
            refs: &[UnresolvedRefRow],
            layout: PackageLayout,
        ) -> Index {
            IndexBuilder::new(nodes, edges, refs, layout).build()
        }

        /// [`Hierarchy::supertypes_of`](hierarchy::Hierarchy::supertypes_of), for
        /// the binder's own tests.
        pub(crate) fn supertypes_for_tests(&self, ty: NodeId) -> &[NodeId] {
            self.hierarchy.supertypes_of(ty)
        }

        /// [`Invalidation::ref_affected`].
        pub(crate) fn ref_affected(&self, r: &UnresolvedRefRow, dirty: &HashSet<String>) -> bool {
            self.invalidation().ref_affected(r, dirty)
        }

        /// [`Invalidation::hierarchy_touched`].
        pub(crate) fn hierarchy_touched(&self, dirty: &HashSet<String>) -> bool {
            self.invalidation().hierarchy_touched(dirty)
        }

        /// [`Invalidation::walks_hierarchy`].
        pub(crate) fn walks_hierarchy(&self, path: &str) -> bool {
            self.invalidation().walks_hierarchy(path)
        }

        /// [`PackageLayout::moves_import_roots`].
        pub(crate) fn moves_import_roots(&self, path: &str) -> bool {
            self.layout.moves_import_roots(path)
        }

        /// [`PackageLayout::is_package_shaped`].
        pub(crate) fn is_package_shaped(&self, path: &str) -> bool {
            self.layout.is_package_shaped(path)
        }

        /// [`PackageLayout::resolves_lexical_constants`].
        pub(crate) fn resolves_lexical_constants(&self, path: &str) -> bool {
            self.layout.resolves_lexical_constants(path)
        }

        /// [`ModuleIndex::is_path_specifier_file`](module_index::ModuleIndex::is_path_specifier_file).
        pub(crate) fn is_path_specifier_file(&self, path: &str) -> bool {
            self.modules.is_path_specifier_file(path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_key_derives_crate_and_modules_from_src_layout() {
        let layout = PackageLayout::rust_stems_for_tests();
        let key = |path: &str| layout.module_key(path);
        assert_eq!(
            key("logos-core/src/extract/mod.rs"),
            ("logos_core".to_string(), vec!["extract".to_string()])
        );
        assert_eq!(key("logos-core/src/lib.rs"), ("logos_core".to_string(), vec![]));
        assert_eq!(
            key("src/engine.rs"),
            ("crate".to_string(), vec!["engine".to_string()])
        );
        assert_eq!(
            key("src/a/b.rs"),
            ("crate".to_string(), vec!["a".to_string(), "b".to_string()])
        );
        // No src/ layout (flat fixtures): everything is a module under
        // the anonymous crate.
        assert_eq!(key("alpha.rs"), ("crate".to_string(), vec!["alpha".to_string()]));
    }

    #[test]
    fn exactly_one_is_the_only_acceptance_rule() {
        assert_eq!(exactly_one(&[]), Res::NotFound);
        assert_eq!(exactly_one(&[NodeId(1)]), Res::Found(NodeId(1)));
        assert_eq!(exactly_one(&[NodeId(1), NodeId(2)]), Res::Ambiguous);
    }

    /// Every binder source file, path and text: this file first, then each
    /// module under `binder/` in path order (S-827) — so a guard that reads the
    /// binder reads all of it wherever a rung's code sits.
    fn binder_sources() -> Vec<(String, String)> {
        let resolve = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/resolve");
        let mut files = vec![resolve.join("binder.rs")];
        let mut dirs = vec![resolve.join("binder")];
        let mut rungs = Vec::new();
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).expect("the binder directory is readable") {
                let path = entry.expect("a directory entry").path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    rungs.push(path);
                }
            }
        }
        rungs.sort();
        files.extend(rungs);
        files
            .into_iter()
            .map(|path| {
                let text = std::fs::read_to_string(&path).expect("a binder source is readable");
                (path.display().to_string(), text)
            })
            .collect()
    }

    /// S-607 ([FR-RS-47]): one associated-item lookup replaced the Rust
    /// `Self::m` arm, its module rung, the proven-receiver arm and the
    /// `Type::func` collapse — and no twin of them survives in any binder file
    /// ([`binder_sources`], S-827). The names are spelt in pieces so this
    /// test's own text never matches.
    ///
    /// [FR-RS-47]: ../../../docs/specs/requirements/FR-RS-47.md
    #[test]
    fn the_replaced_rust_arms_are_gone_and_every_route_reaches_the_one_lookup() {
        let sources = binder_sources();
        assert!(sources.len() > 1, "the binder's modules are read, not only this file");
        for gone in [
            concat!("fn resolve_self", "_type_call("),
            concat!("fn module", "_candidate("),
            concat!("fn type", "_method("),
            concat!("trait_impl", "_methods"),
        ] {
            for (path, src) in &sources {
                assert!(!src.contains(gone), "`{gone}` survives in {path}");
            }
        }
        // The collapse is reached only by a language without impl blocks: the
        // impl-block branch returns before it, in the one file holding both.
        let branch_at = concat!("if self.impl_blocks && i == segs", ".len() - 2");
        let (path, src) = sources.iter().find(|(_, src)| src.contains(branch_at)).expect("the Rust branch");
        let branch = src.find(branch_at).expect("the Rust branch");
        let collapse = src
            .find(concat!("// The `Type::func` collapse of every", " other language"))
            .unwrap_or_else(|| panic!("the collapse, beside the branch in {path}"));
        assert!(branch < collapse, "the impl-block branch must decide first");
        // `Self::m`/`self.m()`, a proven `x.m()`, a written `T::m()` and a
        // qualified `<T as Tr>::m()` (S-608): four call sites of the one
        // lookup, no fifth rule — on whatever reaches it: `self`, another
        // rung, a rung's `cx`, or the path `Ctx::lookup` called as a function
        // (S-827).
        let calls = |spelling: &str| -> usize { sources.iter().map(|(_, src)| src.matches(spelling).count()).sum() };
        assert_eq!(calls(concat!(".look", "up(")) + calls(concat!("::look", "up(")), 4);
    }

    /// The type a parameter declaration `name: T` names behind a shared or
    /// mutable reference, its lifetime and any leading path dropped:
    /// `&'c Ctx<'a>` and `&super::Ctx<'a>` → `Ctx<'a>`. `None` for a
    /// parameter that is no reference.
    fn referenced_type(param: &str) -> Option<&str> {
        let (_, ty) = param.split_once(':')?;
        let ty = ty.trim().strip_prefix("&")?.trim_start();
        let ty = if ty.starts_with("'") { ty.split_once(' ').map_or(ty, |(_, rest)| rest.trim_start()) } else { ty };
        let ty = ty.strip_prefix("mut ").unwrap_or(ty).trim_start();
        let path_end = ty.find("<").unwrap_or(ty.len());
        Some(ty[..path_end].rfind("::").map_or(ty, |at| &ty[at + 2..]))
    }

    /// A signature after its `fn ` split at its parameter list's `(`: the
    /// name with its generics, and what follows the `(`. A generic list is
    /// skipped whole, so a bound's own `(` (`F: Fn() -> bool`) is not taken
    /// for the parameter list.
    fn parameter_list(sig: &str) -> Option<(&str, &str)> {
        let mut depth = 0usize;
        let mut prev = ' ';
        for (at, c) in sig.char_indices() {
            match c {
                '<' => depth += 1,
                '>' if prev != '-' => depth = depth.saturating_sub(1),
                '(' if depth == 0 => return Some((&sig[..at], &sig[at + 1..])),
                _ => {}
            }
            prev = c;
        }
        None
    }

    /// S-827 ([CR-258] §3.2): a concern of the per-row context is a rung type
    /// borrowing the row's [`Ctx`], never a free function taking it — that
    /// would only hide its methods from the god-container count. No function
    /// of any binder file takes `&Ctx` as its first parameter.
    ///
    /// [CR-258]: ../../../docs/requests/CR-258-the-binders-ctx-and-index-are-split-by-concern.md
    #[test]
    fn no_binder_function_takes_the_row_context_as_its_first_parameter() {
        let mut offenders = Vec::new();
        for (path, src) in binder_sources() {
            for (at, _) in src.match_indices("fn ") {
                let Some((name, params)) = parameter_list(&src[at + 3..]) else { continue };
                let first = params.split([',', ')']).next().unwrap_or("");
                let named = referenced_type(first).filter(|ty| *ty == "Ctx" || ty.starts_with("Ctx<"));
                if named.is_some() {
                    offenders.push(format!("{path}: fn {name}"));
                }
            }
        }
        assert!(offenders.is_empty(), "free functions over the row context: {offenders:?}");
        // The signature reading the scan rests on.
        assert_eq!(referenced_type("cx: &'c Ctx<'a>"), Some("Ctx<'a>"));
        assert_eq!(referenced_type("ctx: &Ctx<'_>"), Some("Ctx<'_>"));
        assert_eq!(referenced_type("cx: &super::Ctx<'_>"), Some("Ctx<'_>"));
        assert_eq!(referenced_type("cx: &'c crate::resolve::binder::Ctx<'a>"), Some("Ctx<'a>"));
        assert_eq!(referenced_type("ix: &mut Index"), Some("Index"));
        assert_eq!(referenced_type("&self"), None);
        assert_eq!(referenced_type("source: NodeId"), None);
        assert_eq!(
            parameter_list("f<F: Fn() -> bool>(cx: &Ctx<'_>, keep: F) -> Res"),
            Some(("f<F: Fn() -> bool>", "cx: &Ctx<'_>, keep: F) -> Res"))
        );
    }

    /// The type a parameter declaration `name: T` names, by reference — one
    /// or several, `mut`, a lifetime — or by value, its leading path dropped:
    /// `ix: &Index`, `ix: &'a mut super::Index`, `ix: &&Index` and `mut index:
    /// Index` → `Index`. `None` for a receiver (`self`, `&self`).
    fn parameter_type(param: &str) -> Option<&str> {
        let (_, ty) = param.split_once(':')?;
        let mut ty = ty.trim();
        while let Some(referenced) = ty.strip_prefix('&') {
            let referenced = referenced.trim_start();
            ty = if referenced.starts_with('\'') {
                referenced.split_once(' ').map_or(referenced, |(_, rest)| rest.trim_start())
            } else {
                referenced
            };
            ty = ty.strip_prefix("mut ").unwrap_or(ty).trim_start();
        }
        let ty = ty.strip_prefix("mut ").unwrap_or(ty).trim_start();
        let path_end = ty.find(['<', ' ', ',', ')']).unwrap_or(ty.len());
        Some(ty[..path_end].rfind("::").map_or(&ty[..path_end], |at| &ty[at + 2..path_end]))
    }

    /// `item` past what may precede `fn` on its line: attributes
    /// (`#[inline]`), a visibility (`pub`, `pub(in crate::resolve)`) and the
    /// qualifiers `const`, `async`, `unsafe` and `extern "C"`.
    fn past_fn_prefix(item: &str) -> &str {
        let mut item = item.trim_start();
        loop {
            let before = item;
            if let Some(attr) = item.strip_prefix("#[") {
                item = attr.split_once(']').map_or(item, |(_, rest)| rest.trim_start());
            }
            if let Some(rest) = item.strip_prefix("pub") {
                item = match rest.strip_prefix('(') {
                    Some(scope) => scope.split_once(')').map_or(item, |(_, rest)| rest.trim_start()),
                    None if rest.starts_with(' ') => rest.trim_start(),
                    None => item,
                };
            }
            for qualifier in ["const ", "async ", "unsafe "] {
                item = item.strip_prefix(qualifier).map_or(item, str::trim_start);
            }
            if let Some(rest) = item.strip_prefix("extern ") {
                let rest = rest.trim_start();
                item = match rest.strip_prefix('"') {
                    Some(abi) => abi.split_once('"').map_or(rest, |(_, rest)| rest.trim_start()),
                    None => rest,
                };
            }
            if item == before {
                return item;
            }
        }
    }

    /// Whether the `fn` on line `at` of `lines` is an associated item: the
    /// nearest line above it at a lesser indentation, the one opening its
    /// block, is an `impl` or a `trait` header. A free function sits at a
    /// file's top level, in a `mod` block, or in a function's body.
    fn is_associated(lines: &[&str], at: usize) -> bool {
        let indent = |l: &str| l.len() - l.trim_start().len();
        let own = indent(lines[at]);
        if own == 0 {
            return false;
        }
        let opener = lines[..at].iter().rev().find(|l| !l.trim().is_empty() && indent(l) < own);
        opener.is_some_and(|l| {
            let head = past_fn_prefix(l);
            head.starts_with("impl") || head.starts_with("trait ")
        })
    }

    /// The lookup index, its builder and the types its concerns are: what no
    /// free binder function may take first in place of being one's method.
    const INDEX_TYPES: [&str; 8] = [
        "Index",
        "IndexBuilder",
        "ModuleIndex",
        "PackageIndex",
        "Hierarchy",
        "ImplIndex",
        "DeclarationFacts",
        "Invalidation",
    ];

    /// S-828 ([CR-258] §3.2): a query concern of the lookup index is a
    /// sub-index the [`Index`] owns, and its construction is the
    /// [`IndexBuilder`]'s — never a free function taking the index, which would
    /// only hide its methods from the god-container count. No free function of
    /// any binder file — at a file's top level, in a `mod` block or in a
    /// function's body, whatever its visibility, qualifiers or attributes, its
    /// signature on one line or several — takes the `Index`, its builder or a
    /// sub-index ([`INDEX_TYPES`]) as its first parameter, by reference or by
    /// value. An associated function of a type (`IncludeScope::in_std(ix, …)`)
    /// counts toward that type, so it hides nothing and is not scanned; nor is
    /// a type behind an alias or a wrapper (`Box<Index>`).
    ///
    /// [CR-258]: ../../../docs/requests/CR-258-the-binders-ctx-and-index-are-split-by-concern.md
    #[test]
    fn no_binder_function_takes_the_lookup_index_as_its_first_parameter() {
        // The binder's entry points bind against the index by contract, as
        // `bind` and `residue` do: what the passes call, never a concern's
        // query. Each must still be one, so the list cannot go stale.
        const ENTRY_POINTS: [&str; 1] = ["bind_constant_action"];
        let mut entry_points = Vec::new();
        let mut offenders = Vec::new();
        for (path, src) in binder_sources() {
            let lines: Vec<&str> = src.lines().collect();
            let mut start = 0;
            for (at, line) in lines.iter().enumerate() {
                let line_start = start;
                start += line.len() + 1;
                let Some(after) = past_fn_prefix(line).strip_prefix("fn ") else { continue };
                if is_associated(&lines, at) {
                    continue;
                }
                // The signature read from the line's `fn` on: a parameter
                // list may continue on the lines below.
                let sig = &src[line_start + line.len() - after.len()..];
                let Some((name, params)) = parameter_list(sig) else { continue };
                let first = params.split([',', ')']).next().unwrap_or("");
                if !parameter_type(first).is_some_and(|ty| INDEX_TYPES.contains(&ty)) {
                    continue;
                }
                if ENTRY_POINTS.contains(&name) {
                    entry_points.push(name.to_string());
                } else {
                    offenders.push(format!("{path}: fn {name}"));
                }
            }
        }
        assert!(offenders.is_empty(), "free functions over the lookup index: {offenders:?}");
        assert_eq!(entry_points, ENTRY_POINTS, "each listed entry point still takes the index first");
        // The signature reading the scan rests on.
        assert_eq!(parameter_type("ix: &Index"), Some("Index"));
        assert_eq!(parameter_type("ix: &'a Index"), Some("Index"));
        assert_eq!(parameter_type("ix: &mut super::Index"), Some("Index"));
        assert_eq!(parameter_type("ix: &&Index"), Some("Index"));
        assert_eq!(parameter_type("mut index: Index"), Some("Index"));
        assert_eq!(parameter_type("index: crate::resolve::binder::Index"), Some("Index"));
        assert_eq!(parameter_type("builder: IndexBuilder"), Some("IndexBuilder"));
        assert_eq!(parameter_type("&self"), None);
        assert_eq!(parameter_type("mut self"), None);
        assert_eq!(parameter_type("refs: &[UnresolvedRefRow]"), Some("[UnresolvedRefRow]"));
        let (_, params) = parameter_list("f(\n    ix: &Index,\n    r: &UnresolvedRefRow,\n) -> bool").expect("a list");
        assert_eq!(parameter_type(params.split([',', ')']).next().unwrap_or("")), Some("Index"));
        for item in [
            "pub(in crate::resolve) fn f(ix: &Index)",
            "pub(self) fn f(ix: &Index)",
            "const fn f(ix: &Index)",
            "pub(crate) async fn f(ix: &Index)",
            "#[inline] fn f(ix: &Index)",
            "#[allow(dead_code)] pub unsafe extern \"C\" fn f(ix: &Index)",
        ] {
            assert!(past_fn_prefix(item).starts_with("fn f("), "{item}");
        }
        let lines = ["impl Index {", "    fn m(&self) {}", "}", "mod nested {", "    fn f(ix: &Index) {}", "}"];
        assert!(is_associated(&lines, 1));
        assert!(!is_associated(&lines, 4));
        assert!(!is_associated(&["fn outer() {", "    fn inner(ix: &Index) {}", "}"], 1));
        assert!(is_associated(&["pub(super) trait T {", "    fn m(ix: &Index);", "}"], 1));
    }
}
