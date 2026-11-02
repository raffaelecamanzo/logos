//! A call through the caller's own type (S-493, S-509, S-607, S-608,
//! [FR-RS-11], [FR-RS-47]): `Self::m`, `self.m()` and a qualified
//! `<T as Tr>::m()` through the one associated-item lookup, or among the
//! methods recorded with the caller's self type in a language with no `impl`
//! block; and whether a trait's items may be called where the call is
//! ([`SelfTypes::trait_in_scope`]).
//!
//! [FR-RS-11]: ../../../../docs/specs/requirements/FR-RS-11.md
//! [FR-RS-47]: ../../../../docs/specs/requirements/FR-RS-47.md

use std::ops::Deref;

use crate::graph_store::UnresolvedRefRow;
use crate::model::{NodeId, NodeKind, ReceiverShape};

use super::module_index::ModuleIndex;
use super::{
    exactly_one, split, Ctx, Index, ModKey, Named, Res, Residue, Syntax, Want,
    MAX_ALIAS_DEPTH, SELF_TYPE_HEAD,
};
use super::impl_index::ImplHeader;
use super::proven_receivers::ProvenReceivers;
use super::rust_globs::RustGlobs;
use super::trait_dispatch::TraitDispatch;

/// A call through the caller's own type, and the traits whose items it may call.
#[derive(Clone, Copy)]
pub(super) struct SelfTypes<'c, 'a> {
    pub(super) cx: &'c Ctx<'a>,
}

impl<'a> Deref for SelfTypes<'_, 'a> {
    type Target = Ctx<'a>;

    fn deref(&self) -> &Ctx<'a> {
        self.cx
    }
}

impl SelfTypes<'_, '_> {
    /// Resolve `Self::m` — a call through the caller's own receiver in a
    /// language that records a method's receiver type **name** but no `impl`
    /// block (Go's `s.F()`, S-509; [FR-RS-11]) — among the callables recorded
    /// with the caller's self type in the caller's crate
    /// ([`DeclarationFacts::methods_by_self_type`](super::declaration_facts::DeclarationFacts::methods_by_self_type)), or `None` when this is no such call:
    /// a target of any other shape, or a caller with no recorded self type,
    /// whose row takes the path it always took. An impl-block language (Rust)
    /// never reaches here: its calls bind through [`lookup`](Ctx::lookup)
    /// (S-607).
    ///
    /// The one acceptance rule ([`exactly_one`], [NFR-RA-05]), on two rungs.
    ///
    /// 1. **The crate** — when the crate declares exactly one type of the self
    ///    type's name, every candidate is a method of the caller's type: exactly
    ///    one binds.
    /// 2. **The caller's module** — where the receiver's type name denotes the
    ///    caller's own type. Taken when the crate declares that name for several
    ///    types, and when rung 1 found several: exactly one module-local
    ///    candidate binds.
    ///
    /// A self type the crate declares no type of, or one the caller's file
    /// imports from outside the crate, binds nothing: [`Residue::ExternalType`].
    /// Zero candidates is [`Residue::SupertypeUnreached`]; candidates the
    /// module does not narrow to one are [`Residue::OverloadAmbiguous`] under a
    /// type name the crate declares once, [`Residue::TypeAmbiguous`] otherwise.
    /// Either stays unbound and never falls through to a wider scope.
    ///
    /// [FR-RS-11]: ../../../../docs/specs/requirements/FR-RS-11.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    pub(super) fn resolve_recorded_self_type_call(&self, target: &str) -> Option<Res> {
        let ix: &Index = self.ix;
        let modules: &ModuleIndex = &ix.modules;
        let (head, name) = target.split_once("::")?;
        if head != SELF_TYPE_HEAD || name.is_empty() || name.contains("::") {
            return None;
        }
        let self_type = ix.facts.self_types.get(&self.source)?;
        let caller = ix.info.get(&self.source)?;
        let krate = &caller.crate_name;
        let key = (krate.clone(), self_type.clone(), name.to_string());
        let candidates = ix
            .facts.methods_by_self_type
            .get(&key)
            .map_or(&[][..], Vec::as_slice);
        let declared = ix.facts.type_names.get(&(krate.clone(), self_type.clone()));
        if declared.is_none() || modules.imports_foreign(caller, self_type) {
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
            _ => {
                let own = ix.nearest_module(self.source);
                let local: Vec<NodeId> = candidates
                    .iter()
                    .copied()
                    .filter(|&c| own.is_some() && ix.nearest_module(c) == own)
                    .collect();
                match exactly_one(&local) {
                    found @ Res::Found(_) => found,
                    _ => {
                        self.note(Want::Callable, || {
                            if one_type {
                                Residue::OverloadAmbiguous
                            } else {
                                Residue::TypeAmbiguous
                            }
                        });
                        Res::Ambiguous
                    }
                }
            }
        };
        Some(res)
    }

    /// Resolve a qualified `<T as Tr>::m` in an impl-block language (S-608,
    /// [FR-RS-47] rule 5) — the target S-606 records for the call — as
    /// `lookup(T, m)` with path syntax, restricted to `T`'s impl of `Tr`, else
    /// `Tr`'s default body ([`Named`]). `T` and `Tr` are read in the caller's
    /// scope and module model only, as an impl header is
    /// ([`header_type`](SelfTypes::header_type)); `Self` is the caller's own impl's
    /// self type, and in a trait's default body the call fans out through `Tr`
    /// ([`trait_call`](super::trait_dispatch::TraitDispatch::trait_call)). A `T` the repository does not
    /// declare — a generic parameter, `std::io::Error` — is
    /// [`Residue::ExternalType`]; one the scope names twice
    /// [`Residue::TypeAmbiguous`]. A `Tr` the graph cannot place names the
    /// unplaced trait impls of `T` by its simple name. `None` for a target
    /// that is no qualified path.
    ///
    /// [FR-RS-47]: ../../../../docs/specs/requirements/FR-RS-47.md
    pub(super) fn resolve_qualified_call(&self, target: &str) -> Option<Res> {
        let qualified = target.strip_prefix('<')?;
        let scoped = self.scope_only.replace(true);
        let res = self.qualified_call(qualified);
        self.scope_only.set(scoped);
        Some(res)
    }

    /// [`resolve_qualified_call`](SelfTypes::resolve_qualified_call) past its `<`.
    fn qualified_call(&self, qualified: &str) -> Res {
        let trait_dispatch = TraitDispatch { cx: self.cx };
        let parts = qualified
            .rsplit_once(">::")
            .and_then(|(inner, name)| Some((inner.rsplit_once(" as ")?, name)))
            .filter(|(_, name)| !name.is_empty() && !name.contains("::"));
        let Some(((ty, tr), name)) = parts else {
            self.note(Want::Callable, || Residue::ExternalType {
                candidates: Vec::new(),
            });
            return Res::NotFound;
        };
        let named = match self.header_type(tr) {
            Res::Found(id) if self.ix.info.get(&id).is_some_and(|i| i.kind == NodeKind::Trait) => Named::Placed(id),
            Res::NotFound => Named::Unplaced(tr.rsplit("::").next().unwrap_or(tr)),
            Res::Found(_) => {
                self.note(Want::Callable, || Residue::SupertypeUnreached);
                return Res::NotFound;
            }
            Res::Ambiguous => {
                self.note(Want::Callable, || Residue::TypeAmbiguous);
                return Res::Ambiguous;
            }
        };
        // `<Self as Tr>`: the caller's own impl's self type, or — in a trait's
        // default body, whose `Self` is any implementor — a call through `Tr`.
        let ty = match (ty == SELF_TYPE_HEAD, self.own_header(), self.own_trait(), named) {
            (false, ..) => self.header_type(ty),
            (true, Some(header), ..) => header.self_type,
            (true, None, Some(_), Named::Placed(tr)) => return trait_dispatch.trait_call(tr, name),
            (true, ..) => Res::NotFound,
        };
        let ty = match ty {
            Res::Found(ty) => ty,
            Res::Ambiguous => {
                self.note(Want::Callable, || Residue::TypeAmbiguous);
                return Res::Ambiguous;
            }
            Res::NotFound => {
                self.note(Want::Callable, || Residue::ExternalType {
                    candidates: Vec::new(),
                });
                return Res::NotFound;
            }
        };
        self.lookup(ty, name, Syntax::Path, Some(named))
    }

    /// Resolve `Self::m` in an impl-block language (S-607, [FR-RS-47]) — a
    /// written `Self::m()`, path syntax, or the `Self::m` row extraction
    /// records for a `self.m()`, which carries receiver shape `self` (S-606)
    /// and is method syntax — through [`self_type_call`](SelfTypes::self_type_call).
    /// `None` for a target of any other shape.
    ///
    /// [FR-RS-47]: ../../../../docs/specs/requirements/FR-RS-47.md
    pub(super) fn resolve_self_path_call(&self, r: &UnresolvedRefRow) -> Option<Res> {
        let (head, name) = r.target.split_once("::")?;
        if head != SELF_TYPE_HEAD || name.is_empty() || name.contains("::") {
            return None;
        }
        let syntax = if r.receiver == Some(ReceiverShape::SelfInstance) {
            Syntax::Method
        } else {
            Syntax::Path
        };
        Some(self.self_type_call(name, syntax))
    }

    /// The header of the `impl` block the source is a function of (S-607), if
    /// any.
    pub(super) fn own_header(&self) -> Option<&ImplHeader> {
        self.ix.impls.assoc_owner.get(&self.source).map(|&h| &self.ix.impls.impl_headers[h])
    }

    /// The trait the source is a default body of (S-608), if any: a function
    /// whose `Contains` parent is a trait.
    pub(super) fn own_trait(&self) -> Option<NodeId> {
        let parent = *self.ix.parent.get(&self.source)?;
        self.ix.info.get(&parent).is_some_and(|i| i.kind == NodeKind::Trait).then_some(parent)
    }

    /// `name` called through the caller's own type, `T` being the self type of
    /// the `impl` block the caller is a function of ([`lookup`](Ctx::lookup)
    /// with `syntax`; S-607, [FR-RS-11] as amended by [FR-RS-47]) — in any
    /// crate, inherent before trait in any module.
    ///
    /// A caller that is a trait's default body is typed by that trait: the
    /// call fans out to [FR-RS-08]'s set ([`trait_call`](super::trait_dispatch::TraitDispatch::trait_call),
    /// S-608, [FR-RS-47] rule 5). A caller in no impl block or trait — a free
    /// function — proves no type: [`Residue::NoReceiverEvidence`]. A header
    /// naming no repository type (`impl Tr for Vec<X>`, `for u8`, a generic
    /// parameter) is [`Residue::ExternalType`]: the type's own methods are not
    /// in the graph. One its scope names twice is [`Residue::TypeAmbiguous`].
    ///
    /// [FR-RS-08]: ../../../../docs/specs/requirements/FR-RS-08.md
    /// [FR-RS-11]: ../../../../docs/specs/requirements/FR-RS-11.md
    /// [FR-RS-47]: ../../../../docs/specs/requirements/FR-RS-47.md
    pub(super) fn self_type_call(&self, name: &str, syntax: Syntax) -> Res {
        let trait_dispatch = TraitDispatch { cx: self.cx };
        let Some(header) = self.own_header() else {
            if let Some(tr) = self.own_trait() {
                return trait_dispatch.trait_call(tr, name);
            }
            self.note(Want::Callable, || Residue::NoReceiverEvidence);
            return Res::NotFound;
        };
        match header.self_type {
            Res::Found(ty) => self.lookup(ty, name, syntax, None),
            Res::Ambiguous => {
                self.note(Want::Callable, || Residue::TypeAmbiguous);
                Res::Ambiguous
            }
            Res::NotFound => {
                self.note(Want::Callable, || Residue::ExternalType {
                    candidates: Vec::new(),
                });
                Res::NotFound
            }
        }
    }

    /// Whether a function of the `impl` block `header` — or a default its
    /// trait lends through it (S-608) — may be called here (S-607,
    /// [FR-RS-47]): an inherent block's, and one of a trait the graph cannot
    /// place (an external trait — `Default`, `Display` — which the prelude or
    /// an import may bring), always may. A repository trait is in scope when
    /// it is the trait of the caller's own impl (CRA-02), the caller's module
    /// declares it, or the caller's file imports it — by name, renamed,
    /// `as _`, or through a glob, following `pub use` re-exports
    /// ([`imports_trait`](SelfTypes::imports_trait)).
    ///
    /// [FR-RS-47]: ../../../../docs/specs/requirements/FR-RS-47.md
    pub(super) fn trait_in_scope(&self, header: &ImplHeader) -> bool {
        let ix: &Index = self.ix;
        let Some(tr) = header.trait_node else {
            return true;
        };
        self.own_header().is_some_and(|own| own.trait_node == Some(tr))
            || ix.nearest_module(tr).is_some_and(|m| Some(m) == ix.nearest_module(self.source))
            || self.imports_trait(tr)
    }

    /// Whether the source's file brings the trait `tr` into scope (S-607): a
    /// `use` whose path ends in its name and resolves to it (`use a::Tr`,
    /// `use a::Tr as T2`, `use a::Tr as _`), or a glob whose module declares
    /// or re-exports it ([`type_in_module`](super::rust_globs::RustGlobs::type_in_module)) — and a glob
    /// of an **ancestor** module (`use super::*`) also brings in what that
    /// module's own globs do, private ones included, as Rust's glob of a parent
    /// brings in its imports ([`resolve_receiver_type`](super::proven_receivers::ProvenReceivers::resolve_receiver_type)
    /// reads an ancestor's glob alike). One file holds one import scope, its
    /// inline modules' included.
    fn imports_trait(&self, tr: NodeId) -> bool {
        let rust_globs = RustGlobs { cx: self.cx };
        let proven_receivers = ProvenReceivers { cx: self.cx };
        let Some(name) = self.ix.info.get(&tr).map(|i| i.name.as_str()) else {
            return false;
        };
        let Some(scope) = self.scope() else {
            return false;
        };
        let named = scope
            .alias_expansions
            .values()
            .flatten()
            .filter(|path| path.last().is_some_and(|last| last == name))
            .any(|path| proven_receivers.resolve_receiver_type(path) == Res::Found(tr));
        let own = self.source_module();
        let ancestor = |(krate, mods): &ModKey| {
            own.as_ref().is_some_and(|(k, m)| k == krate && m.starts_with(mods))
        };
        named
            || scope.globs.iter().any(|glob| {
                proven_receivers.module_key_of(glob, MAX_ALIAS_DEPTH).is_some_and(|key| {
                    rust_globs.type_in_module(&key, name, MAX_ALIAS_DEPTH) == Res::Found(tr)
                        || (ancestor(&key)
                            && rust_globs.module_scope(&key).is_some_and(|parent| {
                                rust_globs.through_reexported_globs(&key, &parent.globs, name, MAX_ALIAS_DEPTH)
                                    == Res::Found(tr)
                            }))
                })
            })
    }

    /// The text of an `impl` header's path (S-606: generics stripped) resolved
    /// to the one repository type it names, from the header's own scope
    /// ([`resolve_receiver_type`](super::proven_receivers::ProvenReceivers::resolve_receiver_type), S-607). A
    /// header of any other shape — `()`, `[u8]`, `dyn Tr`, `fn(u8)` — names no
    /// repository type.
    pub(super) fn header_type(&self, text: &str) -> Res {
        let proven_receivers = ProvenReceivers { cx: self.cx };
        let segs = split(text);
        let path_like = segs
            .iter()
            .all(|s| !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_'));
        if segs.is_empty() || !path_like {
            return Res::NotFound;
        }
        proven_receivers.resolve_receiver_type(&segs)
    }
}
