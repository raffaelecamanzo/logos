//! A trait-typed call fanned out to the set of its targets (S-281, S-608,
//! [FR-RS-08]): every repository impl of the method and the trait's own default
//! body ([`TraitDispatch::fan_out`]), recorded on the row's context for
//! [`super::bind_traced`] to turn into edges.
//!
//! [FR-RS-08]: ../../../../docs/specs/requirements/FR-RS-08.md

use std::ops::Deref;

use crate::model::{NodeId, NodeKind};

use super::impl_index::ImplIndex;
use super::{exactly_one, Ctx, Dispatch, Fan, Index, Res, Residue, Want};
use super::self_types::SelfTypes;

/// A trait-typed call fanned out to every target it may dispatch to.
#[derive(Clone, Copy)]
pub(super) struct TraitDispatch<'c, 'a> {
    pub(super) cx: &'c Ctx<'a>,
}

impl<'a> Deref for TraitDispatch<'_, 'a> {
    type Target = Ctx<'a>;

    fn deref(&self) -> &Ctx<'a> {
        self.cx
    }
}

impl TraitDispatch<'_, '_> {
    /// Fan out a trait-typed receiver call `T::f` to the SET of that trait
    /// method's targets (S-281, [CR-073], [FR-RS-08]; S-608, [FR-RS-47] rule 5).
    ///
    /// `target` is the trait-qualified form extraction emits for a *provable*
    /// trait-typed receiver — `&dyn T`, `impl T`, a generic parameter bounded
    /// by `T` — the method the last `::` segment, the head its trait, or its
    /// several bounds joined by `+` (`A+B::f`). Each bound is read as the
    /// caller names it ([`bound_trait`](TraitDispatch::bound_trait)); one the graph
    /// cannot place (`Send`, `Clone`, an imported `std::io::Write`) provides
    /// nothing it can see. The one bound whose set
    /// ([`fan_out`](TraitDispatch::fan_out)) is not empty decides the call. Two such
    /// bounds bind nothing ([`Residue::OverloadAmbiguous`]): rustc rejects the
    /// call as ambiguous. So does one beside a bound whose name reaches two
    /// repository traits, which may provide the method too; alone, such a
    /// bound is [`Residue::TypeAmbiguous`]. No provider is
    /// [`Residue::ExternalType`] beside an unplaced bound — whose method it
    /// may be — and [`Residue::SupertypeUnreached`] otherwise.
    ///
    /// Never fabricate ([NFR-RA-05]): every target is a real indexed node reached
    /// through the **proven** trait `T`, so no same-named method on an unrelated
    /// type, free function, or external target can enter the set.
    ///
    /// [CR-073]: ../../../../docs/requests/CR-073-trait-object-dynamic-dispatch-reachability.md
    /// [FR-RS-08]: ../../../../docs/specs/requirements/FR-RS-08.md
    /// [FR-RS-47]: ../../../../docs/specs/requirements/FR-RS-47.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    pub(super) fn resolve_dyn_dispatch(&self, target: &str) -> Res {
        // A malformed target (`::f`, `T::`, a single segment) cannot reach here
        // (`target.contains("::")` gated the call) but a missing head is
        // handled defensively as an honest miss.
        let Some((head, method)) = target.rsplit_once("::") else {
            return Res::NotFound;
        };
        let (mut unplaced, mut undecided) = (false, false);
        let mut providers: Vec<Fan> = Vec::new();
        for bound in head.split('+') {
            match self.bound_trait(bound) {
                Res::Found(tr) => {
                    let fan = self.fan_out(tr, method);
                    if !fan.targets.is_empty() {
                        providers.push(fan);
                    }
                }
                Res::NotFound => unplaced = true,
                Res::Ambiguous => undecided = true,
            }
        }
        match <[Fan; 1]>::try_from(providers) {
            Ok([fan]) if !undecided => self.fan_out_to(fan),
            Err(providers) if providers.is_empty() && undecided => {
                self.note(Want::Callable, || Residue::TypeAmbiguous);
                Res::Ambiguous
            }
            Err(providers) if providers.is_empty() => {
                self.note(Want::Callable, || {
                    if unplaced {
                        Residue::ExternalType {
                            candidates: Vec::new(),
                        }
                    } else {
                        Residue::SupertypeUnreached
                    }
                });
                Res::NotFound
            }
            _ => {
                self.note(Want::Callable, || Residue::OverloadAmbiguous);
                Res::Ambiguous
            }
        }
    }

    /// The repository trait a trait-typed call's bound `name` — a simple name
    /// (S-281, S-608) — denotes at the caller: the one its scope names
    /// ([`header_type`](super::self_types::SelfTypes::header_type), by scope and module model only),
    /// else, where the caller's file imports no item of that name, the one
    /// repository trait of the name (a bound written as a path,
    /// `&dyn a::b::Tr`, records its last segment) — [`Res::Ambiguous`] when
    /// the scope, or the repository, names two. An import that names an item
    /// outside the repository (`use std::io::Write;`) shadows a repository
    /// trait of the same name, as Rust's own `use` does: [`Res::NotFound`].
    fn bound_trait(&self, name: &str) -> Res {
        let self_types = SelfTypes { cx: self.cx };
        let scoped = self.scope_only.replace(true);
        let res = self_types.header_type(name);
        self.scope_only.set(scoped);
        match res {
            Res::Found(id) if self.ix.info.get(&id).is_some_and(|i| i.kind == NodeKind::Trait) => res,
            Res::NotFound if !self.scope().is_some_and(|s| s.aliases.contains_key(name)) => {
                let traits: Vec<NodeId> = self
                    .ix
                    .by_name
                    .get(name)
                    .into_iter()
                    .flatten()
                    .copied()
                    .filter(|id| self.ix.info.get(id).is_some_and(|i| i.kind == NodeKind::Trait))
                    .collect();
                exactly_one(&traits)
            }
            Res::Ambiguous => Res::Ambiguous,
            Res::Found(_) | Res::NotFound => Res::NotFound,
        }
    }

    /// The SET a call through the trait `tr` fans out to (S-281, [FR-RS-08] as
    /// amended by [FR-RS-47]; S-608): every concrete repository impl of its
    /// method `name` ([`ImplIndex::impls_of`](super::impl_index::ImplIndex::impls_of)), plus the trait's own default body
    /// (a [`Want::Callable`] member of the trait node). Union reachability:
    /// any impl — or the default, for an impl that does not override, an
    /// implementor outside the repository included — is a legitimate runtime
    /// dispatch target ([FR-AN-01]); the default stays in the set even where
    /// every repository impl overrides it, as FR-RS-08's set always held it
    /// (its "zero loss of a previously-resolved edge"). A
    /// required signature binds nothing: it is no node of the index
    /// ([`bindable`](super::bindable)). Id-sorted and deduped for a deterministic edge order
    /// ([NFR-RA-06]).
    ///
    /// Each impl is reached by dispatch, the default is the trait's own method
    /// (S-712, [CR-226] BR-76): the set's [`Dispatch`] marks the impls, named
    /// by the trait method's symbol, and leaves the default unmarked. A set
    /// whose trait method the graph does not hold carries none.
    ///
    /// [FR-RS-08]: ../../../../docs/specs/requirements/FR-RS-08.md
    /// [FR-RS-47]: ../../../../docs/specs/requirements/FR-RS-47.md
    /// [FR-AN-01]: ../../../../docs/specs/requirements/FR-AN-01.md
    /// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
    /// [CR-226]: ../../../../docs/requests/CR-226-dispatch-is-not-a-dependency.md
    fn fan_out(&self, tr: NodeId, name: &str) -> Fan {
        let ix: &Index = self.ix;
        let impls: &ImplIndex = &ix.impls;
        let own = ix.members_named(tr, name, Want::Callable);
        let mut targets = impls.impls_of(tr, name).to_vec();
        targets.extend(&own);
        targets.sort_unstable();
        targets.dedup();
        let implementors: Vec<NodeId> = targets.iter().copied().filter(|t| !own.contains(t)).collect();
        let dispatch = impls
            .trait_method_symbol(tr, name)
            .filter(|_| !implementors.is_empty())
            .map(|method| Dispatch {
                method: method.to_string(),
                implementors,
            });
        Fan { targets, dispatch }
    }

    /// A call through the trait `tr` — a trait body's `self.m()` / `Self::m()`,
    /// a written `Tr::m(x)` — fanned out to [`fan_out`](TraitDispatch::fan_out)'s set
    /// (S-608, [FR-RS-47] rule 5). An empty set is
    /// [`Residue::SupertypeUnreached`]: neither an impl nor a default supplies
    /// the method (a supertrait's is not read).
    ///
    /// [FR-RS-47]: ../../../../docs/specs/requirements/FR-RS-47.md
    pub(super) fn trait_call(&self, tr: NodeId, name: &str) -> Res {
        let fan = self.fan_out(tr, name);
        if fan.targets.is_empty() {
            self.note(Want::Callable, || Residue::SupertypeUnreached);
            return Res::NotFound;
        }
        self.fan_out_to(fan)
    }

    /// Record `fan` as the row's fan-out ([`fanned`](Ctx::fanned)), which
    /// [`bind_traced`](super::bind_traced) binds, and answer [`Res::NotFound`] to the rung.
    fn fan_out_to(&self, fan: Fan) -> Res {
        *self.fanned.borrow_mut() = Some(fan);
        Res::NotFound
    }
}
