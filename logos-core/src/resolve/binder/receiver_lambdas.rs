//! A bare call inside a lambda with a receiver (S-834, [CR-261] K4,
//! [FR-RS-31]): `with(repo) { save() }`, `repo.apply { save() }`,
//! `buildString { append("x") }`. Kotlin reads the lambda's receiver before
//! every scope outside the lambda, so the call is `this.save()` on that
//! receiver first.
//!
//! Extraction marks such a call ([`LambdaReceiver`], the kernel's, carried as
//! the row's alias) and leaves the rest of the row as the bare call it was — on
//! the enclosing class's instance, or the free call the scopes bind — and this
//! rung decides the marked row before every other
//! ([`read_receiver`](ReceiverLambdas::read_receiver)); what it falls through
//! to, the dispatcher binds as that bare call and hands back to
//! [`keep_bare_call`](ReceiverLambdas::keep_bare_call):
//!
//! - **a proven receiver** `T` binds `T`'s member of the name, own or
//!   inherited, or else an extension of `T` in scope — as the call `x.m()` on
//!   that receiver does
//!   ([`resolve_proven_receiver_call`](super::proven_receivers::ProvenReceivers::resolve_proven_receiver_call)).
//!   With none, and with `T`'s whole hierarchy in the graph, the call falls
//!   through to the bare call it was — unless another receiver lambda
//!   encloses this one, whose own receiver Kotlin reads next: then it is
//!   read as an unproven receiver's;
//! - **a receiver nothing proves** — `buildString { … }`, a DSL builder, an
//!   untyped `x`, a `T` the repository does not declare or whose hierarchy it
//!   does not wholly hold — may declare the name, so the bare call keeps only what no receiver can
//!   answer: a member of the enclosing class (a row on the instance), a member
//!   of a class around the caller or of its companion the scopes reach
//!   ([`enclosing_member`](ReceiverLambdas::enclosing_member)), and a class it
//!   constructs (`X(…)`). It binds no top-level, imported or package
//!   function — an extension is one too: `buildString { append("x") }` never
//!   reaches a `fun Repo.append` that no receiver in scope may be — and
//!   records [`Residue::NoReceiverEvidence`].
//!
//! What the rung reads past the bare call — `T`, its members and its
//! supertypes — is what the sync rule
//! [`reads_lambda_receiver`](super::Invalidation::reads_lambda_receiver) adds to
//! [`Invalidation::ref_affected`](super::Invalidation::ref_affected): the row is selected as
//! the proven `T::m` row is.
//!
//! [CR-261]: ../../../../docs/requests/CR-261-kotlin-resolution-follow-ups.md
//! [FR-RS-31]: ../../../../docs/specs/requirements/FR-RS-31.md

use std::collections::HashSet;
use std::ops::Deref;

use super::proven_receivers::ProvenReceivers;
use super::supertype_climb::SupertypeClimb;
use super::{
    is_class_like, proven_twin, split, Ctx, Index, LambdaReceiver, Outcome, Res, Residue, Want, MAX_CONTAINS_DEPTH,
};
use crate::graph_store::UnresolvedRefRow;
use crate::model::{EdgeKind, NodeId, ReceiverShape};

/// What the proven receiver's type answers for the name.
enum Member {
    /// One member or extension binds.
    Bound(NodeId),
    /// The lookup decided against binding: two candidates, or a type a
    /// function of its name also spells. Its reason is the row's.
    Decided,
    /// `T` is a repository type whose whole hierarchy the graph holds, and
    /// neither it nor an extension of it answers: the call is not `T`'s.
    Absent,
    /// Nothing says whether `T` declares the name.
    Unknown,
}

/// What the rung decided of a marked row ([`ReceiverLambdas::read_receiver`]).
pub(super) enum LambdaRead {
    /// The proven receiver answered: the row's outcome.
    Decided(Outcome),
    /// The receiver did not decide: the dispatcher binds the bare call the row
    /// was and [`ReceiverLambdas::keep_bare_call`] judges what that bound.
    /// `answered` — the proven type, whose hierarchy the graph wholly holds,
    /// declares no such member and no other receiver lambda encloses this one:
    /// the bare call is what Kotlin reads next.
    BareCall { answered: bool },
}

/// A bare call inside a lambda with a receiver.
#[derive(Clone, Copy)]
pub(super) struct ReceiverLambdas<'c, 'a> {
    pub(super) cx: &'c Ctx<'a>,
}

impl<'a> Deref for ReceiverLambdas<'_, 'a> {
    type Target = Ctx<'a>;

    fn deref(&self) -> &Ctx<'a> {
        self.cx
    }
}

impl ReceiverLambdas<'_, '_> {
    /// Read the receiver of `r` when extraction marked it a bare call inside a
    /// lambda with a receiver; `None` for every other row.
    pub(super) fn read_receiver(&self, r: &UnresolvedRefRow) -> Option<LambdaRead> {
        if r.kind != EdgeKind::Calls {
            return None;
        }
        let mark = LambdaReceiver::of(r.alias.as_deref()?)?;
        let mut answered = false;
        if let Some(ty) = mark.ty {
            match self.receiver_member(r, ty) {
                Member::Bound(target) => {
                    return Some(LambdaRead::Decided(Outcome::Bound {
                        source: self.source,
                        target,
                        kind: EdgeKind::Calls,
                        payload: r.payload.clone(),
                    }))
                }
                Member::Decided => return Some(LambdaRead::Decided(Outcome::Unbound)),
                Member::Absent => answered = !mark.enclosed,
                Member::Unknown => {}
            }
            // The type's miss is not the bare call's reason.
            self.miss.take();
        }
        Some(LambdaRead::BareCall { answered })
    }

    /// What the marked row `r` binds, given the `outcome` of the bare call it
    /// was ([`LambdaRead::BareCall`]): that outcome when the receiver answered
    /// or the call is on the instance, or when it bound what no receiver can
    /// answer — a member of the enclosing class's scope or a class it
    /// constructs; else nothing, for [`Residue::NoReceiverEvidence`].
    pub(super) fn keep_bare_call(&self, r: &UnresolvedRefRow, answered: bool, outcome: Outcome) -> Outcome {
        let on_instance = r.receiver == Some(ReceiverShape::SelfInstance);
        let kept = match outcome {
            Outcome::Bound { kind: EdgeKind::Instantiates, .. } => true,
            Outcome::Bound { target, .. } => self.enclosing_member(target),
            _ => false,
        };
        if answered || on_instance || kept {
            return outcome;
        }
        self.miss.take();
        self.note(Want::Callable, || Residue::NoReceiverEvidence);
        Outcome::Unbound
    }

    /// Whether `target` is a member of a class-like declaration around the
    /// caller, or of the companion of one: the enclosing class's scope, which
    /// a bare call reaches as a member, never as a top-level, imported or
    /// package function. An imported companion member of another class is
    /// not one.
    fn enclosing_member(&self, target: NodeId) -> bool {
        let ix: &Index = self.ix;
        let class_like = |id: &NodeId| ix.info.get(id).is_some_and(|i| is_class_like(i.kind));
        let Some(&owner) = ix.parent.get(&target).filter(|id| class_like(id)) else {
            return false;
        };
        let mut around = HashSet::new();
        let mut cursor = ix.parent.get(&self.source).copied();
        for _ in 0..MAX_CONTAINS_DEPTH {
            let Some(id) = cursor else {
                break;
            };
            if class_like(&id) {
                around.insert(id);
            }
            cursor = ix.parent.get(&id).copied();
        }
        around.contains(&owner) || ix.parent.get(&owner).is_some_and(|outer| around.contains(outer))
    }

    /// What the proven type `ty` answers for `r`'s name: the proven twin's
    /// binding, by scope and module model only, as every proven row's is; on
    /// a miss, whether `ty` names a repository type whose hierarchy is closed.
    fn receiver_member(&self, r: &UnresolvedRefRow, ty: &str) -> Member {
        let proven_receivers = ProvenReceivers { cx: self.cx };
        let scoped = self.scope_only.replace(true);
        let member = match proven_receivers.resolve_proven_receiver_call(&proven_twin(r, ty)) {
            Res::Found(target) => Member::Bound(target),
            Res::Ambiguous => Member::Decided,
            Res::NotFound => match proven_receivers.structural_receiver_type(&split(ty)) {
                Res::Found(class) => {
                    let supertype_climb = SupertypeClimb { cx: self.cx };
                    let climb = supertype_climb.supertype_walk(vec![class], HashSet::from([class]), &r.target);
                    if climb.res == Res::NotFound && climb.closed {
                        Member::Absent
                    } else {
                        Member::Unknown
                    }
                }
                _ => Member::Unknown,
            },
        };
        self.scope_only.set(scoped);
        member
    }
}

#[cfg(test)]
mod tests {
    // The mark is the kernel's; its round trip stays tested beside the rung
    // that reads it.
    use super::LambdaReceiver;

    #[test]
    fn every_mark_reads_back_from_its_alias_and_no_other_alias_is_one() {
        for mark in [
            LambdaReceiver { ty: None, enclosed: false },
            LambdaReceiver { ty: Some("Repo"), enclosed: false },
            LambdaReceiver { ty: Some("a::b::Repo"), enclosed: true },
        ] {
            assert_eq!(LambdaReceiver::of(&mark.alias()), Some(mark));
        }
        for other in ["local-value", "receiver-lambdas", "receiver-lambda:", "Repo", ""] {
            assert_eq!(LambdaReceiver::of(other), None, "{other}");
        }
    }
}
