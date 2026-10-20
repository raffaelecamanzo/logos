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
    /// The SCIP scheme every symbol Logos extracts carries.
    pub const SCHEME: &'static str = "logos";

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

    /// The repository coordinate in the SCIP package slot ([ADR-85]), or
    /// `None` when the slot is empty (`.`, a coordinate-less symbol) or the
    /// symbol is local.
    ///
    /// [ADR-85]: ../../../../docs/specs/architecture/decisions/ADR-85.md
    pub fn coordinate(&self) -> Option<&str> {
        self.fields().map(|f| f.package).filter(|p| *p != EMPTY_FIELD)
    }

    /// Whether this is a global symbol whose package slot is empty — the form
    /// every symbol had before [ADR-85] put the repository coordinate there.
    /// A local symbol has no package slot and is never coordinate-less.
    ///
    /// [ADR-85]: ../../../../docs/specs/architecture/decisions/ADR-85.md
    pub fn is_coordinate_less(&self) -> bool {
        self.fields().is_some_and(|f| f.package == EMPTY_FIELD)
    }

    /// The descriptor tail: everything after the four coordinate fields
    /// (scheme, manager, package, version) — the path and scope that identify
    /// the declaration within its repository. Two repositories' symbols for
    /// one repo-relative declaration share it. `None` for a local symbol.
    pub fn descriptor_tail(&self) -> Option<&str> {
        self.fields().map(|f| f.tail)
    }

    /// The coordinate-less symbol whose descriptor tail is `tail`:
    /// `logos . . . <tail>`, the inverse of
    /// [`descriptor_tail`](Self::descriptor_tail) for a symbol Logos extracts,
    /// whose manager and version are always empty ([ADR-85]).
    /// [`with_coordinate`](Self::with_coordinate) places it under a coordinate.
    ///
    /// # Errors
    /// Returns an error when `tail` is not a valid descriptor tail — a bare
    /// name with no descriptor suffix, for one.
    ///
    /// [ADR-85]: ../../../../docs/specs/architecture/decisions/ADR-85.md
    pub fn from_descriptor_tail(tail: &str) -> anyhow::Result<Self> {
        Self::parse(&format!("{} {EMPTY_FIELD} {EMPTY_FIELD} {EMPTY_FIELD} {tail}", Self::SCHEME))
    }

    /// The fields of a global symbol, or `None` for a local one (`local <id>`,
    /// two fields, where a global symbol has at least four).
    ///
    /// Splitting the canonical string on its first four spaces is exact: the
    /// codec's formatter writes each coordinate field verbatim and a coordinate
    /// never contains a space ([`crate::config::is_valid_coordinate`]), while
    /// a space inside the descriptors (a backtick-quoted name) stays in the
    /// tail.
    fn fields(&self) -> Option<Fields<'_>> {
        let mut parts = self.0.splitn(5, ' ');
        Some(Fields {
            scheme: parts.next()?,
            manager: parts.next()?,
            package: parts.next()?,
            version: parts.next()?,
            tail: parts.next().unwrap_or(""),
        })
    }

    /// This symbol under another coordinate: the package slot replaced, every
    /// other field and the descriptor tail kept byte for byte. `None` (or an
    /// empty coordinate) renders the empty slot `.`, the coordinate-less form.
    /// A local symbol has no package slot and comes back unchanged.
    ///
    /// # Errors
    /// Returns an error when `coordinate` contains whitespace, which would
    /// shift every later field, or when the rebuilt string is not a valid SCIP
    /// symbol.
    pub fn with_coordinate(&self, coordinate: Option<&str>) -> anyhow::Result<Self> {
        let Some(f) = self.fields() else {
            return Ok(self.clone());
        };
        let package = match coordinate {
            Some(c) if c.chars().any(char::is_whitespace) => {
                anyhow::bail!("a symbol coordinate cannot contain whitespace: {c:?}")
            }
            Some(c) if !c.is_empty() => c,
            _ => EMPTY_FIELD,
        };
        let mut raw = [f.scheme, f.manager, package, f.version].join(" ");
        if !f.tail.is_empty() {
            raw.push(' ');
            raw.push_str(f.tail);
        }
        Self::parse(&raw)
    }
}

/// The SCIP marker for an empty scheme/package field.
const EMPTY_FIELD: &str = ".";

/// A global symbol's four coordinate fields and its descriptor tail, borrowed
/// from the canonical string ([`LogosSymbol::fields`]).
struct Fields<'a> {
    scheme: &'a str,
    manager: &'a str,
    package: &'a str,
    version: &'a str,
    tail: &'a str,
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

    /// Coordinates spelling every character `[project] id` admits beyond
    /// letters and digits — `/`, `.`, `@`, `:`, `-`, `_` — in the shapes the
    /// derivation and a hand-written id produce.
    const COORDINATES: &[&str] = &[
        "github.com/acme/svc-a",
        "h/o/r",
        "logos",
        "my_repo",
        "git@h:o/r",
        "a/b.c@d:e-f",
        "host:2222/o/r",
        "...",
        "a..b",
        "-",
        "@scope/pkg",
    ];

    #[test]
    fn the_codec_round_trips_every_coordinate_character_byte_identically() {
        // UAT-EX-01 / CR-222 CRA-01: a coordinate in the package slot survives
        // parse → format unchanged, so the codec needs no escaping for it.
        for coordinate in COORDINATES {
            for tail in ["src/`lib.rs`/run().", "topic/orders#", "`my file.rs`/f().", "layer/domain/"] {
                let raw = format!("logos . {coordinate} . {tail}");
                let sym = LogosSymbol::parse(&raw).unwrap_or_else(|e| panic!("{raw:?}: {e}"));
                assert_eq!(sym.as_str(), raw, "{raw:?} round-trips byte for byte");
                assert_eq!(sym.coordinate(), Some(*coordinate), "{raw:?}");
                assert_eq!(sym.descriptor_tail(), Some(tail), "{raw:?}");
                assert!(!sym.is_coordinate_less(), "{raw:?}");
            }
        }
    }

    #[test]
    fn a_coordinate_less_symbol_is_told_from_a_canonical_one() {
        let old = LogosSymbol::parse("logos . . . src/`lib.rs`/run().").unwrap();
        assert!(old.is_coordinate_less());
        assert_eq!(old.coordinate(), None);
        assert_eq!(old.descriptor_tail(), Some("src/`lib.rs`/run()."));

        let canonical = LogosSymbol::parse("logos . h/o/r . src/`lib.rs`/run().").unwrap();
        assert!(!canonical.is_coordinate_less());
        assert_eq!(canonical.coordinate(), Some("h/o/r"));

        // A manager or version alone does not make a coordinate: the package
        // slot is the one ADR-85 assigns.
        let versioned = LogosSymbol::parse("logos cargo . 1.0 src/`lib.rs`/run().").unwrap();
        assert!(versioned.is_coordinate_less());
        assert_eq!(versioned.coordinate(), None);
        let policy = LogosSymbol::parse("logos policy h/o/r . layer/domain/").unwrap();
        assert_eq!(policy.coordinate(), Some("h/o/r"));
        assert_eq!(policy.descriptor_tail(), Some("layer/domain/"));

        // A local symbol has no package slot at all.
        let local = LogosSymbol::parse("local 0").unwrap();
        assert!(!local.is_coordinate_less());
        assert_eq!(local.coordinate(), None);
        assert_eq!(local.descriptor_tail(), None);
    }

    #[test]
    fn a_descriptor_tail_rebuilds_its_coordinate_less_symbol() {
        for tail in ["src/`lib.rs`/run().", "topic/orders#", "`my file.rs`/f().", "layer/domain/"] {
            let sym = LogosSymbol::from_descriptor_tail(tail).unwrap_or_else(|e| panic!("{tail:?}: {e}"));
            assert_eq!(sym.as_str(), format!("logos . . . {tail}"));
            assert!(sym.is_coordinate_less());
            assert_eq!(sym.descriptor_tail(), Some(tail), "the inverse of descriptor_tail");
        }
        for not_a_tail in ["", "run", "duplicate_group:0123456789abcdef"] {
            assert!(LogosSymbol::from_descriptor_tail(not_a_tail).is_err(), "{not_a_tail:?} is no tail");
        }
    }

    #[test]
    fn replacing_the_coordinate_keeps_every_other_field() {
        let old = LogosSymbol::parse("logos . . . src/`lib.rs`/S#m(1).").unwrap();
        for coordinate in COORDINATES {
            let new = old.with_coordinate(Some(coordinate)).unwrap();
            assert_eq!(new.as_str(), format!("logos . {coordinate} . src/`lib.rs`/S#m(1)."));
            assert_eq!(new.descriptor_tail(), old.descriptor_tail());
            // Reversible (ADR-85): back to the empty slot, byte for byte.
            assert_eq!(new.with_coordinate(None).unwrap(), old);
            assert_eq!(new.with_coordinate(Some("")).unwrap(), old);
        }
        // From one coordinate to another; manager and version are kept.
        let policy = LogosSymbol::parse("logos policy a/b . boundary/x/y/").unwrap();
        assert_eq!(
            policy.with_coordinate(Some("c/d")).unwrap().as_str(),
            "logos policy c/d . boundary/x/y/"
        );
        // A local symbol has no slot to replace.
        let local = LogosSymbol::parse("local 7").unwrap();
        assert_eq!(local.with_coordinate(Some("h/o/r")).unwrap(), local);
    }

    #[test]
    fn a_coordinate_with_whitespace_is_refused_not_spliced() {
        let old = LogosSymbol::parse("logos . . . src/`lib.rs`/run().").unwrap();
        for bad in ["a b", "a\tb", " a", "a\n"] {
            assert!(old.with_coordinate(Some(bad)).is_err(), "{bad:?} is refused");
        }
    }

    #[test]
    fn node_id_is_a_transparent_i64() {
        let id = NodeId(42);
        assert_eq!(id.get(), 42);
        assert_eq!(serde_json::to_string(&id).unwrap(), "42");
        assert_eq!(id.to_string(), "42");
    }
}
