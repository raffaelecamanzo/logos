//! Unit tests for the pure scanner core of the dispatch pass ([CR-043]):
//! [`scan_source`] against real parsed Rust fixtures, with no store involved.
//! The end-to-end live-rooting behaviour (marker reconcile, dead-code flip)
//! lives in `tests/dispatch_live_rooting.rs`.
//!
//! [CR-043]: ../../../../docs/requests/CR-043-dead-code-detector-precision.md

use super::*;
use crate::plugin::LanguageRegistry;

/// Scan a Rust source snippet with the compiled-in Rust grammar, returning the
/// 1-based start lines recognised as dispatch-method root entries (sorted).
fn entries(source: &str) -> Vec<i64> {
    let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
    let plugin = registry.for_extension("rs").expect("rust plugin");
    let mut parser = Parser::new();
    scan_source(&mut parser, plugin.language(), source)
        .roots
        .into_iter()
        .map(|e| e.start_line)
        .collect()
}

/// Scan a Rust source snippet, returning the recognised function-pointer handoff
/// handler names (sorted, deduped) — the [`scan_source`] `handoffs` field.
fn handoffs(source: &str) -> Vec<String> {
    let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
    let plugin = registry.for_extension("rs").expect("rust plugin");
    let mut parser = Parser::new();
    scan_source(&mut parser, plugin.language(), source).handoffs
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
fn trait_default_method_is_a_dispatch_entry() {
    // A trait *default* method (a `function_item` with a body inside a
    // `trait_item`) is reachable only through `&dyn T` dispatch and has no
    // source-visible caller — the `LanguagePlugin` cluster shape (`is_documentation`,
    // `is_artifact`, `filenames`, …). It is live-rooted under the same
    // false-live-biased posture as trait-impl methods ([CR-073], [AR-05]).
    let got = entries(
        "\
trait Plugin {
    fn is_documentation(&self) -> bool { false }
}
",
    );
    assert_eq!(got, vec![2], "the trait-default body on line 2 is an entry");
}

#[test]
fn bodyless_trait_method_is_not_an_entry() {
    // A trait method *signature* with no default body is a `function_signature_item`,
    // not a `function_item`: it is not even extracted as a node, so it is not a
    // dispatch entry. Only default *bodies* are rooted.
    let got = entries(
        "\
trait Plugin {
    fn required(&self) -> bool;
}
",
    );
    assert!(
        got.is_empty(),
        "a bodyless trait method signature is not a dispatch entry"
    );
}

#[test]
fn trait_default_and_impl_override_are_both_entries() {
    // The default body AND every impl override are rooted (union reachability):
    // the `LanguagePlugin` default plus `impl LanguagePlugin for CompiledPlugin`.
    let got = entries(
        "\
trait Plugin {
    fn is_artifact(&self) -> bool { false }
}
struct Compiled;
impl Plugin for Compiled {
    fn is_artifact(&self) -> bool { true }
}
",
    );
    assert_eq!(
        got,
        vec![2, 6],
        "the trait default (line 2) and the impl override (line 6) are both entries"
    );
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

// ── Function-pointer handoffs (S-276, CR-068) ────────────────────────────────

#[test]
fn fallback_hands_off_its_handler() {
    // `.fallback(spa_fallback)` hands the fallback handler over by value.
    let got = handoffs("fn r() { Router::new().fallback(spa_fallback); }");
    assert_eq!(got, vec!["spa_fallback".to_string()]);
}

#[test]
fn from_fn_hands_off_its_first_argument() {
    // `from_fn(method_guard)` — the middleware handler is argument 0.
    let got = handoffs("fn r() { app.layer(from_fn(method_guard)); }");
    assert_eq!(got, vec!["method_guard".to_string()]);
}

#[test]
fn pathed_from_fn_is_recognised_by_last_segment() {
    // `middleware::from_fn(host_guard)` — the last `::` segment is `from_fn`.
    let got = handoffs("fn r() { app.layer(middleware::from_fn(host_guard)); }");
    assert_eq!(got, vec!["host_guard".to_string()]);
}

#[test]
fn from_fn_with_state_hands_off_its_second_argument() {
    // `from_fn_with_state(intent, intent_guard)` — arg 0 is the state, arg 1 is
    // the handler. Only the handler is a handoff (the state is not a callable).
    let got = handoffs("fn r() { app.layer(from_fn_with_state(intent, intent_guard)); }");
    assert_eq!(got, vec!["intent_guard".to_string()]);
}

#[test]
fn method_router_constructor_hands_off_its_handler() {
    // `route(path, get(handler))` — the method-router constructor inside the
    // `.route(_, …)` second argument hands its handler over by value.
    let got = handoffs("fn r() { Router::new().route(\"/\", get(spa_shell)); }");
    assert_eq!(got, vec!["spa_shell".to_string()]);
}

#[test]
fn chained_method_router_setters_hand_off_every_handler() {
    // `route(path, get(a).post(b))` — the chained `.post(b)` setter is a method
    // call, so the walk recurses through the chain and roots BOTH `a` and `b`
    // (the `web/src/lib.rs` `get(spa_shell).post(chat_turn)` shape).
    let got = handoffs("fn r() { app.route(\"/c\", get(shell).post(submit)); }");
    assert_eq!(got, vec!["shell".to_string(), "submit".to_string()]);
}

#[test]
fn bare_method_router_head_outside_a_route_is_not_a_handoff() {
    // A method-router head is recognised ONLY inside a `route(...)` argument: a
    // bare `head(local_fn)` / `get(local_fn)` elsewhere (a same-named local call)
    // must not falsely live-root its argument (precision, no over-rooting).
    assert!(
        handoffs("fn r() { let x = head(local_fn); }").is_empty(),
        "a bare method-router head outside `route(...)` is not a handoff"
    );
    assert!(
        handoffs("fn r() { let y = any(other_fn); }").is_empty(),
        "a bare `any(...)` outside `route(...)` is not a handoff"
    );
}

#[test]
fn pathed_and_turbofished_router_constructors_are_recognised() {
    // `axum::routing::post(h)` (path-qualified) and `get::<T>(h)` (turbofished).
    let pathed = handoffs("fn r() { app.route(\"/x\", axum::routing::post(create)); }");
    assert_eq!(pathed, vec!["create".to_string()]);
    let turbo = handoffs("fn r() { app.route(\"/x\", get::<u8>(read)); }");
    assert_eq!(turbo, vec!["read".to_string()]);
}

#[test]
fn scoped_handler_takes_its_last_segment() {
    // A cross-module handler path yields its last segment; same-file resolution
    // in `run` then decides whether any callable of that name lives in the file.
    let got = handoffs("fn r() { app.route(\"/\", get(api_v1::overview)); }");
    assert_eq!(got, vec!["overview".to_string()]);
}

#[test]
fn method_call_get_is_not_a_handoff() {
    // `map.get(key)` is an ordinary method call (a `field_expression` head), not
    // an axum router constructor — it must never be mistaken for a route handler.
    let got = handoffs("fn r() { let v = map.get(key); }");
    assert!(got.is_empty(), "a `.get()` method call is not a handoff: {got:?}");
}

#[test]
fn closure_and_non_identifier_handlers_are_not_handoffs() {
    // A closure or other non-path argument is not a resolvable handler name.
    assert!(handoffs("fn r() { app.layer(from_fn(|req, next| next)); }").is_empty());
    assert!(handoffs("fn r() { Router::new().fallback(make_service()); }").is_empty());
}

#[test]
fn handoffs_are_deduped_and_sorted() {
    // Two sites naming the same handler collapse to one; the list is sorted.
    let got = handoffs(
        "\
fn r() {
    Router::new()
        .route(\"/a\", get(zebra))
        .route(\"/b\", get(alpha))
        .fallback(zebra);
}
",
    );
    assert_eq!(got, vec!["alpha".to_string(), "zebra".to_string()]);
}
