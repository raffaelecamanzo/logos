//! Unit tests for the pure scanner core of the framework pass (S-012):
//! [`scan_source`] against real parsed Rust fixtures, with no store involved.
//! The end-to-end promotion behaviour (binding, reconcile, stats) lives in
//! `tests/framework_extraction.rs`.

use super::*;
use crate::plugin::LanguageRegistry;

/// Scan a Rust source snippet with the compiled-in plugin set.
fn scan(source: &str) -> FileMatches {
    let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
    let plugin = registry.for_extension("rs").expect("rust plugin");
    let mut parser = Parser::new();
    scan_source(&mut parser, plugin, source)
}

/// Shorthand for the `(path, method, handler)` projection of scanned routes.
fn routes(source: &str) -> Vec<(String, String, Option<String>)> {
    scan(source)
        .routes
        .into_iter()
        .map(|r| (r.path, r.method, r.handler))
        .collect()
}

// ── Axum `.route` registrations ──────────────────────────────────────────────

#[test]
fn axum_route_with_method_router_is_matched() {
    let got = routes(
        "\
use axum::routing::get;
fn app() {
    let _ = axum::Router::new().route(\"/users\", get(list_users));
}
async fn list_users() {}
",
    );
    assert_eq!(
        got,
        vec![(
            "/users".to_string(),
            "GET".to_string(),
            Some("list_users".to_string())
        )]
    );
}

#[test]
fn axum_chained_router_yields_every_method() {
    let got = routes(
        "\
fn app() {
    let _ = axum::Router::new().route(\"/items\", get(list).post(create));
}
",
    );
    assert_eq!(
        got,
        vec![
            (
                "/items".to_string(),
                "GET".to_string(),
                Some("list".to_string())
            ),
            (
                "/items".to_string(),
                "POST".to_string(),
                Some("create".to_string())
            ),
        ]
    );
}

#[test]
fn axum_scoped_router_path_and_handler_are_kept_verbatim() {
    let got = routes(
        "\
fn app() {
    let _ = r.route(\"/health\", axum::routing::get(handlers::health));
}
",
    );
    assert_eq!(
        got,
        vec![(
            "/health".to_string(),
            "GET".to_string(),
            Some("handlers::health".to_string())
        )]
    );
}

#[test]
fn axum_closure_handler_keeps_route_but_no_handler() {
    let got = routes("fn app() { let _ = r.route(\"/ping\", get(|| async { \"pong\" })); }");
    assert_eq!(got, vec![("/ping".to_string(), "GET".to_string(), None)]);
}

#[test]
fn axum_unknown_chain_links_are_skipped_but_chain_survives() {
    // `.fallback(x)` is not a method router — the chain left of it still
    // yields its registration.
    let got = routes("fn app() { let _ = r.route(\"/a\", get(list).fallback(other)); }");
    assert_eq!(
        got,
        vec![(
            "/a".to_string(),
            "GET".to_string(),
            Some("list".to_string())
        )]
    );
}

// ── Actix builder + attribute forms ──────────────────────────────────────────

#[test]
fn actix_route_builder_to_is_matched() {
    let got = routes("fn app() { let _ = app.route(\"/index\", web::get().to(index)); }");
    assert_eq!(
        got,
        vec![(
            "/index".to_string(),
            "GET".to_string(),
            Some("index".to_string())
        )]
    );
}

#[test]
fn actix_method_attribute_promotes_the_following_fn() {
    let m = scan(
        "\
#[get(\"/health\")]
async fn health() -> impl Responder { \"ok\" }
",
    );
    assert_eq!(m.routes.len(), 1);
    let r = &m.routes[0];
    assert_eq!(
        (r.path.as_str(), r.method.as_str(), r.handler.as_deref()),
        ("/health", "GET", Some("health"))
    );
    assert_eq!(r.start_line, 1, "route anchors at the attribute");
    assert_eq!(r.end_line, 2, "…and spans through the handler fn");
}

#[test]
fn actix_attribute_skips_interleaved_attributes_and_doc_comments() {
    let m = scan(
        "\
#[post(\"/items\")]
/// Creates an item.
#[allow(dead_code)]
async fn create_item() {}
",
    );
    assert_eq!(m.routes.len(), 1);
    assert_eq!(m.routes[0].handler.as_deref(), Some("create_item"));
    assert_eq!(m.routes[0].method, "POST");
}

#[test]
fn non_method_attributes_are_not_routes() {
    let m = scan(
        "\
#[derive(Debug)]
struct S;
#[cfg(feature = \"x\")]
fn not_a_route() {}
#[deprecated(note = \"/looks/like/a/path\")]
fn also_not() {}
",
    );
    assert!(m.routes.is_empty(), "{:?}", m.routes);
}

#[test]
fn method_attribute_on_a_non_function_item_is_ignored() {
    let m = scan("#[get(\"/p\")]\nstruct NotAHandler;\n");
    assert!(m.routes.is_empty(), "{:?}", m.routes);
}

// ── No spurious matches (FR-FW-04 at the scanner level) ─────────────────────

#[test]
fn plain_rust_yields_no_matches() {
    let m = scan(
        "\
use std::collections::HashMap;

pub fn transform(input: &str) -> String {
    let map: HashMap<String, Vec<u32>> = HashMap::new();
    format!(\"{}{}\", input, map.len())
}

pub struct Config { pub retries: u32 }
",
    );
    assert_eq!(m, FileMatches::default());
}

#[test]
fn non_route_string_method_calls_are_not_routes() {
    // `.split("…")` and friends share the anchor shape (`.m("str")`) but are
    // not `route` — the scanner must reject them by name.
    let m = scan("fn f(s: &str) { let _ = s.split(\"/\"); let _ = s.replace(\"/a\", \"b\"); }");
    assert!(m.routes.is_empty(), "{:?}", m.routes);
}

#[test]
fn route_call_without_string_path_is_ignored() {
    // Actix `web::resource("/p").route(web::get().to(h))` — the `.route` arg
    // is not a string literal; a documented v1 limitation, not a match.
    let m = scan("fn app() { let _ = web::resource(\"/p\").route(web::get().to(h)); }");
    assert!(m.routes.is_empty(), "{:?}", m.routes);
}

// ── Shared-state components ──────────────────────────────────────────────────

#[test]
fn axum_state_extractor_yields_a_component_candidate() {
    let m = scan("async fn list(State(s): State<AppState>) {}");
    assert_eq!(
        m.components,
        vec![ComponentMatch {
            type_path: "AppState".to_string()
        }]
    );
}

#[test]
fn actix_data_extractor_and_arc_unwrap() {
    let m = scan(
        "\
async fn a(data: web::Data<AppState>) {}
async fn b(State(s): State<Arc<Shared>>) {}
",
    );
    let paths: Vec<&str> = m.components.iter().map(|c| c.type_path.as_str()).collect();
    assert_eq!(paths, vec!["AppState", "Shared"]);
}

#[test]
fn scoped_state_type_path_is_kept_verbatim() {
    let m = scan("async fn h(State(s): State<state::AppState>) {}");
    assert_eq!(m.components[0].type_path, "state::AppState");
}

#[test]
fn ordinary_generic_params_are_not_components() {
    let m = scan("fn f(v: Vec<String>, m: HashMap<K, V>, o: Option<AppState>) {}");
    assert!(m.components.is_empty(), "{:?}", m.components);
}

#[test]
fn non_path_state_arguments_are_skipped() {
    // Tuples / references inside the extractor are beyond the v1 heuristic.
    let m = scan("async fn h(State(s): State<(A, B)>) {}");
    assert!(m.components.is_empty(), "{:?}", m.components);
}

#[test]
fn only_smart_pointer_wrappers_are_transparent() {
    // `Arc`/`Rc`/`Box` layers unwrap; any other generic wrapper does not —
    // `State<Option<T>>` is not a state type the v1 heuristic understands.
    let m = scan(
        "\
async fn a(State(s): State<Option<AppState>>) {}
async fn b(State(s): State<Box<Arc<Inner>>>) {}
",
    );
    let paths: Vec<&str> = m.components.iter().map(|c| c.type_path.as_str()).collect();
    assert_eq!(paths, vec!["Inner"]);
}

// ── Helpers ──────────────────────────────────────────────────────────────────

#[test]
fn last_segment_strips_scoping() {
    assert_eq!(last_segment("web::get"), "get");
    assert_eq!(last_segment("axum::routing::post"), "post");
    assert_eq!(last_segment("get"), "get");
}

#[test]
fn raw_and_empty_string_literals_are_read_correctly() {
    let got = routes("fn f() { let _ = r.route(r\"/raw\", get(h)); }");
    assert_eq!(got[0].0, "/raw");
    let empty = routes("fn f() { let _ = r.route(\"\", get(h)); }");
    assert_eq!(empty[0].0, "");
}

// ── S-015: declarative-contract helpers ──────────────────────────────────────

/// `matches_detector` accepts the detector itself and whole-`::`-segment
/// extensions, never a sibling sharing a string prefix (FR-FW-04 — a
/// candidate gate that over-matched `axumish` under `axum` would scan files
/// the descriptor never claimed).
#[test]
fn detector_matching_is_whole_segment_only() {
    assert!(matches_detector("axum", "axum"));
    assert!(matches_detector("axum::routing::get", "axum"));
    assert!(matches_detector(
        "org::springframework::web",
        "org::springframework"
    ));
    assert!(!matches_detector("axumish", "axum"));
    assert!(!matches_detector("axum_extra::extract", "axum"));
    assert!(!matches_detector("ax", "axum"));
}

/// One declarative registration site matched by overlapping patterns (a
/// handler-bearing and a handler-less variant) collapses to one route, and
/// the proven handler wins regardless of pattern order (S-015 dedup rule).
#[test]
fn dedup_prefers_the_proven_handler_and_is_first_wins_otherwise() {
    let route = |method: &str, path: &str, handler: Option<&str>, line: u32| RouteMatch {
        path: path.to_string(),
        method: method.to_string(),
        handler: handler.map(str::to_string),
        start_line: line,
        end_line: line,
    };

    // Handler-less first, handler-bearing second: the upgrade fires.
    let mut upgraded = vec![
        route("GET", "/users", None, 3),
        route("GET", "/users", Some("list_users"), 3),
    ];
    dedup_routes(&mut upgraded);
    assert_eq!(upgraded.len(), 1);
    assert_eq!(upgraded[0].handler.as_deref(), Some("list_users"));

    // Both proven: first wins.
    let mut first_wins = vec![
        route("GET", "/users", Some("first"), 3),
        route("GET", "/users", Some("second"), 9),
    ];
    dedup_routes(&mut first_wins);
    assert_eq!(first_wins.len(), 1);
    assert_eq!(first_wins[0].handler.as_deref(), Some("first"));

    // Distinct (method, path) keys both survive, order preserved.
    let mut distinct = vec![
        route("GET", "/users", Some("list"), 3),
        route("POST", "/users", Some("create"), 4),
    ];
    dedup_routes(&mut distinct);
    assert_eq!(distinct.len(), 2);
    assert_eq!(distinct[0].method, "GET");
    assert_eq!(distinct[1].method, "POST");
}
