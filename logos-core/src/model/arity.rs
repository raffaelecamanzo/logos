//! [`ParamRange`] — how many arguments a callable accepts (S-591, [FR-EX-32]).
//!
//! A `Function`/`Method` node records the range `[min, max]` of argument counts
//! a call may pass it: `min` counts the required parameters, a parameter with a
//! default value raises only `max`, and a variadic parameter makes `max`
//! unbounded. A receiver parameter (Rust `self`, Python `self`/`cls`, Go's
//! receiver) is not counted. A callable whose parameters its plugin cannot count
//! reliably records no range at all — **unknown**, which never filters a
//! candidate ([FR-RS-43]).
//!
//! Persisted as `nodes.param_min` / `nodes.param_max` (migration 33): both `NULL`
//! for an unknown range, `param_max` alone `NULL` for an unbounded one.
//!
//! [FR-EX-32]: ../../../../docs/specs/requirements/FR-EX-32.md
//! [FR-RS-43]: ../../../../docs/specs/requirements/FR-RS-43.md

use serde::{Deserialize, Serialize};

/// The argument counts a callable admits: `min..=max`, `max` unbounded when
/// `None` (a variadic parameter).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ParamRange {
    /// The required parameters: the fewest arguments a call may pass.
    pub min: u32,
    /// The most arguments a call may pass, or `None` when a variadic parameter
    /// makes it unbounded.
    pub max: Option<u32>,
}

impl ParamRange {
    /// Whether a call passing `count` arguments fits this range.
    pub const fn admits(self, count: u32) -> bool {
        count >= self.min
            && match self.max {
                Some(max) => count <= max,
                None => true,
            }
    }
}

#[cfg(test)]
mod tests {
    use super::ParamRange;

    /// The bounds are inclusive, and an unbounded range admits every count from
    /// its minimum up.
    #[test]
    fn a_range_admits_exactly_the_counts_between_its_bounds() {
        let bounded = ParamRange { min: 1, max: Some(3) };
        assert!(!bounded.admits(0));
        assert!(bounded.admits(1));
        assert!(bounded.admits(3));
        assert!(!bounded.admits(4));
        let variadic = ParamRange { min: 2, max: None };
        assert!(!variadic.admits(1));
        assert!(variadic.admits(2));
        assert!(variadic.admits(u32::MAX));
    }
}
