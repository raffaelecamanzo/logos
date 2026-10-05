//! The scope-hierarchy binder — the pure core of the resolution engine
//! (S-011, [FR-RS-01], [FR-RS-02], [FR-RS-03], [NFR-RA-05]).
//!
//! [`Index::build`] digests an immutable graph snapshot (nodes, `Contains`
//! edges, the reference ledger) into the lookup structures binding needs;
//! [`bind`] then resolves one ledger row against it. Nothing here touches the
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
//!    the module it names, and [`Ctx::resolve_imported_call`] binds it within
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
//! single-type/static imports, the top-level types of its own package, what its
//! wildcards bring into view, then a path read as a fully-qualified name
//! ([`Ctx::resolve_package_name`], [`Ctx::resolve_package_path`]). An import
//! binds to the type or member it names — a declared-namespace wildcard, which
//! names no type, to the files declaring the namespace
//! ([`Ctx::namespace_files`]) — and the workspace suffix match is never
//! consulted; the aggressive bare-name fallback alone stays policy-gated as
//! everywhere else.
//!
//! [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
//! [FR-RS-13]: ../../../docs/specs/requirements/FR-RS-13.md
//!
//! A file of an **import-root** language (Python; S-519, [FR-RS-14]) keeps
//! rungs 1–5, keyed under its family's own crate: a relative import reads its
//! `.`/`..` level from the file's package ([`Ctx::resolve_relative`]), every
//! directory under its import root descends as a module, a `from pkg import
//! Name` that names nothing `pkg/__init__.py` declares binds to the package
//! ([`Ctx::package_reexport`]), and its workspace fallbacks never leave the
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
//! # Never fabricate ([NFR-RA-05])
//!
//! Every level ends in the same acceptance rule: bind **iff the candidate set
//! has exactly one element**. Zero candidates falls through to the next level;
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

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::config::BindingPolicy;
use crate::extract::doc::heading_slug;
use crate::graph_store::{EdgeRow, NodeRow, UnresolvedRefRow};
use crate::model::{ArtifactRelation, EdgeKind, NodeId, NodeKind, ReceiverShape, RefForm};
use crate::plugin::CallTargets;

use super::go_module::GoModule;
use super::package_key::{normalize_crate, ModuleKey, PackageLayout};
use super::route_method::preferred_candidates;
use super::route_template::route_key;
use crate::extract::refs::is_relative_head;

/// A module's identity: `(crate name, module path segments)`.
type ModKey = ModuleKey;

/// `Contains` membership: scope → name → member nodes, each list id-sorted for
/// a deterministic candidate order ([NFR-RA-06]).
///
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
type Members = HashMap<NodeId, HashMap<String, Vec<NodeId>>>;

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
    /// carry no payload.
    ///
    /// [FR-CG-08]: ../../../docs/specs/requirements/FR-CG-08.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    BoundMany {
        source: NodeId,
        targets: Vec<NodeId>,
        kind: EdgeKind,
        payload: Option<String>,
    },
    /// Zero candidates anywhere, or an ambiguity — the ref stays in the
    /// ledger and is retried on the next sync ([FR-RS-03]).
    ///
    /// [FR-RS-03]: ../../../docs/specs/requirements/FR-RS-03.md
    Unbound,
}

/// An intermediate lookup result. `Ambiguous` is sticky: once a scope level
/// *knows* there are two candidates, no wider level may overrule it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Res {
    Found(NodeId),
    NotFound,
    Ambiguous,
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
    /// Also a `Self::m` call whose self type the crate declares no type of, or
    /// whose caller's file imports it from outside the crate (S-493), with no
    /// candidates.
    ExternalType { candidates: Vec<Vec<String>> },
    /// The type (or the nearest supertype level holding the name) declares two
    /// or more callables of that name, or two static imports each supply one.
    /// Also a `Self::m` call whose self type — a name the crate declares for
    /// one type — records several `m` that the caller's module does not narrow
    /// to one (S-493).
    OverloadAmbiguous,
    /// The type's name reaches two in-graph declarations (a `src/main` and a
    /// `src/test` class of one fully-qualified name). Also a `Self::m` call
    /// whose self type's base name the crate declares for several types, when
    /// the caller's module does not decide the candidate (S-493).
    TypeAmbiguous,
    /// The type is in the graph, and neither it nor any in-graph supertype
    /// declares the name: the chain leaves the graph (a JDK, library or
    /// other-member superclass, the implicit `Object`), stops at an interface,
    /// or cycles. Also a `Self::m` call whose self type records no `m` in its
    /// crate — a trait default, an external trait or a `Deref` target supplies
    /// it, none of which the graph records for that type (S-493).
    SupertypeUnreached,
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
fn is_type_like(kind: NodeKind) -> bool {
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

/// Maximum alias-chain hops before a lookup gives up — terminates resolution
/// on a self-referential import cycle (the S-011 sprint test) instead of
/// recursing forever.
const MAX_ALIAS_DEPTH: u8 = 8;

/// Hard cap on the `Contains`-hierarchy walk in [`Ctx::typed_owner`] (S-039) — a
/// defensive bound against a malformed cycle, far above any real doc/code
/// nesting depth (markdown headings reach 6; module nesting follows directory
/// depth).
const MAX_CONTAINS_DEPTH: u32 = 64;

/// Hard cap on the levels [`Ctx::type_member`] climbs a Java type's in-repository
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
    /// In-scope name → **every** path the file's imports give it, in ledger
    /// order. The package rung reads this, not the first-wins `aliases`: two
    /// static imports may name one method overloaded across two types
    /// (`import static a.A.m; import static b.B.m;`, CR-149), and that name is
    /// then ambiguous rather than the first import's ([NFR-RA-05]).
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    alias_expansions: HashMap<String, Vec<Vec<String>>>,
    /// Glob-imported module paths (`use m::*` → `["m"]`), unresolved form. For a
    /// package-shaped file (CR-149) these are the non-static wildcards
    /// (`import a.b.*`, `import a.b.C.*`, C#'s `using A.B;`), which bring
    /// **types** into scope only — the file's own, then the global ones in force
    /// over it ([`Index::apply_global_globs`], S-518).
    globs: Vec<Vec<String>>,
    /// A package-shaped file's **static** wildcards (`import static a.b.C.*`,
    /// recorded with the alias `*`, [`STATIC_WILDCARD_ALIAS`]): every static
    /// member of the named type comes into scope, methods and fields included.
    static_globs: Vec<Vec<String>>,
}

/// The alias a static wildcard import's `Glob` row carries — "every static
/// member name of the type" (CR-149; C#'s `using static`, S-518). A non-static
/// wildcard carries none, and a global one [`GLOBAL_WILDCARD_ALIAS`].
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
/// ([`build_supertypes`]). A relation row has no alias otherwise.
///
/// [FR-RS-15]: ../../../docs/specs/requirements/FR-RS-15.md
pub(crate) const TRAIT_USE_ALIAS: &str = "use";

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

/// The immutable lookup index one resolution run binds against.
pub(crate) struct Index {
    /// Canonical symbol string → node.
    by_symbol: HashMap<String, NodeId>,
    /// Per-node facts.
    info: HashMap<NodeId, NodeInfo>,
    /// `Contains`: child → parent.
    parent: HashMap<NodeId, NodeId>,
    /// `Contains`: scope → name → members, each list sorted by `NodeId`.
    members: Members,
    /// Module tree: `(crate, path)` → module node (file modules from their
    /// paths, inline `mod`s appended beneath them). A key a bodyless `mod x;`
    /// claims is answered by the one file module it declares (S-585,
    /// [`hand_declarations_to_their_files`]).
    modules: HashMap<ModKey, NodeId>,
    /// The **directory modules** of the import-root languages (S-519,
    /// [FR-RS-14]): every directory between a file's import root and the file
    /// ([`PackageLayout::directory_modules`]). A path descends through one as
    /// through a module, whether or not a package file (`__init__.py`) gives it
    /// a node — a namespace package is a package too — but a directory with no
    /// node is never itself a binding target. Empty for every other language.
    ///
    /// [FR-RS-14]: ../../../docs/specs/requirements/FR-RS-14.md
    dir_modules: HashSet<ModKey>,
    /// The reverse of `modules`, for "what module am I in" walks.
    module_key: HashMap<NodeId, ModKey>,
    /// name → every node carrying it, sorted by `NodeId` (the unique-match
    /// fallback universe).
    by_name: HashMap<String, Vec<NodeId>>,
    /// file path → every node defined in that file, sorted by `NodeId` — the
    /// universe a doc link/path reference resolves against (S-035).
    by_file_path: HashMap<String, Vec<NodeId>>,
    /// Normalized template → the `(METHOD, node)` pairs of every
    /// [`NodeKind::Route`] carrying it, sorted by `NodeId` — the universe an
    /// OpenAPI `ApiOperation`→route reference resolves against (S-069,
    /// [FR-CG-09]). A route whose template does not normalize cleanly
    /// (catch-all, regex) is **absent** from this map and so is never a
    /// candidate — honestly unresolved, never approximately matched
    /// ([NFR-RA-05]).
    ///
    /// Keyed on the **template alone**, not on `(method, template)`: a wildcard
    /// (`ANY`) provider serves every verb, and tuple equality cannot express
    /// that, so the method is resolved *inside* the bucket by the shared
    /// [`route_method`](super::route_method) rule ([CR-109], [ADR-52]).
    ///
    /// [CR-109]: ../../../docs/requests/CR-109-wildcard-method-route-matching.md
    /// [FR-CG-09]: ../../../docs/specs/requirements/FR-CG-09.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    /// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
    routes_by_template: HashMap<String, Vec<(String, NodeId)>>,
    /// file path with its extension removed → the **file-root** module nodes at
    /// that stem, id-sorted — the universe a relative path specifier binds
    /// against (S-439), so `./nav.ts` (recorded `.::nav`) and `./nav` reach
    /// `nav.ts` alike and a `nav.ts` beside a `nav.tsx` is an ambiguity, never a
    /// pick ([NFR-RA-05]).
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    file_roots_by_stem: HashMap<String, Vec<NodeId>>,
    /// directory → the `(file path, file-root module)` pairs directly in it,
    /// id-sorted — the package a Go import path names (S-439).
    file_roots_by_dir: HashMap<String, Vec<(String, NodeId)>>,
    /// The extensions of the files whose import specifiers are **paths**, each
    /// mapped to the extensions a relative specifier in such a file may resolve
    /// to (S-439; [`LanguageRegistry::specifier_target_extensions`]). Empty
    /// unless the run was given the registry ([`Index::with_path_specifiers`]);
    /// empty leaves every non-relative import on the path it took before, and
    /// binds no relative one.
    ///
    /// [`LanguageRegistry::specifier_target_extensions`]: crate::plugin::LanguageRegistry::specifier_target_extensions
    specifier_targets: HashMap<String, HashSet<String>>,
    /// The Go modules the tree declares (S-439, [`super::go_module`]). Empty unless the run was given the tree root
    /// ([`Index::with_path_specifiers`]).
    go_modules: Vec<GoModule>,
    /// file id → scope facts from its import rows.
    file_scopes: HashMap<i64, FileScope>,
    /// file id → specifier target of each of the file's path-grammar `Imports`
    /// rows → the file-root modules that row binds to, id-sorted (empty when it
    /// binds nothing) — the **imported** scope a path-grammar call binds within
    /// (S-440, [`Index::with_imported_bindings`]). Empty until that is called.
    imported: HashMap<i64, HashMap<String, Vec<NodeId>>>,
    /// Normalised crate names present in the graph.
    crates: HashSet<String>,
    /// How every file is keyed: by its package ([CR-149]), by the namespace it
    /// declares (S-518), or by its path — under its plugin's package-file stems
    /// and import roots — plus each language's interop family (S-519). The
    /// registry's layout in production, with the import roots detected from
    /// this graph's files ([`Index::build_with_layout`]); Rust's stems alone
    /// for a synthetic test graph ([`Index::build`]).
    ///
    /// [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    layout: PackageLayout,
    /// Fully-qualified name → the **top-level** type nodes declared under it in
    /// a package-shaped file, id-sorted — the universe a Java import, a
    /// same-package name and a wildcard resolve against ([CR-149]). Two entries
    /// under one name (a `src/main` and a `src/test` declaration) are an
    /// ambiguity, never a pick ([NFR-RA-05]).
    ///
    /// [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    ///
    /// Partitioned by **interop family** first (S-519, [NFR-RA-05];
    /// [`PackageLayout::family`]): a source reaches only the types of its own
    /// family, so a C# import never names a PHP class of the same
    /// fully-qualified spelling, while Java and Kotlin — one `jvm` family —
    /// still name each other's.
    types_by_fqn: HashMap<String, HashMap<Vec<String>, Vec<NodeId>>>,
    /// Declared namespace → the file-root module nodes of the declared-namespace
    /// files declaring it, id-sorted (S-518, [FR-RS-13]) — what a namespace
    /// wildcard (C#'s `using N;`, Kotlin's `import n.*`) binds to: a namespace
    /// has no node of its own, and the files declaring it are the code it names,
    /// as a Go import path names its directory's files (S-439). Empty for every
    /// file of any other module model. Partitioned by interop family first,
    /// as [`types_by_fqn`](Index::types_by_fqn) is (S-519).
    ///
    /// [FR-RS-13]: ../../../docs/specs/requirements/FR-RS-13.md
    files_by_namespace: HashMap<String, HashMap<Vec<String>, Vec<NodeId>>>,
    /// `(trait node, method name)` → the concrete workspace impl method nodes of
    /// that trait method, id-sorted and deduplicated — the fan-out universe for a
    /// `dyn T` method call (S-281, [CR-073], [FR-RS-08]). Built from the
    /// `Implements` reference rows (impl method → its trait), so it is available
    /// on the very first index pass, before any `Implements` edge is committed.
    ///
    /// [CR-073]: ../../../docs/requests/CR-073-trait-object-dynamic-dispatch-reachability.md
    /// [FR-RS-08]: ../../../docs/specs/requirements/FR-RS-08.md
    impls_by_trait_method: HashMap<(NodeId, String), Vec<NodeId>>,
    /// A type → the in-repository types its `Extends` rows bind to as
    /// `Extends`, id-sorted (S-468, [CR-150] §3.2 B; S-522 for every language
    /// whose types record one): one superclass for a class (each base of a
    /// Python class), the super-interfaces for an interface. A supertype bound
    /// as `Implements` — a C# or Kotlin class's interface — is not one, as a
    /// Java class's `implements` is not. Bound from the `Extends` ledger rows
    /// by S-466's own rule ([`bind`]) while the index is built, so it is
    /// available on the very first index pass, before any `Extends` edge is
    /// committed — the [`impls_by_trait_method`](Index::impls_by_trait_method)
    /// precedent. An unbound `Extends` (a JDK, library or other-member
    /// superclass) contributes nothing: the walk ends there.
    ///
    /// [CR-150]: ../../../docs/requests/CR-150-java-receiver-typing-for-method-calls.md
    supertypes: HashMap<NodeId, Vec<NodeId>>,
    /// The name tokens of the type hierarchy: every hierarchy `Extends` row's
    /// target, bound or not (S-468). A sync dirtying one of them may move
    /// a supertype walk whose row spells none of them
    /// ([`Index::hierarchy_touched`]). A type's own name is not needed: a walk
    /// that crosses a type other than its start crosses it as some `Extends`
    /// row's target, and a row starting at a type spells that type's name.
    hierarchy_tokens: HashSet<String>,
    /// The interop families of those rows' sources (S-522,
    /// [`PackageLayout::family`]): the languages whose calls a supertype walk
    /// can bind, so whose calls a moved hierarchy re-selects
    /// ([`Index::walks_hierarchy`]).
    hierarchy_families: HashSet<String>,
    /// The classes that use a trait (S-522, a PHP `use`, [`TRAIT_USE_ALIAS`]).
    /// A used trait's method outranks every inherited one, and an unbound
    /// trait's methods are not in the graph, so a supertype walk reads such a
    /// class's own members and never climbs **through** it
    /// ([`Ctx::supertype_member`]). A `parent::m()` written in it starts at its
    /// parent, which its traits do not touch.
    trait_users: HashSet<NodeId>,
    /// node → the self type its plugin query recorded for it (S-493,
    /// [FR-RS-11]; the `nodes.self_type` column). Empty unless the run was given
    /// the store's self types ([`Index::with_self_types`]); empty binds no call
    /// through a self type, and every `Self::m` row takes the path it took
    /// before.
    ///
    /// [FR-RS-11]: ../../../docs/specs/requirements/FR-RS-11.md
    self_types: HashMap<NodeId, String>,
    /// `(crate, self type, name)` → the callables recorded with that self type,
    /// id-sorted — the universe a `Self::m` call binds within
    /// ([`Ctx::resolve_self_type_call`]). Across a type's inherent and trait
    /// impls and every file of its crate; a same-named type of another crate is
    /// another key, never a candidate. Same-named types of two modules of one
    /// crate share a key, and the call's own module tells them apart.
    methods_by_self_type: HashMap<(String, String, String), Vec<NodeId>>,
    /// `(crate, name)` → how many type-like nodes ([`is_type_like`]) the crate
    /// declares under that name — whether a self type's base name denotes one
    /// type of the crate, which is what lets a candidate outside the caller's
    /// module be the caller's type's method.
    type_names: HashMap<(String, String), usize>,
    /// project-relative path → file id, for the files whose ledger rows name a
    /// source node of the snapshot — how a node reaches its file's
    /// [`FileScope`] (S-493: whether its file imports its self type's name).
    file_ids: HashMap<String, i64>,
    /// Every method of an `impl Trait for X` block — the source of an
    /// `Implements` ledger row outside a package-shaped language (S-281's rows,
    /// whatever their trait resolves to). Read by the self-type call's module
    /// rung (S-493): Rust resolves `Self::m` to an inherent `m` before a trait's.
    trait_impl_methods: HashSet<NodeId>,
}

impl Index {
    /// Digest a snapshot into the binding index.
    ///
    /// A thin orchestrator over focused sub-builders, each owning one lookup
    /// structure. Ordering is load-bearing: `members` lists and `by_name`
    /// lists are id-sorted, `modules`/`by_symbol` are first-wins on a
    /// duplicate key, and the module tree is built by an id-/name-sorted DFS —
    /// so every helper preserves the same canonical iteration order the
    /// monolith had, keeping the built `Index` byte-identical ([NFR-RA-06]).
    ///
    /// The path model for every file, with the package-file stems the rust
    /// plugin declares for `.rs` ([`PackageLayout::rust_stems_for_tests`],
    /// S-519), so a synthetic `src/lib.rs` still names its crate. Test-only
    /// since S-470: both production builders — the resolution pass and the
    /// framework-promotion pass — build with the registry's layout
    /// ([`build_with_layout`](Index::build_with_layout)).
    ///
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    #[cfg(test)]
    pub(crate) fn build(nodes: &[NodeRow], edges: &[EdgeRow], refs: &[UnresolvedRefRow]) -> Index {
        Self::build_with_layout(nodes, edges, refs, PackageLayout::rust_stems_for_tests())
    }

    /// [`build`](Index::build), keying every file by `layout` — in production
    /// the layout the loaded plugins declare ([`PackageLayout::from_registry`]):
    /// a package-shaped file by its package ([CR-149]), a declared-namespace
    /// file by its namespace, a path-model file by its path under its plugin's
    /// stems and import roots, the roots detected here from this graph's files
    /// (S-519). `build` is this with Rust's stems alone.
    ///
    /// [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    pub(crate) fn build_with_layout(
        nodes: &[NodeRow],
        edges: &[EdgeRow],
        refs: &[UnresolvedRefRow],
        layout: PackageLayout,
    ) -> Index {
        // Built once and shared: module-tree construction and containment both
        // look nodes up by id.
        let node_by_id: HashMap<NodeId, &NodeRow> = nodes.iter().map(|n| (n.id, n)).collect();
        // The import roots in force are read off the files this graph holds
        // (S-519), so a cold index and a sync over one tree key alike.
        let layout = layout.with_detected_import_roots(nodes.iter().filter_map(|n| {
            (n.kind == NodeKind::Module).then_some(n.file_path.as_deref()).flatten()
        }));

        // Contains topology first — module-tree construction needs it.
        let (parent, members) = build_containment(edges, &node_by_id);
        let (modules, module_key) =
            build_module_tree(nodes, &parent, &members, &node_by_id, &layout);
        let crates: HashSet<String> = modules.keys().map(|(c, _)| c.clone()).collect();
        let dir_modules = build_directory_modules(nodes, &parent, &layout);

        let info = build_node_info(nodes, &parent, &module_key, &layout);
        let types_by_fqn = build_package_types(nodes, &parent, &members, &node_by_id, &layout);
        let files_by_namespace = build_namespace_files(nodes, &parent, &layout);
        let by_symbol = build_by_symbol(nodes);
        let by_name = build_by_name(nodes);
        let by_file_path = build_by_file_path(nodes);
        let (file_roots_by_stem, file_roots_by_dir) = build_file_roots(nodes, &parent);
        let routes_by_template = build_routes_by_template(nodes);
        let file_scopes = build_file_scopes(refs);
        let impls_by_trait_method =
            build_impls_by_trait_method(refs, &by_symbol, &by_name, &info, &layout);

        let mut index = Index {
            by_symbol,
            info,
            parent,
            members,
            modules,
            dir_modules,
            module_key,
            by_name,
            by_file_path,
            file_roots_by_stem,
            file_roots_by_dir,
            specifier_targets: HashMap::new(),
            go_modules: Vec::new(),
            routes_by_template,
            file_scopes,
            imported: HashMap::new(),
            crates,
            layout,
            types_by_fqn,
            files_by_namespace,
            impls_by_trait_method,
            supertypes: HashMap::new(),
            hierarchy_tokens: HashSet::new(),
            hierarchy_families: HashSet::new(),
            trait_users: HashSet::new(),
            self_types: HashMap::new(),
            methods_by_self_type: HashMap::new(),
            type_names: HashMap::new(),
            file_ids: HashMap::new(),
            trait_impl_methods: HashSet::new(),
        };
        let hierarchy = build_supertypes(refs, &index);
        index.supertypes = hierarchy.supertypes;
        index.hierarchy_tokens = hierarchy.tokens;
        index.hierarchy_families = hierarchy.families;
        index.trait_users = hierarchy.trait_users;
        index.trait_impl_methods = build_trait_impl_methods(refs, &index);
        index.file_ids = refs
            .iter()
            .filter_map(|r| {
                let file_id = r.file_id?;
                let path = index.by_symbol.get(&r.source_symbol).and_then(|id| index.info.get(id))?;
                Some((path.file_path.clone()?, file_id))
            })
            .collect();
        index.apply_global_globs(refs);
        index
    }

    /// Bring each global namespace wildcard ([`GLOBAL_WILDCARD_ALIAS`], S-518)
    /// into view in every package-shaped file of its declaring file's extension
    /// under the declaring file's directory — appended to that file's
    /// [`FileScope::globs`] after its own, in ledger order, so the scope a row
    /// reads is deterministic ([NFR-RA-06]). A file with no ledger row has no
    /// row to bind and is not visited; one with rows but no import of its own
    /// gains a scope here.
    ///
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    fn apply_global_globs(&mut self, refs: &[UnresolvedRefRow]) {
        let declared: Vec<(String, String, Vec<String>)> = refs
            .iter()
            .filter(|r| {
                r.kind == EdgeKind::Imports
                    && r.form == RefForm::Glob
                    && r.alias.as_deref() == Some(GLOBAL_WILDCARD_ALIAS)
            })
            .filter_map(|r| {
                let path = self
                    .by_symbol
                    .get(&r.source_symbol)
                    .and_then(|id| self.info.get(id))
                    .and_then(|i| i.file_path.as_deref())?;
                let dir = path.rsplit_once('/').map_or("", |(dir, _)| dir).to_string();
                Some((dir, extension_of(path), split(&r.target)))
            })
            .collect();
        if declared.is_empty() {
            return;
        }
        for (path, file_id) in &self.file_ids {
            if !self.layout.is_package_shaped(path) {
                continue;
            }
            let ext = extension_of(path);
            for (dir, glob_ext, glob) in &declared {
                let under = dir.is_empty() || path.starts_with(&format!("{dir}/"));
                if !under || *glob_ext != ext {
                    continue;
                }
                // A file importing nothing of its own has no scope yet.
                let scope = self.file_scopes.entry(*file_id).or_default();
                if !scope.globs.contains(glob) {
                    scope.globs.push(glob.clone());
                }
            }
        }
    }

    /// Declare which files write their import specifiers as paths, and the Go
    /// modules a Go import path is anchored on (S-439).
    pub(crate) fn with_path_specifiers(
        mut self,
        specifier_targets: HashMap<String, HashSet<String>>,
        go_modules: Vec<GoModule>,
    ) -> Index {
        self.specifier_targets = specifier_targets;
        self.go_modules = go_modules;
        self
    }

    /// Whether the file of `node` imports `name` from outside its crate — a
    /// `use` whose path is not headed by `crate`, `self`, `super` or one of the
    /// crate's own top-level modules (S-493). Such a self type names a foreign
    /// type, whatever type of that name the crate declares. A name brought in
    /// by a glob is not seen; the crate's own declarations still decide it.
    fn imports_foreign(&self, node: &NodeInfo, name: &str) -> bool {
        let Some(path) = node
            .file_path
            .as_deref()
            .and_then(|p| self.file_ids.get(p))
            .and_then(|id| self.file_scopes.get(id))
            .and_then(|scope| scope.aliases.get(name))
        else {
            return false;
        };
        let Some(head) = path.first() else { return false };
        let local = matches!(head.as_str(), "crate" | "self" | "super")
            || self
                .modules
                .contains_key(&(node.crate_name.clone(), vec![head.clone()]));
        !local
    }

    /// Give the index the self types the store records per node (S-493,
    /// [FR-RS-11]; [`GraphStore::node_self_types`]) — what a `self.m()` /
    /// `Self::m()` call binds through. Rows naming a node the snapshot does not
    /// hold, and non-callables, contribute no candidate.
    ///
    /// [FR-RS-11]: ../../../docs/specs/requirements/FR-RS-11.md
    /// [`GraphStore::node_self_types`]: crate::graph_store::GraphStore::node_self_types
    pub(crate) fn with_self_types(mut self, self_types: Vec<(NodeId, String)>) -> Index {
        let mut by_type: HashMap<(String, String, String), Vec<NodeId>> = HashMap::new();
        for (id, self_type) in &self_types {
            let Some(info) = self.info.get(id) else { continue };
            // A method of a type its file imports from outside the crate is a
            // foreign type's: never a candidate for a call on a crate type.
            if !Want::Callable.admits(info.kind) || self.imports_foreign(info, self_type) {
                continue;
            }
            by_type
                .entry((info.crate_name.clone(), self_type.clone(), info.name.clone()))
                .or_default()
                .push(*id);
        }
        for ids in by_type.values_mut() {
            ids.sort_unstable();
            ids.dedup();
        }
        let mut type_names: HashMap<(String, String), usize> = HashMap::new();
        for info in self.info.values().filter(|i| is_type_like(i.kind)) {
            *type_names
                .entry((info.crate_name.clone(), info.name.clone()))
                .or_default() += 1;
        }
        self.self_types = self_types.into_iter().collect();
        self.methods_by_self_type = by_type;
        self.type_names = type_names;
        self
    }

    /// Bind every path-grammar `Imports` row of `refs` and record what each
    /// bound to, per file — the imported scope [`Ctx::resolve_imported_call`]
    /// consults (S-440, [CR-142] D2, [FR-RS-03]).
    ///
    /// The scope is the **outcome of the import's own binding** ([`bind`], under
    /// the same `policy`), so a call can only ever resolve into a file its
    /// import edge reaches, however that edge came to bind. Computed here, before
    /// any call is bound, so one pass binds imports and the calls through them
    /// alike: a cold index needs no second run to see its own import edges.
    /// Call after [`Index::with_path_specifiers`] — without the specifier context
    /// no file is path-grammar and the scope stays empty.
    ///
    /// [CR-142]: ../../../docs/requests/CR-142-cross-file-call-resolution-is-rust-only.md
    /// [FR-RS-03]: ../../../docs/specs/requirements/FR-RS-03.md
    pub(crate) fn with_imported_bindings(
        mut self,
        refs: &[UnresolvedRefRow],
        policy: BindingPolicy,
    ) -> Index {
        let mut imported: HashMap<i64, HashMap<String, Vec<NodeId>>> = HashMap::new();
        for r in refs {
            if r.kind != EdgeKind::Imports {
                continue;
            }
            let Some(file_id) = r.file_id else { continue };
            let path_grammar = self
                .by_symbol
                .get(&r.source_symbol)
                .and_then(|id| self.info.get(id))
                .and_then(|i| i.file_path.as_deref())
                .is_some_and(|p| self.is_path_specifier_file(p));
            if !path_grammar {
                continue;
            }
            let targets = match bind(r, &self, policy) {
                Outcome::Bound { target, .. } => vec![target],
                Outcome::BoundMany { targets, .. } => targets,
                Outcome::Unbound => Vec::new(),
            };
            imported
                .entry(file_id)
                .or_default()
                .entry(r.target.clone())
                .or_default()
                .extend(targets);
        }
        for by_target in imported.values_mut() {
            for targets in by_target.values_mut() {
                targets.sort();
                targets.dedup();
            }
        }
        self.imported = imported;
        self
    }

    /// Whether a file at `path` writes its import specifiers as paths (S-439) —
    /// the incremental run re-binds every import such a file records
    /// (`resolve::is_affected`).
    pub(crate) fn is_path_specifier_file(&self, path: &str) -> bool {
        self.specifier_targets.contains_key(&extension_of(path))
    }

    /// The top-level package-shaped type nodes declared under the
    /// fully-qualified name `fqn`, id-sorted — empty when no file of this graph
    /// declares it, two or more when several do (a `src/main` and a `src/test`
    /// declaration). The index the import rungs read ([CR-149]), exposed so the
    /// framework pass finds a constant's declaring type the same way (S-470)
    /// rather than deriving a second FQN from a path.
    ///
    /// [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    ///
    /// Read within the interop family of the file at `source_path` (S-519): a
    /// type of another family is never the type a source names.
    pub(crate) fn package_types(&self, source_path: &str, fqn: &[String]) -> &[NodeId] {
        self.layout
            .family(source_path)
            .and_then(|family| self.types_by_fqn.get(&family))
            .and_then(|types| types.get(fqn))
            .map_or(&[], Vec::as_slice)
    }

    /// Every fully-qualified name a top-level package-shaped type of this graph
    /// is declared under, sorted — what a workspace compares across members to
    /// tell a type another member declares from one no member does (S-468,
    /// [CR-150] §3.2 C). Read off the same index the import rungs bind against,
    /// never a second derivation.
    ///
    /// [CR-150]: ../../../docs/requests/CR-150-java-receiver-typing-for-method-calls.md
    pub(crate) fn declared_type_names(&self) -> Vec<Vec<String>> {
        let mut names: Vec<Vec<String>> = self
            .types_by_fqn
            .values()
            .flat_map(|types| types.keys().cloned())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Whether `dirty` — the tokens a sync added or removed — names a type an
    /// `Extends` row of the type hierarchy names (S-468). A supertype walk that crosses
    /// such a type can change its answer although the calling row spells none
    /// of its names, so the incremental run re-binds every call that can walk
    /// it ([`walks_hierarchy`](Index::walks_hierarchy)) when this holds
    /// (`resolve::is_affected`).
    pub(crate) fn hierarchy_touched(&self, dirty: &HashSet<String>) -> bool {
        self.hierarchy_tokens.iter().any(|t| dirty.contains(t))
    }

    /// Whether a call written in the file at `path` can bind through the type
    /// hierarchy (S-468; S-522): a package-shaped file, or one of an interop
    /// family whose types record an `Extends` the hierarchy holds — a Kotlin
    /// `.kts` script's call can climb a `.kt` class's. A family that records
    /// none — Rust — has no call a moved hierarchy can move.
    pub(crate) fn walks_hierarchy(&self, path: &str) -> bool {
        self.layout.is_package_shaped(path)
            || self
                .layout
                .family(path)
                .is_some_and(|family| self.hierarchy_families.contains(&family))
    }

    /// Whether adding or removing the file at `path` can move the detected
    /// import roots (S-519, [`PackageLayout::moves_import_roots`]).
    pub(crate) fn moves_import_roots(&self, path: &str) -> bool {
        self.layout.moves_import_roots(path)
    }

    /// Whether the file at `path` is keyed under import roots (S-519).
    pub(crate) fn has_import_roots(&self, path: &str) -> bool {
        self.layout.has_import_roots(path)
    }

    /// Whether the file at `path` is keyed by its package ([CR-149]).
    ///
    /// [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    pub(crate) fn is_package_shaped(&self, path: &str) -> bool {
        self.layout.is_package_shaped(path)
    }

    /// The in-repository supertypes of `ty` its `Extends` rows bind to.
    fn supertypes_of(&self, ty: NodeId) -> &[NodeId] {
        self.supertypes.get(&ty).map_or(&[], Vec::as_slice)
    }

    /// [`supertypes_of`](Index::supertypes_of), for the binder's own tests.
    #[cfg(test)]
    pub(crate) fn supertypes_for_tests(&self, ty: NodeId) -> &[NodeId] {
        self.supertypes_of(ty)
    }

    /// The project-relative file of node `id`, when it has one — how the
    /// framework pass reads a [`package_types`](Index::package_types) answer's
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

    /// The concrete workspace impl method nodes of trait method `(trait, name)`,
    /// id-sorted; empty when none are recorded.
    fn impls_of(&self, trait_node: NodeId, name: &str) -> &[NodeId] {
        self.impls_by_trait_method
            .get(&(trait_node, name.to_string()))
            .map_or(&[], Vec::as_slice)
    }

    /// Members of `scope` named `name`, filtered by `want`.
    fn members_named(&self, scope: NodeId, name: &str, want: Want) -> Vec<NodeId> {
        self.members
            .get(&scope)
            .and_then(|m| m.get(name))
            .map(|ids| {
                ids.iter()
                    .copied()
                    .filter(|id| self.info.get(id).is_some_and(|i| want.admits(i.kind)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The module key of the nearest enclosing module of `node` (itself
    /// included), if any.
    fn nearest_module(&self, node: NodeId) -> Option<&ModKey> {
        let mut cursor = Some(node);
        while let Some(id) = cursor {
            if let Some(key) = self.module_key.get(&id) {
                return Some(key);
            }
            cursor = self.parent.get(&id).copied();
        }
        None
    }

    /// Whether re-binding `r` could change its outcome given `dirty` — the tokens
    /// a sync added or removed (see [`tokens`](super::tokens)).
    ///
    /// A row is affected when a token of its target lands in `dirty`, or when an
    /// `as`-alias in the row's file rewrites that target's head into a path that
    /// does. Globs need no expansion: a glob-imported call resolves under its own
    /// bare name, already a target token; only renaming aliases route a reference
    /// to a name it does not spell. The alias chase is bounded by
    /// [`MAX_ALIAS_DEPTH`], the same cap [`Ctx::resolve_path`] honours, so a
    /// self-referential import cannot loop here either.
    ///
    /// A package-shaped file's globs **do** need their own tokens ([CR-149]): a
    /// wildcard's target is a type or package whose identity can change while
    /// the member name a call spells does not — a second `C` under
    /// `import static a.b.C.*` makes `m()` ambiguous without touching `m`. So a
    /// row of such a file is also affected when a token of any of its globs is
    /// dirty. Every other language's selection is unchanged.
    ///
    /// A `Self::m` row (S-493) is also affected when a token of its caller's
    /// self type is dirty: a second type of that name elsewhere in the crate
    /// moves its binding to the caller's module ([`Ctx::resolve_self_type_call`])
    /// without touching `m`.
    ///
    /// [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    pub(crate) fn ref_affected(&self, r: &UnresolvedRefRow, dirty: &HashSet<String>) -> bool {
        if super::tokens(&r.target).iter().any(|t| dirty.contains(t)) {
            return true;
        }
        // A `Self::m` call (S-493) also reads how many types its caller's self
        // type names in the crate, a name its target does not spell.
        if r.target.split_once("::").is_some_and(|(head, _)| head == SELF_TYPE_HEAD) {
            let self_type = self
                .by_symbol
                .get(&r.source_symbol)
                .and_then(|id| self.self_types.get(id));
            if self_type.is_some_and(|ty| super::tokens(ty).iter().any(|t| dirty.contains(t))) {
                return true;
            }
        }
        let Some(file_id) = r.file_id else {
            return false;
        };
        let Some(scope) = self.file_scopes.get(&file_id) else {
            return false;
        };
        let package_shaped = || {
            self.by_symbol
                .get(&r.source_symbol)
                .and_then(|id| self.info.get(id))
                .and_then(|i| i.file_path.as_deref())
                .is_some_and(|p| self.layout.is_package_shaped(p))
        };
        if !(scope.globs.is_empty() && scope.static_globs.is_empty())
            && scope
                .globs
                .iter()
                .chain(&scope.static_globs)
                .flatten()
                .flat_map(|seg| super::tokens(seg))
                .any(|t| dirty.contains(&t))
            && package_shaped()
        {
            return true;
        }
        // A package-shaped row reads every expansion of its head, not only the
        // first (`FileScope::alias_expansions`), so each must be able to select
        // it — and so does an import-root file's row (S-519).
        let first = r.target.split("::").next().unwrap_or_default();
        let import_root = || {
            self.by_symbol
                .get(&r.source_symbol)
                .and_then(|id| self.info.get(id))
                .and_then(|i| i.file_path.as_deref())
                .is_some_and(|p| self.layout.has_import_roots(p))
        };
        if scope
            .alias_expansions
            .get(first)
            .is_some_and(|all| all.len() > 1)
            && scope.alias_expansions[first]
                .iter()
                .flatten()
                .flat_map(|seg| super::tokens(seg))
                .any(|t| dirty.contains(&t))
            && (package_shaped() || import_root())
        {
            return true;
        }
        // Chase the head segment through `as`-alias rewrites: each hop's path may
        // name something dirty even though the written target does not.
        let mut head = first.to_string();
        for _ in 0..MAX_ALIAS_DEPTH {
            let Some(path) = scope.aliases.get(&head) else {
                return false;
            };
            if path
                .iter()
                .flat_map(|seg| super::tokens(seg))
                .any(|t| dirty.contains(&t))
            {
                return true;
            }
            let Some(next) = path.first() else {
                return false;
            };
            head = next.clone();
        }
        false
    }
}

/// `Contains` topology: child→parent, and scope→name→members with each member
/// list id-sorted for a deterministic candidate order regardless of edge
/// iteration order. `parent` records every `Contains` edge; a `members` entry
/// is added only when the target is a known node (its name is needed).
fn build_containment(
    edges: &[EdgeRow],
    node_by_id: &HashMap<NodeId, &NodeRow>,
) -> (HashMap<NodeId, NodeId>, Members) {
    let mut parent: HashMap<NodeId, NodeId> = HashMap::new();
    let mut members: Members = HashMap::new();
    for e in edges {
        if e.kind != EdgeKind::Contains {
            continue;
        }
        parent.insert(e.target, e.source);
        if let Some(n) = node_by_id.get(&e.target) {
            members
                .entry(e.source)
                .or_default()
                .entry(n.name.clone())
                .or_default()
                .push(e.target);
        }
    }
    for by_name in members.values_mut() {
        for list in by_name.values_mut() {
            list.sort();
        }
    }
    (parent, members)
}

/// The path-derived module tree as `(modules, module_key)` — the forward
/// `(crate, path)`→node map and its reverse. A parentless `Module` node is a
/// *file module* keyed by its file path; inline `mod`s hang beneath it. File
/// roots are visited id-sorted so the first id wins a (rare) path tie, exactly
/// as the monolith did — except at a key a bodyless `mod x;` claims, which goes
/// to the file it declares ([`hand_declarations_to_their_files`], S-585).
fn build_module_tree(
    nodes: &[NodeRow],
    parent: &HashMap<NodeId, NodeId>,
    members: &Members,
    node_by_id: &HashMap<NodeId, &NodeRow>,
    layout: &PackageLayout,
) -> (HashMap<ModKey, NodeId>, HashMap<NodeId, ModKey>) {
    let mut modules: HashMap<ModKey, NodeId> = HashMap::new();
    let mut module_key: HashMap<NodeId, ModKey> = HashMap::new();
    let mut file_roots: Vec<&NodeRow> = nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Module && !parent.contains_key(&n.id))
        .collect();
    // First-by-id wins a (rare) path tie — except among import-root files
    // (S-519), where a module and its stub (`foo.py`, `foo.pyi`) share a key
    // routinely and a node id changes on every re-extraction: the path decides
    // there, so a sync keeps the winner a cold index picks ([NFR-RA-06]). They
    // sit in their family's own crate, so the two orders never compete for a key.
    file_roots.sort_by_key(|n| {
        let import_root_path = n
            .file_path
            .as_deref()
            .filter(|p| layout.has_import_roots(p));
        (import_root_path, n.id)
    });
    let mut files_at: HashMap<ModKey, Vec<(NodeId, String)>> = HashMap::new();
    for root in file_roots {
        let Some(path) = &root.file_path else {
            continue; // an orphaned module node cannot anchor a tree
        };
        let key = layout.module_key(path);
        modules.entry(key.clone()).or_insert(root.id);
        module_key.insert(root.id, key.clone());
        files_at
            .entry(key.clone())
            .or_default()
            .push((root.id, extension_of(path)));
        append_inline_modules(
            root.id,
            key,
            members,
            node_by_id,
            &mut modules,
            &mut module_key,
        );
    }
    hand_declarations_to_their_files(nodes, parent, members, &files_at, &mut modules, &module_key);
    (modules, module_key)
}

/// Give each key a bodyless `mod x;` declaration claims to the file module it
/// declares (S-585, [FR-RS-41]). The declaration is a node of the declaring
/// file, appended beneath it like an inline `mod`, so its key is exactly the
/// key of `x.rs` or `x/mod.rs` under the declaring module's directory — and
/// the first-by-id tie-break used to hand the key to whichever came first,
/// usually the empty declaration, so every path through it found nothing.
///
/// The key goes to the **one** file module of the declaration's own language
/// (its file extension) at it — the path model keys `src/x.js` where it keys
/// `src/x.rs`, and a Rust declaration never names a JavaScript file. With none
/// (a `#[path]` attribute, which is not read, a missing file, or only another
/// language's file at that path) the declaration keeps the key, taking it back
/// from a foreign file that won it, and binds nothing beneath; with two (`x.rs`
/// beside `x/mod.rs`, which rustc refuses) the declaration takes it back from
/// whichever file won, so neither is guessed ([NFR-RA-05]). Where no file
/// answers and two declarations claim one key (`mod x;` in both `src/lib.rs`
/// and `src/main.rs`), the first in path order holds it. Declarations are
/// visited in path order, never node order, so the outcome reads no node id: a
/// sync and a cold index agree ([NFR-RA-06]). No node, symbol or edge changes
/// — only which node answers the key.
///
/// A declaration is a nested `Module` with no `Contains` child on a single
/// line. An inline `mod x { … }` with contents, or with braces over several
/// lines, is not one, and keeps the key exactly as before — also against a
/// declaration of the same key from the other crate root (`src/lib.rs` and
/// `src/main.rs` share the crate's root key): only a file module or another
/// declaration holding a key is ever displaced.
///
/// [FR-RS-41]: ../../../docs/specs/requirements/FR-RS-41.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
fn hand_declarations_to_their_files(
    nodes: &[NodeRow],
    parent: &HashMap<NodeId, NodeId>,
    members: &Members,
    files_at: &HashMap<ModKey, Vec<(NodeId, String)>>,
    modules: &mut HashMap<ModKey, NodeId>,
    module_key: &HashMap<NodeId, ModKey>,
) {
    let is_declaration = |n: &NodeRow| {
        n.kind == NodeKind::Module
            && parent.contains_key(&n.id)
            && !members.contains_key(&n.id)
            && n.start_line == n.end_line
    };
    let mut declarations: Vec<&NodeRow> = nodes.iter().filter(|n| is_declaration(n)).collect();
    // A sync renumbers a re-extracted file's nodes; the path does not move.
    declarations.sort_by(|a, b| {
        (a.file_path.as_deref(), a.start_line, a.name.as_str())
            .cmp(&(b.file_path.as_deref(), b.start_line, b.name.as_str()))
    });
    let is_declared: HashSet<NodeId> = declarations.iter().map(|d| d.id).collect();
    let mut claimed: HashSet<&ModKey> = HashSet::new();
    for declaration in declarations {
        let Some(key) = module_key.get(&declaration.id) else { continue };
        let files = files_at.get(key).map_or(&[][..], Vec::as_slice);
        let language = declaration.file_path.as_deref().map(extension_of);
        let declared: Vec<NodeId> = files
            .iter()
            .filter(|(_, ext)| language.as_ref() == Some(ext))
            .map(|(id, _)| *id)
            .collect();
        let holder = modules.get(key).copied();
        let file_holds = holder.is_some_and(|h| files.iter().any(|(id, _)| *id == h));
        if !file_holds && !holder.is_some_and(|h| is_declared.contains(&h)) {
            continue; // an inline module with contents holds it
        }
        // Otherwise the first declaration in path order holds the key.
        let first = claimed.insert(key);
        match declared.as_slice() {
            [file] => {
                modules.insert(key.clone(), *file);
            }
            _ if first => {
                modules.insert(key.clone(), declaration.id);
            }
            _ => {}
        }
    }
}

/// Append every inline `mod` beneath `root` to the module maps, depth-first
/// (`Contains` is a tree). Each scope's `Module` children are visited
/// `(name, id)`-sorted so keys land deterministically regardless of map
/// iteration order; `modules` is first-wins per key, `module_key` last-wins
/// (a node has one parent, so it is appended once). The per-scope children are
/// pre-flattened into one sorted list and the non-module guard is a guard
/// clause, so the DFS body stays within nesting depth 3.
fn append_inline_modules(
    root: NodeId,
    root_key: ModKey,
    members: &Members,
    node_by_id: &HashMap<NodeId, &NodeRow>,
    modules: &mut HashMap<ModKey, NodeId>,
    module_key: &mut HashMap<NodeId, ModKey>,
) {
    let mut stack = vec![(root, root_key)];
    while let Some((scope, scope_key)) = stack.pop() {
        for (name, id) in sorted_children(members, scope) {
            // Only inline `mod`s extend the module tree; skip every other kind.
            let kind = node_by_id.get(&id).map(|n| n.kind);
            if kind != Some(NodeKind::Module) {
                continue;
            }
            let mut child_key = scope_key.clone();
            child_key.1.push(name.clone());
            modules.entry(child_key.clone()).or_insert(id);
            module_key.insert(id, child_key.clone());
            stack.push((id, child_key));
        }
    }
}

/// `scope`'s `Contains` children as a flat `(name, id)` list sorted by name then
/// id — the canonical visitation order the module-tree DFS appends in. Empty
/// when the scope has no members.
fn sorted_children(members: &Members, scope: NodeId) -> Vec<(&String, NodeId)> {
    let Some(by_name) = members.get(&scope) else {
        return Vec::new();
    };
    let mut children: Vec<(&String, NodeId)> = by_name
        .iter()
        .flat_map(|(name, ids)| ids.iter().map(move |&id| (name, id)))
        .collect();
    children.sort_by(|a, b| a.0.cmp(b.0).then(a.1.cmp(&b.1)));
    children
}

/// Per-node binding facts ([`NodeInfo`]) for every node.
fn build_node_info(
    nodes: &[NodeRow],
    parent: &HashMap<NodeId, NodeId>,
    module_key: &HashMap<NodeId, ModKey>,
    layout: &PackageLayout,
) -> HashMap<NodeId, NodeInfo> {
    let mut info: HashMap<NodeId, NodeInfo> = HashMap::new();
    for n in nodes {
        info.insert(
            n.id,
            NodeInfo {
                kind: n.kind,
                crate_name: crate_of_node(n, parent, module_key, layout),
                name: n.name.clone(),
                file_path: n.file_path.clone(),
            },
        );
    }
    info
}

/// The crate a node belongs to (for crate-first fallbacks): the nearest module
/// ancestor's crate, walking the `Contains` chain from the node itself; failing
/// that, the crate derived from the node's own file path, else empty.
fn crate_of_node(
    n: &NodeRow,
    parent: &HashMap<NodeId, NodeId>,
    module_key: &HashMap<NodeId, ModKey>,
    layout: &PackageLayout,
) -> String {
    let mut cursor = Some(n.id);
    while let Some(id) = cursor {
        if let Some((c, _)) = module_key.get(&id) {
            return c.clone();
        }
        cursor = parent.get(&id).copied();
    }
    match &n.file_path {
        Some(path) => layout.module_key(path).0,
        None => String::new(),
    }
}

/// The package-shaped type universe ([CR-149]): every top-level type-like
/// member of a package-shaped file's root module, keyed by its fully-qualified
/// name ([`PackageLayout::type_fqn`], the one FQN derivation). A package's
/// types named `n` are the entry `package ++ [n]`, so a wildcard and the
/// same-package rung read this map too. `nodes` is id-ordered and each file's
/// children are visited name-/id-sorted, but every list is sorted explicitly
/// anyway, so the map is deterministic regardless of visit order
/// ([NFR-RA-06]). Empty under the default layout.
///
/// Partitioned by the declaring file's interop family (S-519,
/// [`PackageLayout::family`]).
///
/// [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
fn build_package_types(
    nodes: &[NodeRow],
    parent: &HashMap<NodeId, NodeId>,
    members: &Members,
    node_by_id: &HashMap<NodeId, &NodeRow>,
    layout: &PackageLayout,
) -> HashMap<String, HashMap<Vec<String>, Vec<NodeId>>> {
    let mut by_family: HashMap<String, HashMap<Vec<String>, Vec<NodeId>>> = HashMap::new();
    for root in nodes {
        if root.kind != NodeKind::Module || parent.contains_key(&root.id) {
            continue;
        }
        let Some(path) = &root.file_path else { continue };
        if !layout.is_package_shaped(path) {
            continue;
        }
        let Some(family) = layout.family(path) else { continue };
        let by_fqn = by_family.entry(family).or_default();
        for (name, id) in sorted_children(members, root.id) {
            if !node_by_id.get(&id).is_some_and(|n| is_type_like(n.kind)) {
                continue;
            }
            if let Some(fqn) = layout.type_fqn(path, name) {
                by_fqn.entry(fqn).or_default().push(id);
            }
        }
    }
    for list in by_family.values_mut().flat_map(HashMap::values_mut) {
        list.sort();
        list.dedup();
    }
    by_family
}

/// The directory modules of every import-root file (S-519,
/// [`Index::dir_modules`]): each directory between a file-root module's import
/// root and its file ([`PackageLayout::directory_modules`]). Empty under a
/// layout that declares no import roots.
fn build_directory_modules(
    nodes: &[NodeRow],
    parent: &HashMap<NodeId, NodeId>,
    layout: &PackageLayout,
) -> HashSet<ModKey> {
    nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Module && !parent.contains_key(&n.id))
        .filter_map(|n| n.file_path.as_deref())
        .flat_map(|path| layout.directory_modules(path))
        .collect()
}

/// Declared namespace → the file-root modules of the declared-namespace files
/// declaring it (S-518, [`Index::files_by_namespace`]): every parentless
/// `Module` node whose file the layout knows a declared namespace for
/// ([`PackageLayout::package_of`], the one reading of it), id-sorted
/// ([NFR-RA-06]). Empty under a layout that knows no namespace.
///
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
fn build_namespace_files(
    nodes: &[NodeRow],
    parent: &HashMap<NodeId, NodeId>,
    layout: &PackageLayout,
) -> HashMap<String, HashMap<Vec<String>, Vec<NodeId>>> {
    let mut by_family: HashMap<String, HashMap<Vec<String>, Vec<NodeId>>> = HashMap::new();
    for root in nodes {
        if root.kind != NodeKind::Module || parent.contains_key(&root.id) {
            continue;
        }
        let Some(path) = root.file_path.as_deref() else { continue };
        if !layout.declares_namespaces(path) {
            continue;
        }
        let (Some(family), Some(namespace)) = (layout.family(path), layout.package_of(path)) else {
            continue;
        };
        by_family
            .entry(family)
            .or_default()
            .entry(namespace)
            .or_default()
            .push(root.id);
    }
    for list in by_family.values_mut().flat_map(HashMap::values_mut) {
        list.sort();
        list.dedup();
    }
    by_family
}

/// Canonical symbol → node. First-wins on a (model-prohibited) duplicate
/// symbol: `nodes` arrives id-ordered from `all_nodes()`, so this matches the
/// store's `node_id_for_symbol` min-id pick (a no-op in practice, ADR-07).
fn build_by_symbol(nodes: &[NodeRow]) -> HashMap<String, NodeId> {
    let mut by_symbol: HashMap<String, NodeId> = HashMap::with_capacity(nodes.len());
    for n in nodes {
        by_symbol
            .entry(n.symbol.as_str().to_string())
            .or_insert(n.id);
    }
    by_symbol
}

/// name → every node carrying it, id-sorted — the unique-match fallback
/// universe.
fn build_by_name(nodes: &[NodeRow]) -> HashMap<String, Vec<NodeId>> {
    let mut by_name: HashMap<String, Vec<NodeId>> = HashMap::new();
    for n in nodes {
        by_name.entry(n.name.clone()).or_default().push(n.id);
    }
    for list in by_name.values_mut() {
        list.sort();
    }
    by_name
}

/// file path → its nodes, for doc link/path resolution (S-035). `nodes` is
/// id-ordered from `all_nodes()`, so each list is already sorted.
fn build_by_file_path(nodes: &[NodeRow]) -> HashMap<String, Vec<NodeId>> {
    let mut by_file_path: HashMap<String, Vec<NodeId>> = HashMap::new();
    for n in nodes {
        if let Some(path) = &n.file_path {
            by_file_path.entry(path.clone()).or_default().push(n.id);
        }
    }
    by_file_path
}

/// The file-root module nodes (a parentless [`NodeKind::Module`] bound to a
/// file — one per code file) keyed two ways for path-specifier binding
/// (S-439): by the file path with its extension removed, and by directory.
/// `nodes` is id-ordered, so every list is already sorted ([NFR-RA-06]).
///
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
#[allow(clippy::type_complexity)]
fn build_file_roots(
    nodes: &[NodeRow],
    parent: &HashMap<NodeId, NodeId>,
) -> (
    HashMap<String, Vec<NodeId>>,
    HashMap<String, Vec<(String, NodeId)>>,
) {
    let mut by_stem: HashMap<String, Vec<NodeId>> = HashMap::new();
    let mut by_dir: HashMap<String, Vec<(String, NodeId)>> = HashMap::new();
    for n in nodes {
        if n.kind != NodeKind::Module || parent.contains_key(&n.id) {
            continue;
        }
        let Some(path) = &n.file_path else { continue };
        let (dir, name) = match path.rfind('/') {
            Some(i) => (&path[..i], &path[i + 1..]),
            None => ("", path.as_str()),
        };
        let stem = match name.rsplit_once('.') {
            Some((stem, _)) if !stem.is_empty() => &path[..path.len() - name.len() + stem.len()],
            _ => path.as_str(),
        };
        by_stem.entry(stem.to_string()).or_default().push(n.id);
        by_dir
            .entry(dir.to_string())
            .or_default()
            .push((path.clone(), n.id));
    }
    (by_stem, by_dir)
}

/// Normalized template → its `(METHOD, route node)` candidates, for the OpenAPI
/// operation→route match (S-069). A route name is `"METHOD /path"`; the bucket
/// key is the positionally-normalized template (parameter names/syntax erased)
/// and the upper-cased method rides with the node. A route whose template does
/// not normalize cleanly is skipped, so it can never become a candidate
/// ([FR-CG-09], [NFR-RA-05]). `nodes` is id-ordered, so each bucket is already
/// sorted by id.
///
/// Bucketing on the template alone is what lets a wildcard (`ANY`) provider be
/// compared against a concrete consumer method at all: the compatibility and
/// precedence rule then runs over the bucket in
/// [`resolve_route`](Ctx::resolve_route) ([CR-109], [FR-CG-09]).
///
/// [CR-109]: ../../../docs/requests/CR-109-wildcard-method-route-matching.md
fn build_routes_by_template(nodes: &[NodeRow]) -> HashMap<String, Vec<(String, NodeId)>> {
    let mut routes_by_template: HashMap<String, Vec<(String, NodeId)>> = HashMap::new();
    for n in nodes {
        if n.kind != NodeKind::Route {
            continue;
        }
        let Some((method, template)) = route_key(&n.name) else {
            continue;
        };
        routes_by_template
            .entry(template)
            .or_default()
            .push((method, n.id));
    }
    routes_by_template
}

/// Per-file scope facts (aliases + globs) from the ledger's `Imports` rows,
/// bound or not. Re-extracted files' rows were replaced by `persist_file`
/// before this pass; untouched files' rows persist — the map is whole at bind
/// time.
fn build_file_scopes(refs: &[UnresolvedRefRow]) -> HashMap<i64, FileScope> {
    let mut file_scopes: HashMap<i64, FileScope> = HashMap::new();
    for r in refs {
        if r.kind != EdgeKind::Imports {
            continue;
        }
        let Some(file_id) = r.file_id else { continue };
        let scope = file_scopes.entry(file_id).or_default();
        let path: Vec<String> = r.target.split("::").map(str::to_string).collect();
        match r.form {
            RefForm::Glob if r.alias.as_deref() == Some(STATIC_WILDCARD_ALIAS) => {
                scope.static_globs.push(path)
            }
            RefForm::Glob => scope.globs.push(path),
            _ => {
                if let Some(alias) = &r.alias {
                    scope
                        .alias_expansions
                        .entry(alias.clone())
                        .or_default()
                        .push(path.clone());
                    scope.aliases.entry(alias.clone()).or_insert(path);
                }
            }
        }
    }
    file_scopes
}

/// `(trait node, method name)` → the impl method nodes of that trait method,
/// id-sorted and deduplicated (S-281, [CR-073], [FR-RS-08]).
///
/// Built from the `Implements` reference rows an extraction pass emits, one per
/// method of an `impl T for X` block (`source` = the impl method's symbol,
/// `target` = the trait's written path). The trait is resolved by its **last
/// path segment** to the one workspace [`NodeKind::Trait`] of that name — a
/// non-unique or unindexed (external) trait contributes nothing, so a `dyn`
/// call whose trait is ambiguous or external never fabricates an impl set
/// ([NFR-RA-05]). Keyed by the method's own name (`info[source].name`) so the
/// fan-out is a direct lookup. Deterministic: lists are id-sorted and deduped,
/// mirroring [`build_by_name`] ([NFR-RA-06]).
///
/// [CR-073]: ../../../docs/requests/CR-073-trait-object-dynamic-dispatch-reachability.md
/// [FR-RS-08]: ../../../docs/specs/requirements/FR-RS-08.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
fn build_impls_by_trait_method(
    refs: &[UnresolvedRefRow],
    by_symbol: &HashMap<String, NodeId>,
    by_name: &HashMap<String, Vec<NodeId>>,
    info: &HashMap<NodeId, NodeInfo>,
    layout: &PackageLayout,
) -> HashMap<(NodeId, String), Vec<NodeId>> {
    let mut map: HashMap<(NodeId, String), Vec<NodeId>> = HashMap::new();
    for r in refs {
        if r.kind != EdgeKind::Implements {
            continue;
        }
        let Some(&impl_method) = by_symbol.get(&r.source_symbol) else {
            continue; // the impl method's own node is not indexed — skip
        };
        // A type's `Implements` (S-466 for Java, S-522 for every language)
        // is the type-relation arm's: its source is a class rather than an
        // impl method, and it has no place in the `dyn T` fan-out universe.
        if relation_want_of(r, info.get(&impl_method), layout).is_some() {
            continue;
        }
        let last = r.target.rsplit("::").next().unwrap_or(&r.target);
        // Resolve the trait by the same unique-name rule the query-time fan-out
        // uses ([`Index::trait_by_name`]) so index-build and bind agree; an
        // ambiguous / external trait contributes nothing (never fabricate an impl
        // set, [NFR-RA-05]).
        let Some(trait_node) = unique_trait(by_name, info, last) else {
            continue;
        };
        let Some(method_name) = info.get(&impl_method).map(|i| i.name.clone()) else {
            continue;
        };
        map.entry((trait_node, method_name)).or_default().push(impl_method);
    }
    for ids in map.values_mut() {
        ids.sort_unstable();
        ids.dedup();
    }
    map
}

/// The methods of `impl Trait for X` blocks ([`Index::trait_impl_methods`]):
/// the sources of the `Implements` rows extraction emits per such method
/// (S-281), outside a package-shaped language, whose `Implements` row is a
/// class's.
fn build_trait_impl_methods(refs: &[UnresolvedRefRow], ix: &Index) -> HashSet<NodeId> {
    refs.iter()
        .filter(|r| r.kind == EdgeKind::Implements)
        .filter_map(|r| ix.by_symbol.get(&r.source_symbol).copied())
        .filter(|id| {
            ix.info.get(id).is_some_and(|i| {
                Want::Callable.admits(i.kind)
                    && !i
                        .file_path
                        .as_deref()
                        .is_some_and(|p| ix.layout.is_package_shaped(p))
            })
        })
        .collect()
}

/// The type hierarchy (S-468, [CR-150] §3.2 B; S-522): each type's
/// in-repository supertypes, the name tokens a sync must watch to keep a walk
/// over it fresh, and the families whose rows it holds ([`Index::supertypes`],
/// [`Index::hierarchy_tokens`], [`Index::hierarchy_families`]).
struct Hierarchy {
    supertypes: HashMap<NodeId, Vec<NodeId>>,
    tokens: HashSet<String>,
    families: HashSet<String>,
    trait_users: HashSet<NodeId>,
}

/// Build the [`Hierarchy`] from the `Extends` rows the type-relation arm binds
/// ([`type_relation_want`]): a package-shaped source's, and in every language a
/// type's.
///
/// Each row is bound by [`bind`] itself — S-466's type-relation arm, which
/// reads the source's scope and module model and never the policy-gated
/// fallbacks (`scope_only`), so the policy passed here moves nothing. Only a
/// row that binds as `Extends` is a supertype: a silent-syntax supertype that
/// binds an interface records `Implements` ([`relation_edge_kind`]) and stays
/// out, as a Java class's `implements` does. A row of any other arm is
/// skipped, and the map stays empty for a graph without one — a Rust graph —
/// exactly as before.
///
/// The walk reads one superclass per level, nearest first — the order of
/// single inheritance. Two shapes break it, and neither binds a guess (S-522,
/// [NFR-RA-05]):
///
/// - **several bases** — two or more rows that may each name a base class: a
///   row bound to a class, or an unbound one (an external base). Python's
///   `class C(A, B)` looks a method up in its MRO (`C, A, A's bases, B`), not
///   level by level, and an unbound base ahead of a bound one may supply it.
///   Such a class **has no supertypes here**, so a `self`/`super` call climbs no
///   further than its own members. Where the list leaves kinds unsaid
///   ([`Want::Supertype`], C#, Kotlin) its one base class sits among
///   interfaces, so an unbound entry there is not counted. An interface's
///   super-interfaces are pooled, as before;
/// - **a used trait** (a PHP `use`, [`TRAIT_USE_ALIAS`]) — the trait's method
///   outranks every inherited one, and an unbound trait's methods are not in
///   the graph. Such a class is a [`trait_users`](Index::trait_users) entry,
///   which no walk climbs through.
///
/// Only the subtype's own rows count. A capture-before-delete `Symbol` row
/// ([ADR-10]) is filed under the **supertype's** file and outlives the
/// subtype's `extends` clause until that file is re-extracted, so reading it
/// would lend a type a supertype it no longer declares — and bind new calls
/// through it — where a cold index has none. Its target is also a whole
/// symbol, whose path tokens would re-select every package-shaped call on
/// every Java sync. The source's own `Path` row is always present while the
/// clause is, so skipping the capture loses no supertype.
///
/// [CR-150]: ../../../docs/requests/CR-150-java-receiver-typing-for-method-calls.md
/// [ADR-10]: ../../../docs/specs/architecture/decisions/ADR-10.md
fn build_supertypes(refs: &[UnresolvedRefRow], ix: &Index) -> Hierarchy {
    let mut hierarchy = Hierarchy {
        supertypes: HashMap::new(),
        tokens: HashSet::new(),
        families: HashSet::new(),
        trait_users: HashSet::new(),
    };
    let mut bases: HashMap<NodeId, usize> = HashMap::new();
    for r in refs {
        if r.form == RefForm::Symbol || !matches!(r.kind, EdgeKind::Extends | EdgeKind::Implements) {
            continue;
        }
        let Some(&source_id) = ix.by_symbol.get(&r.source_symbol) else {
            continue;
        };
        let source = ix.info.get(&source_id);
        let Some(want) = relation_want_of(r, source, &ix.layout) else {
            continue;
        };
        if r.kind == EdgeKind::Implements {
            if r.alias.as_deref() == Some(TRAIT_USE_ALIAS) {
                hierarchy.trait_users.insert(source_id);
            }
            continue;
        }
        hierarchy.tokens.extend(super::tokens(&r.target));
        hierarchy.families.extend(
            source
                .and_then(|i| i.file_path.as_deref())
                .and_then(|p| ix.layout.family(p)),
        );
        let class = source.is_some_and(|i| i.kind != NodeKind::Interface);
        match bind(r, ix, BindingPolicy::Strict) {
            Outcome::Bound {
                target,
                kind: EdgeKind::Extends,
                ..
            } => {
                hierarchy.supertypes.entry(source_id).or_default().push(target);
                *bases.entry(source_id).or_default() += usize::from(class);
            }
            Outcome::Unbound if want != Want::Supertype => {
                *bases.entry(source_id).or_default() += usize::from(class);
            }
            _ => {}
        }
    }
    for (id, _) in bases.into_iter().filter(|&(_, n)| n > 1) {
        hierarchy.supertypes.remove(&id);
    }
    for ids in hierarchy.supertypes.values_mut() {
        ids.sort_unstable();
        ids.dedup();
    }
    hierarchy
}

/// The one workspace [`NodeKind::Trait`] node named `name`, or `None` when zero
/// or several carry it — the single source of truth for the never-fabricate
/// trait-resolution rule ([NFR-RA-05]) shared by [`Index::trait_by_name`] (query
/// time) and [`build_impls_by_trait_method`] (index-build time), so the two can
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
/// `Extends` relates like to like, so it reads the source's own kind: an
/// interface's binds an interface, a class's a class — or, where its language
/// leaves the kind unsaid (`kind_follows_target`, C# and Kotlin), a class, an
/// interface or a trait, its edge kind following the target
/// ([`relation_edge_kind`]). `Implements` binds an interface or a trait.
///
/// Every other row gets `None` and keeps the arm it had. That includes Rust's
/// `Implements`, which is sourced at an impl **method**, so the S-281 trait bind
/// and its `dyn` fan-out are untouched.
///
/// [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
/// [FR-RS-14]: ../../../docs/specs/requirements/FR-RS-14.md
/// [FR-RS-15]: ../../../docs/specs/requirements/FR-RS-15.md
fn type_relation_want(
    r: &UnresolvedRefRow,
    source_kind: Option<NodeKind>,
    package_shaped: bool,
    kind_follows_target: bool,
) -> Option<Want> {
    if !matches!(r.form, RefForm::Path | RefForm::Symbol) {
        return None;
    }
    let from_type = source_kind.is_some_and(is_class_like);
    let supertype = from_type && matches!(r.kind, EdgeKind::Extends | EdgeKind::Implements);
    if !package_shaped && !supertype {
        return None;
    }
    match r.kind {
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
    type_relation_want(
        r,
        source.map(|i| i.kind),
        path.is_some_and(|p| layout.is_package_shaped(p)),
        path.is_some_and(|p| layout.supertype_kind_follows_target(p)),
    )
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

/// Why `r` — a `Calls` row from a package-shaped source, or in any language a
/// `Self::m` call through the caller's own type (S-493) or a receiver call
/// bound by its shape (S-514) — stays unbound (S-468, [CR-150] §3.2 C), or
/// `None` when it is no such row, or when it binds
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
    if !ix.layout.is_package_shaped(source_file) {
        // Outside a package-shaped language only a call through the caller's
        // own type (S-493, [`Ctx::resolve_self_type_call`]) and a receiver
        // call (S-514, the `RefForm::Method` arm) record a reason.
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
/// row but a `Self::m` call through the caller's own type, S-493, and a
/// receiver call, S-514). Meaningful only for an
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
    let source_file = source_info.and_then(|i| i.file_path.as_deref());
    let source_package = source_file.and_then(|p| ix.layout.package_of(p));
    let source_family = source_file.and_then(|p| ix.layout.family(p));
    let relation = relation_want_of(r, source_info, &ix.layout);
    let ctx = Ctx {
        source,
        file_id: r.file_id,
        ix,
        source_package,
        source_family,
        policy,
        in_glob_resolution: Cell::new(false),
        bare_path_call: Cell::new(false),
        scope_only: relation.is_some(),
        lexical_start: Cell::new(source),
        miss: RefCell::new(None),
    };
    let outcome = bind_in(&ctx, r, relation);
    (outcome, ctx.miss.into_inner())
}

/// The body of [`bind`], against a context built for `r`.
fn bind_in(ctx: &Ctx<'_>, r: &UnresolvedRefRow, relation: Option<Want>) -> Outcome {
    let (source, ix) = (ctx.source, ctx.ix);
    let bound = |target: NodeId| Outcome::Bound {
        source,
        target,
        kind: r.kind,
        // Non-artifact binds carry no payload; an artifact bind routes through
        // `Ctx::resolve_artifact` below, which stamps the relation class.
        payload: r.payload.clone(),
    };

    // A cross-artifact reference (CR-011, FR-CG-07): bind under the same
    // exactly-one-candidate discipline as code and docs, dispatched by
    // (kind, form). The relation class rides on `r.payload` onto the edge so
    // navigation can surface which relation a binding expresses; externals were
    // classified out before the ledger, so every ledger row here is a genuine
    // workspace-relative candidate ([NFR-RA-05], [ADR-26]).
    if r.kind.is_config_reference() {
        return ctx.resolve_artifact(r);
    }

    // A member-access fact (CR-005, FR-EX-08): bind to a `Field` of the source
    // method's own class-like container, on an exactly-one candidate — never
    // through the scope hierarchy and never policy-widened, so an own-field
    // access can only ever bind to a field of its own type or stay unresolved
    // (NFR-RA-05). The `target` is the bare field name (the `Method` ref form).
    if r.kind == EdgeKind::Accesses {
        return match ctx.resolve_member_access(&r.target) {
            Res::Found(target) => bound(target),
            _ => Outcome::Unbound,
        };
    }

    // A type relation (S-466, CR-149 §3.2 B, FR-EX-10; S-522, FR-RS-15): any
    // relation from a package-shaped source, and in every language a type's
    // `Extends` or `Implements` ([`type_relation_want`]). Bound only to the one
    // in-repository type of a kind the relation admits, reached through the
    // source's scope and its module model — never a workspace guess — see
    // [`Ctx::resolve_type_relation`]. This is where the `Implements` bind is
    // widened beyond Rust `Trait` targets: a type's `Implements` binds an
    // `Interface` or a `Trait`, and never reaches the trait rule below. A
    // supertype whose syntax was silent takes its edge kind from the target.
    if let Some(want) = relation {
        return match ctx.resolve_type_relation(r, want) {
            Res::Found(target) => Outcome::Bound {
                source,
                target,
                kind: relation_edge_kind(r.kind, want, ix.info.get(&target).map(|i| i.kind)),
                payload: r.payload.clone(),
            },
            _ => Outcome::Unbound,
        };
    }

    // A trait-implementation fact (S-281, CR-073, FR-RS-08): an `impl T for X`
    // method points at its trait `T`. Bind the impl method to the one workspace
    // Trait node named by the target's last segment — never on zero or several
    // (an external / ambiguous trait stays an honest miss, NFR-RA-05). This edge
    // is the structural link the `dyn T` fan-out below enumerates impls from; it
    // is a structural fact, not a code coupling, so hydration fences it out of
    // the dependency subgraph the gated metrics run on (mirroring `Accesses`).
    // A type's `Implements`, and a package-shaped source's, took the
    // type-relation arm above: only an impl method's reaches here.
    if r.kind == EdgeKind::Implements {
        let last = r.target.rsplit("::").next().unwrap_or(&r.target);
        return match ix.trait_by_name(last) {
            Some(target) => bound(target),
            None => Outcome::Unbound,
        };
    }

    match r.form {
        // Capture-before-delete refs carry an exact canonical symbol: pure
        // lookup, no inference (ADR-10).
        RefForm::Symbol => match ix.by_symbol.get(&r.target) {
            Some(&target) => bound(target),
            None => Outcome::Unbound,
        },
        RefForm::Glob => {
            let segs = split(&r.target);
            // A package-shaped wildcard (CR-149) names a type or a package. The
            // row binds to the one type it names (`import static a.b.C.*` → `C`);
            // a package has no node, so `import a.b.*` binds nothing — its
            // members still come into scope through the file's globs
            // ([`Ctx::glob_members`]), which is what the row is for.
            //
            // A declared-namespace file's wildcard naming no type names a
            // namespace (S-518): it binds to every other file declaring it
            // ([`Ctx::namespace_files`]), as a Go import path binds to its
            // directory's files. Java's package wildcard binds nothing still.
            if ctx.source_package.is_some() {
                return match ctx.resolve_fqn(&segs, Want::Any) {
                    Res::Found(t) if ix.info.get(&t).is_some_and(|i| is_type_like(i.kind)) => {
                        bound(t)
                    }
                    Res::NotFound => ctx.namespace_files(&segs).unwrap_or(Outcome::Unbound),
                    _ => Outcome::Unbound,
                };
            }
            match ctx.resolve_path(&segs, Want::Module, MAX_ALIAS_DEPTH) {
                Res::Found(target) => bound(target),
                _ => Outcome::Unbound,
            }
        }
        RefForm::Path => {
            // A documentation link/path (S-035): resolve the href (path +
            // optional `#anchor`) against the doc/code file at that path, never
            // through the code scope hierarchy ([FR-DG-03], [FR-DG-04]).
            if r.kind == EdgeKind::DocReference {
                return match ctx.resolve_doc_link(&r.target) {
                    Res::Found(target) => ctx.bind_doc_ref(source, target),
                    _ => Outcome::Unbound,
                };
            }
            // A path-grammar module specifier (S-439, [CR-142] D1): a relative
            // specifier binds against the importing file's directory, a Go import
            // path against the module that declares it — never through the
            // member-path scope hierarchy, which reads `a::b` as names.
            if r.kind == EdgeKind::Imports {
                if let Some(outcome) = ctx.resolve_specifier(&r.target) {
                    return outcome;
                }
            }
            // A call through the caller's own type (S-493, [FR-RS-11]):
            // `self.m()` / `Self::m()` inside a method with a recorded self
            // type binds only among that type's methods in the caller's crate,
            // never through the scope hierarchy.
            if r.kind == EdgeKind::Calls {
                if let Some(resolved) = ctx.resolve_self_type_call(&r.target) {
                    return match resolved {
                        Res::Found(target) => bound(target),
                        _ => Outcome::Unbound,
                    };
                }
            }
            // A call through an import of a path-grammar file (S-440, [CR-142]
            // D2): bound within what that import binds to, never through the
            // member-path hierarchy, whose workspace fallback would read a
            // specifier path as names.
            if r.kind == EdgeKind::Calls {
                if let Some(resolved) = ctx.resolve_imported_call(&r.target) {
                    return match resolved {
                        Res::Found(target) => bound(target),
                        _ => Outcome::Unbound,
                    };
                }
            }
            let want = if r.kind == EdgeKind::Calls {
                ctx.call_want()
            } else {
                Want::Any
            };
            let segs = split(&r.target);
            // A *single-segment* bare-path call enables the CR-068 Part B tie-break
            // (free function over same-named associated methods, [FR-RS-07]). Gated
            // on `segs.len() == 1` so the flag is provably inert for a
            // path-qualified call (`Type::f`, routed through `descend`) and for an
            // import (`Want::Any`); scoped to this resolution. A receiver-method
            // call never reaches it: it binds by its receiver's shape (S-514).
            ctx.bare_path_call
                .set(r.kind == EdgeKind::Calls && segs.len() == 1);
            let resolved = ctx.resolve_path(&segs, want, MAX_ALIAS_DEPTH);
            ctx.bare_path_call.set(false);
            match resolved {
                // A class a declared call constructs (S-521, [FR-RS-16]) —
                // unless a factory function of its name rivals it.
                Res::Found(target) if ctx.constructs(want, target) => {
                    if ctx.rival_function(target) {
                        ctx.note(want, || Residue::TypeAmbiguous);
                        return Outcome::Unbound;
                    }
                    Outcome::Bound {
                        source,
                        target,
                        kind: EdgeKind::Instantiates,
                        payload: r.payload.clone(),
                    }
                }
                Res::Found(target) => bound(target),
                Res::NotFound if r.kind == EdgeKind::Imports => ctx
                    .package_reexport(&segs)
                    .map_or(Outcome::Unbound, bound),
                _ => Outcome::Unbound,
            }
        }
        RefForm::Method => {
            // A documentation code-name token (S-035): bind to the one code
            // symbol of that name workspace-wide — never on zero or several
            // ([FR-DG-04], [NFR-RA-05]). Policy-independent: doc→code is always
            // exactly-one-or-nothing.
            if r.kind == EdgeKind::DocReference {
                return match ctx.resolve_doc_code_name(&r.target) {
                    Res::Found(target) => ctx.bind_doc_ref(source, target),
                    _ => Outcome::Unbound,
                };
            }
            // A trait-object dynamic-dispatch call (S-281, CR-073, FR-RS-08):
            // extraction encodes a *provable* `&dyn T` receiver on `p.f()` as a
            // trait-qualified `T::f` target (a bare `.f()` stays a single segment,
            // never reaches here, and binds by its receiver's shape below,
            // [FR-RS-12]). Fan out to the SET of that trait method's
            // targets: the trait's own default body (a member of the trait node)
            // ∪ every concrete workspace impl of it. Every target is a real
            // indexed node reached through the *proven* trait `T` — never a
            // same-named method on an unrelated type, never a free function, never
            // a stdlib/external target ([NFR-RA-05]). A trait that is external or
            // ambiguous, or one with neither a default body nor a workspace impl,
            // yields no target and stays an honest miss.
            if r.kind == EdgeKind::Calls && r.target.contains("::") {
                return ctx.resolve_dyn_dispatch(source, &r.target);
            }
            // A receiver call (`x.f()` → `f`): extraction records the bare
            // method name and its receiver's SHAPE, never its type ([FR-EX-13];
            // a typed receiver is a `RefForm::Path` row bound above). The shape
            // decides where `f` may be found ([FR-RS-12]):
            //
            // - `self` — among the caller's own class's members, then up its
            //   proven `Extends` chain; for a caller with a recorded self type,
            //   through that type ([`Ctx::resolve_self_receiver`]);
            // - `super` — only up that chain, never the caller's own class
            //   ([`Ctx::resolve_super_receiver`]);
            // - `other`, or no shape at all — nowhere. The caller's lexical,
            //   class or module scope says where the CALLER is, not what the
            //   receiver is: the scope walk this replaces started at the caller
            //   itself, so `other.m()` inside `m` bound to `m` ([CR-169],
            //   correcting [CR-066]'s "same-scope evidence"). The row stays in
            //   `unresolved_refs` as `no-receiver-evidence` and retries on sync
            //   ([FR-RS-03], [FR-RS-06], [NFR-RA-05]).
            //
            // [FR-EX-13]: ../../../docs/specs/requirements/FR-EX-13.md
            // [FR-RS-06]: ../../../docs/specs/requirements/FR-RS-06.md
            // [FR-RS-12]: ../../../docs/specs/requirements/FR-RS-12.md
            // [CR-066]: ../../../docs/requests/CR-066-receiver-method-overbinding.md
            // [CR-169]: ../../../docs/requests/CR-169-a-call-on-another-object-never-binds-to-the-callers-own-method.md
            let resolved = match r.receiver {
                Some(ReceiverShape::SelfInstance) => ctx.resolve_self_receiver(&r.target),
                Some(ReceiverShape::Super) => ctx.resolve_super_receiver(&r.target),
                Some(ReceiverShape::Other) | None => {
                    ctx.note(Want::Callable, || Residue::NoReceiverEvidence);
                    Res::NotFound
                }
            };
            match resolved {
                Res::Found(target) => bound(target),
                _ => Outcome::Unbound,
            }
        }
    }
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

/// Fold a link `path_part`'s `.`/`..`/leading-`/` against `base_dir` (the
/// directory segments to resolve relative to) into a normalised
/// project-relative path, or `None` if it escapes the repository root.
///
/// Pure and deterministic — the link and its target agree because both sides
/// share this normalisation and [`heading_slug`].
fn fold_path(base_dir: &[&str], path_part: &str) -> Option<String> {
    // A leading `/` is repo-root-relative; otherwise resolve against `base_dir`.
    let mut segs: Vec<&str> = if path_part.starts_with('/') {
        Vec::new()
    } else {
        base_dir.to_vec()
    };
    for seg in path_part.split('/').filter(|s| !s.is_empty()) {
        match seg {
            "." => {}
            ".." => {
                // An escape above the repo root is not a resolvable target.
                segs.pop()?;
            }
            other => segs.push(other),
        }
    }
    Some(segs.join("/"))
}

/// The lower-cased extension of `path` (`""` when it has none) — the key the
/// path-specifier maps are built on ([`crate::plugin::LanguageRegistry::specifier_target_extensions`]).
fn extension_of(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

/// The directory segments of `file` (the path with its file name dropped).
fn dir_segments(file: &str) -> Vec<&str> {
    let mut dir: Vec<&str> = file.split('/').collect();
    dir.pop(); // drop the file name, keep the directory
    dir.into_iter().filter(|s| !s.is_empty()).collect()
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
    policy: BindingPolicy,
    /// Re-entrancy guard for [`through_globs`](Ctx::through_globs): set while a
    /// glob's own module path is being resolved, so that resolution cannot fan
    /// back out through the file's glob set. Without it, a file with `G` glob
    /// imports drives `O(G^MAX_ALIAS_DEPTH)` work per reference (CR-016).
    in_glob_resolution: Cell<bool>,
    /// Enable the CR-068 Part B free-function/associated-method tie-break in
    /// [`prefer_free_functions`](Ctx::prefer_free_functions) for the duration of
    /// one resolution. Set **only** while resolving a single-segment bare-**path**
    /// call ([`RefForm::Path`] + [`EdgeKind::Calls`]) — precisely the shape that
    /// must bind the one free function over same-named associated methods
    /// ([FR-RS-07]). A receiver **method** call ([`RefForm::Method`]) never
    /// reaches [`resolve_name`](Ctx::resolve_name) at all (S-514): it binds by
    /// its receiver's shape.
    ///
    /// [FR-RS-07]: ../../../docs/specs/requirements/FR-RS-07.md
    bare_path_call: Cell<bool>,
    /// Bind by scope and module model only: set for a type relation (S-466,
    /// [CR-149]; S-522 for every language), whose target must be the one
    /// in-repository type its source's lexical scope, imports, package,
    /// namespace, modules or wildcards name. The policy-gated workspace
    /// fallbacks are off for it — the aggressive policy's **name** match and
    /// the balanced one's module-path **suffix** match — because a same-named
    /// type in a module the file never imports is not the type it wrote
    /// ([NFR-RA-05]): an import of a JDK `List` must not bind an in-house one,
    /// nor a Django `models.Model` base a repository's own `Model`. The
    /// hierarchy is built under the strict policy ([`build_supertypes`]), so the
    /// edge and the supertype walk agree under every policy.
    ///
    /// [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    scope_only: bool,
    /// Where [`resolve_name`](Ctx::resolve_name)'s lexical chain — and a
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
}

impl Ctx<'_> {
    fn scope(&self) -> Option<&FileScope> {
        self.file_id.and_then(|id| self.ix.file_scopes.get(&id))
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

    /// `true` when a top-level `Function` of `class`'s own name sits in the
    /// class's package, within its family (S-521): Kotlin's factory function
    /// `fun Foo(s: String): Foo` beside `class Foo`. The package rungs read
    /// types only, so the function is no candidate there and the class alone
    /// would answer — but the call names both, and stays unbound ([NFR-RA-05]).
    /// A class with no package (a path-model file) is never rivalled here: its
    /// language's rungs already see functions and classes together.
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn rival_function(&self, class: NodeId) -> bool {
        let Some(info) = self.ix.info.get(&class) else {
            return false;
        };
        let Some(path) = info.file_path.as_deref() else {
            return false;
        };
        let Some(package) = self.ix.layout.package_of(path) else {
            return false;
        };
        let family = self.ix.layout.family(path);
        let top_level = |id: &NodeId| {
            self.ix
                .parent
                .get(id)
                .and_then(|p| self.ix.info.get(p))
                .is_some_and(|p| p.kind == NodeKind::Module)
        };
        self.ix.by_name.get(&info.name).into_iter().flatten().any(|id| {
            self.ix.info.get(id).is_some_and(|f| {
                f.kind == NodeKind::Function
                    && top_level(id)
                    && f.file_path.as_deref().is_some_and(|fp| {
                        self.ix.layout.package_of(fp).as_ref() == Some(&package)
                            && self.ix.layout.family(fp) == family
                    })
            })
        })
    }

    /// Every path an import-root file's imports give `head`, when they give it
    /// more than one (S-519): `try: from .fast import parse` / `except
    /// ImportError: from .slow import parse` names two declarations, and the
    /// first-wins alias map would pick one ([NFR-RA-05]). `None` for any other
    /// source, or a name imported once — the alias map answers those.
    fn rival_expansions(&self, head: &str) -> Option<&[Vec<String>]> {
        let source_file = self.ix.info.get(&self.source)?.file_path.as_deref()?;
        if !self.ix.layout.has_import_roots(source_file) {
            return None;
        }
        let all = self.scope()?.alias_expansions.get(head)?;
        (all.len() > 1).then_some(all.as_slice())
    }

    /// `rest` resolved under each of `expansions`, exactly-one across all of
    /// them (two imports of one declaration agree) — sticky on an ambiguity.
    fn resolve_expansions(&self, expansions: &[Vec<String>], rest: &[String], want: Want, depth: u8) -> Res {
        let mut found: Vec<NodeId> = Vec::new();
        for expansion in expansions {
            let mut path = expansion.clone();
            path.extend(rest.iter().cloned());
            match self.resolve_path(&path, want, depth - 1) {
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
    fn in_family_crate(&self) -> bool {
        self.ix
            .nearest_module(self.source)
            .is_some_and(|(krate, _)| self.ix.layout.is_family_crate(krate))
    }

    /// Resolve a multi-or-single segment path by the scope hierarchy.
    fn resolve_path(&self, segs: &[String], want: Want, depth: u8) -> Res {
        if segs.is_empty() || depth == 0 {
            return Res::NotFound;
        }
        // 0) A relative import of an import-root language (S-519, [FR-RS-14]):
        //    `.`/`..` heads are its level, read from the source's package.
        if let Some(resolved) = self.resolve_relative(segs, want) {
            return resolved;
        }
        if segs.len() == 1 {
            return self.resolve_name(&segs[0], want, depth);
        }

        let head = segs[0].as_str();
        let rest = &segs[1..];
        let source_mod = self.source_module();

        // 1) `crate::…` — the source's crate root.
        if head == "crate" {
            if let Some((krate, _)) = &source_mod {
                return self.descend(krate, &[], rest, want);
            }
            return Res::NotFound;
        }
        // 2) `self::…` — the source's module.
        if head == "self" {
            if let Some((krate, mods)) = &source_mod {
                return self.descend(krate, mods, rest, want);
            }
            return Res::NotFound;
        }
        // 3) `super::…` (possibly chained) — ancestors of the source module.
        if head == "super" {
            let Some((krate, mods)) = &source_mod else {
                return Res::NotFound;
            };
            let supers = segs.iter().take_while(|s| s.as_str() == "super").count();
            if supers > mods.len() {
                return Res::NotFound; // more supers than module depth
            }
            let base = &mods[..mods.len() - supers];
            return self.descend(krate, base, &segs[supers..], want);
        }
        // 4) A package-shaped source (CR-149): the rest of the path is decided
        //    by the package rungs alone, in the language's own order — its
        //    lexical, imported, same-package and on-demand heads, then the
        //    fully-qualified name. The module tree below names no package
        //    directory, and the workspace suffix match would read a
        //    fully-qualified name as a guess.
        if let Some(package) = &self.source_package {
            return self.resolve_package_path(package, segs, want, depth);
        }
        // 4b) A `use`-alias head: substitute and resolve the expansion
        //     (depth-limited — an import cycle terminates as NotFound). An
        //     import-root file importing the head twice reads every import.
        if let Some(rivals) = self.rival_expansions(head) {
            match self.resolve_expansions(rivals, rest, want, depth) {
                Res::NotFound => {}
                decided => return decided,
            }
        } else if let Some(alias_path) = self.scope().and_then(|s| s.aliases.get(head)) {
            let mut expanded = alias_path.clone();
            expanded.extend(rest.iter().cloned());
            match self.resolve_path(&expanded, want, depth - 1) {
                Res::NotFound => {} // fall through to wider scopes
                decided => return decided,
            }
        }
        // 5) A crate-name head (`logos_core::…`) — never from an import-root
        //    family's crate, which names no other crate (S-519).
        let norm = normalize_crate(head);
        if self.ix.crates.contains(&norm) && !self.in_family_crate() {
            match self.descend(&norm, &[], rest, want) {
                Res::NotFound => {} // a same-named module may still match below
                decided => return decided,
            }
        }
        // 6) Relative to the source module (`child_mod::item`), then to the
        //    crate root.
        if let Some((krate, mods)) = &source_mod {
            match self.descend(krate, mods, segs, want) {
                Res::NotFound => {}
                decided => return decided,
            }
            match self.descend(krate, &[], segs, want) {
                Res::NotFound => {}
                decided => return decided,
            }
        }
        // 7) Through the file's glob imports: candidates across every glob
        //    module, exactly-one overall.
        match self.through_globs(segs, want, depth) {
            Res::NotFound => {}
            decided => return decided,
        }
        // 8) Policy-gated workspace fallback: unique module-path-suffix match —
        //    never for a type relation ([`scope_only`](Ctx::scope_only)). A
        //    receiver-method call never reaches here (S-514, CR-066).
        if self.policy != BindingPolicy::Strict && !self.scope_only {
            return self.suffix_match(segs, want);
        }
        Res::NotFound
    }

    /// A path headed by a relative level (`.`, `..`; [`is_relative_head`]) from
    /// a file keyed under import roots (S-519, [FR-RS-14]) — `from .rules
    /// import Rule` recorded `.::rules::Rule`, `from .._internal import x`
    /// recorded `..::_internal::x`. `.` is the source's package
    /// ([`PackageLayout::package_dir`]) and each `..` one package up; the rest
    /// descends from there, and nothing follows it (`from . import *`) names
    /// the package itself. A level must leave at least one package: climbing to
    /// the import root itself (`from .. import x` in a top-level package, `from
    /// . import x` in a top-level script) names nothing, as the language
    /// refuses it ("attempted relative import beyond top-level package").
    ///
    /// `None` for any other path or source — a relative specifier of a
    /// path-grammar language is bound by [`resolve_specifier`](Ctx::resolve_specifier),
    /// and no other language records a relative head.
    ///
    /// [FR-RS-14]: ../../../docs/specs/requirements/FR-RS-14.md
    fn resolve_relative(&self, segs: &[String], want: Want) -> Option<Res> {
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
            let module = self.ix.modules.get(&(krate, base.to_vec())).filter(|id| admitted(id));
            return Some(module.map_or(Res::NotFound, |&id| Res::Found(id)));
        }
        Some(self.descend(&krate, base, rest, want))
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
    /// [FR-RS-14]: ../../../docs/specs/requirements/FR-RS-14.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn package_reexport(&self, segs: &[String]) -> Option<NodeId> {
        let source_file = self.ix.info.get(&self.source)?.file_path.as_deref()?;
        if !self.ix.layout.has_import_roots(source_file) {
            return None;
        }
        let (_, package) = segs.split_last()?;
        if package.is_empty() {
            return None;
        }
        let Res::Found(module) = self.resolve_path(package, Want::Module, MAX_ALIAS_DEPTH) else {
            return None;
        };
        let file = self.ix.info.get(&module)?.file_path.as_deref()?;
        let own = self.ix.nearest_module(self.source) == self.ix.module_key.get(&module);
        (self.ix.layout.is_package_file(file) && !own).then_some(module)
    }

    /// Resolve a type relation (S-466, [CR-149] §3.2 B, [FR-EX-10]; S-522,
    /// [FR-RS-15]) to the one type of a kind `want` admits.
    ///
    /// A `Path` row takes the source's own scope order — the lexical chain (a
    /// nested or same-file type), then its module model's rungs. A
    /// package-shaped source takes S-465's package rungs: single-type imports
    /// (final for the name they import), the source's package or namespace, its
    /// wildcards, and for a qualified name the fully-qualified index
    /// ([`resolve_package_path`](Ctx::resolve_package_path)). Any other source
    /// — a Python class, a namespace file whose namespace is unknown — takes the
    /// module tree: its imports, its modules, its globs. Neither ever takes a
    /// policy-gated workspace guess ([`scope_only`](Ctx::scope_only)). A target
    /// headed [`FULLY_QUALIFIED_HEAD`] is read by the fully-qualified index
    /// alone, and a header never names its own declaration
    /// ([`is_own_header`](Ctx::is_own_header)). `want` filters the candidates
    /// of every rung, so a same-named type of the wrong kind is
    /// no candidate at all (on the fully-qualified rung it is applied after
    /// the exactly-one test, so a `src/main`/`src/test` pair stays ambiguous).
    /// A type with no source here — the JDK, a library, a generated class —
    /// resolves to nothing and stays in `unresolved_refs` ([NFR-RA-05]).
    ///
    /// **Header scope.** An `Extends` or `Implements` names its type in the
    /// declaration's header, where the declaration's own member types are not
    /// in scope (JLS §6.3): it is read from the enclosing scope
    /// ([`lexical_start`](Ctx::lexical_start)). A `TypeUses` sourced at a type
    /// may come from either place — a superclass's type argument (header) or a
    /// constructor parameter (body; constructors are not nodes) — and one row
    /// cannot say which, so it binds only where both readings agree, or where
    /// the header reading finds nothing (a header name that resolves only to a
    /// member type does not compile). Two different types is a refusal, never
    /// a pick ([NFR-RA-05]).
    ///
    /// A capture-before-delete `Symbol` row ([ADR-10]) is still a pure lookup,
    /// but of a target whose kind the relation also admits: an edit that turns
    /// the superclass into an interface leaves the relation unbound on sync,
    /// exactly as a cold index would ([NFR-RA-06]).
    ///
    /// [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    /// [FR-EX-10]: ../../../docs/specs/requirements/FR-EX-10.md
    /// [FR-RS-15]: ../../../docs/specs/requirements/FR-RS-15.md
    /// [ADR-10]: ../../../docs/specs/architecture/decisions/ADR-10.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    fn resolve_type_relation(&self, r: &UnresolvedRefRow, want: Want) -> Res {
        if r.form == RefForm::Symbol {
            return match self.ix.by_symbol.get(&r.target) {
                Some(&t) if self.ix.info.get(&t).is_some_and(|i| want.admits(i.kind)) => {
                    Res::Found(t)
                }
                _ => Res::NotFound,
            };
        }
        let segs = split(&r.target);
        if segs.first().map(String::as_str) == Some(FULLY_QUALIFIED_HEAD) {
            return self.resolve_fqn(&segs[1..], want);
        }
        let header = || -> Res {
            let Some(&enclosing) = self.ix.parent.get(&self.source) else {
                return Res::NotFound;
            };
            self.lexical_start.set(enclosing);
            let resolved = self.resolve_path(&segs, want, MAX_ALIAS_DEPTH);
            self.lexical_start.set(self.source);
            resolved
        };
        match r.kind {
            EdgeKind::Extends | EdgeKind::Implements => header(),
            EdgeKind::TypeUses
                if self.ix.info.get(&self.source).is_some_and(|i| is_type_like(i.kind)) =>
            {
                match (self.resolve_path(&segs, want, MAX_ALIAS_DEPTH), header()) {
                    (Res::Found(body), Res::Found(head)) if body != head => Res::Ambiguous,
                    (body, _) => body,
                }
            }
            _ => self.resolve_path(&segs, want, MAX_ALIAS_DEPTH),
        }
    }

    /// Resolve `Self::m` — a `self.m()` or `Self::m()` call (S-493, [FR-RS-11])
    /// — among the callables recorded with the **caller's** self type in the
    /// caller's crate ([`Index::methods_by_self_type`]), or `None` when this is
    /// no such call: a target of any other shape, or a caller with no recorded
    /// self type (a trait's default method, a free function), whose row takes
    /// the path it always took.
    ///
    /// The one acceptance rule ([`exactly_one`], [NFR-RA-05]), on two rungs.
    ///
    /// 1. **The crate** — when the crate declares exactly one type of the self
    ///    type's base name, every candidate is a method of the caller's type,
    ///    wherever in the crate its impl block is: exactly one binds.
    /// 2. **The caller's module** — where the impl header's name denotes the
    ///    caller's own type. Taken when the crate declares that name for several
    ///    types — a lone candidate elsewhere may then be another type's method,
    ///    so only the module decides — and when rung 1 found several: one type's
    ///    inherent `m` beside a trait impl's `m` (`Engine::start`), or two
    ///    impl blocks both defining it. Exactly one module-local candidate
    ///    binds, unless it is a trait-impl method while an inherent candidate
    ///    lies outside the module: Rust resolves `Self::m` to the inherent one
    ///    wherever it is written, so the module cannot decide that pair.
    ///
    /// A self type the crate declares no type of (a library type, a generic
    /// parameter), or one the caller's file imports from outside the crate,
    /// binds nothing: [`Residue::ExternalType`]. Its inherent methods are not
    /// in the graph and Rust prefers them to any trait method recorded here, so
    /// no candidate is evidence — and a same-named crate type is another type.
    /// Its query records no self type at all for a header path-qualified
    /// outside the crate (`impl Ext for std::io::Error`).
    ///
    /// Zero candidates — the method comes from a trait's default body, an
    /// external trait or a `Deref` target, none of which records this self type
    /// — is [`Residue::SupertypeUnreached`]. Candidates the module does not
    /// narrow to one are [`Residue::OverloadAmbiguous`] under a type name the
    /// crate declares once, [`Residue::TypeAmbiguous`] otherwise. Either stays
    /// unbound and never falls through to a wider scope: the receiver's type is
    /// known, and a same-named callable elsewhere is not its method.
    ///
    /// [FR-RS-11]: ../../../docs/specs/requirements/FR-RS-11.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_self_type_call(&self, target: &str) -> Option<Res> {
        let (head, name) = target.split_once("::")?;
        if head != SELF_TYPE_HEAD || name.is_empty() || name.contains("::") {
            return None;
        }
        let self_type = self.ix.self_types.get(&self.source)?;
        let krate = &self.ix.info.get(&self.source)?.crate_name;
        let key = (krate.clone(), self_type.clone(), name.to_string());
        let candidates = self
            .ix
            .methods_by_self_type
            .get(&key)
            .map_or(&[][..], Vec::as_slice);
        let declared = self.ix.type_names.get(&(krate.clone(), self_type.clone()));
        let caller = self.ix.info.get(&self.source)?;
        if declared.is_none() || self.ix.imports_foreign(caller, self_type) {
            // A type the crate does not declare — a library type, a generic
            // parameter — has inherent methods the graph cannot see, which
            // Rust prefers to any trait method recorded here.
            self.note(Want::Callable, || Residue::ExternalType {
                candidates: Vec::new(),
            });
            return Some(Res::NotFound);
        }
        let one_type = declared == Some(&1);
        let res = match exactly_one(candidates) {
            Res::NotFound => {
                self.note(Want::Callable, || Residue::SupertypeUnreached);
                Res::NotFound
            }
            found @ Res::Found(_) if one_type => found,
            _ => match self.module_candidate(candidates) {
                Some(found) => Res::Found(found),
                None => {
                    self.note(Want::Callable, || {
                        if one_type {
                            Residue::OverloadAmbiguous
                        } else {
                            Residue::TypeAmbiguous
                        }
                    });
                    Res::Ambiguous
                }
            },
        };
        Some(res)
    }

    /// Rung 2 of [`resolve_self_type_call`](Ctx::resolve_self_type_call): the
    /// one of `candidates` in the caller's own module, unless it is a trait-impl
    /// method while an inherent candidate lies outside the module.
    fn module_candidate(&self, candidates: &[NodeId]) -> Option<NodeId> {
        let own = self.ix.nearest_module(self.source)?;
        let local: Vec<NodeId> = candidates
            .iter()
            .copied()
            .filter(|&c| self.ix.nearest_module(c) == Some(own))
            .collect();
        let Res::Found(one) = exactly_one(&local) else {
            return None;
        };
        let inherent_elsewhere = candidates
            .iter()
            .any(|c| !local.contains(c) && !self.ix.trait_impl_methods.contains(c));
        (!(self.ix.trait_impl_methods.contains(&one) && inherent_elsewhere)).then_some(one)
    }

    /// Resolve a member-access fact to the one `Field` of the source method's
    /// own class-like container named `field` (CR-005, [FR-EX-08]).
    ///
    /// The container is the caller's own class ([`caller_class`](Ctx::caller_class));
    /// the field binds iff that container directly contains **exactly one**
    /// `Field` of that name — the same single acceptance rule every binder level
    /// shares ([NFR-RA-05]). A language whose methods are not lexically nested
    /// under their type (so no class-like ancestor is found) or whose fields are
    /// not extracted as nodes yields no candidate, so the access stays honestly
    /// unresolved and retries on sync — never fabricated.
    ///
    /// [FR-EX-08]: ../../../docs/specs/requirements/FR-EX-08.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_member_access(&self, field: &str) -> Res {
        match self.caller_class() {
            Some(class) => exactly_one(&self.ix.members_named(class, field, Want::Field)),
            None => Res::NotFound,
        }
    }

    /// The caller's own class: the nearest class-bearing container
    /// ([`is_class_like`]) on the source's `Contains` ancestry, the source
    /// itself included — `None` when there is none (a free function, a method
    /// of a language whose methods sit at module level). Bounded by
    /// [`MAX_CONTAINS_DEPTH`] against a malformed `Contains` cycle.
    fn caller_class(&self) -> Option<NodeId> {
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
    /// ([`type_member`](Ctx::type_member), the [FR-RS-10] walk). A free function
    /// or a module-level sibling is never a candidate: only the class's own
    /// `Contains` children are.
    ///
    /// A caller with a recorded self type (Rust, Go: a method at module level)
    /// binds through it instead — the one self-type arm,
    /// [`resolve_self_type_call`](Ctx::resolve_self_type_call), never a second.
    /// Extraction records such a call as `Self::name` already, so this reaches
    /// it only for a row extracted without that rewrite.
    ///
    /// A caller in no class records [`Residue::NoReceiverEvidence`]: the
    /// receiver is its own instance, but nothing says of what.
    ///
    /// [FR-RS-10]: ../../../docs/specs/requirements/FR-RS-10.md
    /// [FR-RS-12]: ../../../docs/specs/requirements/FR-RS-12.md
    fn resolve_self_receiver(&self, name: &str) -> Res {
        if self.ix.self_types.contains_key(&self.source) {
            if let Some(res) = self.resolve_self_type_call(&format!("{SELF_TYPE_HEAD}::{name}")) {
                return res;
            }
        }
        match self.caller_class() {
            Some(class) => self.type_member(class, name),
            None => {
                self.note(Want::Callable, || Residue::NoReceiverEvidence);
                Res::NotFound
            }
        }
    }

    /// Bind a receiver call `super.name()` — shape `super` (S-514,
    /// [FR-RS-12]): only through a proven `Extends` of the caller's own class,
    /// to exactly one callable at the nearest supertype level holding one. The
    /// caller's own class is never a level, so it never binds the caller's own
    /// method. No proven `Extends` — an external base, or a language whose
    /// plugin records none — is [`Residue::SupertypeUnreached`].
    ///
    /// [FR-RS-12]: ../../../docs/specs/requirements/FR-RS-12.md
    fn resolve_super_receiver(&self, name: &str) -> Res {
        let Some(class) = self.caller_class() else {
            self.note(Want::Callable, || Residue::NoReceiverEvidence);
            return Res::NotFound;
        };
        // A class whose own `Extends` binds to itself (`interface I extends I`,
        // which parses) is never its own base level.
        let bases: Vec<NodeId> = self
            .ix
            .supertypes_of(class)
            .iter()
            .copied()
            .filter(|&base| base != class)
            .collect();
        self.supertype_member(bases, HashSet::from([class]), name)
    }

    /// Fan out a trait-object dynamic-dispatch call `T::f` to the SET of that
    /// trait method's targets (S-281, [CR-073], [FR-RS-08]).
    ///
    /// `target` is the trait-qualified form extraction emits for a *provable*
    /// `&dyn T` receiver — the method is the last `::` segment, the trait its
    /// predecessor. The fan-out set is the trait's own default body (a
    /// [`Want::Callable`] member of the resolved [`NodeKind::Trait`] node) ∪ every
    /// concrete workspace impl of that method ([`Index::impls_of`]). Union
    /// reachability: any impl — or the default, for an impl that does not override
    /// — is a legitimate runtime dispatch target ([FR-AN-01]).
    ///
    /// Never fabricate ([NFR-RA-05]): every target is a real indexed node reached
    /// through the **proven** trait `T`, so no same-named method on an unrelated
    /// type, free function, or external target can enter the set. A trait that is
    /// external or ambiguously named, or one with neither a default body nor a
    /// workspace impl of `f`, yields an empty set and stays an honest miss. The
    /// set is id-sorted and deduped for a deterministic edge order ([NFR-RA-06]).
    ///
    /// [CR-073]: ../../../docs/requests/CR-073-trait-object-dynamic-dispatch-reachability.md
    /// [FR-RS-08]: ../../../docs/specs/requirements/FR-RS-08.md
    /// [FR-AN-01]: ../../../docs/specs/requirements/FR-AN-01.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    fn resolve_dyn_dispatch(&self, source: NodeId, target: &str) -> Outcome {
        // `T::f` — the method is the last `::` segment, the trait its predecessor.
        // A malformed target (`::f`, `T::`, a single segment) cannot reach here
        // (`target.contains("::")` gated the call) but a missing predecessor is
        // handled defensively as an honest miss.
        let mut segs = target.rsplit("::");
        let (Some(method), Some(trait_name)) = (segs.next(), segs.next()) else {
            return Outcome::Unbound;
        };
        let Some(trait_node) = self.ix.trait_by_name(trait_name) else {
            return Outcome::Unbound; // external / ambiguous trait — honest miss
        };
        let mut targets = self.ix.members_named(trait_node, method, Want::Callable);
        targets.extend_from_slice(self.ix.impls_of(trait_node, method));
        targets.sort_unstable();
        targets.dedup();
        if targets.is_empty() {
            return Outcome::Unbound;
        }
        Outcome::BoundMany {
            source,
            targets,
            kind: EdgeKind::Calls,
            payload: None,
        }
    }

    /// Bind one cross-artifact reference under never-fabricate (CR-011,
    /// [ADR-26], [FR-CG-07]).
    ///
    /// The third and fourth matcher clients after code and docs, dispatched by
    /// `(kind, form)` to a substrate resolution primitive:
    ///
    /// - **`ArtifactRef` + `Path`** — a workspace-relative artifact path (a proto
    ///   import, a shell `source`, a Terraform local module source) binds to the
    ///   one [`NodeKind::ConfigFile`] at that path
    ///   ([`resolve_artifact_path`](Ctx::resolve_artifact_path)).
    /// - **`ArtifactRef` + `Method`** — a literal artifact name (a proto type
    ///   reference, a GraphQL type reference) binds to the one artifact node of
    ///   that name ([`resolve_artifact_name`](Ctx::resolve_artifact_name)).
    /// - **`ArtifactBinding` + `Method`** — a schema-declared type name binds to
    ///   the one type-like **code** symbol of that name, with no synthesized
    ///   candidates ([`resolve_code_type_name`](Ctx::resolve_code_type_name)).
    ///
    /// Every primitive ends in [`exactly_one`], so an ambiguous or unindexed
    /// reference stays unbound and retries on sync ([NFR-RA-05]). The consumer
    /// stories extend this dispatch with their richer matchers (e.g. the OpenAPI
    /// positional-template `Route` match) by adding an arm here. On a bind, the
    /// relation class (`r.payload`) is stamped onto the edge so navigation can
    /// surface which relation it expresses ([FR-CG-11]).
    ///
    /// [ADR-26]: ../../../docs/specs/architecture/decisions/ADR-26.md
    /// [FR-CG-07]: ../../../docs/specs/requirements/FR-CG-07.md
    /// [FR-CG-11]: ../../../docs/specs/requirements/FR-CG-11.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_artifact(&self, r: &UnresolvedRefRow) -> Outcome {
        // The relation class travels on the ledger row's payload; recover it so a
        // name match can be fenced to the relation's own artifact kind.
        let relation = r.payload.as_deref().and_then(ArtifactRelation::from_wire);
        // A Terraform local module call is the one multi-target relation: its
        // `source` names a directory, so it fans out to every admitted `.tf`
        // `ConfigFile` in that directory (CR-011, FR-CG-08). It returns directly
        // rather than through the single-target `exactly_one` gate below.
        if r.kind == EdgeKind::ArtifactRef
            && r.form == RefForm::Path
            && relation == Some(ArtifactRelation::TfModuleCall)
        {
            return self.resolve_module_dir(&r.target, r.payload.clone());
        }
        let found = match (r.kind, r.form) {
            (EdgeKind::ArtifactRef, RefForm::Path) => self.resolve_artifact_path(&r.target),
            (EdgeKind::ArtifactRef, RefForm::Method) => self
                .resolve_artifact_name(&r.target, relation.and_then(ArtifactRelation::target_kind)),
            (EdgeKind::ArtifactBinding, RefForm::Method) => self.resolve_code_type_name(&r.target),
            // The OpenAPI `ApiOperation`→`route` match (S-069): the target is the
            // operation rendered `"METHOD /template"`, bound to the one route
            // whose positionally-normalized template matches and whose method
            // serves it — `ANY` is a wildcard, and an exact-method route outranks
            // it ([CR-109]; see `resolve_route`).
            (EdgeKind::ArtifactBinding, RefForm::Path) => self.resolve_route(&r.target),
            // Other (kind, form) shapes are consumer-story extension points:
            // unbound here, never fabricated.
            _ => Res::NotFound,
        };
        match found {
            Res::Found(target) => Outcome::Bound {
                source: self.source,
                target,
                kind: r.kind,
                payload: r.payload.clone(),
            },
            _ => Outcome::Unbound,
        }
    }

    /// Resolve a workspace-relative artifact path to the one
    /// [`NodeKind::ConfigFile`] at that path (CR-011, [FR-CG-07]).
    ///
    /// Folds the target relative to the source artifact's directory and then to
    /// the repository root — the same two-interpretation walk a documentation
    /// path takes ([`resolve_doc_link`](Ctx::resolve_doc_link)), minus the
    /// `#anchor` (artifact imports carry none). Accepts iff exactly one
    /// `ConfigFile` lives at the resolved path; a path that does not (yet) resolve
    /// stays unbound and retries on the sync that indexes its target.
    ///
    /// [FR-CG-07]: ../../../docs/specs/requirements/FR-CG-07.md
    fn resolve_artifact_path(&self, target: &str) -> Res {
        let Some(source_file) = self
            .ix
            .info
            .get(&self.source)
            .and_then(|i| i.file_path.as_deref())
        else {
            return Res::NotFound;
        };
        let source_rel = fold_path(&dir_segments(source_file), target);
        let root_rel = fold_path(&[], target);
        for path in [source_rel, root_rel].into_iter().flatten() {
            match self.config_file_at(&path) {
                Res::NotFound => {}
                decided => return decided,
            }
        }
        Res::NotFound
    }

    /// The one [`NodeKind::ConfigFile`] defined at `path`, or nothing.
    fn config_file_at(&self, path: &str) -> Res {
        let Some(ids) = self.ix.by_file_path.get(path) else {
            return Res::NotFound;
        };
        let candidates: Vec<NodeId> = ids
            .iter()
            .copied()
            .filter(|id| {
                self.ix
                    .info
                    .get(id)
                    .is_some_and(|i| i.kind == NodeKind::ConfigFile)
            })
            .collect();
        exactly_one(&candidates)
    }

    /// Resolve a Terraform local module call to **every** admitted `.tf`
    /// [`NodeKind::ConfigFile`] in its source directory (CR-011, [FR-CG-08]).
    ///
    /// A module `source = "./modules/net"` names a *directory*; the relation binds
    /// the calling `module` block to each `.tf` file living **directly** in that
    /// directory ([`config_tf_files_in_dir`](Ctx::config_tf_files_in_dir)). The
    /// directory is folded relative to the calling artifact's own directory then to
    /// the repository root — the two-interpretation walk
    /// [`resolve_artifact_path`](Ctx::resolve_artifact_path) uses, the first that
    /// resolves to at least one file winning. A directory with no indexed `.tf`
    /// stays unbound and retries on the sync that indexes its members; registry and
    /// remote sources were classified external before the ledger, so a row here is
    /// always a genuine local-path candidate ([NFR-RA-05]).
    ///
    /// [FR-CG-08]: ../../../docs/specs/requirements/FR-CG-08.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_module_dir(&self, target: &str, payload: Option<String>) -> Outcome {
        let Some(source_file) = self
            .ix
            .info
            .get(&self.source)
            .and_then(|i| i.file_path.as_deref())
        else {
            return Outcome::Unbound;
        };
        let source_rel = fold_path(&dir_segments(source_file), target);
        let root_rel = fold_path(&[], target);
        for dir in [source_rel, root_rel].into_iter().flatten() {
            let targets = self.config_tf_files_in_dir(&dir);
            if !targets.is_empty() {
                return Outcome::BoundMany {
                    source: self.source,
                    targets,
                    kind: EdgeKind::ArtifactRef,
                    payload,
                };
            }
        }
        Outcome::Unbound
    }

    /// Bind a path-grammar module specifier (S-439, [CR-142] D1, [FR-RS-01]),
    /// or `None` when `target` is not one — the row then takes the member-path
    /// scope hierarchy exactly as before, which is every import of a
    /// name-grammar language (Rust, Python, Java, …).
    ///
    /// - A **relative** specifier (target headed by `.`/`..`, which only
    ///   `extract::refs::specifier_segments` produces) is always decided here:
    ///   [`resolve_relative_specifier`](Ctx::resolve_relative_specifier).
    /// - Any other specifier from a path-grammar file is decided here too: a Go
    ///   import path under a declared module by
    ///   [`resolve_go_package`](Ctx::resolve_go_package), and everything else —
    ///   a bare package specifier (`react`, `next/link`) — **unbound**. A bare
    ///   specifier names a package, never a workspace file, so the member-path
    ///   hierarchy (which would read `react` as a name and bind it to a
    ///   workspace `react.ts`) is never consulted for one ([NFR-RA-05]).
    ///   `tsconfig` path aliases, the one way a bare specifier can name a
    ///   workspace file, are [FR-RS-02] and not this rung.
    ///
    /// [CR-142]: ../../../docs/requests/CR-142-cross-file-call-resolution-is-rust-only.md
    /// [FR-RS-01]: ../../../docs/specs/requirements/FR-RS-01.md
    /// [FR-RS-02]: ../../../docs/specs/requirements/FR-RS-02.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_specifier(&self, target: &str) -> Option<Outcome> {
        let segs = split(target);
        let source_file = self.ix.info.get(&self.source)?.file_path.as_deref()?;
        let targets = self.ix.specifier_targets.get(&extension_of(source_file));
        // A relative import of an import-root language names modules, not a
        // path: the member-path hierarchy reads its level (S-519,
        // [`resolve_relative`](Ctx::resolve_relative)).
        if targets.is_none() && self.ix.layout.has_import_roots(source_file) {
            return None;
        }
        if segs.first().is_some_and(|h| is_relative_head(h)) {
            let resolved = targets.map_or(Res::NotFound, |targets| {
                self.resolve_relative_specifier(source_file, &segs, targets)
            });
            return Some(match resolved {
                Res::Found(target) => Outcome::Bound {
                    source: self.source,
                    target,
                    kind: EdgeKind::Imports,
                    payload: None,
                },
                _ => Outcome::Unbound,
            });
        }
        targets?;
        // Only a Go file can name a Go package: a TypeScript `import 'shared'`
        // beside a `go.mod` declaring `module shared` is a package specifier,
        // never a path into the Go tree.
        let go_package = if extension_of(source_file) == "go" {
            self.resolve_go_package(source_file, &segs)
        } else {
            None
        };
        Some(go_package.unwrap_or(Outcome::Unbound))
    }

    /// The **imported** rung for a path-grammar file (S-440, [CR-142] D2,
    /// [FR-RS-03]), or `None` for a single-segment name or a file whose
    /// specifiers are not paths (every Rust call, so Rust binds exactly as
    /// before).
    ///
    /// `target` is `<import target>::<name>`, the form extraction records for a
    /// call through a named import or through an imported module's qualifier
    /// (`extract::ImportBindings`). It is the **only** multi-segment call a
    /// path-grammar file records — its `@ref.call` captures are plain
    /// identifiers — so every such row is decided here, `Some`, including one
    /// whose prefix names no import of the file: no candidate, unbound. The candidates are the top-level
    /// [`NodeKind::Function`]s named `name` in the file-root modules the file's
    /// `Imports` row for `<import target>` **bound** to
    /// ([`Index::with_imported_bindings`]); exactly one binds ([NFR-RA-05]).
    ///
    /// Why this is not the Rust alias step, and its own over-binding reasoning:
    ///
    /// - The evidence is the import the call went through, so a same-named
    ///   function in a module the file did **not** import that name from is
    ///   never a candidate — `helper` imported from `./a` never reaches `./b`'s
    ///   `helper`, and never both.
    /// - An import that binds nothing (an external package: `react`,
    ///   `github.com/lib/pq`), or a module that defines no such function (a
    ///   barrel re-exporting it), decides the call **unbound** — `Some`, not
    ///   `None` — so it never falls through to the workspace suffix match,
    ///   which would bind `pq.Open` to a workspace `pq/pq.go`.
    /// - Only a `Function` is a candidate: what a package or module qualifier
    ///   reaches is a top-level function. A method is reached through a value,
    ///   whose type the call does not state, and stays under the [FR-RS-06]
    ///   receiver discipline as a bare method name.
    /// - The [FR-RS-07] tie-break and the [FR-RS-08] trait fan-out are not
    ///   consulted: there is no associated method to break a tie with among
    ///   top-level functions, and no trait object in either language.
    ///
    /// [CR-142]: ../../../docs/requests/CR-142-cross-file-call-resolution-is-rust-only.md
    /// [FR-RS-03]: ../../../docs/specs/requirements/FR-RS-03.md
    /// [FR-RS-06]: ../../../docs/specs/requirements/FR-RS-06.md
    /// [FR-RS-07]: ../../../docs/specs/requirements/FR-RS-07.md
    /// [FR-RS-08]: ../../../docs/specs/requirements/FR-RS-08.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_imported_call(&self, target: &str) -> Option<Res> {
        let (module, name) = target.rsplit_once("::")?;
        let source_file = self.ix.info.get(&self.source)?.file_path.as_deref()?;
        if !self.ix.is_path_specifier_file(source_file) {
            return None;
        }
        let roots = self
            .file_id
            .and_then(|f| self.ix.imported.get(&f))
            .and_then(|by_target| by_target.get(module));
        let candidates: Vec<NodeId> = roots
            .into_iter()
            .flatten()
            .flat_map(|&root| self.ix.members_named(root, name, Want::Callable))
            .filter(|id| {
                self.ix
                    .info
                    .get(id)
                    .is_some_and(|i| i.kind == NodeKind::Function)
            })
            .collect();
        Some(exactly_one(&candidates))
    }

    /// A relative specifier, resolved against the importing file's directory by
    /// the same fold doc links use ([`fold_path`]), then matched to the one
    /// file-root module whose extension-less path it names, else to that
    /// directory's `index` file — the order a TypeScript/JavaScript resolver
    /// tries them. Exactly-one-or-nothing at each step ([NFR-RA-05]): a
    /// `nav.ts` beside a `nav.tsx` stays unbound, and a specifier escaping the
    /// repository root names nothing.
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_relative_specifier(
        &self,
        source_file: &str,
        segs: &[String],
        targets: &HashSet<String>,
    ) -> Res {
        let Some(path) = fold_path(&dir_segments(source_file), &segs.join("/")) else {
            return Res::NotFound;
        };
        // Only a file of an extension the importing language resolves to is a
        // candidate: a `helper.py` beside `helper.ts` neither answers `./helper`
        // nor makes it ambiguous.
        let at_stem = |stem: &str| {
            let candidates: Vec<NodeId> = self
                .ix
                .file_roots_by_stem
                .get(stem)
                .into_iter()
                .flatten()
                .copied()
                .filter(|id| {
                    self.ix
                        .info
                        .get(id)
                        .and_then(|i| i.file_path.as_deref())
                        .is_some_and(|p| targets.contains(&extension_of(p)))
                })
                .collect();
            exactly_one(&candidates)
        };
        match at_stem(&path) {
            Res::NotFound => {}
            decided => return decided,
        }
        if path.is_empty() {
            at_stem("index")
        } else {
            at_stem(&format!("{path}/index"))
        }
    }

    /// A Go import path, bound to the **package** it names — every non-test
    /// `.go` file directly in the package directory, the same one-row fan-out a
    /// Terraform module directory takes ([`resolve_module_dir`](Ctx::resolve_module_dir)).
    ///
    /// Consulted only for a `.go` importer. Returns `Some` only for an import
    /// path that falls under a module the tree declares ([`super::go_module`])
    /// and names a directory holding at least one such file; every other case
    /// returns `None`, which [`resolve_specifier`](Ctx::resolve_specifier)
    /// decides **unbound**.
    ///
    /// - The path is matched against the declared module paths on a
    ///   whole-segment prefix, so the dotted host is compared as written, never
    ///   split, and `example.com/shopfront` is not under `example.com/shop`.
    /// - The **longest** matching module path wins — a nested module
    ///   (`example.com/shop/tools`) owns its subtree, as it does for the Go
    ///   toolchain.
    /// - Two `go.mod`s declaring the **same** module path (several
    ///   `examples/*/go.mod` saying `module example`) are told apart only by
    ///   the importer: the one whose root holds it — its nearest — is the
    ///   module it builds in. An importer in neither leaves the import unbound
    ///   rather than picking one ([NFR-RA-05]).
    ///
    /// A path under no declared module is a standard-library or third-party
    /// import (`net/http`, `context`, `github.com/lib/pq`): the module
    /// declaration is the evidence that it is external, and no directory that
    /// merely shares its last segments may stand in for it ([NFR-RA-05]).
    /// Without a `go.mod` there is no evidence where the module path ends, so
    /// nothing is bound on a guess. `_test.go` files are not part of the
    /// package another package imports.
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_go_package(&self, source_file: &str, segs: &[String]) -> Option<Outcome> {
        let spec = segs.join("/");
        let mut under: Vec<(&GoModule, &str)> = self
            .ix
            .go_modules
            .iter()
            .filter_map(|m| {
                let rest = spec.strip_prefix(m.path.as_str())?;
                match rest.strip_prefix('/') {
                    Some(rest) => Some((m, rest)),
                    None if rest.is_empty() => Some((m, rest)),
                    None => None, // a longer segment: `example.com/shopfront`
                }
            })
            .collect();
        // Under no declared module: external — the caller decides it unbound.
        let longest = under.iter().map(|(m, _)| m.path.len()).max()?;
        under.retain(|(m, _)| m.path.len() == longest);
        if under.len() > 1 {
            let owns = |root: &str| root.is_empty() || source_file.starts_with(&format!("{root}/"));
            let nearest = under
                .iter()
                .filter(|(m, _)| owns(&m.root))
                .map(|(m, _)| m.root.len())
                .max()?;
            under.retain(|(m, _)| owns(&m.root) && m.root.len() == nearest);
        }
        let [(module, rest)] = under[..] else {
            return None;
        };
        let dir = match (module.root.is_empty(), rest.is_empty()) {
            (true, _) => rest.to_string(),
            (false, true) => module.root.clone(),
            (false, false) => format!("{}/{rest}", module.root),
        };
        let targets: Vec<NodeId> = self
            .ix
            .file_roots_by_dir
            .get(&dir)
            .into_iter()
            .flatten()
            .filter(|(path, _)| path.ends_with(".go") && !path.ends_with("_test.go"))
            .map(|&(_, id)| id)
            .collect();
        (!targets.is_empty()).then_some(Outcome::BoundMany {
            source: self.source,
            targets,
            kind: EdgeKind::Imports,
            payload: None,
        })
    }

    /// Every `.tf` [`NodeKind::ConfigFile`] whose file lives **directly** in `dir`
    /// (no recursion into nested module directories), `NodeId`-sorted for a
    /// deterministic edge set ([NFR-RA-06]). The fan-out targets of a local module
    /// call ([`resolve_module_dir`](Ctx::resolve_module_dir)).
    fn config_tf_files_in_dir(&self, dir: &str) -> Vec<NodeId> {
        let mut out: Vec<NodeId> = Vec::new();
        for (path, ids) in &self.ix.by_file_path {
            if !path.ends_with(".tf") {
                continue;
            }
            let parent = match path.rfind('/') {
                Some(i) => &path[..i],
                None => "",
            };
            if parent != dir {
                continue;
            }
            for &id in ids {
                if self
                    .ix
                    .info
                    .get(&id)
                    .is_some_and(|i| i.kind == NodeKind::ConfigFile)
                {
                    out.push(id);
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// Resolve a literal artifact name to the one artifact node carrying it
    /// (CR-011, [FR-CG-08]).
    ///
    /// The artifact→artifact name match — a proto type reference to its
    /// `ProtoMessage`, a GraphQL type reference to its `GqlType`, a Terraform
    /// `var`/`local`/`module` reference to its `TfBlock`, a SQL clause to its
    /// `SqlObject`. When the relation declares a specific target
    /// ([`ArtifactRelation::target_kind`]) the candidate set is fenced to **that**
    /// artifact kind, so a name shared across formats (a proto type and a same-named
    /// `TfBlock`) can never cross-bind ([NFR-RA-05], [ADR-26]); `want_kind` is
    /// `None` only for a relation with no declared artifact target, where any
    /// artifact-layer ([`NodeKind::is_config`]) node is a candidate. Code symbols
    /// are always excluded, and the single [`exactly_one`] gate keeps
    /// never-fabricate — a duplicated or absent name stays unresolved.
    ///
    /// [FR-CG-08]: ../../../docs/specs/requirements/FR-CG-08.md
    /// [ADR-26]: ../../../docs/specs/architecture/decisions/ADR-26.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_artifact_name(&self, name: &str, want_kind: Option<NodeKind>) -> Res {
        // A **keyless** row names nothing and can bind to nothing. The broker arm
        // records its `topic-not-literal` refusals as keyless ledger rows ([CR-107]),
        // and this is the gate that keeps one from binding to some artifact node that
        // happens to carry an empty name — a fabricated edge out of a refusal, which
        // is the opposite of what recording it is for ([NFR-RA-05]).
        //
        // [CR-107]: ../../../docs/requests/CR-107-broker-topic-capture-drops-placeholder-and-array-literals.md
        if name.trim().is_empty() {
            return Res::NotFound;
        }
        let Some(all) = self.ix.by_name.get(name) else {
            return Res::NotFound;
        };
        let admits = |kind: NodeKind| match want_kind {
            // A declared relation target fences candidates to exactly that kind.
            Some(want) => kind == want,
            // No declared target: any artifact-layer node, code excluded.
            None => kind.is_config(),
        };
        let candidates: Vec<NodeId> = all
            .iter()
            .copied()
            .filter(|id| self.ix.info.get(id).is_some_and(|i| admits(i.kind)))
            .collect();
        exactly_one(&candidates)
    }

    /// Resolve a schema-declared type name to the one **type-like code** symbol
    /// carrying it (CR-011, [FR-CG-10]).
    ///
    /// The artifact→code binding for a literal declared name (a proto/GraphQL type
    /// → the struct/class/… that implements it). Only type-like code kinds are
    /// candidates ([`is_type_like`]) — **no synthesized candidates**, no codegen
    /// case-mapping, no resolver conventions ([ADR-26]) — and the single
    /// [`exactly_one`] gate means a common name (`User` declared in two places) or
    /// an absent one stays unresolved ([NFR-RA-05]).
    ///
    /// [FR-CG-10]: ../../../docs/specs/requirements/FR-CG-10.md
    /// [ADR-26]: ../../../docs/specs/architecture/decisions/ADR-26.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_code_type_name(&self, name: &str) -> Res {
        let Some(all) = self.ix.by_name.get(name) else {
            return Res::NotFound;
        };
        let candidates: Vec<NodeId> = all
            .iter()
            .copied()
            .filter(|id| self.ix.info.get(id).is_some_and(|i| is_type_like(i.kind)))
            .collect();
        exactly_one(&candidates)
    }

    /// Resolve an OpenAPI `ApiOperation` to the one framework-extracted `route`
    /// node it specifies (S-069, CR-011, [FR-CG-09]).
    ///
    /// `target` is the operation rendered `"METHOD /template"` (the same shape a
    /// [`NodeKind::Route`] node's `name` carries). Both sides are reduced to the
    /// shared `(METHOD, positionally-normalized template)`
    /// [`route_key`](super::route_template::route_key): parameter names and
    /// syntax are erased, but the static skeleton must match exactly. The
    /// candidate set is that template's bucket in [`Index`], narrowed by the
    /// shared [`route_method`](super::route_method) rule — an `ANY` provider
    /// serves every verb, an exact-method provider of the same template is the
    /// sole candidate beside a wildcard one ([CR-109]) — and then reduced by the
    /// shared [`exactly_one`] gate. So two equally-specific routes sharing a
    /// normalized template leave the operation unresolved, a method mismatch
    /// never binds, and an operation whose key does not normalize (or matches no
    /// route) stays in the ledger for the next sync ([NFR-RA-05]). A
    /// catch-all/regex route is absent from the index entirely, so it is never
    /// approximately matched.
    ///
    /// The cross-member bridge ([`crate::federation::bridge`]) and the coverage
    /// read-model narrow their own provider buckets through the very same rule,
    /// so no two of the three sites can drift on which input binds ([ADR-52]).
    ///
    /// [CR-109]: ../../../docs/requests/CR-109-wildcard-method-route-matching.md
    /// [FR-CG-09]: ../../../docs/specs/requirements/FR-CG-09.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    /// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
    fn resolve_route(&self, target: &str) -> Res {
        // A **keyless** row names nothing and can bind to nothing — the same guard
        // [`Ctx::resolve_artifact_name`] carries for the broker arm's refusals
        // ([CR-107]), stated here because the HTTP client-call arm's refusals
        // ([CR-120], S-374) arrive on this path instead: they are
        // `(ArtifactBinding, Path)` rows, so `resolve_artifact` dispatches them
        // here. `route_key` refuses an empty `"METHOD /template"` on its own, so
        // this is explicit rather than load-bearing — and explicit is the point:
        // the property is a never-fabricate invariant ([NFR-RA-05]), not an
        // incidental consequence of a `split_once`.
        //
        // [CR-107]: ../../../docs/requests/CR-107-broker-topic-capture-drops-placeholder-and-array-literals.md
        // [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
        if target.trim().is_empty() {
            return Res::NotFound;
        }
        let Some((method, template)) = route_key(target) else {
            // The operation's own template does not normalize (or the target is
            // malformed): never approximately matched.
            return Res::NotFound;
        };
        let Some(bucket) = self.ix.routes_by_template.get(&template) else {
            return Res::NotFound;
        };
        let candidates: Vec<NodeId> = preferred_candidates(
            bucket.iter().map(|(m, id)| (Some(m.as_str()), id)),
            Some(&method),
        )
        .into_iter()
        .copied()
        .collect();
        exactly_one(&candidates)
    }

    /// CR-068 Part B bare-path method exclusion, expressed as a **tie-break**
    /// ([FR-RS-07]): while resolving a single-segment bare-path call (gated on
    /// [`bare_path_call`](Ctx::bare_path_call)), a free [`NodeKind::Function`] at a
    /// scope outranks same-named [`NodeKind::Method`]s there. So a same-module bare
    /// call binds
    /// the one free function even when same-named associated methods collapse to
    /// that module scope (`impl` is not a captured scope, [`is_type_like`]) — the
    /// `graph_store` `insert_node`/`insert_edge`/`upsert_symbol` cluster the graph
    /// previously left [`Res::Ambiguous`].
    ///
    /// A **tie-break, not a filter**: methods are dropped only when a free
    /// function is actually present. So it is strictly monotonic and
    /// never-fabricate ([NFR-RA-05]):
    /// - one free fn + same-named methods → binds the free fn (the recovery);
    /// - two-or-more free fns → still ambiguous, stays unresolved;
    /// - no free fn (a language whose free callables are `Method`, e.g. a Ruby
    ///   top-level `def`, or a lone associated method) → the full callable set
    ///   stands, so no previously-resolved edge is lost.
    ///
    /// Only methods are dropped. A class or macro a [`Want::DeclaredCall`]
    /// admits (S-521) stays beside the free function, so a function and a class
    /// of one name in one scope are two candidates here exactly as on every
    /// other rung. Under [`Want::Callable`] the candidates are functions and
    /// methods alone, so dropping the methods is what keeping the functions was.
    ///
    /// Path-qualified (`Type::f` via [`descend`](Ctx::descend)) and typed calls
    /// never reach this step, and a receiver-method call ([`RefForm::Method`])
    /// never reaches the scope walk at all — it binds by its receiver's shape
    /// (S-514). The step is gated on [`bare_path_call`](Ctx::bare_path_call).
    ///
    /// [FR-RS-07]: ../../../docs/specs/requirements/FR-RS-07.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn prefer_free_functions(&self, mut candidates: Vec<NodeId>) -> Vec<NodeId> {
        if !self.bare_path_call.get() {
            return candidates;
        }
        let kind = |id: &NodeId| self.ix.info.get(id).map(|i| i.kind);
        if candidates.iter().any(|id| kind(id) == Some(NodeKind::Function)) {
            candidates.retain(|id| kind(id) != Some(NodeKind::Method));
        }
        candidates
    }

    /// Resolve a bare name by the scope hierarchy (function-local outward).
    ///
    /// Reached only for a **single-segment** name (multi-segment paths route
    /// through [`descend`](Ctx::descend)). A receiver-method call never arrives
    /// here (it binds by its receiver's shape, S-514); the CR-068 Part B
    /// free-function tie-break ([`prefer_free_functions`]) fires only for a
    /// bare-path call, gated on [`bare_path_call`](Ctx::bare_path_call).
    fn resolve_name(&self, name: &str, want: Want, depth: u8) -> Res {
        // 1) Lexical Contains chain, innermost first: nested decls of the
        //    source itself (or of its enclosing scope, for a declaration's
        //    header — `lexical_start`), then each enclosing scope up to the
        //    file module.
        let mut cursor = Some(self.lexical_start.get());
        while let Some(scope) = cursor {
            let mut members = self.prefer_free_functions(self.ix.members_named(scope, name, want));
            members.retain(|&id| !self.is_own_header(id));
            match exactly_one(&members) {
                Res::NotFound => {}
                decided => return decided, // found — or a *known* ambiguity
            }
            cursor = self.ix.parent.get(&scope).copied();
        }
        // A package-shaped source (CR-149) continues on its own rungs: its
        // imports, its package, its wildcards — never the module tree.
        if let Some(package) = &self.source_package {
            return self.resolve_package_name(package, name, want, depth);
        }
        // 2) A child module of the source module, a crate-root module
        //    (sibling files are linked via the path-derived module tree, not
        //    via Contains), or an extern crate's root (`use other::*` /
        //    `use other;` name the crate itself) — for a lookup that admits a
        //    module: never a call's, nor a type relation's (S-522).
        if want.admits(NodeKind::Module) {
            if let Some((krate, mods)) = self.source_module() {
                let mut child = mods.clone();
                child.push(name.to_string());
                if let Some(&id) = self.ix.modules.get(&(krate.clone(), child)) {
                    return Res::Found(id);
                }
                if let Some(&id) = self.ix.modules.get(&(krate, vec![name.to_string()])) {
                    return Res::Found(id);
                }
            }
            let norm = normalize_crate(name);
            if let Some(&id) = self
                .ix
                .modules
                .get(&(norm, Vec::new()))
                .filter(|_| !self.in_family_crate())
            {
                return Res::Found(id);
            }
        }
        // 3) The file's `use` aliases — every one of them, exactly-one, when an
        //    import-root file imports the name twice.
        if let Some(rivals) = self.rival_expansions(name) {
            match self.resolve_expansions(rivals, &[], want, depth) {
                Res::NotFound => {}
                decided => return decided,
            }
        } else if let Some(alias_path) = self.scope().and_then(|s| s.aliases.get(name)) {
            match self.resolve_path(alias_path, want, depth - 1) {
                Res::NotFound => {}
                decided => return decided,
            }
        }
        // 4) The file's glob imports.
        let single = [name.to_string()];
        match self.through_globs(&single, want, depth) {
            Res::NotFound => {}
            decided => return decided,
        }
        // 5) Workspace unique-name fallback — aggressive only for bare names,
        //    never for a type relation ([`scope_only`](Ctx::scope_only)). A
        //    receiver-method call never reaches here (S-514, CR-066).
        if self.policy == BindingPolicy::Aggressive && !self.scope_only {
            return self.unique_by_name(name, want);
        }
        Res::NotFound
    }

    /// A bare name from a package-shaped source, after the lexical chain
    /// ([CR-149], [FR-RS-03]) — Java's scope order for a simple name:
    ///
    /// 1. **imported** — every single-type or single-static import naming it
    ///    (the file's alias expansions), each resolved through
    ///    [`resolve_path`](Ctx::resolve_path), exactly-one across all of them.
    ///    **Final** when the file imports the name at all: a single-type or
    ///    single-static import shadows the package and the wildcards (JLS
    ///    §6.4.1), so an import of the JDK's `Map` or a library's `Message`
    ///    that binds nothing here leaves the name unresolved rather than
    ///    reaching a same-package or wildcard type of that name (S-466 review);
    /// 2. **same package** — a top-level type of the source's own package,
    ///    visible without an import;
    /// 3. **on-demand** — a member of a type, or a type of a package, the file
    ///    imports with a wildcard ([`glob_members`](Ctx::glob_members));
    /// 4. the policy-gated workspace name fallback, as for every other
    ///    language — except for a type relation, which binds by scope and
    ///    package key only ([`scope_only`](Ctx::scope_only)).
    ///
    /// A **receiver**-method call ([`RefForm::Method`]) takes none of these
    /// rungs: its target is its receiver's type's member ([CR-150]), which no
    /// import names; it binds by its receiver's shape and never reaches this
    /// walk (S-514, [CR-066]).
    ///
    /// Each rung is exactly-one; a known ambiguity stops the walk ([NFR-RA-05]).
    ///
    /// [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    /// [CR-066]: ../../../docs/requests/CR-066-receiver-method-overbinding.md
    /// [CR-150]: ../../../docs/requests/CR-150-java-receiver-typing-for-method-calls.md
    /// [FR-RS-03]: ../../../docs/specs/requirements/FR-RS-03.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_package_name(&self, package: &[String], name: &str, want: Want, depth: u8) -> Res {
        if let Some(expansions) = self.scope().and_then(|s| s.alias_expansions.get(name)) {
            let mut imported: Vec<NodeId> = Vec::new();
            for alias_path in expansions {
                match self.resolve_path(alias_path, want, depth - 1) {
                    Res::Found(id) => imported.push(id),
                    Res::Ambiguous => return Res::Ambiguous,
                    Res::NotFound => {}
                }
            }
            imported.sort();
            imported.dedup();
            let decided = exactly_one(&imported);
            if decided == Res::Ambiguous {
                // Two imports each supply a callable of this name.
                self.note(want, || Residue::OverloadAmbiguous);
            }
            return decided;
        }
        let same_package: Vec<NodeId> = self
            .package_type(package, name)
            .iter()
            .copied()
            .filter(|id| self.ix.info.get(id).is_some_and(|i| want.admits(i.kind)))
            .collect();
        match exactly_one(&same_package) {
            Res::NotFound => {}
            decided => return decided,
        }
        match self.glob_members(name, want, false) {
            Some(found) => match exactly_one(&found) {
                Res::NotFound => {}
                Res::Ambiguous => {
                    // Two wildcards each supply a callable of this name.
                    self.note(want, || Residue::OverloadAmbiguous);
                    return Res::Ambiguous;
                }
                decided => return decided,
            },
            None => {
                // A wildcard's type is declared twice, and one declaration
                // could supply the name.
                self.note(want, || Residue::TypeAmbiguous);
                return Res::Ambiguous;
            }
        }
        if self.policy == BindingPolicy::Aggressive && !self.scope_only {
            return self.unique_by_name(name, want);
        }
        Res::NotFound
    }

    /// A multi-segment path from a package-shaped source ([CR-149]), its head
    /// read in the language's order for a simple type name (JLS §6.5.5): a
    /// member type in lexical scope (`Inner.Deep` inside `Outer`); else the
    /// file's single-type import of it — **final**, as in
    /// [`resolve_package_name`](Ctx::resolve_package_name), so `Map.Entry`
    /// under `import java.util.Map` never reaches a same-package `Map`; else a
    /// type of the source's own package; else one a wildcard brings into view;
    /// else the whole path read as a fully-qualified name
    /// ([`resolve_fqn`](Ctx::resolve_fqn)). The first rung whose head names a
    /// type decides — a simple type name obscures a package of the same
    /// spelling, as the language rules it.
    ///
    /// [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    fn resolve_package_path(&self, package: &[String], segs: &[String], want: Want, depth: u8) -> Res {
        let Some((head, rest)) = segs.split_first() else {
            return Res::NotFound;
        };
        let mut cursor = Some(self.lexical_start.get());
        while let Some(scope) = cursor {
            let mut heads = self.member_types(scope, head);
            heads.retain(|&id| !self.is_own_header(id));
            if let Some(decided) = self.walk_from(&heads, rest, want) {
                return decided;
            }
            cursor = self.ix.parent.get(&scope).copied();
        }
        if let Some(alias_path) = self.scope().and_then(|s| s.aliases.get(head)) {
            let mut expanded = alias_path.clone();
            expanded.extend(rest.iter().cloned());
            return self.resolve_path(&expanded, want, depth - 1);
        }
        if let Some(decided) = self.walk_from(self.package_type(package, head), rest, want) {
            return decided;
        }
        match self.glob_members(head, Want::Any, true) {
            Some(found) => {
                if let Some(decided) = self.walk_from(&found, rest, want) {
                    return decided;
                }
            }
            None => {
                self.note(want, || Residue::TypeAmbiguous);
                return Res::Ambiguous;
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
    /// reads them: the source's own package, each non-static wildcard, then the
    /// path as written when it is qualified. Its single-type import never
    /// appears here — an imported head is expanded and resolved as the path
    /// written. A simple name as written would name a type of the default
    /// package, which a file in a named package cannot see (JLS §7.5); a
    /// default-package source reaches it as its own package, first.
    fn type_candidates(&self, package: &[String], segs: &[String]) -> Vec<Vec<String>> {
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
        push(package);
        if let Some(scope) = self.scope() {
            for glob in &scope.globs {
                push(glob);
            }
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
    /// [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_fqn(&self, segs: &[String], want: Want) -> Res {
        let Some(family_types) = self.family_types() else {
            return Res::NotFound;
        };
        for split_at in (1..=segs.len()).rev() {
            if let Some(types) = family_types.get(&segs[..split_at]) {
                return self
                    .walk_from(types, &segs[split_at..], want)
                    .unwrap_or(Res::NotFound);
            }
        }
        Res::NotFound
    }

    /// The files a declared-namespace source's namespace wildcard names (S-518,
    /// [FR-RS-13]): every **other** file declaring the namespace `segs`
    /// ([`Index::files_by_namespace`]), as one [`Outcome::BoundMany`] — `None`
    /// when the source is not a declared-namespace file, or no other file
    /// declares it (a framework namespace, `System`, stays unbound,
    /// [NFR-RA-05]).
    ///
    /// [FR-RS-13]: ../../../docs/specs/requirements/FR-RS-13.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn namespace_files(&self, segs: &[String]) -> Option<Outcome> {
        let source_file = self
            .ix
            .info
            .get(&self.source)
            .and_then(|i| i.file_path.as_deref())?;
        if !self.ix.layout.declares_namespaces(source_file) {
            return None;
        }
        let own_file = self.ix.by_file_path.get(source_file);
        let targets: Vec<NodeId> = self
            .ix
            .files_by_namespace
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
        })
    }

    /// The top-level types named `name` in `package`, id-sorted — of the
    /// source's own interop family only (S-519).
    fn package_type(&self, package: &[String], name: &str) -> &[NodeId] {
        let mut fqn = package.to_vec();
        fqn.push(name.to_string());
        self.family_types()
            .and_then(|types| types.get(&fqn))
            .map_or(&[], Vec::as_slice)
    }

    /// The fully-qualified type index of the source's interop family (S-519,
    /// [`Index::types_by_fqn`]); `None` when no file of that family declares a
    /// type, or the source has no file.
    fn family_types(&self) -> Option<&HashMap<Vec<String>, Vec<NodeId>>> {
        self.ix.types_by_fqn.get(self.source_family.as_deref()?)
    }

    /// Walk `rest` down from the one type in `candidates`: `None` when there is
    /// none (the caller's next rung decides), else the walk's result — sticky
    /// [`Res::Ambiguous`] for two candidates. Every segment but the last must
    /// name exactly one nested type; the last names a member admitted by
    /// `want`, and an empty `rest` is the type itself.
    ///
    /// A **call** ([`Want::is_call`]) takes its last segment through
    /// [`type_member`](Ctx::type_member): the type's own callable, else its
    /// in-repository supertypes' (S-468). Every other lookup reads the type's
    /// own members only, as before.
    fn walk_from(&self, candidates: &[NodeId], rest: &[String], want: Want) -> Option<Res> {
        let mut cursor = match exactly_one(candidates) {
            Res::NotFound => return None,
            Res::Ambiguous => {
                self.note(want, || Residue::TypeAmbiguous);
                return Some(Res::Ambiguous);
            }
            Res::Found(ty) => ty,
        };
        let Some((last, inner)) = rest.split_last() else {
            let admitted = self.ix.info.get(&cursor).is_some_and(|i| want.admits(i.kind));
            return Some(if admitted { Res::Found(cursor) } else { Res::NotFound });
        };
        for seg in inner {
            match exactly_one(&self.member_types(cursor, seg)) {
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
            return Some(self.type_member(cursor, last));
        }
        Some(exactly_one(&self.ix.members_named(cursor, last, want)))
    }

    /// The one callable `name` of type `ty` (S-468, [CR-150] §3.2 B, [FR-RS-10]):
    /// exactly one among `ty`'s own `Contains` children; failing that, the
    /// in-repository `Extends` chain ([`Index::supertypes`]) climbed nearest
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
    /// [CR-150]: ../../../docs/requests/CR-150-java-receiver-typing-for-method-calls.md
    /// [FR-RS-10]: ../../../docs/specs/requirements/FR-RS-10.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn type_member(&self, ty: NodeId, name: &str) -> Res {
        self.supertype_member(vec![ty], HashSet::from([ty]), name)
    }

    /// [`type_member`](Ctx::type_member)'s walk, from the level `level` up —
    /// `seen` holds the types already visited, which the walk never revisits.
    /// An empty first level is a chain that never entered the graph:
    /// [`Residue::SupertypeUnreached`]. A class that uses a trait
    /// ([`Index::trait_users`], S-522) is read for its own members and never
    /// climbed through, so the walk ends there too.
    fn supertype_member(&self, mut level: Vec<NodeId>, mut seen: HashSet<NodeId>, name: &str) -> Res {
        level.sort_unstable();
        level.dedup();
        seen.extend(level.iter().copied());
        for _ in 0..MAX_SUPERTYPE_DEPTH {
            if level.is_empty() {
                break;
            }
            let mut found: Vec<NodeId> = level
                .iter()
                .flat_map(|&t| self.ix.members_named(t, name, Want::Callable))
                .collect();
            found.sort_unstable();
            found.dedup();
            match exactly_one(&found) {
                Res::NotFound => {}
                Res::Ambiguous => {
                    self.note(Want::Callable, || Residue::OverloadAmbiguous);
                    return Res::Ambiguous;
                }
                decided => return decided,
            }
            // A class that uses a trait is never climbed through (S-522).
            let mut next: Vec<NodeId> = level
                .iter()
                .filter(|t| !self.ix.trait_users.contains(t))
                .flat_map(|&t| self.ix.supertypes_of(t).iter().copied())
                .filter(|s| seen.insert(*s))
                .collect();
            if next.is_empty() {
                break;
            }
            next.sort_unstable();
            level = next;
        }
        self.note(Want::Callable, || Residue::SupertypeUnreached);
        Res::NotFound
    }

    /// The type-like members of `scope` named `name`.
    fn member_types(&self, scope: NodeId, name: &str) -> Vec<NodeId> {
        self.ix
            .members_named(scope, name, Want::Any)
            .into_iter()
            .filter(|id| self.ix.info.get(id).is_some_and(|i| is_type_like(i.kind)))
            .collect()
    }

    /// What the file's wildcards bring into view under `name` ([CR-149]):
    /// for a glob naming a type, `C`'s members named `name` — every static
    /// member for `import static a.b.C.*`, member types only for
    /// `import a.b.C.*`; for one naming a package (`import a.b.*`), the
    /// package's top-level types named `name`. `types_only` keeps type-like
    /// candidates alone — a path head must be a type. Deduplicated and id-sorted across
    /// every glob, so the caller's exactly-one test is over the union; `None`
    /// when a glob's own type name is ambiguous and one of its declarations
    /// has a member named `name` — an ambiguity no wider rung may overrule
    /// ([NFR-RA-05]). A name none of them declares is not blocked.
    ///
    /// No recursion through the file's globs: a glob's own target is read as a
    /// fully-qualified name only ([`resolve_fqn`](Ctx::resolve_fqn)), so the
    /// CR-016 fan-out [`through_globs`](Ctx::through_globs) guards against
    /// cannot arise here.
    ///
    /// [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn glob_members(&self, name: &str, want: Want, types_only: bool) -> Option<Vec<NodeId>> {
        let Some(scope) = self.scope() else {
            return Some(Vec::new());
        };
        // A non-static wildcard imports types only — a static one, every static
        // member — so a method is only ever reached through `import static`.
        let globs = scope
            .globs
            .iter()
            .map(|g| (g, true))
            .chain(scope.static_globs.iter().map(|g| (g, types_only)));
        let mut found: Vec<NodeId> = Vec::new();
        for (glob, types_only) in globs {
            let admits = |id: &NodeId| {
                self.ix.info.get(id).is_some_and(|i| {
                    want.admits(i.kind) && (!types_only || is_type_like(i.kind))
                })
            };
            match self.resolve_fqn(glob, Want::Any) {
                Res::Found(ty) if self.ix.info.get(&ty).is_some_and(|i| is_type_like(i.kind)) => {
                    found.extend(self.ix.members_named(ty, name, want).into_iter().filter(admits));
                }
                Res::Found(_) => {}
                // Two declarations of the wildcard's type: `name` is ambiguous
                // only if one of them could supply it. A name neither declares
                // — an unrelated import's head, say — never came from this
                // wildcard, so it must not be blocked by it.
                Res::Ambiguous => {
                    let could_supply = self
                        .family_types()
                        .and_then(|types| types.get(glob.as_slice()))
                        // `None`: ambiguous below a nested segment — stay conservative.
                        .is_none_or(|types| {
                            types.iter().any(|&ty| {
                                self.ix.members_named(ty, name, want).iter().any(admits)
                            })
                        });
                    if could_supply {
                        return None;
                    }
                }
                Res::NotFound => {
                    found.extend(self.package_type(glob, name).iter().copied().filter(admits));
                }
            }
        }
        found.sort();
        found.dedup();
        Some(found)
    }

    /// Resolve `segs` as members reached through each of the file's glob
    /// imports; accept iff exactly one distinct target across all globs.
    ///
    /// Each glob's *own* module path is resolved via [`resolve_path`](Ctx::resolve_path),
    /// whose import step lands back here — so on a file carrying `G` glob
    /// imports a single lookup fans out `G`-wide at every recursion level, i.e.
    /// `O(G^MAX_ALIAS_DEPTH)` work per reference. A real trigger (CR-016): a file
    /// with 14 `use super::*` / `use super::sub::*` imports drove a single bind
    /// to ~1.5e9 operations (~150 s on one core); across the parallel bind pass
    /// many such refs detonate at once and peg every core — the dominant cost of
    /// a cold full index (measured on the self-graph: a cold index climbed to a
    /// load of 27 within 27 s before this guard, 3.8 s at flat load after).
    ///
    /// The `in_glob_resolution` guard breaks the recursion: while a glob's
    /// module is being resolved, a re-entry here returns `NotFound` at once.
    /// This is **outcome-preserving** — a glob module reachable *only* through
    /// the same in-scope glob set never converged under the depth budget anyway
    /// (it bottomed out at `NotFound`), so we return that result without the
    /// exponential. The sync≡reindex equivalence net and the real self-graph
    /// (edge set unchanged) gate the equivalence.
    fn through_globs(&self, segs: &[String], want: Want, depth: u8) -> Res {
        let Some(scope) = self.scope() else {
            return Res::NotFound;
        };
        // Re-entrant glob-module resolution contributes nothing (see above).
        if self.in_glob_resolution.get() {
            return Res::NotFound;
        }
        let mut found: Vec<NodeId> = Vec::new();
        for glob in &scope.globs {
            // Resolve the glob's module itself (depth-limited like an alias),
            // with glob fan-out suppressed for the duration so the lookup cannot
            // recurse back through the file's glob set.
            self.in_glob_resolution.set(true);
            let resolved = self.resolve_path(glob, Want::Module, depth.saturating_sub(1));
            self.in_glob_resolution.set(false);
            let module = match resolved {
                Res::Found(m) => m,
                Res::Ambiguous => return Res::Ambiguous,
                Res::NotFound => continue,
            };
            let Some(key) = self.ix.module_key.get(&module) else {
                continue;
            };
            match self.descend(&key.0, &key.1, segs, want) {
                Res::Found(id) => found.push(id),
                Res::Ambiguous => return Res::Ambiguous,
                Res::NotFound => {}
            }
        }
        found.sort();
        found.dedup();
        exactly_one(&found)
    }

    /// Walk `segs` down the module tree from `(krate, base)`.
    fn descend(&self, krate: &str, base: &[String], segs: &[String], want: Want) -> Res {
        let mut key: ModKey = (krate.to_string(), base.to_vec());
        for (i, seg) in segs.iter().enumerate() {
            let is_last = i == segs.len() - 1;

            if !is_last {
                // Try the segment as a child module in place (push, check,
                // pop on miss) — no per-step key clone. An import-root
                // language's directory is a module whether or not a package
                // file gives it a node (S-519).
                key.1.push(seg.clone());
                if self.ix.modules.contains_key(&key) || self.ix.dir_modules.contains(&key) {
                    continue;
                }
                key.1.pop();
                // The `Type::func` collapse: associated items live at module
                // scope (S-007 does not capture `impl`), so when the
                // next-to-last segment names a type in the current module,
                // the final segment is looked up among the module's
                // functions.
                if i == segs.len() - 2 {
                    if let Some(&scope_node) = self.ix.modules.get(&key) {
                        let type_here = self
                            .ix
                            .members_named(scope_node, seg, Want::Any)
                            .into_iter()
                            .any(|id| self.ix.info.get(&id).is_some_and(|n| is_type_like(n.kind)));
                        if type_here {
                            return exactly_one(&self.ix.members_named(
                                scope_node,
                                &segs[i + 1],
                                Want::Callable,
                            ));
                        }
                    }
                }
                return Res::NotFound;
            }

            // Final segment: a child module (preferred for imports) or a
            // member item of the current module. Only a lookup that admits a
            // module takes the child — never a call's, nor a type relation's.
            if want.admits(NodeKind::Module) {
                let mut child = key.1.clone();
                child.push(seg.clone());
                if let Some(&m) = self.ix.modules.get(&(key.0.clone(), child)) {
                    return Res::Found(m);
                }
            }
            if want == Want::Module {
                return Res::NotFound;
            }
            let Some(&scope_node) = self.ix.modules.get(&key) else {
                return Res::NotFound;
            };
            return exactly_one(&self.ix.members_named(scope_node, seg, want));
        }
        Res::NotFound
    }

    /// Workspace fallback for a multi-segment path (balanced+): the final
    /// segment's name matches and the module the node sits in ends with the
    /// leading segments — crate-first, then workspace, exactly-one at each
    /// step ([FR-RS-03] crate → workspace levels).
    ///
    /// A **module** candidate sits in its parent: its own key ends in its own
    /// name, so `pkg::mod` reaches the module `[…, pkg, mod]` through its parent
    /// key `[…, pkg]` (S-519, [FR-RS-14]). Compared against its own key, a
    /// module could only ever match a path that spelled its name twice.
    ///
    /// [FR-RS-14]: ../../../docs/specs/requirements/FR-RS-14.md
    fn suffix_match(&self, segs: &[String], want: Want) -> Res {
        let (prefix, last) = segs.split_at(segs.len() - 1);
        let Some(all) = self.ix.by_name.get(&last[0]) else {
            return Res::NotFound;
        };
        let matches_suffix = |id: &NodeId| -> bool {
            let Some(info) = self.ix.info.get(id) else {
                return false;
            };
            if !want.admits(info.kind) {
                return false;
            }
            match self.ix.module_key.get(id) {
                // A module: its parent's key must end with the prefix; a crate
                // root has no parent to match.
                Some((_, own)) => own
                    .split_last()
                    .is_some_and(|(_, parent)| parent.ends_with(prefix)),
                // Any other node: its enclosing module's key must.
                None => self
                    .ix
                    .nearest_module(*id)
                    .is_some_and(|(_, mods)| mods.ends_with(prefix)),
            }
        };
        let candidates: Vec<NodeId> = all.iter().copied().filter(matches_suffix).collect();
        self.prefer_crate(&candidates)
    }

    /// Workspace unique-name fallback (method calls at balanced+, bare names
    /// at aggressive): crate-first, then workspace, exactly-one at each step.
    fn unique_by_name(&self, name: &str, want: Want) -> Res {
        let Some(all) = self.ix.by_name.get(name) else {
            return Res::NotFound;
        };
        let candidates: Vec<NodeId> = all
            .iter()
            .copied()
            .filter(|id| self.ix.info.get(id).is_some_and(|i| want.admits(i.kind)))
            .collect();
        self.prefer_crate(&candidates)
    }

    /// The crate → workspace acceptance step shared by every workspace
    /// fallback ([FR-RS-03] hierarchy order): exactly-one among the source
    /// crate's candidates wins; a crate-level ambiguity is final; only an
    /// empty crate set escalates to the workspace-wide exactly-one test.
    ///
    /// [FR-RS-03]: ../../../docs/specs/requirements/FR-RS-03.md
    fn prefer_crate(&self, candidates: &[NodeId]) -> Res {
        let source_crate = self
            .ix
            .info
            .get(&self.source)
            .map(|i| i.crate_name.as_str())
            .unwrap_or_default();
        let in_crate: Vec<NodeId> = candidates
            .iter()
            .copied()
            .filter(|id| {
                self.ix
                    .info
                    .get(id)
                    .is_some_and(|i| i.crate_name == source_crate)
            })
            .collect();
        match exactly_one(&in_crate) {
            // An import-root family's crate is a closed namespace (S-519): its
            // fallback never escalates to another language's modules.
            Res::NotFound if self.ix.layout.is_family_crate(source_crate) => Res::NotFound,
            Res::NotFound => exactly_one(candidates),
            decided => decided,
        }
    }

    /// Resolve a documentation link/path reference to its target node (S-035,
    /// [FR-DG-03]/[FR-DG-04]).
    ///
    /// The href is normalised relative to the source doc's path. With a
    /// `#anchor`, the target is the one [`NodeKind::DocSection`] in that file
    /// whose [`heading_slug`] matches; without one, the [`NodeKind::DocFile`] at
    /// that path, else the file-root module of a code file. Every branch ends in
    /// [`exactly_one`] — a missing or ambiguous target stays unresolved
    /// ([NFR-RA-05]).
    ///
    /// [FR-DG-03]: ../../../docs/specs/requirements/FR-DG-03.md
    /// [FR-DG-04]: ../../../docs/specs/requirements/FR-DG-04.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_doc_link(&self, target: &str) -> Res {
        let Some(source_file) = self
            .ix
            .info
            .get(&self.source)
            .and_then(|i| i.file_path.as_deref())
        else {
            return Res::NotFound;
        };
        let (path_part, anchor) = split_anchor(target);

        // A bare `#anchor` targets the source file itself.
        if path_part.is_empty() {
            return self.resolve_doc_at_path(source_file, anchor.as_deref());
        }

        // Try two interpretations of the path, in order, falling through only on
        // a clean miss (never on a known ambiguity — that would fabricate):
        //   1. relative to the source doc's directory — markdown-link semantics;
        //   2. relative to the repository root — explicit repo-file-path
        //      semantics (FR-DG-04), e.g. an inline-code `crate/src/x.rs`.
        // A leading `/` already forces root in both, so they coincide there.
        let source_rel = fold_path(&dir_segments(source_file), path_part);
        let root_rel = fold_path(&[], path_part);
        for path in [source_rel, root_rel].into_iter().flatten() {
            match self.resolve_doc_at_path(&path, anchor.as_deref()) {
                Res::NotFound => {}
                decided => return decided,
            }
        }
        Res::NotFound
    }

    /// Resolve an already-normalised doc target `path` (+ optional `anchor`) to a
    /// node: with an anchor, the one [`NodeKind::DocSection`] in that file whose
    /// [`heading_slug`] matches; without one, the [`NodeKind::DocFile`] there,
    /// else the file-root module of a code file. Always exactly-one-or-nothing.
    fn resolve_doc_at_path(&self, path: &str, anchor: Option<&str>) -> Res {
        let in_file = self.ix.by_file_path.get(path);

        if let Some(anchor) = anchor {
            // Re-slugify the anchor so a link and its heading agree on casing and
            // punctuation; well-formed anchors are already slugs (idempotent).
            let want = heading_slug(anchor);
            let Some(ids) = in_file else {
                return Res::NotFound;
            };
            let candidates: Vec<NodeId> = ids
                .iter()
                .copied()
                .filter(|id| {
                    self.ix.info.get(id).is_some_and(|i| {
                        is_doc_section_kind(i.kind) && heading_slug(&i.name) == want
                    })
                })
                .collect();
            return exactly_one(&candidates);
        }

        // No anchor: a DocFile at that path (a doc target) wins; otherwise the
        // file-root module of a code file the link points at.
        if let Some(ids) = in_file {
            let doc_files: Vec<NodeId> = ids
                .iter()
                .copied()
                .filter(|id| {
                    self.ix
                        .info
                        .get(id)
                        .is_some_and(|i| is_doc_file_kind(i.kind))
                })
                .collect();
            if !doc_files.is_empty() {
                return exactly_one(&doc_files);
            }
        }
        // A package-shaped file (CR-149) is found by its path, never by its
        // module key: a `src/main` and a `src/test` file of one package and
        // name share a key, and the tree keeps only the first.
        if self.ix.layout.is_package_shaped(path) {
            let roots: Vec<NodeId> = in_file
                .into_iter()
                .flatten()
                .copied()
                .filter(|id| {
                    !self.ix.parent.contains_key(id)
                        && self.ix.info.get(id).is_some_and(|i| i.kind == NodeKind::Module)
                })
                .collect();
            return exactly_one(&roots);
        }
        match self.ix.modules.get(&self.ix.layout.module_key(path)) {
            Some(&m) => Res::Found(m),
            None => Res::NotFound,
        }
    }

    /// Resolve a documentation code-name token to the one *code* symbol of that
    /// name in the workspace (S-035, [FR-DG-04]).
    ///
    /// Documentation nodes are excluded so a token binds to code, never to
    /// another doc; the single [`exactly_one`] gate keeps the never-fabricate
    /// invariant — a name shared by two symbols, or by none, stays unresolved
    /// ([NFR-RA-05]).
    ///
    /// [FR-DG-04]: ../../../docs/specs/requirements/FR-DG-04.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_doc_code_name(&self, name: &str) -> Res {
        let Some(all) = self.ix.by_name.get(name) else {
            return Res::NotFound;
        };
        let candidates: Vec<NodeId> = all
            .iter()
            .copied()
            .filter(|id| self.ix.info.get(id).is_some_and(|i| !is_doc_kind(i.kind)))
            .collect();
        exactly_one(&candidates)
    }

    /// Turn a resolved documentation reference into an edge, elevating it to a
    /// typed trace when it connects two swe-skills artifacts (S-039, [FR-DG-07]).
    ///
    /// A hyperlink expresses a *trace* when the node it lives in and the node it
    /// resolves to are each owned by a typed artifact: [`typed_owner`] walks each
    /// endpoint up the `Contains` hierarchy to its nearest enclosing
    /// `Requirement`/`Adr`/`Story` (or itself). So a link in a `Requirement`
    /// file's `## Dependencies` section traces from the *requirement*, and a
    /// `Story` section's link to a requirement traces the "implements" relation —
    /// both as [`EdgeKind::TracesTo`] between the typed owners. A link touching a
    /// generic `DocFile`/`DocSection` or a code symbol (neither has a typed owner)
    /// stays a plain [`EdgeKind::DocReference`] from the resolved endpoints, and a
    /// link within a single artifact (`a == b`, an intra-file anchor) is not a
    /// self-trace.
    ///
    /// Re-typing never fabricates: the reference was already bound through the
    /// exactly-one-candidate ledger ([NFR-RA-05]); this only labels and elevates
    /// the edge the binder proved.
    ///
    /// [`typed_owner`]: Ctx::typed_owner
    /// [FR-DG-07]: ../../../docs/specs/requirements/FR-DG-07.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn bind_doc_ref(&self, source: NodeId, target: NodeId) -> Outcome {
        if let (Some(a), Some(b)) = (self.typed_owner(source), self.typed_owner(target)) {
            if a != b {
                return Outcome::Bound {
                    source: a,
                    target: b,
                    kind: EdgeKind::TracesTo,
                    payload: None,
                };
            }
        }
        Outcome::Bound {
            source,
            target,
            kind: EdgeKind::DocReference,
            payload: None,
        }
    }

    /// The nearest typed swe-skills artifact (`Requirement`/`Adr`/`Story`) that
    /// *owns* `id` — `id` itself if it is typed, else the nearest such ancestor
    /// reached by walking the `Contains` hierarchy upward, or `None` if none
    /// (a generic doc node, or a code symbol) (S-039, [FR-DG-07]).
    ///
    /// `Contains` is a tree, so the walk normally terminates at a typed node or
    /// the root. The hard [`MAX_CONTAINS_DEPTH`] cap is a defence-in-depth guard
    /// against a malformed cycle ever reaching the binder — mirroring the bounded
    /// [`MAX_ALIAS_DEPTH`] alias walk — so a corrupt edge degrades to "no typed
    /// owner" (a plain `DocReference`) instead of spinning forever.
    fn typed_owner(&self, mut id: NodeId) -> Option<NodeId> {
        for _ in 0..MAX_CONTAINS_DEPTH {
            if is_typed_doc_kind(self.ix.info.get(&id)?.kind) {
                return Some(id);
            }
            id = *self.ix.parent.get(&id)?;
        }
        None
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
}
