//! The `scip` containment seam — the **only** module in the workspace allowed
//! to name a `scip::*` (rust-protobuf) type ([ADR-06], [NFR-MA-07], [AR-07]).
//!
//! Every other module — and every public signature — works exclusively with
//! Logos newtypes ([`super::LogosSymbol`] et al.). The `scip` crate's
//! `Option`/`SpecialFields`-heavy generated structs are quarantined here so the
//! core never couples to rust-protobuf's ergonomics, and so a future swap to a
//! vendored `.proto` + `prost` stays a bounded, single-module change.
//!
//! ## The `logos-proto` swap seam ([NFR-MA-07])
//!
//! The [`proto`] alias below is the indirection point: today it re-exports the
//! upstream `scip` crate; if v2 ever vendors its own protobuf lineage, only this
//! one `use` changes and the rest of `convert.rs` is re-pointed in place. Keeping
//! the alias here (rather than reaching for `scip::` directly) means the seam is
//! expressed in code, not just in prose.
//!
//! Nothing in this module is re-exported beyond `pub(crate)`; the seam is sealed
//! at the crate boundary.
//!
//! [ADR-06]: ../../../../docs/specs/architecture/decisions/ADR-06.md
//! [NFR-MA-07]: ../../../../docs/specs/requirements/NFR-MA-07.md
//! [AR-07]: ../../../../docs/specs/architecture.md

/// The single `scip` re-export — the `logos-proto` swap seam ([NFR-MA-07]).
///
/// Re-pointing this `use` (e.g. to a vendored `prost` module) is the entire
/// migration surface envisaged by [ADR-06]'s reversibility analysis.
///
/// [ADR-06]: ../../../../docs/specs/architecture/decisions/ADR-06.md
/// [NFR-MA-07]: ../../../../docs/specs/requirements/NFR-MA-07.md
mod proto {
    pub use scip;
}

use proto::scip;

/// Parse a SCIP symbol string and return its **canonical** form.
///
/// The string is validated and normalised by routing it through the `scip`
/// codec's parser *and* formatter (`parse_symbol` → `format_symbol`). For an
/// already-canonical input the output is byte-identical, which is what the
/// round-trip golden test relies on ([FR-EX-01], [UAT-EX-01]).
///
/// Confining both halves of the codec here is what lets [`super::LogosSymbol`]
/// be a plain `String` newtype with no `scip` type in its public surface.
///
/// # Errors
/// Returns an [`anyhow::Error`] (Correctness-class per [ADR-14]) if `raw` is not
/// a grammatically valid SCIP symbol string. The `scip` error is rendered into
/// the message because `scip::symbol::SymbolError` implements neither `Display`
/// nor `std::error::Error`.
///
/// [FR-EX-01]: ../../../../docs/specs/requirements/FR-EX-01.md
/// [UAT-EX-01]: ../../../../docs/specs/requirements/UAT-EX-01.md
/// [ADR-14]: ../../../../docs/specs/architecture/decisions/ADR-14.md
pub(crate) fn canonicalize_symbol(raw: &str) -> anyhow::Result<String> {
    let parsed = scip::symbol::parse_symbol(raw)
        .map_err(|err| anyhow::anyhow!("invalid SCIP symbol {raw:?}: {err:?}"))?;
    Ok(scip::symbol::format_symbol(parsed))
}

/// The names of `symbol`'s last two descriptors — a member and the
/// declaration holding it (`…/Tr#m().` → `("Tr", "m")`) — or `None` when it
/// has fewer than two, or is not a SCIP symbol. What a dispatch row's `via`
/// label is read from ([`super::dispatch::via_label`]).
pub(crate) fn owner_and_member(symbol: &str) -> Option<(String, String)> {
    let parsed = scip::symbol::parse_symbol(symbol).ok()?;
    let [.., owner, member] = parsed.descriptors.as_slice() else {
        return None;
    };
    Some((owner.name.clone(), member.name.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A canonical 5-field global symbol of the exact shape FR-EX-01 mandates:
    /// `logos cargo <crate> <ver> <path>#fn().`
    const GOLDEN: &str = "logos cargo logos-core 0.1.0 src/model/mod.rs/LogosSymbol#parse().";

    #[test]
    fn canonical_symbol_round_trips_byte_for_byte() {
        // The codec fidelity guarantee that [`super::super::LogosSymbol`] is
        // built on: parse → format of a canonical string is the identity.
        assert_eq!(canonicalize_symbol(GOLDEN).unwrap(), GOLDEN);
    }

    #[test]
    fn local_symbol_round_trips() {
        assert_eq!(canonicalize_symbol("local 0").unwrap(), "local 0");
    }

    #[test]
    fn non_canonical_but_valid_symbol_is_normalised() {
        // An identifier wrapped in unnecessary backticks is a *valid* SCIP
        // symbol but not its canonical form; the codec's formatter strips the
        // redundant escaping. Asserting the output differs from the input
        // proves canonicalize_symbol genuinely runs parse → *format* — a
        // regression that bypassed `format_symbol` (returning the raw input)
        // would pass the canonical round-trip golden but fail here (ADR-06
        // round-trip fitness function).
        let non_canonical = "logos cargo logos-core 0.1.0 src/model/mod.rs/`LogosSymbol`#parse().";
        let canonical = "logos cargo logos-core 0.1.0 src/model/mod.rs/LogosSymbol#parse().";
        let normalised = canonicalize_symbol(non_canonical).unwrap();
        assert_ne!(
            normalised, non_canonical,
            "input was already canonical — test no longer exercises normalisation"
        );
        assert_eq!(normalised, canonical);
    }

    #[test]
    fn malformed_symbol_is_a_correctness_error() {
        // Missing the manager/name/version package fields — the exact failure
        // the task's illustrative (non-canonical) example would hit.
        let err =
            canonicalize_symbol("rust-analyzer 0.1.0 src/lib.rs/MyStruct#method().").unwrap_err();
        assert!(
            err.to_string().contains("invalid SCIP symbol"),
            "unexpected error message: {err}"
        );
    }

    #[test]
    fn empty_symbol_is_rejected() {
        assert!(canonicalize_symbol("").is_err());
    }
}
