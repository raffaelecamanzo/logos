//! The arm a row takes, by its kind and form — the body of [`super::bind`],
//! against the context built for the row. Each arm hands the row to the rung
//! of its concern, under the one acceptance rule ([NFR-RA-05]).
//!
//! [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md

use std::ops::Deref;

use crate::graph_store::UnresolvedRefRow;
use crate::model::{EdgeKind, NodeId, ReceiverShape, RefForm};

use super::{
    is_proven_receiver_call, is_type_like, split, Ctx, Outcome, Res, Residue, Want, MAX_ALIAS_DEPTH,
};
use super::artifacts::Artifacts;
use super::call_reach::CallReach;
use super::doc_references::DocReferences;
use super::import_roots::ImportRootPaths;
use super::imported_scope::ImportedScope;
use super::includes::IncludeLookup;
use super::lexical_constants::LexicalConstants;
use super::local_binding::LocalBindings;
use super::package_paths::PackagePaths;
use super::proven_receivers::ProvenReceivers;
use super::receiver_lambdas::{LambdaRead, ReceiverLambdas};
use super::receiver_shapes::ReceiverShapes;
use super::scope_hierarchy::ScopeHierarchy;
use super::self_types::SelfTypes;
use super::specifiers::Specifiers;
use super::trait_dispatch::TraitDispatch;
use super::type_relations::TypeRelations;
use super::unbound_reasons::UnboundReasons;

/// The arm a row takes, by its kind and form.
#[derive(Clone, Copy)]
pub(super) struct RowArms<'c, 'a> {
    pub(super) cx: &'c Ctx<'a>,
}

impl<'a> Deref for RowArms<'_, 'a> {
    type Target = Ctx<'a>;

    fn deref(&self) -> &Ctx<'a> {
        self.cx
    }
}

impl RowArms<'_, '_> {
    /// The body of [`super::bind`], against the context built for `r`: the arm
    /// its kind and form take.
    pub(super) fn bind(&self, r: &UnresolvedRefRow, relation: Option<Want>) -> Outcome {
        let scope_hierarchy = ScopeHierarchy { cx: self.cx };
        let type_relations = TypeRelations { cx: self.cx };
        let receiver_shapes = ReceiverShapes { cx: self.cx };
        let specifiers = Specifiers { cx: self.cx };
        let artifacts = Artifacts { cx: self.cx };
        let package_paths = PackagePaths { cx: self.cx };
        let includes = IncludeLookup { cx: self.cx };
        let local_bindings = LocalBindings { cx: self.cx };
        let (source, ix) = (self.source, self.ix);
        let bound = |target: NodeId| Outcome::Bound {
            source,
            target,
            kind: r.kind,
            // Non-artifact binds carry no payload; an artifact bind routes through
            // `Artifacts::resolve_artifact` below, which stamps the relation class.
            payload: r.payload.clone(),
        };

        // A cross-artifact reference (CR-011, FR-CG-07): bind under the same
        // exactly-one-candidate discipline as code and docs, dispatched by
        // (kind, form). The relation class rides on `r.payload` onto the edge so
        // navigation can surface which relation a binding expresses; externals were
        // classified out before the ledger, so every ledger row here is a genuine
        // workspace-relative candidate ([NFR-RA-05], [ADR-26]).
        if r.kind.is_config_reference() {
            return artifacts.resolve_artifact(r);
        }

        // A member-access fact (CR-005, FR-EX-08): bind to a `Field` of the source
        // method's own class-like container, on an exactly-one candidate — never
        // through the scope hierarchy and never policy-widened, so an own-field
        // access can only ever bind to a field of its own type or stay unresolved
        // (NFR-RA-05). The `target` is the bare field name (the `Method` ref form).
        if r.kind == EdgeKind::Accesses {
            return match receiver_shapes.resolve_member_access(&r.target) {
                Res::Found(target) => bound(target),
                _ => Outcome::Unbound,
            };
        }


        // A type relation (S-466, CR-149 §3.2 B, FR-EX-10; S-522, FR-RS-15): any
        // relation from a package-shaped source, and in every language a type's
        // `Extends` or `Implements` ([`type_relation_want`]). Bound only to the one
        // in-repository type of a kind the relation admits, reached through the
        // source's scope and its module model — never a workspace guess — see
        // [`TypeRelations::resolve_type_relation`](super::type_relations::TypeRelations::resolve_type_relation). This is where the `Implements` bind is
        // widened beyond Rust `Trait` targets: a type's `Implements` binds an
        // `Interface` or a `Trait`, and never reaches the trait rule below. A
        // supertype whose syntax was silent takes its edge kind from the target.
        // A partial type part's own row binds here too ([`TypeRelations::bind_type_relation`](super::type_relations::TypeRelations::bind_type_relation)).
        if let Some(want) = relation {
            return type_relations.bind_type_relation(r, want);
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

        // A bare call a local binding in force shadows (S-594, [FR-RS-44]): a
        // local value binds nothing, a local import resolves through it alone —
        // first, before the include scope and the lexical walk, which would read
        // past it ([`LocalBindings::resolve_local_binding_call`](super::local_binding::LocalBindings::resolve_local_binding_call)).
        //
        // A row of an include-roots language (S-579, [FR-RS-38]): an `#include`,
        // a `using`, a Path-form call — bound through the including file's scopes
        // and the declarations its includes make visible, never through the module
        // tree or a workspace guess ([`IncludeLookup::resolve_include_row`](super::includes::IncludeLookup::resolve_include_row)). Then a
        // path-grammar re-export or export alias (S-534) — an `export * from 's'`
        // `Glob` row or an exported `Path` row — is decided by what it names
        // ([`Specifiers::resolve_export_row`](super::specifiers::Specifiers::resolve_export_row)), before either form's own arm. The two never
        // both answer: a C++ file records no TypeScript export.
        //
        // [FR-RS-38]: ../../../../docs/specs/requirements/FR-RS-38.md
        // [FR-RS-44]: ../../../../docs/specs/requirements/FR-RS-44.md
        // A bare call inside a lambda with a receiver (S-834, [FR-RS-31]) reads
        // that receiver first, then the bare call it was
        // ([`bind_receiver_lambda_call`](RowArms::bind_receiver_lambda_call)).
        //
        // [FR-RS-31]: ../../../../docs/specs/requirements/FR-RS-31.md
        if let Some(outcome) = local_bindings
            .resolve_local_binding_call(r)
            .or_else(|| self.bind_receiver_lambda_call(r))
            .or_else(|| includes.resolve_include_row(r))
            .or_else(|| specifiers.resolve_export_row(r))
        {
            return outcome;
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
                // ([`Wildcards::glob_members`](super::wildcards::Wildcards::glob_members)), which is what the row is for.
                //
                // A declared-namespace file's wildcard naming no type names a
                // namespace (S-518): it binds to every other file declaring it
                // ([`PackagePaths::namespace_files`](super::package_paths::PackagePaths::namespace_files)), as a Go import path binds to its
                // directory's files. Java's package wildcard binds nothing still.
                if self.source_package.is_some() {
                    return match package_paths.resolve_glob_path(&segs) {
                        Res::Found(t) if ix.info.get(&t).is_some_and(|i| is_type_like(i.kind)) => {
                            bound(t)
                        }
                        Res::NotFound => package_paths.namespace_files(&segs).unwrap_or(Outcome::Unbound),
                        _ => Outcome::Unbound,
                    };
                }
                match scope_hierarchy.resolve_path(&segs, Want::Module, MAX_ALIAS_DEPTH) {
                    Res::Found(target) => bound(target),
                    _ => Outcome::Unbound,
                }
            }
            RefForm::Path => self.bind_path(r, bound),
            RefForm::Method => self.bind_method(r, bound),
        }
    }

    /// A bare call inside a lambda with a receiver (S-834): the rung reads the
    /// receiver first ([`ReceiverLambdas::read_receiver`](super::receiver_lambdas::ReceiverLambdas::read_receiver)); when that decides
    /// nothing, this arm binds the bare call the row was, its mark dropped, and
    /// the rung judges what that bound ([`ReceiverLambdas::keep_bare_call`](super::receiver_lambdas::ReceiverLambdas::keep_bare_call)).
    /// The dispatcher binds, never the rung: a rung reaches the kernel only.
    /// `None` for an unmarked row.
    fn bind_receiver_lambda_call(&self, r: &UnresolvedRefRow) -> Option<Outcome> {
        let receiver_lambdas = ReceiverLambdas { cx: self.cx };
        match receiver_lambdas.read_receiver(r)? {
            LambdaRead::Decided(outcome) => Some(outcome),
            LambdaRead::BareCall { answered } => {
                let outcome = self.bind(&UnresolvedRefRow { alias: None, ..r.clone() }, None);
                Some(receiver_lambdas.keep_bare_call(r, answered, outcome))
            }
        }
    }

    /// [`bind`](RowArms::bind)'s arm for a `Path`-form row.
    fn bind_path(&self, r: &UnresolvedRefRow, bound: impl Fn(NodeId) -> Outcome) -> Outcome {
        let unbound_reasons = UnboundReasons { cx: self.cx };
        let scope_hierarchy = ScopeHierarchy { cx: self.cx };
        let import_roots = ImportRootPaths { cx: self.cx };
        let self_types = SelfTypes { cx: self.cx };
        let proven_receivers = ProvenReceivers { cx: self.cx };
        let call_reach = CallReach { cx: self.cx };
        let specifiers = Specifiers { cx: self.cx };
        let imported_scope = ImportedScope { cx: self.cx };
        let constants = LexicalConstants { cx: self.cx };
        let doc_references = DocReferences { cx: self.cx };
        let source = self.source;
        // A documentation link/path (S-035): resolve the href (path +
        // optional `#anchor`) against the doc/code file at that path, never
        // through the code scope hierarchy ([FR-DG-03], [FR-DG-04]).
        if r.kind == EdgeKind::DocReference {
            return match doc_references.resolve_doc_link(&r.target) {
                Res::Found(target) => doc_references.bind_doc_ref(source, target),
                _ => Outcome::Unbound,
            };
        }
        // A path-grammar module specifier (S-439, [CR-142] D1): a relative
        // specifier binds against the importing file's directory, a Go import
        // path against the module that declares it — never through the
        // member-path scope hierarchy, which reads `a::b` as names.
        if r.kind == EdgeKind::Imports {
            if let Some(outcome) = specifiers.resolve_specifier(&r.target) {
                return outcome;
            }
        }
        // A constant of a lexical-constant language (S-565, [FR-RS-33]):
        // a call on it (`Foo::bar`, never a single segment — a bare call
        // outside a class is the free call it always was), the class
        // `.new` instantiates, and the scope a compact declaration names
        // bind by the constant rung alone — never through the module tree,
        // whose keys are paths, nor a workspace guess
        // ([`LexicalConstants::resolve_constant_row`](super::lexical_constants::LexicalConstants::resolve_constant_row)). A superclass written as a path
        // (`< ActiveRecord::Base`, a framework fingerprint) reaches the rung
        // as a call too: there it could bind only a `def self.Base` of
        // `ActiveRecord`, never the class `Base` itself.
        //
        // [FR-RS-33]: ../../../../docs/specs/requirements/FR-RS-33.md
        let on_constant = match r.kind {
            EdgeKind::Calls => r.target.contains("::"),
            EdgeKind::Instantiates | EdgeKind::TypeUses => true,
            _ => false,
        };
        if self.lexical_constants && on_constant {
            return match constants.resolve_constant_row(r) {
                Res::Found(target) => bound(target),
                _ => Outcome::Unbound,
            };
        }
        // A Rust call on a receiver whose declared type the file proves
        // (S-587, [FR-RS-42]): `x.m()` retyped to `T::m`, its `other` shape
        // kept. It is not a written `T::m()` — it is method syntax — so it
        // binds through the one lookup with method syntax (S-607,
        // [`ProvenReceivers::resolve_proven_receiver_call`](super::proven_receivers::ProvenReceivers::resolve_proven_receiver_call)), never through a scope. A
        // TypeScript one (S-537, [FR-RS-21]) binds among its type's members
        // ([`ProvenReceivers::resolve_structural_receiver_call`](super::proven_receivers::ProvenReceivers::resolve_structural_receiver_call)), a Python one
        // (S-546, [FR-RS-24]) among its class's methods
        // ([`ProvenReceivers::resolve_import_root_receiver_call`](super::proven_receivers::ProvenReceivers::resolve_import_root_receiver_call)), and a Kotlin one
        // (S-563, [FR-RS-32]) or a Scala one (S-576) among its type's own
        // members ([`ProvenReceivers::resolve_proven_receiver_call`](super::proven_receivers::ProvenReceivers::resolve_proven_receiver_call)).
        //
        // [FR-RS-42]: ../../../../docs/specs/requirements/FR-RS-42.md
        // [FR-RS-21]: ../../../../docs/specs/requirements/FR-RS-21.md
        // [FR-RS-24]: ../../../../docs/specs/requirements/FR-RS-24.md
        // [FR-RS-32]: ../../../../docs/specs/requirements/FR-RS-32.md
        if is_proven_receiver_call(r) {
            return match proven_receivers.resolve_proven_receiver_call(r) {
                Res::Found(target) => bound(target),
                _ => Outcome::Unbound,
            };
        }
        // A call through the caller's own type (S-493, [FR-RS-11]):
        // `self.m()` / `Self::m()` binds only among that type's methods,
        // never through the scope hierarchy — through the one lookup in an
        // impl-block language (S-607, [`SelfTypes::resolve_self_path_call`](super::self_types::SelfTypes::resolve_self_path_call)),
        // else among the methods recorded with the caller's self type
        // ([`SelfTypes::resolve_recorded_self_type_call`](super::self_types::SelfTypes::resolve_recorded_self_type_call), Go). A qualified
        // `<T as Tr>::m()` binds through the one lookup too (S-608,
        // [`SelfTypes::resolve_qualified_call`](super::self_types::SelfTypes::resolve_qualified_call)).
        if r.kind == EdgeKind::Calls {
            let resolved = if self.impl_blocks {
                self_types.resolve_qualified_call(&r.target).or_else(|| self_types.resolve_self_path_call(r))
            } else {
                self_types.resolve_recorded_self_type_call(&r.target)
            };
            if let Some(resolved) = resolved {
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
            if let Some(resolved) = imported_scope.resolve_imported_call(r) {
                return match resolved {
                    Res::Found(target) => bound(target),
                    _ => Outcome::Unbound,
                };
            }
        }
        // A call through an import-root file's imported module or class
        // (S-544, [FR-RS-23]): bound within what the qualifier's import
        // names, never through the module tree's `Type::func` collapse nor
        // a workspace guess. A class it binds is constructed.
        //
        // [FR-RS-23]: ../../../../docs/specs/requirements/FR-RS-23.md
        if let Some(outcome) = import_roots.resolve_module_qualified_call(r) {
            return outcome;
        }
        let want = if r.kind == EdgeKind::Calls {
            self.call_want()
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
        let bare = r.kind == EdgeKind::Calls && segs.len() == 1;
        self.bare_path_call.set(bare);
        let resolved = scope_hierarchy.resolve_path(&segs, want, MAX_ALIAS_DEPTH);
        self.bare_path_call.set(false);
        match resolved {
            // A declaration a bare call cannot reach, whichever rung found
            // it ([`CallReach::bare_call_cannot_reach`](super::call_reach::CallReach::bare_call_cannot_reach)).
            Res::Found(target) if bare && call_reach.bare_call_cannot_reach(target, want) => Outcome::Unbound,
            // A class a declared call constructs (S-521, [FR-RS-16]) —
            // unless a factory function of its name rivals it.
            Res::Found(target) if self.constructs(want, target) => {
                if unbound_reasons.rival_function(target) {
                    self.note(want, || Residue::TypeAmbiguous);
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
            Res::NotFound if r.kind == EdgeKind::Imports => import_roots
                .package_reexport(&segs)
                .map_or(Outcome::Unbound, bound),
            // Some rung held callables of the name, and none admitted the
            // call's arguments (S-592) — unless a rung that decided first
            // recorded its own reason.
            Res::NotFound if self.passed_over.get() => {
                self.note(want, || Residue::NoApplicableOverload);
                Outcome::Unbound
            }
            _ => Outcome::Unbound,
        }
    }

    /// [`bind`](RowArms::bind)'s arm for a `Method`-form row: a receiver call, a doc code-name token.
    fn bind_method(&self, r: &UnresolvedRefRow, bound: impl Fn(NodeId) -> Outcome) -> Outcome {
        let receiver_shapes = ReceiverShapes { cx: self.cx };
        let trait_dispatch = TraitDispatch { cx: self.cx };
        let doc_references = DocReferences { cx: self.cx };
        let source = self.source;
        // A documentation code-name token (S-035): bind to the one code
        // symbol of that name workspace-wide — never on zero or several
        // ([FR-DG-04], [NFR-RA-05]). Policy-independent: doc→code is always
        // exactly-one-or-nothing.
        if r.kind == EdgeKind::DocReference {
            return match doc_references.resolve_doc_code_name(&r.target) {
                Res::Found(target) => doc_references.bind_doc_ref(source, target),
                _ => Outcome::Unbound,
            };
        }
        // A trait-typed receiver call (S-281, CR-073, FR-RS-08; S-608):
        // extraction encodes a *provable* `&dyn T`, `impl T` or `T`-bounded
        // receiver on `p.f()` as a trait-qualified `T::f` target (a bare
        // `.f()` stays a single segment, never reaches here, and binds by its
        // receiver's shape below, [FR-RS-12]). Fan out to the SET of that
        // trait method's targets ([`TraitDispatch::fan_out`](super::trait_dispatch::TraitDispatch::fan_out)): every concrete
        // workspace impl of it, and the trait's own default body unless
        // every impl overrides it. Every target is a real indexed node
        // reached through the *proven* trait `T` — never a same-named method
        // on an unrelated type, never a free function, never a
        // stdlib/external target ([NFR-RA-05]). A trait that is external or
        // ambiguous, or one with neither a default body nor a workspace
        // impl, yields no target and stays an honest miss.
        if r.kind == EdgeKind::Calls && r.target.contains("::") {
            return match trait_dispatch.resolve_dyn_dispatch(&r.target) {
                Res::Found(target) => bound(target),
                _ => Outcome::Unbound,
            };
        }
        // A receiver call (`x.f()` → `f`): extraction records the bare
        // method name and its receiver's SHAPE, never its type ([FR-EX-13];
        // a typed receiver is a `RefForm::Path` row bound above — a Rust
        // one among its proven type's methods, S-588, a TypeScript one
        // among its type's members, S-537, a Python one among its class's
        // methods, S-546). The shape decides
        // where `f` may be found ([FR-RS-12]):
        //
        // - `self` — among the caller's own class's members, then up its
        //   proven `Extends` chain; for a function of an `impl` block,
        //   through its header's type by the one lookup (S-607), and for a
        //   Go method through its recorded self type
        //   ([`ReceiverShapes::resolve_self_receiver`](super::receiver_shapes::ReceiverShapes::resolve_self_receiver));
        // - `super` — only up that chain, never the caller's own class
        //   ([`ReceiverShapes::resolve_super_receiver`](super::receiver_shapes::ReceiverShapes::resolve_super_receiver));
        // - `other`, or no shape at all — nowhere. The caller's lexical,
        //   class or module scope says where the CALLER is, not what the
        //   receiver is: the scope walk this replaces started at the caller
        //   itself, so `other.m()` inside `m` bound to `m` ([CR-169],
        //   correcting [CR-066]'s "same-scope evidence"). The row stays in
        //   `unresolved_refs` as `no-receiver-evidence` and retries on sync
        //   ([FR-RS-03], [FR-RS-06], [NFR-RA-05]) — unless its language
        //   declares extension functions, and exactly one of the name is
        //   in scope ([`ExtensionCalls::resolve_extension_call`](super::extension_calls::ExtensionCalls::resolve_extension_call), S-560, [FR-RS-31]).
        //
        // [FR-EX-13]: ../../../../docs/specs/requirements/FR-EX-13.md
        // [FR-RS-06]: ../../../../docs/specs/requirements/FR-RS-06.md
        // [FR-RS-12]: ../../../../docs/specs/requirements/FR-RS-12.md
        // [FR-RS-31]: ../../../../docs/specs/requirements/FR-RS-31.md
        // [CR-066]: ../../../../docs/requests/CR-066-receiver-method-overbinding.md
        // [CR-169]: ../../../../docs/requests/CR-169-a-call-on-another-object-never-binds-to-the-callers-own-method.md
        let resolved = match r.receiver {
            Some(ReceiverShape::SelfInstance) => receiver_shapes.resolve_self_receiver(&r.target),
            Some(ReceiverShape::Super) => receiver_shapes.resolve_super_receiver(&r.target),
            Some(ReceiverShape::Other) | None => receiver_shapes.resolve_other_receiver(&r.target),
        };
        match resolved {
            Res::Found(target) => bound(target),
            _ => Outcome::Unbound,
        }
    }
}
