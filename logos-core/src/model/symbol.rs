//! [`LogosSymbol`] — the canonical symbol-identity newtype — and [`NodeId`].
//!
//! `LogosSymbol` is the backbone of the graph: every node, edge, and annotation
//! hangs off a stable symbol identity ([NFR-RA-03]). It wraps a *canonical* SCIP
//! symbol string; construction goes through the [`convert`](super::convert) seam
//! so the `scip` codec — not hand-rolled string surgery — guarantees fidelity
//! ([ADR-06], [FR-EX-01]).
//!
//! [NFR-RA-03]: ../../../../docs/specs/requirements/NFR-RA-03.md
//! [ADR-06]: ../../../../docs/specs/architecture/decisions/ADR-06.md
//! [FR-EX-01]: ../../../../docs/specs/requirements/FR-EX-01.md

use std::fmt;

use serde::{Deserialize, Serialize};

use super::convert;

/// A canonical SCIP symbol string ([FR-EX-01]).
///
/// The wrapped `String` is always in `scip`-canonical form because every
/// constructor routes through the codec: [`LogosSymbol::parse`] for borrowed
/// input and the `TryFrom<String>` impl (used by `Deserialize`) for owned
/// input. Serialises transparently as its string (newtype), so JSON read-models
/// carry the bare symbol — no `scip` type ever crosses a public boundary
/// ([NFR-MA-07]). `Deserialize` is *validated*, not transparent: a malformed
/// symbol in a payload fails deserialization via `#[serde(try_from = "String")]`
/// rather than producing an un-canonical value.
///
/// [FR-EX-01]: ../../../../docs/specs/requirements/FR-EX-01.md
/// [NFR-MA-07]: ../../../../docs/specs/requirements/NFR-MA-07.md
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct LogosSymbol(String);

impl LogosSymbol {
    /// Parse and canonicalise a SCIP symbol string.
    ///
    /// Validates `raw` against the SCIP symbol grammar and stores its canonical
    /// form. Because canonicalisation is idempotent, an already-canonical input
    /// is preserved byte-for-byte — the property the round-trip golden test
    /// asserts ([UAT-EX-01]).
    ///
    /// # Errors
    /// Returns an [`anyhow::Error`] if `raw` is not a valid SCIP symbol string.
    ///
    /// [UAT-EX-01]: ../../../../docs/specs/requirements/UAT-EX-01.md
    pub fn parse(raw: &str) -> anyhow::Result<Self> {
        Ok(LogosSymbol(convert::canonicalize_symbol(raw)?))
    }

    /// Render back to the canonical SCIP symbol string.
    ///
    /// For a symbol built via [`LogosSymbol::parse`] from a canonical input,
    /// `to_scip_string()` returns that input unchanged (round-trip identity).
    pub fn to_scip_string(&self) -> String {
        self.0.clone()
    }

    /// Borrow the canonical SCIP symbol string without allocating.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for LogosSymbol {
    type Error = anyhow::Error;

    /// Validate and canonicalise an owned symbol string.
    ///
    /// This is the hook `#[serde(try_from = "String")]` uses, so `Deserialize`
    /// is validated rather than transparent — a malformed symbol in a
    /// serialized payload fails deserialization instead of silently producing
    /// an un-canonical [`LogosSymbol`].
    fn try_from(raw: String) -> anyhow::Result<Self> {
        Self::parse(&raw)
    }
}

impl fmt::Display for LogosSymbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The database identity of a node — the SQLite rowid assigned by `graph-store`.
///
/// Distinct from [`LogosSymbol`]: `NodeId` is a storage-local handle (compact,
/// cheap to copy, not portable across databases), whereas `LogosSymbol` is the
/// portable, content-derived symbol identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct NodeId(pub i64);

impl NodeId {
    /// The underlying rowid.
    pub const fn get(self) -> i64 {
        self.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The FR-EX-01-canonical golden symbol: `logos cargo <crate> <ver> <path>#fn().`
    const GOLDEN: &str = "logos cargo logos-core 0.1.0 src/model/mod.rs/LogosSymbol#parse().";

    #[test]
    fn golden_symbol_round_trips_byte_for_byte() {
        // UAT-EX-01 / FR-EX-01: parse → format is the identity on a canonical
        // SCIP symbol string.
        let sym = LogosSymbol::parse(GOLDEN).expect("golden symbol must parse");
        assert_eq!(sym.to_scip_string(), GOLDEN);
    }

    #[test]
    fn parse_then_parse_again_is_stable() {
        let once = LogosSymbol::parse(GOLDEN).unwrap();
        let twice = LogosSymbol::parse(once.as_str()).unwrap();
        assert_eq!(once, twice);
        assert_eq!(twice.to_scip_string(), GOLDEN);
    }

    #[test]
    fn local_symbol_round_trips() {
        let sym = LogosSymbol::parse("local 0").unwrap();
        assert_eq!(sym.to_scip_string(), "local 0");
    }

    #[test]
    fn malformed_symbol_is_rejected() {
        assert!(LogosSymbol::parse("not a real symbol").is_err());
        assert!(LogosSymbol::parse("").is_err());
    }

    #[test]
    fn serialises_transparently_as_its_string() {
        let sym = LogosSymbol::parse(GOLDEN).unwrap();
        let json = serde_json::to_string(&sym).unwrap();
        assert_eq!(json, format!("{GOLDEN:?}"));
    }

    #[test]
    fn deserialize_validates_and_canonicalises() {
        // `#[serde(try_from = "String")]` routes through `parse`, so a valid
        // symbol deserializes to its canonical form.
        let sym: LogosSymbol = serde_json::from_str(&format!("{GOLDEN:?}")).unwrap();
        assert_eq!(sym.to_scip_string(), GOLDEN);
        // Round-trips through serde: serialize → deserialize is the identity.
        let json = serde_json::to_string(&sym).unwrap();
        assert_eq!(serde_json::from_str::<LogosSymbol>(&json).unwrap(), sym);
    }

    #[test]
    fn deserialize_rejects_malformed_symbol() {
        // A malformed symbol must fail deserialization (validated, not
        // transparent) — the SF-4 guarantee for downstream graph-store DTOs.
        let err = serde_json::from_str::<LogosSymbol>("\"not a real symbol\"").unwrap_err();
        assert!(
            err.to_string().contains("invalid SCIP symbol"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn try_from_owned_string_canonicalises() {
        let sym = LogosSymbol::try_from(GOLDEN.to_string()).unwrap();
        assert_eq!(sym.to_scip_string(), GOLDEN);
        assert!(LogosSymbol::try_from("not a real symbol".to_string()).is_err());
    }

    #[test]
    fn display_matches_canonical_string() {
        let sym = LogosSymbol::parse(GOLDEN).unwrap();
        assert_eq!(sym.to_string(), GOLDEN);
    }

    #[test]
    fn node_id_is_a_transparent_i64() {
        let id = NodeId(42);
        assert_eq!(id.get(), 42);
        assert_eq!(serde_json::to_string(&id).unwrap(), "42");
        assert_eq!(id.to_string(), "42");
    }
}
