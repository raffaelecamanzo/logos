//! [`FeatureGate`] — the pluggable gate the composing binary supplies ([ADR-75]).
//!
//! The core gates nothing on its own: it defines the contract, and a `main` that
//! supplies no gate enables no feature, so its host activates only the
//! extensions that name none.
//!
//! [ADR-75]: ../../docs/specs/architecture/decisions/ADR-75.md

use std::collections::BTreeSet;

/// Reports which features are enabled ([ADR-75]). Supplied by the composing
/// binary, never by the core.
///
/// The host asks on every evaluation and caches no answer. What a surface reads
/// per request — the extensions list, the overview fields — follows an answer
/// that changes at run time; what a surface composes once at start-up — web
/// routes, MCP tools, CLI subcommands, the process-wide registration — keeps the
/// answer it was composed with.
///
/// [ADR-75]: ../../docs/specs/architecture/decisions/ADR-75.md
pub trait FeatureGate: Send + Sync {
    /// The features enabled now.
    fn enabled(&self) -> BTreeSet<String>;
}
