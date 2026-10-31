//! The **dispatch marker** — the payload a trait-typed call's fan-out stamps on
//! each edge to an implementation (S-712, [CR-226] BR-76, [FR-RS-08]).
//!
//! A call through a trait binds to the trait's own method and to every
//! implementation of it ([FR-RS-47] rule 5). The edge to the trait's method is
//! the caller's dependency and carries no marker. Each edge to an
//! implementation carries `dispatch:<trait method symbol>`: the caller reaches
//! it at run time, but depends only on the trait. A direct call to the same
//! implementation from the same caller leaves the edge unmarked — a dependency
//! proven directly is never hidden by dispatch.
//!
//! Every reader and writer of the marker goes through this module, so its
//! spelling has one owner.
//!
//! [CR-226]: ../../../../docs/requests/CR-226-dispatch-is-not-a-dependency.md
//! [FR-RS-08]: ../../../../docs/specs/requirements/FR-RS-08.md
//! [FR-RS-47]: ../../../../docs/specs/requirements/FR-RS-47.md

/// The prefix every dispatch payload starts with — the SQL pattern the store
/// matches it by is [`DISPATCH_GLOB`].
pub const DISPATCH_PREFIX: &str = "dispatch:";

/// [`DISPATCH_PREFIX`] as an SQL `GLOB` pattern: `dispatch:*`. `GLOB`, not
/// `LIKE`, because `LIKE` folds ASCII case and would admit `Dispatch:`; the
/// prefix holds none of `GLOB`'s metacharacters (`*`, `?`, `[`).
pub const DISPATCH_GLOB: &str = "dispatch:*";

/// The payload of an edge reached by dispatch through the trait method
/// `method_symbol`.
pub fn dispatch_payload(method_symbol: &str) -> String {
    format!("{DISPATCH_PREFIX}{method_symbol}")
}

/// The trait method symbol a dispatch `payload` names, or `None` for any other
/// payload.
pub fn dispatched_method(payload: &str) -> Option<&str> {
    payload.strip_prefix(DISPATCH_PREFIX)
}

/// The `via` label of a row reached through a dispatch edge whose trait method
/// is `method_symbol`: `"<Trait>::<method>"`, read off the symbol's last two
/// descriptors. `None` when the symbol has fewer than two, or does not parse.
pub fn via_label(method_symbol: &str) -> Option<String> {
    let (owner, member) = super::convert::owner_and_member(method_symbol)?;
    Some(format!("{owner}::{member}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMIT: &str = "logos . github.com/o/r . src/`events.rs`/EventSink#emit().";

    #[test]
    fn a_payload_round_trips_its_method_and_no_other_payload_names_one() {
        let payload = dispatch_payload(EMIT);
        assert_eq!(payload, format!("dispatch:{EMIT}"));
        assert_eq!(dispatched_method(&payload), Some(EMIT));
        // The relation tokens other edges carry, and a near miss, name none.
        for other in ["partial", "dispatch", "dispatched:x", " dispatch:x", "Dispatch:x"] {
            assert_eq!(dispatched_method(other), None, "{other:?}");
        }
    }

    #[test]
    fn the_label_is_the_trait_and_its_method() {
        assert_eq!(via_label(EMIT).as_deref(), Some("EventSink::emit"));
        // A trait inside an inline module is labelled by the trait alone.
        let nested = "logos . github.com/o/r . src/`lib.rs`/sinks/Sink#flush().";
        assert_eq!(via_label(nested).as_deref(), Some("Sink::flush"));
        // A symbol with one descriptor, a local one, or garbage, labels nothing.
        for unlabelled in ["logos . github.com/o/r . emit().", "local sym42", "not a symbol"] {
            assert_eq!(via_label(unlabelled), None, "{unlabelled:?}");
        }
    }
}
