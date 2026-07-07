//! ABI assertion at grammar load ([FR-PL-02], [FR-PL-03], [NFR-PC-03]).
//!
//! tree-sitter grammars are C parsers generated against a specific tree-sitter
//! ABI version. Linking a grammar whose ABI the runtime cannot understand is
//! undefined behaviour, so the registry verifies every grammar's ABI *before*
//! it is used and degrades a mismatch to a skip-and-warn ([ADR-09]). The
//! `tree_sitter_language::LanguageFn` indirection already decouples each
//! grammar crate's *compile-time* tree-sitter version from the workspace; this
//! check is the *load-time* backstop for the residual runtime skew.
//!
//! The supported range is injected as an [`AbiRange`] rather than read from the
//! runtime constants directly so a unit test can force a mismatch by supplying
//! a range that excludes a real grammar — exercising [UAT-PL-02] without a
//! second build artifact.
//!
//! [FR-PL-02]: ../../../docs/specs/requirements/FR-PL-02.md
//! [FR-PL-03]: ../../../docs/specs/requirements/FR-PL-03.md
//! [NFR-PC-03]: ../../../docs/specs/requirements/NFR-PC-03.md
//! [ADR-09]: ../../../docs/specs/architecture/decisions/ADR-09.md
//! [UAT-PL-02]: ../../../docs/specs/requirements/UAT-PL-02.md

use super::error::SkipReason;

/// The inclusive ABI version range a tree-sitter runtime can load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AbiRange {
    /// Minimum compatible ABI version.
    pub min: usize,
    /// Maximum (current) ABI version.
    pub max: usize,
}

impl AbiRange {
    /// The range of the tree-sitter runtime actually linked into this binary.
    ///
    /// Reads [`tree_sitter::MIN_COMPATIBLE_LANGUAGE_VERSION`] and
    /// [`tree_sitter::LANGUAGE_VERSION`] — the authoritative bounds for the
    /// production load path.
    pub const fn runtime() -> Self {
        Self {
            min: tree_sitter::MIN_COMPATIBLE_LANGUAGE_VERSION,
            max: tree_sitter::LANGUAGE_VERSION,
        }
    }

    /// `true` if `abi` is loadable by this runtime.
    pub const fn contains(&self, abi: usize) -> bool {
        self.min <= abi && abi <= self.max
    }
}

/// Assert a grammar's ABI before it is used.
///
/// Two independent checks, both of which must hold:
/// 1. The *compiled* grammar's ABI must lie within `range` — the real
///    UB-safety guard ([NFR-PC-03]).
/// 2. The descriptor's *declared* `abi_version` must equal the compiled ABI —
///    an integrity cross-check that catches an asset that has drifted from the
///    grammar it ships with.
///
/// Returns `Ok(())` when the grammar is safe to load, or the [`SkipReason`] to
/// record and warn about otherwise ([FR-PL-03]).
pub fn assert_abi(declared: usize, compiled: usize, range: &AbiRange) -> Result<(), SkipReason> {
    if !range.contains(compiled) {
        return Err(SkipReason::AbiUnsupported {
            compiled,
            min: range.min,
            max: range.max,
        });
    }
    if declared != compiled {
        return Err(SkipReason::DescriptorAbiDisagrees { declared, compiled });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_range_is_nonempty_and_well_ordered() {
        let r = AbiRange::runtime();
        assert!(r.min <= r.max, "runtime ABI range must be well-ordered");
    }

    #[test]
    fn in_range_matching_declared_is_ok() {
        let range = AbiRange { min: 13, max: 15 };
        assert_eq!(assert_abi(15, 15, &range), Ok(()));
    }

    #[test]
    fn compiled_abi_above_range_is_unsupported() {
        let range = AbiRange { min: 13, max: 15 };
        assert_eq!(
            assert_abi(99, 99, &range),
            Err(SkipReason::AbiUnsupported {
                compiled: 99,
                min: 13,
                max: 15
            })
        );
    }

    #[test]
    fn compiled_abi_below_range_is_unsupported() {
        let range = AbiRange { min: 13, max: 15 };
        assert_eq!(
            assert_abi(12, 12, &range),
            Err(SkipReason::AbiUnsupported {
                compiled: 12,
                min: 13,
                max: 15
            })
        );
    }

    #[test]
    fn declared_disagreeing_with_compiled_is_a_mismatch() {
        let range = AbiRange { min: 13, max: 15 };
        // Compiled ABI 15 is in range, but the descriptor lied (declared 14).
        assert_eq!(
            assert_abi(14, 15, &range),
            Err(SkipReason::DescriptorAbiDisagrees {
                declared: 14,
                compiled: 15
            })
        );
    }
}
