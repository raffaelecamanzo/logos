//! Read-model types returned by [`crate::Engine`] methods.
//!
//! All types implement [`serde::Serialize`] so adapter surfaces can
//! serialise them to JSON without touching the core (ADR-01).
//!
//! One member is not a type: [`quality::absence`] states the cross-surface
//! absence taxonomy — the vocabulary and the rules every surface keeps when it
//! reports a figure it does not have ([S-434]). It declares no type and
//! serialises nothing; it is here because the four Rust classifiers it governs,
//! [`quality::SignalAbsence`], [`quality::EvaluatedSetAbsence`],
//! [`quality::CrossFileAbsence`] and [`navigation::DenominatorAbsence`], are.
//!
//! [S-434]: ../../../docs/planning/journal.md#s-434-one-absence-taxonomy-audited-across-the-three-reporting-surfaces
//!
//! Re-export everything so callers can `use logos_core::models::*`.

pub mod navigation;
pub mod outcome;
pub mod pipeline;
pub mod quality;

pub use navigation::*;
pub use outcome::*;
pub use pipeline::*;
pub use quality::*;
