//! Unit tests for the pure scanner core of the dispatch pass ([CR-043]):
//! [`scan_source`] against real parsed Rust fixtures, with no store involved.
//! The end-to-end live-rooting behaviour (marker reconcile, dead-code flip)
//! lives in `tests/dispatch_live_rooting.rs`.
//!
//! [CR-043]: ../../../../docs/requests/CR-043-dead-code-detector-precision.md

use super::*;
use crate::plugin::LanguageRegistry;

/// Scan a Rust source snippet with the compiled-in Rust grammar, returning the
/// 1-based start lines recognised as dispatch entries (sorted).
fn entries(source: &str) -> Vec<i64> {
    let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
    let plugin = registry.for_extension("rs").expect("rust plugin");
    let mut parser = Parser::new();
    scan_source(&mut parser, plugin.language(), source)
        .into_iter()
        .map(|e| e.start_line)
        .collect()
}

#[test]
fn trait_impl_method_is_a_dispatch_entry() {
    // `impl Trait for Type` methods are framework-dispatched (vtable / trait
    // object), so each is live-rooted — the `on_event`/`record_str` shape.
    let got = entries(
        "\
struct Layer;
trait Sink { fn on_event(&self); }
impl Sink for Layer {
    fn on_event(&self) {}
}
",
    );
    assert_eq!(got, vec![4], "the trait-impl method on line 4 is an entry");
}

#[test]
fn generic_trait_impl_method_is_a_dispatch_entry() {
    // The real `impl<S: Subscriber> Layer<S> for TelemetryLayer` shape: the
    // `trait:` field is a `generic_type`, still a trait impl.
    let got = entries(
        "\
struct T;
impl<S> Handler<S> for T {
    fn on_event(&self, _s: S) {}
}
",
    );
    assert_eq!(got, vec![3]);
}

#[test]
fn inherent_impl_method_without_attribute_is_not_an_entry() {
    // A plain inherent-impl method is reachable by ordinary call binding, so it
    // is NOT live-rooted — preserving the detector's ability to flag it dead.
    let got = entries(
        "\
struct S;
impl S {
    fn helper(&self) {}
}
",
    );
    assert!(got.is_empty(), "an inherent method is not a dispatch entry");
}

#[test]
fn dispatch_attribute_method_is_an_entry() {
    // The rmcp `#[tool]` shape on an inherent-impl method — the macro generates
    // the router that invokes it, so it has no source-visible caller.
    let got = entries(
        "\
struct S;
impl S {
    #[tool(description = \"x\")]
    async fn session_end(&self) {}
}
",
    );
    assert_eq!(got, vec![4], "the #[tool] method on line 4 is an entry");
}

#[test]
fn pathed_dispatch_attribute_is_recognised_by_last_segment() {
    // `#[rmcp::tool]` — the last `::` segment is `tool`.
    let got = entries(
        "\
struct S;
impl S {
    #[rmcp::tool]
    fn t(&self) {}
}
",
    );
    assert_eq!(got, vec![4]);
}

#[test]
fn non_dispatch_attribute_is_not_an_entry() {
    // `#[inline]`/`#[must_use]` etc. on an inherent method do not mark dispatch.
    let got = entries(
        "\
struct S;
impl S {
    #[inline]
    fn helper(&self) {}
}
",
    );
    assert!(got.is_empty());
}

#[test]
fn free_function_is_never_an_entry() {
    // The scanner only considers methods inside `impl` blocks; a free function
    // (even an attributed one) is left to ordinary reachability.
    let got = entries(
        "\
#[tool]
fn standalone() {}
",
    );
    assert!(got.is_empty());
}

#[test]
fn comment_between_attribute_and_method_does_not_detach_it() {
    // A doc/line comment between the attribute run and the item keeps the
    // attribute attached (mirrors the extraction test-marker walk).
    let got = entries(
        "\
struct S;
impl S {
    #[tool]
    // a comment between the attribute and the fn
    fn t(&self) {}
}
",
    );
    assert_eq!(got, vec![5], "the fn on line 5 is still an entry");
}

#[test]
fn multiple_entries_are_sorted_and_deduped() {
    let got = entries(
        "\
trait Tr { fn a(&self); fn b(&self); }
struct S;
impl Tr for S {
    fn a(&self) {}
    fn b(&self) {}
}
impl S {
    #[tool]
    fn c(&self) {}
}
",
    );
    assert_eq!(got, vec![4, 5, 9]);
}

#[test]
fn empty_and_unparsable_source_yields_nothing() {
    assert!(entries("").is_empty());
    // Still parses to *something*, but no impl methods.
    assert!(entries("fn main() {}").is_empty());
}

#[test]
fn block_comment_between_attribute_and_method_does_not_detach_it() {
    // The `block_comment` skip branch of `has_dispatch_attribute` (the line-comment
    // case is covered above).
    let got = entries(
        "\
struct S;
impl S {
    #[tool]
    /* a block comment between the attribute and the fn */
    fn t(&self) {}
}
",
    );
    assert_eq!(got, vec![5], "the fn on line 5 is still an entry");
}

#[test]
fn dispatch_attribute_recognised_among_stacked_attributes_either_order() {
    // The attribute run is walked backward through every preceding `attribute_item`;
    // a non-dispatch attr must not stop the walk before the dispatch attr is seen,
    // in either ordering.
    let dispatch_above = entries(
        "\
struct S;
impl S {
    #[tool]
    #[allow(dead_code)]
    fn a(&self) {}
}
",
    );
    assert_eq!(dispatch_above, vec![5], "#[tool] above #[allow] is found");

    let dispatch_below = entries(
        "\
struct S;
impl S {
    #[allow(dead_code)]
    #[tool]
    fn b(&self) {}
}
",
    );
    assert_eq!(dispatch_below, vec![5], "#[tool] below #[allow] is found");
}
