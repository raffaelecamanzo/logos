//! The one associated-item lookup (S-607, S-608, [FR-RS-47]) — the kernel's
//! own rung: a call to a type's method decided among the functions of every
//! `impl` block whose header resolves to the type, and the trait defaults those
//! blocks lend it, retrying a method call on the type's `Deref` target
//! ([`Ctx::lookup`]). It stays a method of [`Ctx`]; it sits here, beside the
//! rungs, because its recursion runs through them (a trait's scope reads the
//! module tree, a path's type segment is decided by the lookup).
//!
//! [FR-RS-47]: ../../../../docs/specs/requirements/FR-RS-47.md

use std::collections::HashSet;

use crate::model::{NodeId, NodeKind, ReceiverMode};

use super::{exactly_one, Candidate, Ctx, Named, Res, Residue, Syntax, Want, MAX_DEREF_DEPTH};
use super::impl_index::ImplIndex;
use super::self_types::SelfTypes;
use super::trait_dispatch::TraitDispatch;

impl Ctx<'_> {
    /// The one associated-item lookup (S-607, S-608, [FR-RS-47]): the one
    /// callable `name` among the type `ty`'s items, as `syntax` calls it —
    /// `self.m()`, `Self::m()`, a proven `x.m()`, a written `T::m()` and, with
    /// `named`, a qualified `<T as Tr>::m()` alike.
    ///
    /// Each type is probed ([`probe`](Ctx::probe)). A probe that reaches no
    /// candidate — none of the name, or none callable with this syntax or in
    /// scope — retries, with method syntax and no named trait, on the type's
    /// recorded `Deref` target ([`ImplIndex::deref_targets`](super::impl_index::ImplIndex::deref_targets), rule 4): each type
    /// once, to [`MAX_DEREF_DEPTH`] hops, so a cycle terminates. A target the
    /// repository does not declare is [`Residue::ExternalType`] (its methods
    /// are not in the graph), one two `Deref` impls disagree on
    /// [`Residue::TypeAmbiguous`]. Path syntax never retries: `T::m()` names
    /// `T`'s own items.
    ///
    /// [FR-RS-47]: ../../../../docs/specs/requirements/FR-RS-47.md
    pub(super) fn lookup(&self, ty: NodeId, name: &str, syntax: Syntax, named: Option<Named<'_>>) -> Res {
        let mut ty = ty;
        let mut seen: HashSet<NodeId> = HashSet::from([ty]);
        loop {
            let missed = match self.probe(ty, name, syntax, named) {
                Ok(decided) => return decided,
                Err(missed) => missed,
            };
            let retry = syntax == Syntax::Method && named.is_none() && seen.len() <= MAX_DEREF_DEPTH;
            match self.ix.impls.deref_targets.get(&ty).filter(|_| retry) {
                Some(&Res::Found(next)) if !seen.contains(&next) => {
                    seen.insert(next);
                    ty = next;
                }
                Some(Res::NotFound) => {
                    self.note(Want::Callable, || Residue::ExternalType {
                        candidates: Vec::new(),
                    });
                    return Res::NotFound;
                }
                Some(Res::Ambiguous) => {
                    self.note(Want::Callable, || Residue::TypeAmbiguous);
                    return Res::Ambiguous;
                }
                _ => {
                    self.note(Want::Callable, || missed);
                    return Res::NotFound;
                }
            }
        }
    }

    /// One type's step of the [`lookup`](Ctx::lookup): `Ok` with what it
    /// decided (a reason noted for a miss), or `Err` with the reason it reached
    /// no candidate, which a `Deref` retry may overrule.
    ///
    /// 1. **Universe** — the functions of every recorded `impl` block whose
    ///    self type resolves to `ty` ([`ImplIndex::assoc_items`](super::impl_index::ImplIndex::assoc_items)), from any crate
    ///    and any file, and the **default bodies** of the repository traits
    ///    `ty` implements where its block does not override them
    ///    ([`ImplIndex::trait_impls`](super::impl_index::ImplIndex::trait_impls), S-608) — an empty `impl Greet for X {}`
    ///    included. A default ranks as its trait's function. With `named`
    ///    (`<T as Tr>::m`), only the functions of `ty`'s impl of that trait,
    ///    else its default. None at all is `Err(SupertypeUnreached)` — a
    ///    derive may supply it, or the type's `Deref` target — or
    ///    `Err(NotACallable)` when `name` is one of `ty`'s enum variants.
    /// 2. **Syntax** — method syntax drops every candidate recorded as not
    ///    taking `self` ([`DeclarationFacts::without_self`](super::declaration_facts::DeclarationFacts::without_self)); path syntax keeps it. An
    ///    unknown fact never filters.
    /// 3. **Scope** — a trait's function or default is no candidate where its
    ///    trait is not in scope ([`trait_in_scope`](super::self_types::SelfTypes::trait_in_scope)),
    ///    unless the call names the trait. Nothing left after 2 and 3 is
    ///    `Err(SupertypeUnreached)`.
    /// 4. **Arity** — the argument count as the syntax passes it
    ///    ([`admitted_by_count`](Ctx::admitted_by_count)); nothing left is
    ///    [`Residue::NoApplicableOverload`].
    /// 5. **Precedence** — an inherent candidate beats a trait's, in any
    ///    module, and an impl's override beats the default it overrides (the
    ///    default never entered the universe). A repository trait's function
    ///    left out by 3 beside external traits' functions alone binds nothing:
    ///    an external trait, never placed in or out of scope, does not win by
    ///    default. With method syntax, inherent candidates that all take
    ///    `&self`/`&mut self` beside a trait's taking `self` by value bind
    ///    nothing: rustc's by-value probe may pick the trait's, and which is
    ///    not decided here ([`Residue::OverloadAmbiguous`]).
    /// 6. **Exactly one** — two of the deciding rank are
    ///    [`Residue::OverloadAmbiguous`] ([NFR-RA-05]): two traits in scope
    ///    each supplying `name`, a default among them.
    ///
    /// A trait as `ty` (`Tr::m(x)`) is dispatched on its first argument, which
    /// nothing proves: the call fans out to [FR-RS-08]'s set
    /// ([`trait_call`](super::trait_dispatch::TraitDispatch::trait_call), S-608).
    ///
    /// [FR-RS-08]: ../../../../docs/specs/requirements/FR-RS-08.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    fn probe(&self, ty: NodeId, name: &str, syntax: Syntax, named: Option<Named<'_>>) -> Result<Res, Residue> {
        let self_types = SelfTypes { cx: self };
        let trait_dispatch = TraitDispatch { cx: self };
        let ix = self.ix;
        let impls: &ImplIndex = &ix.impls;
        if ix.info.get(&ty).is_some_and(|i| i.kind == NodeKind::Trait) {
            return Ok(trait_dispatch.trait_call(ty, name));
        }
        let header = |c: &Candidate| &ix.impls.impl_headers[c.block];
        let own = ix.impls.assoc_items.get(&(ty, name.to_string())).map_or(&[][..], Vec::as_slice);
        let mut universe: Vec<Candidate> = own
            .iter()
            .filter_map(|&id| Some(Candidate { id, block: *ix.impls.assoc_owner.get(&id)? }))
            .collect();
        for &block in ix.impls.trait_impls.get(&ty).map_or(&[][..], Vec::as_slice) {
            if impls.block_overrides(ix, block, name) {
                continue;
            }
            let Some(tr) = ix.impls.impl_headers[block].trait_node else { continue };
            universe.extend(ix.members_named(tr, name, Want::Callable).into_iter().map(|id| Candidate { id, block }));
        }
        if let Some(named) = named {
            universe.retain(|c| named.names(header(c)));
        }
        let mut ids = HashSet::new();
        universe.retain(|c| ids.insert(c.id));
        let reachable: Vec<Candidate> = universe
            .iter()
            .copied()
            .filter(|c| syntax == Syntax::Path || !ix.facts.without_self.contains(&c.id))
            .collect();
        let mut candidates: Vec<Candidate> = reachable
            .iter()
            .copied()
            .filter(|c| named.is_some() || self_types.trait_in_scope(header(c)))
            .collect();
        let scoped_out = candidates.len() < reachable.len();
        if candidates.is_empty() {
            return Err(if universe.is_empty() && self.is_variant(ty, name) {
                Residue::NotACallable
            } else {
                Residue::SupertypeUnreached
            });
        }
        let admitted = self.admitted_by_count(candidates.iter().map(|c| c.id).collect(), syntax);
        candidates.retain(|c| admitted.contains(&c.id));
        if candidates.is_empty() {
            self.note(Want::Callable, || Residue::NoApplicableOverload);
            return Ok(Res::NotFound);
        }
        let (traits, inherent): (Vec<Candidate>, Vec<Candidate>) =
            candidates.into_iter().partition(|c| header(c).trait_path.is_some());
        let mode = |c: &Candidate| ix.facts.receiver_modes.get(&c.id).copied();
        let by_value_rival = syntax == Syntax::Method
            && !inherent.is_empty()
            && inherent.iter().all(|c| matches!(mode(c), Some(ReceiverMode::Ref | ReceiverMode::RefMut)))
            && traits.iter().any(|c| mode(c) == Some(ReceiverMode::Value));
        // A repository trait's function left out of scope, beside external
        // traits' alone: the scope read may have missed a form that brings the
        // repository trait in, and the external one is unplaced — never let it
        // win by default.
        let unplaced_only =
            scoped_out && inherent.is_empty() && traits.iter().all(|c| header(c).trait_node.is_none());
        let res = if by_value_rival || unplaced_only {
            Res::Ambiguous
        } else {
            let deciding: Vec<NodeId> = if inherent.is_empty() { &traits } else { &inherent }.iter().map(|c| c.id).collect();
            exactly_one(&deciding)
        };
        if res == Res::Ambiguous {
            self.note(Want::Callable, || Residue::OverloadAmbiguous);
        }
        Ok(res)
    }

    /// `candidates` less every one whose parameter range excludes the call's
    /// argument count as `syntax` passes it (S-607, [FR-RS-43] as amended by
    /// [FR-RS-47]): method syntax passes the receiver outside the count, which
    /// the range already excludes; path syntax passes it as the first
    /// argument to a candidate that takes `self` (`T::m(&x, 1)` calls
    /// `fn m(&self, a)`), and none to one that does not. An unknown count,
    /// range or `takes_self` fact never drops a candidate.
    ///
    /// [FR-RS-43]: ../../../../docs/specs/requirements/FR-RS-43.md
    /// [FR-RS-47]: ../../../../docs/specs/requirements/FR-RS-47.md
    fn admitted_by_count(&self, mut candidates: Vec<NodeId>, syntax: Syntax) -> Vec<NodeId> {
        let Some(count) = self.call_args else {
            return candidates;
        };
        let ix = self.ix;
        candidates.retain(|c| {
            let Some(range) = ix.facts.arities.get(c) else { return true };
            match syntax {
                Syntax::Method => range.admits(count),
                Syntax::Path if ix.facts.with_self.contains(c) => count.checked_sub(1).is_some_and(|n| range.admits(n)),
                Syntax::Path if ix.facts.without_self.contains(c) => range.admits(count),
                Syntax::Path => true,
            }
        });
        candidates
    }
}
