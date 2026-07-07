//! Read-model types returned by [`crate::Engine`] methods.
//!
//! All types implement [`serde::Serialize`] so adapter surfaces can
//! serialise them to JSON without touching the core (ADR-01).
//!
//! Re-export everything so callers can `use logos_core::models::*`.

pub mod navigation;
pub mod pipeline;
pub mod quality;

pub use navigation::*;
pub use pipeline::*;
pub use quality::*;
