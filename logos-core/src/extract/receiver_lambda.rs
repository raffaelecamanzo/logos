//! A bare call inside a lambda with a receiver is marked as one (S-834,
//! [CR-261] K4, [FR-RS-31]): `with(repo) { save() }`, `buildString {
//! append("x") }`. Kotlin reads the lambda's receiver before every scope
//! outside the lambda, so the free call — or the call on the enclosing class's
//! instance — the earlier passes recorded is the receiver's call first.
//!
//! The lambdas are the ones the `references` query's `@ref.receiver.rebinds`
//! marker names — the one marker set S-563's receiver typing reads — and the
//! receiver pass ([`super::receiver`]) records, for each bare call inside one,
//! the type the file proves for that lambda's receiver, if any
//! ([`ReceiverLambdaCalls`]). Once the free-call pass ([`super::free_call`])
//! has decided each bare call's shape, the row is marked with what was proven
//! ([`LambdaReceiver`], carried as its alias), and otherwise left the bare
//! call it was, for the binder's receiver-lambda rung to read the receiver
//! first and fall back to:
//!
//! - a **free call** (Path form, no receiver) is marked whatever the file
//!   proves, so that an unproven receiver binds no top-level function;
//! - a **call on the instance** is marked only under a proven receiver, whose
//!   member outranks the enclosing class's; under an unproven one it stays the
//!   class's call, which a receiver of unknown type does not change;
//! - every other row — a call through a local binding (S-594), a call on
//!   another object, a typed `T::m` — is left as it is.
//!
//! [CR-261]: ../../../docs/requests/CR-261-kotlin-resolution-follow-ups.md
//! [FR-RS-31]: ../../../docs/specs/requirements/FR-RS-31.md

use super::RefFact;
use crate::model::{EdgeKind, ReceiverShape, RefForm};
use crate::resolve::LambdaReceiver;

/// One bare call inside a lambda with a receiver.
struct LambdaCall {
    /// The call's row.
    row: usize,
    /// The receiver's proven type, as a proven row `T::m` spells it.
    ty: Option<String>,
    /// Whether another receiver lambda encloses the call's innermost one.
    enclosed: bool,
}

/// The bare calls of one file that sit inside a lambda with a receiver.
#[derive(Default)]
pub(super) struct ReceiverLambdaCalls {
    calls: Vec<LambdaCall>,
}

impl ReceiverLambdaCalls {
    /// Note the bare call at `row`, inside a receiver lambda the file proves
    /// the type `ty` for, or none; `enclosed` when another receiver lambda
    /// encloses that one.
    pub(super) fn push(&mut self, row: usize, ty: Option<String>, enclosed: bool) {
        self.calls.push(LambdaCall { row, ty, enclosed });
    }

    /// Mark each noted row the free-call pass left a free call, or — under a
    /// proven receiver — a call on the instance.
    pub(super) fn apply(self, out: &mut [RefFact]) {
        for call in self.calls {
            let fact = &mut out[call.row];
            if fact.kind != EdgeKind::Calls || fact.alias.is_some() || fact.target.contains("::") {
                continue;
            }
            let free = fact.form == RefForm::Path && fact.receiver.is_none();
            let on_instance = fact.form == RefForm::Method && fact.receiver == Some(ReceiverShape::SelfInstance);
            let mark = match call.ty.as_deref() {
                Some(ty) if free || on_instance => LambdaReceiver { ty: Some(ty), enclosed: call.enclosed },
                None if free => LambdaReceiver { ty: None, enclosed: false },
                _ => continue,
            };
            fact.alias = Some(mark.alias());
        }
    }
}
