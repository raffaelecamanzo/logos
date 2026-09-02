//! Unit tests for the pure scanner core of the framework pass (S-012):
//! [`scan_source`] against real parsed fixtures — Rust for the legacy
//! structural anchors, Java for the declarative capture contract (S-328) —
//! with no store involved. The end-to-end promotion behaviour (binding,
//! reconcile, stats) lives in `tests/framework_extraction.rs`, and its
//! per-language face in `tests/multilang.rs`.

use super::*;
use crate::plugin::LanguageRegistry;

/// Scan a source snippet with the plugin registered for `ext` — the capture
/// dialects are per-language, so each language arm enters through its own
/// extension.
fn scan_lang(ext: &str, source: &str) -> FileMatches {
    let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
    let plugin = registry
        .for_extension(ext)
        .unwrap_or_else(|| panic!("{ext} plugin"));
    let mut parser = Parser::new();
    scan_source(&mut parser, plugin, source)
}

/// Scan a Rust source snippet with the compiled-in plugin set.
fn scan(source: &str) -> FileMatches {
    scan_lang("rs", source)
}

/// The `(path, method, handler)` projection of a scan's routes.
fn route_triples(m: FileMatches) -> Vec<(String, String, Option<String>)> {
    m.routes
        .into_iter()
        .map(|r| (r.path, r.method, r.handler))
        .collect()
}

/// Shorthand for the `(path, method, handler)` projection of scanned routes.
fn routes(source: &str) -> Vec<(String, String, Option<String>)> {
    route_triples(scan(source))
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
        origin: PathOrigin::default(),
        at: 0,
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

/// The named-over-positional precedence pass (S-328) is scoped to one
/// registration site and leaves anchor-less matches — every legacy Rust
/// walker — untouched.
#[test]
fn outranked_positional_paths_are_dropped_only_within_their_own_site() {
    let route = |path: &str, site: Option<usize>, named: bool| RouteMatch {
        path: path.to_string(),
        method: "GET".to_string(),
        handler: Some("h".to_string()),
        start_line: 1,
        end_line: 1,
        origin: PathOrigin { site, named },
        at: site.unwrap_or(0),
    };

    let mut routes = vec![
        // Site 1 proves a named path: its positional literal is outranked.
        route("/positional", Some(1), false),
        route("/named", Some(1), true),
        // Site 2 has only a positional one — a different site's named path
        // never suppresses it.
        route("/other", Some(2), false),
        // The legacy walkers name no site and always survive.
        route("/legacy", None, false),
    ];
    drop_outranked_paths(&mut routes);
    let kept: Vec<&str> = routes.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(kept, ["/named", "/other", "/legacy"]);

    // With no named path anywhere, nothing is dropped.
    let mut positional_only = vec![route("/a", Some(1), false), route("/b", None, false)];
    drop_outranked_paths(&mut positional_only);
    assert_eq!(positional_only.len(), 2);
}

/// Precedence never costs a proven handler: where the dropped positional
/// match is the one that named a handler, the surviving named path inherits
/// it rather than losing its `RoutesTo` edge. One site is one registration,
/// so the handler is the same method by construction — nothing is fabricated.
#[test]
fn a_dropped_positional_match_hands_its_proven_handler_to_the_survivor() {
    let route = |path: &str, handler: Option<&str>, named: bool| RouteMatch {
        path: path.to_string(),
        method: "GET".to_string(),
        handler: handler.map(str::to_string),
        start_line: 1,
        end_line: 1,
        origin: PathOrigin {
            site: Some(7),
            named,
        },
        at: 7,
    };

    let mut routes = vec![
        route("/x", Some("handle"), false),
        route("/x", None, true),
        route("/y", None, true),
    ];
    drop_outranked_paths(&mut routes);
    let kept: Vec<(&str, Option<&str>)> = routes
        .iter()
        .map(|r| (r.path.as_str(), r.handler.as_deref()))
        .collect();
    assert_eq!(kept, [("/x", Some("handle")), ("/y", Some("handle"))]);
}

// ── Prefix composition, language-independent (S-329) ─────────────────────────

/// Separator normalisation, exhaustively: the four slash combinations plus the
/// two degenerate ends ([FR-FW-05]). `/v1//users` and `/v1users` are the two
/// failures the rule exists to prevent, so both are asserted absent by
/// construction — every case names its exact expected output.
#[test]
fn joining_a_prefix_and_a_path_yields_exactly_one_separator() {
    for (prefix, path, want) in [
        // The four cases the acceptance criterion enumerates.
        ("/v1", "/users", "/v1/users"),
        ("/v1", "users", "/v1/users"),
        ("/v1/", "/users", "/v1/users"),
        ("/v1/", "users", "/v1/users"),
        // Multi-segment on both sides — the rule is about the seam only.
        ("/api/v1/", "/users/{id}", "/api/v1/users/{id}"),
        // A prefix that is just the root contributes no segment.
        ("/", "/users", "/users"),
        // An unwritten method path leaves the prefix as the whole path, and a
        // path that is only a slash keeps the trailing one Spring maps.
        ("/v1", "", "/v1"),
        ("/v1", "/", "/v1/"),
        // Surrounding whitespace in a literal is not part of the path.
        ("  /v1  ", "  /users  ", "/v1/users"),
        // Composition joins; it does not absolutise (see `join_route_path`).
        ("v1", "/users", "v1/users"),
        ("", "/users", "/users"),
    ] {
        let got = join_route_path(prefix, path);
        assert_eq!(got, want, "join({prefix:?}, {path:?})");
        assert!(!got.contains("//"), "join({prefix:?}, {path:?}) = {got:?}");
    }
}

/// A [`PrefixScope`] built by hand, the way a query match would.
#[cfg(test)]
fn scope(start: usize, end: usize, literals: &[(&str, usize, usize)], opaque: &[(usize, usize)]) -> PrefixScope {
    PrefixScope {
        start,
        end,
        literals: literals
            .iter()
            .map(|(text, s, e)| PrefixLiteral {
                text: (*text).to_string(),
                start: *s,
                end: *e,
                resolvable: is_resolvable_prefix(text),
            })
            .collect(),
        opaque: opaque.to_vec(),
    }
}

/// A [`RouteMatch`] at byte `at`.
#[cfg(test)]
fn route_at(path: &str, at: usize) -> RouteMatch {
    RouteMatch {
        path: path.to_string(),
        method: "GET".to_string(),
        handler: Some("h".to_string()),
        start_line: 1,
        end_line: 1,
        origin: PathOrigin::default(),
        at,
    }
}

/// The reuse claim of [S-329], asserted without a query: composition is driven
/// entirely by [`PrefixScope`] byte ranges and [`RouteMatch::at`], so a second
/// dialect inherits every rule below by naming the captures and adding no code
/// (S-330). If this test can express a language's behaviour, that language
/// needs no implementation.
#[test]
fn composition_is_driven_by_capture_data_alone() {
    let mut out = FileMatches {
        routes: vec![
            // Inside the literal scope 0..100 → composed.
            route_at("/users", 10),
            // Inside the nested scope 20..40 → the *innermost* prefix wins,
            // exactly as Spring binds a nested type to its own prefix.
            route_at("/inner", 30),
            // Inside the opaque scope 200..300 → refused, never partial.
            route_at("/orphan", 250),
            // Inside a *boundary* that declared no prefix, itself nested in
            // the literal scope: the boundary wins and contributes nothing,
            // so the route keeps its own path and is not refused (BR-46).
            route_at("/boundary", 60),
            // Containment is half-open, so the scope's first byte is inside it
            // and the byte at `end` belongs to the next sibling.
            route_at("/at-start", 0),
            route_at("/at-end", 100),
            // Outside every scope → untouched, and not refused (BR-46).
            route_at("/free", 500),
        ],
        pathless: vec![
            // Prefix-only inside the literal scope, and a pathless
            // registration with no prefix at all (dropped, not refused).
            route_at("", 15),
            route_at("", 500),
        ],
        prefixes: vec![
            scope(0, 100, &[("/v1", 1, 5)], &[]),
            scope(20, 40, &[("/v1/inner", 21, 30)], &[]),
            scope(50, 70, &[], &[]),
            scope(200, 300, &[], &[(201, 205)]),
        ],
        ..FileMatches::default()
    };
    compose_prefixes(&mut out);

    let mut got: Vec<String> = out.routes.iter().map(|r| r.path.clone()).collect();
    got.sort();
    assert_eq!(
        got,
        [
            "/at-end",
            "/boundary",
            "/free",
            "/v1",
            "/v1/at-start",
            "/v1/inner/inner",
            "/v1/users"
        ]
    );
    // Exactly one refusal — the opaque scope's route. The unprefixed pathless
    // candidate contributed none.
    assert_eq!(out.refusals, [RouteRefusal::PathNotComposed]);
    // The pending list is always drained: an empty `path` never escapes.
    assert!(out.pathless.is_empty());
    assert!(
        out.routes.iter().all(|r| !r.path.is_empty()),
        "{:?}",
        out.routes
    );
}

/// A type declaring several literal prefixes fans each route out over them,
/// deterministically (sorted, deduplicated) whatever order the query matched
/// them in ([NFR-RA-06]).
#[test]
fn several_prefixes_on_one_scope_fan_the_route_out_deterministically() {
    let mut out = FileMatches {
        routes: vec![route_at("/users", 5)],
        prefixes: vec![scope(
            0,
            10,
            &[("/b", 1, 2), ("/a", 3, 4), ("/b", 5, 6)],
            &[],
        )],
        ..FileMatches::default()
    };
    compose_prefixes(&mut out);
    let got: Vec<&str> = out.routes.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(got, ["/a/users", "/b/users"]);
}

/// Scopes that share a range are **combined**, not arbitrated — and the outcome
/// is identical whichever order the query matched them in ([NFR-RA-06]). This
/// is the ordinary case, not an exotic one: the bare type-boundary pattern and
/// a prefix pattern both match the same declaration on every prefixed class.
#[test]
fn scopes_sharing_a_range_are_combined_in_either_order() {
    let boundary = scope(0, 100, &[], &[]);
    let declared = scope(0, 100, &[("/v1", 1, 5)], &[]);
    for (label, prefixes) in [
        ("boundary first", vec![boundary.clone(), declared.clone()]),
        ("declared first", vec![declared, boundary]),
    ] {
        let mut out = FileMatches {
            routes: vec![route_at("/users", 10)],
            prefixes,
            ..FileMatches::default()
        };
        compose_prefixes(&mut out);
        let got: Vec<&str> = out.routes.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(got, ["/v1/users"], "{label}");
        assert!(out.refusals.is_empty(), "{label}: {:?}", out.refusals);
    }
}

/// An opaque capture disqualifies the prefix literals it **overlaps**, not the
/// whole scope — and in both directions. This is the seam that lets a second
/// grammar refuse its own unreadable syntax with no Rust change (S-330): a
/// Kotlin `"$BASE/v1"` string template captures its `interpolation` child,
/// which sits *inside* the literal.
#[test]
fn an_opaque_capture_disqualifies_only_the_literals_it_overlaps() {
    // A fragment INSIDE a literal (Kotlin's `"$BASE/v1"`): the literal spans
    // 10..20, the interpolation 11..16 — the literal is unreadable.
    let mut inside = FileMatches {
        routes: vec![route_at("/users", 50)],
        prefixes: vec![scope(0, 100, &[("$BASE/v1", 10, 20)], &[(11, 16)])],
        ..FileMatches::default()
    };
    compose_prefixes(&mut inside);
    assert!(inside.routes.is_empty(), "{:?}", inside.routes);
    assert_eq!(inside.refusals, [RouteRefusal::PathNotComposed]);

    // A mixed list: the opaque node overlaps only the second element, so the
    // first still composes and nothing is refused.
    let mut mixed = FileMatches {
        routes: vec![route_at("/users", 50)],
        prefixes: vec![scope(0, 100, &[("/a", 10, 14)], &[(16, 20)])],
        ..FileMatches::default()
    };
    compose_prefixes(&mut mixed);
    let got: Vec<&str> = mixed.routes.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(got, ["/a/users"]);
    assert!(mixed.refusals.is_empty(), "{:?}", mixed.refusals);

    // An opaque capture IDENTICAL to a literal is that literal restated — the
    // exemption that lets a query mark the whole path position opaque with a
    // supertype pattern without having to exclude the literal case.
    let mut identical = FileMatches {
        routes: vec![route_at("/users", 50)],
        prefixes: vec![scope(0, 100, &[("/a", 10, 14)], &[(10, 14)])],
        ..FileMatches::default()
    };
    compose_prefixes(&mut identical);
    let got: Vec<&str> = identical.routes.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(got, ["/a/users"]);
    assert!(identical.refusals.is_empty(), "{:?}", identical.refusals);
}

/// One refused **registration** is one refusal, however many path candidates
/// it produced: a list-valued method path is several `RouteMatch`es for one
/// annotation, and `routes_not_composed` counts registrations. A match with no
/// anchor names no site and is counted on its own.
#[test]
fn a_refused_registration_is_counted_once_however_many_paths_it_wrote() {
    let sited = |path: &str, site: usize| RouteMatch {
        origin: PathOrigin {
            site: Some(site),
            named: true,
        },
        ..route_at(path, 50)
    };
    let mut out = FileMatches {
        // Two paths from ONE annotation (site 7), one from another (site 9),
        // and one anchor-less match.
        routes: vec![
            sited("/a", 7),
            sited("/b", 7),
            sited("/c", 9),
            route_at("/d", 50),
        ],
        prefixes: vec![scope(0, 100, &[], &[(1, 5)])],
        ..FileMatches::default()
    };
    compose_prefixes(&mut out);
    assert!(out.routes.is_empty(), "{:?}", out.routes);
    assert_eq!(out.refusals.len(), 3, "{:?}", out.refusals);
}

/// A scope that captured both a literal and an unreadable prefix composes on
/// the literal: what the source establishes is promoted and the rest is
/// dropped, the same rule a mixed method-path list already follows (S-328).
/// Only a scope with *no* readable literal is a refusal.
#[test]
fn a_part_literal_prefix_composes_on_its_literal_and_is_not_refused() {
    let mut out = FileMatches {
        routes: vec![route_at("/users", 5)],
        prefixes: vec![scope(0, 10, &[("/a", 1, 3)], &[(6, 8)])],
        ..FileMatches::default()
    };
    compose_prefixes(&mut out);
    let got: Vec<&str> = out.routes.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(got, ["/a/users"]);
    assert!(out.refusals.is_empty(), "{:?}", out.refusals);
}

/// A prefix that is a literal to the grammar but not an address is refused, and
/// the text-level rule covers every form no query can decompose ([CR-101] §3.3).
#[test]
fn a_literal_that_is_not_an_address_is_not_a_resolvable_prefix() {
    for bad in [
        "${api.base}",           // property placeholder
        "/v1/${tenant}",         // placeholder mid-path
        "#{cfg.base}",           // SpEL
        "\n/v1",                 // a Java text block's stray newline
        "\"\"/v1",                // ...and its stray quotes
    ] {
        assert!(!is_resolvable_prefix(bad), "{bad:?} must not be resolvable");
    }
    for good in ["/v1", "/", "", "/v1/{id}", "/ete/v1"] {
        assert!(is_resolvable_prefix(good), "{good:?} must be resolvable");
    }
}

/// A pathless registration whose only prefix is empty establishes nothing, so
/// it drops silently rather than promoting a route named `"GET "`.
#[test]
fn a_pathless_registration_under_an_empty_prefix_is_dropped() {
    let mut out = FileMatches {
        pathless: vec![route_at("", 50)],
        prefixes: vec![scope(0, 100, &[("", 10, 12)], &[])],
        ..FileMatches::default()
    };
    compose_prefixes(&mut out);
    assert!(out.routes.is_empty(), "{:?}", out.routes);
    assert!(out.refusals.is_empty(), "{:?}", out.refusals);
}

/// Composition **cannot** live in a query file: a tree-sitter pattern matches
/// and captures, it has no string arithmetic to join two literals with. What a
/// query file therefore has to do is name the shared captures the interpreter
/// interprets — and it has to name them as a **pair**.
///
/// This guard is written over *every* shipped `frameworks.scm`, not over Java's
/// alone, and it is inverted deliberately: rather than listing the queries that
/// must carry prefix captures, it requires every query that names one to name
/// the other. A closed list cannot notice what it does not name, so the moment
/// S-330 adds Kotlin prefix patterns this test already covers them — and a
/// query that captures `@fw.route.prefix` while forgetting
/// `@fw.route.prefix.scope` fails here instead of silently composing nothing
/// (`generic_match` drops the literals, every route keeps its bare method path,
/// and no test would otherwise notice).
#[test]
fn every_framework_query_naming_a_prefix_also_names_its_scope() {
    let mut checked = 0;
    for entry in crate::plugin::grammars::compiled() {
        for query in entry.embedded_queries {
            if !query.relative_path.ends_with("frameworks.scm") {
                continue;
            }
            // Count the bare capture, not any longer name that starts with it:
            // a plain `contains("@fw.route.prefix")` is satisfied vacuously by
            // `@fw.route.prefix.scope` and would pin nothing.
            let names_literal = capture_occurrences(query.source, "fw.route.prefix") > 0;
            let names_scope = capture_occurrences(query.source, "fw.route.prefix.scope") > 0;
            let names_opaque = capture_occurrences(query.source, "fw.route.prefix.opaque") > 0;
            assert_eq!(
                names_literal, names_scope,
                "{}: @fw.route.prefix and @fw.route.prefix.scope are a pair",
                query.label
            );
            assert!(
                !names_opaque || names_scope,
                "{}: @fw.route.prefix.opaque needs @fw.route.prefix.scope too",
                query.label
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "no frameworks.scm was checked");
}

/// Occurrences of `@<name>` in a query, counting only the capture whose name is
/// exactly `name` — `@fw.route.prefix.scope` is not an occurrence of
/// `@fw.route.prefix`.
#[cfg(test)]
fn capture_occurrences(source: &str, name: &str) -> usize {
    let needle = format!("@{name}");
    source
        .match_indices(&needle)
        .filter(|(at, _)| {
            source[at + needle.len()..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_alphanumeric() && c != '.' && c != '_' && c != '-')
        })
        .count()
}

/// The Java query names each shared capture as its own capture, not merely as a
/// prefix of a longer one — asserted by exact-name counting so a rename to
/// `@fw.route.prefix.value` fails here rather than passing vacuously.
#[test]
fn the_java_query_delegates_composition_by_naming_the_shared_captures() {
    let query = include_str!("../../../plugins/java/queries/frameworks.scm");
    for capture in [
        "fw.route.prefix",
        "fw.route.prefix.scope",
        "fw.route.prefix.opaque",
    ] {
        assert!(
            capture_occurrences(query, capture) > 0,
            "the Java query must name @{capture}"
        );
    }
}

/// A `$`-introduced reference — a property placeholder or a Kotlin string
/// template — is never a joinable prefix, whichever syntax wrote it, while a
/// dollar sign that introduces nothing still is ([NFR-RA-05], S-330).
///
/// The rule is text-level because the grammars disagree about what they model:
/// `tree-sitter-kotlin-ng` gives `"${BASE}/v1"` an `interpolation` child a query
/// could capture but gives `"$BASE/v1"` two plain `string_content` runs with
/// nothing to name, and Java's `"${api.base}"` is one flat literal. One rule in
/// the shared interpreter therefore covers strictly more than any per-language
/// capture could, in every language rather than in one.
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[test]
fn a_template_reference_is_never_a_resolvable_prefix() {
    for refused in [
        "${api.base}",
        "${api.base}/v1",
        "$BASE",
        "$BASE/v1",
        "${BASE}/v1",
        "/v1/$BASE",
        "/price$/x/${api.base}",
        "$_private",
        "#{cfg.base}",
    ] {
        assert!(
            !is_resolvable_prefix(refused),
            "{refused} names an unresolved reference"
        );
    }
    for resolvable in ["/v1", "/price$", "/a$", "/v1/{id}", "", "/a$1"] {
        assert!(
            is_resolvable_prefix(resolvable),
            "{resolvable} is a joinable path"
        );
    }
}

/// The reuse claim of [S-329] proved the way [S-330] had to satisfy it:
/// composition driven by a **second language's query**, dropped in as data with
/// no Rust change at all ([FR-PL-04] makes
/// `.logos/plugins/<lang>/queries/frameworks.scm` shadow the embedded one, so a
/// query really is the whole per-language surface).
///
/// The override written here is deliberately *minimal* — narrower than the
/// Kotlin query [S-330] went on to ship — and it shadows that query, so this
/// test keeps proving what it always proved: naming the three shared captures
/// and nothing else is enough. Separator normalisation, the innermost scope,
/// the prefix-only fallback and the refusal all come for free; if this test can
/// express a dialect's behaviour, that dialect needs no implementation. A
/// second composition implementation would make this redundant, which is
/// exactly the failure [S-330] was meant to detect (and
/// `no_language_specific_composition_code_exists` asserts directly).
///
/// [FR-PL-04]: ../../../docs/specs/requirements/FR-PL-04.md
#[test]
#[cfg(feature = "lang-kotlin")]
fn a_second_language_inherits_composition_from_its_query_alone() {
    let root = tempfile::tempdir().expect("tempdir");
    let qdir = root.path().join(".logos/plugins/kotlin/queries");
    std::fs::create_dir_all(&qdir).expect("query dir");
    // Kotlin shapes: an annotation WITH arguments is a `constructor_invocation`;
    // a class-level one sits in the class's `modifiers`. Nothing here joins
    // anything — it only names the captures.
    std::fs::write(
        qdir.join("frameworks.scm"),
        r#"
(class_declaration
  (modifiers
    (annotation
      (constructor_invocation
        (user_type (identifier) @fw.route.prefix.name)
        (value_arguments
          (value_argument
            (string_literal) @fw.route.prefix))))))  @fw.route.prefix.scope

(function_declaration
  (modifiers
    (annotation
      (constructor_invocation
        (user_type (identifier) @fw.route.method)
        (value_arguments
          (value_argument
            (string_literal) @fw.route.path)))))
  name: (identifier) @fw.route.handler)
"#,
    )
    .expect("write override");

    let registry = LanguageRegistry::load(root.path()).expect("registry loads");
    let plugin = registry.for_extension("kt").expect("kotlin plugin");
    let mut parser = Parser::new();
    let matches = scan_source(
        &mut parser,
        plugin,
        r#"
@RequestMapping("/v1/")
class UserController {
    @GetMapping("/users")
    fun listUsers(): String { return "" }
}
"#,
    );
    assert_eq!(
        route_triples(matches),
        vec![(
            "/v1/users".to_string(),
            "GET".to_string(),
            Some("listUsers".to_string())
        )],
        "a second language composes from its query alone"
    );
}

// ── Java Spring mapping annotations (S-328) ──────────────────────────────────

/// Contract-first Spring code names its paths, never positions them: the
/// annotation `@RequestMapping(method = …, value = "/v1/x", produces = …)` is
/// what OpenAPI codegen emits, and a bare positional literal is the shape it
/// never writes (FR-FW-05, BR-46).
#[cfg(feature = "lang-java")]
mod java_spring {
    use super::*;

    /// The `(path, method, handler)` projection of a scanned Java snippet,
    /// sorted so a test asserts the promoted *set*, not tree-sitter's match
    /// order.
    fn java_routes(source: &str) -> Vec<(String, String, Option<String>)> {
        sorted_triples(&scan_lang("java", source))
    }

    /// A method wrapped in the minimal legal class body.
    fn in_class(members: &str) -> String {
        format!("public class C {{\n{members}\n}}\n")
    }

    #[test]
    fn named_value_argument_yields_the_route() {
        let got = java_routes(&in_class(
            r#"    @RequestMapping(method = RequestMethod.GET, value = "/v1/x", produces = "application/json")
    public String getX() { return ""; }"#,
        ));
        assert_eq!(
            got,
            vec![(
                "/v1/x".to_string(),
                "ANY".to_string(),
                Some("getX".to_string())
            )]
        );
    }

    #[test]
    fn named_path_argument_is_an_alias_for_value() {
        let got = java_routes(&in_class(
            r#"    @GetMapping(path = "/v1/y")
    public String getY() { return ""; }"#,
        ));
        assert_eq!(
            got,
            vec![(
                "/v1/y".to_string(),
                "GET".to_string(),
                Some("getY".to_string())
            )]
        );
    }

    #[test]
    fn list_valued_paths_yield_one_route_each() {
        let got = java_routes(&in_class(
            r#"    @GetMapping(value = {"/a", "/b"})
    public String get() { return ""; }"#,
        ));
        assert_eq!(
            got,
            vec![
                ("/a".to_string(), "GET".to_string(), Some("get".to_string())),
                ("/b".to_string(), "GET".to_string(), Some("get".to_string())),
            ]
        );
    }

    #[test]
    fn named_argument_wins_over_a_positional_one() {
        // Mixing a positional literal with a named argument is not legal
        // Java, and tree-sitter puts the LEADING argument inside an `ERROR`
        // node that neither pattern reaches through. So exactly one path
        // survives, and which one is decided by the parser's recovery, not by
        // the interpreter's rank: named first here, positional first below.
        // Pinned as a regression guard on that recovery shape — the
        // precedence pass itself is covered by
        // `outranked_positional_paths_are_dropped_only_within_their_own_site`,
        // and is only reachable from a dialect whose grammar admits the mixed
        // form (Kotlin, S-330).
        let got = java_routes(&in_class(
            r#"    @RequestMapping("/positional", value = "/named")
    public String get() { return ""; }"#,
        ));
        assert_eq!(
            got,
            vec![(
                "/named".to_string(),
                "ANY".to_string(),
                Some("get".to_string())
            )]
        );

        let reversed = java_routes(&in_class(
            r#"    @RequestMapping(value = "/named", "/positional")
    public String get() { return ""; }"#,
        ));
        assert_eq!(
            reversed,
            vec![(
                "/positional".to_string(),
                "ANY".to_string(),
                Some("get".to_string())
            )]
        );
    }

    /// The wiring the precedence pass depends on, asserted at the source
    /// level rather than on hand-built values: both patterns must anchor, the
    /// `named` rank must follow the capture the path came from, and two
    /// annotations on one method must be two distinct sites. Without this the
    /// query could stop anchoring and every behavioural test would stay green.
    #[test]
    fn path_origins_record_the_annotation_site_and_the_named_rank() {
        let m = scan_lang(
            "java",
            &in_class(
                r#"    @GetMapping("/read")
    @PostMapping(value = "/write")
    public String both() { return ""; }"#,
            ),
        );
        let mut got: Vec<(&str, Option<usize>, bool)> = m
            .routes
            .iter()
            .map(|r| (r.path.as_str(), r.origin.site, r.origin.named))
            .collect();
        got.sort();
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[0].0, "/read");
        assert!(!got[0].2, "a positional path is not named: {got:?}");
        assert_eq!(got[1].0, "/write");
        assert!(got[1].2, "a `value =` path is named: {got:?}");
        let (read_site, write_site) = (got[0].1, got[1].1);
        assert!(read_site.is_some(), "positional pattern must anchor: {got:?}");
        assert!(write_site.is_some(), "named pattern must anchor: {got:?}");
        assert_ne!(
            read_site, write_site,
            "each annotation is its own site: {got:?}"
        );
    }

    #[test]
    fn both_alias_keys_on_one_annotation_yield_a_route_each() {
        // `value` and `path` on one annotation is a Spring `@AliasFor`
        // conflict the application would reject at startup. The query
        // promotes each as written rather than adjudicating it — pinned so
        // the choice is visible rather than incidental.
        let got = java_routes(&in_class(
            r#"    @GetMapping(value = "/x", path = "/y")
    public String get() { return ""; }"#,
        ));
        assert_eq!(
            got,
            vec![
                ("/x".to_string(), "GET".to_string(), Some("get".to_string())),
                ("/y".to_string(), "GET".to_string(), Some("get".to_string())),
            ]
        );
    }

    #[test]
    fn non_literal_named_paths_promote_nothing() {
        // The header's honesty claim: a constant reference or a concatenation
        // leaves no literal, so nothing is promoted — never a guessed path
        // (NFR-RA-05).
        for arguments in [
            r#"value = BASE + "/x""#,
            "value = BASE",
            "value = Paths.USERS",
        ] {
            let m = scan_lang(
                "java",
                &in_class(&format!(
                    "    @GetMapping({arguments})\n    public String get() {{ return \"\"; }}"
                )),
            );
            assert!(m.routes.is_empty(), "{arguments}: {:?}", m.routes);
        }
    }

    #[test]
    fn a_mixed_list_promotes_only_its_literal_elements() {
        // The other half of the same claim: a list mixing a literal and a
        // non-literal promotes what it can establish and drops the rest
        // silently. Reporting that as `path-not-composed` is S-329's.
        let got = java_routes(&in_class(
            r#"    @GetMapping(value = {"/a", BASE + "/b"})
    public String get() { return ""; }"#,
        ));
        assert_eq!(
            got,
            vec![("/a".to_string(), "GET".to_string(), Some("get".to_string()))]
        );
    }

    #[test]
    fn property_placeholder_paths_are_promoted_verbatim() {
        // A placeholder IS a written literal, so it is captured — and kept as
        // written. Resolving it against the property sources is out of scope
        // (FR-FW-05), and the route must not pretend otherwise.
        let got = java_routes(&in_class(
            r#"    @GetMapping(value = "${api.base}/users")
    public String get() { return ""; }"#,
        ));
        assert_eq!(
            got,
            vec![(
                "${api.base}/users".to_string(),
                "GET".to_string(),
                Some("get".to_string())
            )]
        );
    }

    #[test]
    fn a_named_path_on_one_annotation_never_suppresses_another() {
        // Precedence is per registration site: two annotations on one method
        // are two sites, so the positional one keeps its route.
        let got = java_routes(&in_class(
            r#"    @GetMapping("/read")
    @PostMapping(value = "/write")
    public String both() { return ""; }"#,
        ));
        assert_eq!(
            got,
            vec![
                (
                    "/read".to_string(),
                    "GET".to_string(),
                    Some("both".to_string())
                ),
                (
                    "/write".to_string(),
                    "POST".to_string(),
                    Some("both".to_string())
                ),
            ]
        );
    }

    #[test]
    fn positional_literal_form_is_unchanged() {
        let got = java_routes(&in_class(
            r#"    @GetMapping("/users")
    public String listUsers() { return ""; }"#,
        ));
        assert_eq!(
            got,
            vec![(
                "/users".to_string(),
                "GET".to_string(),
                Some("listUsers".to_string())
            )]
        );
    }

    #[test]
    fn interface_declared_handler_yields_exactly_one_route() {
        // The contract-first shape: the interface declares the mapping, the
        // implementation is a bare `@RestController`. Exactly one route —
        // neither duplicated by the implementation nor missed for being
        // declared on an abstract method.
        let m = scan_lang(
            "java",
            r#"
interface UserApi {
    @GetMapping(path = "/users")
    String listUsers();
}

@RestController
class UserController implements UserApi {
    @Override
    public String listUsers() { return ""; }
}
"#,
        );
        assert_eq!(
            route_triples(m),
            vec![(
                "/users".to_string(),
                "GET".to_string(),
                Some("listUsers".to_string())
            )]
        );
    }

    #[test]
    fn annotation_absent_from_the_method_table_promotes_nothing() {
        // The [framework_methods] gate (FR-FW-04): an unmapped annotation
        // promotes nothing whatever arguments it carries — named, positional
        // or list-valued.
        let m = scan_lang(
            "java",
            &in_class(
                r#"    @Operation(value = "/v1/x")
    @ApiResponse(path = {"/a", "/b"})
    @Deprecated("/positional")
    public String getX() { return ""; }"#,
            ),
        );
        assert!(m.routes.is_empty(), "{:?}", m.routes);
    }

    #[test]
    fn non_path_named_arguments_never_become_paths() {
        // `produces`/`consumes` are string-valued too — only `value`/`path`
        // name a URL.
        let m = scan_lang(
            "java",
            &in_class(
                r#"    @GetMapping(produces = "application/json", consumes = "text/plain")
    public String getX() { return ""; }"#,
            ),
        );
        assert!(m.routes.is_empty(), "{:?}", m.routes);
    }

    /// The sorted `(path, method, handler)` projection of an already-scanned
    /// file — for the tests that assert routes *and* refusals from one scan.
    fn sorted_triples(m: &FileMatches) -> Vec<(String, String, Option<String>)> {
        let mut got: Vec<(String, String, Option<String>)> = m
            .routes
            .iter()
            .map(|r| (r.path.clone(), r.method.clone(), r.handler.clone()))
            .collect();
        got.sort();
        got
    }

    /// A handler method wrapped in a class carrying `@RequestMapping(<args>)`.
    fn in_prefixed_class(arguments: &str, members: &str) -> String {
        format!(
            "@RequestMapping({arguments})\n@RestController\npublic class C {{\n{members}\n}}\n"
        )
    }

    /// The canonical handler: a named-argument `@GetMapping` on `/users`.
    const USERS_HANDLER: &str = r#"    @GetMapping(value = "/users")
    public String listUsers() { return ""; }"#;

    #[test]
    fn class_level_prefix_composes_with_the_method_path() {
        let got = java_routes(
            r#"
@RequestMapping("/api/v1")
@RestController
public class C {
    @GetMapping(value = "/users")
    public String listUsers() { return ""; }
}
"#,
        );
        assert_eq!(
            got,
            vec![(
                "/api/v1/users".to_string(),
                "GET".to_string(),
                Some("listUsers".to_string())
            )]
        );
    }

    /// Separator normalisation over real Java source, not just the pure
    /// joiner: whichever side writes the slash, the promoted route carries
    /// exactly one ([FR-FW-05]).
    #[test]
    fn prefix_composition_normalises_the_separator_over_real_source() {
        for (prefix, path) in [
            (r#""/v1""#, "/users"),
            (r#""/v1""#, "users"),
            (r#""/v1/""#, "/users"),
            (r#""/v1/""#, "users"),
        ] {
            let got = java_routes(&in_prefixed_class(
                prefix,
                &format!(
                    "    @GetMapping(value = \"{path}\")\n    public String listUsers() {{ return \"\"; }}"
                ),
            ));
            assert_eq!(
                got,
                vec![(
                    "/v1/users".to_string(),
                    "GET".to_string(),
                    Some("listUsers".to_string())
                )],
                "prefix {prefix}, path {path}"
            );
        }
    }

    /// A handler in a prefixed class whose own annotation carries no path
    /// takes the prefix as its full path ([FR-FW-05]).
    #[test]
    fn a_prefixed_handler_with_no_method_path_takes_the_prefix() {
        let got = java_routes(&in_prefixed_class(
            r#""/v1/users""#,
            "    @GetMapping\n    public String listUsers() { return \"\"; }",
        ));
        assert_eq!(
            got,
            vec![(
                "/v1/users".to_string(),
                "GET".to_string(),
                Some("listUsers".to_string())
            )]
        );
    }

    /// The other half of the pathless rule: with no prefix in scope there is
    /// nothing to take, so the bare annotation promotes nothing — and it is
    /// **not** a composition failure, so it reports no reason ([BR-46]).
    ///
    /// [BR-46]: ../../../docs/specs/software-spec.md#310-framework-extraction
    #[test]
    fn a_pathless_annotation_with_no_prefix_promotes_nothing_and_is_not_refused() {
        let m = scan_lang(
            "java",
            &in_class("    @GetMapping\n    public String listUsers() { return \"\"; }"),
        );
        assert!(m.routes.is_empty(), "{:?}", m.routes);
        assert!(m.refusals.is_empty(), "{:?}", m.refusals);
    }

    /// [BR-46] directly: a handler with no prefix in scope composes to its
    /// method path alone and is never reported `path-not-composed`.
    ///
    /// [BR-46]: ../../../docs/specs/software-spec.md#310-framework-extraction
    #[test]
    fn an_unprefixed_handler_keeps_its_method_path_and_is_not_refused() {
        let m = scan_lang("java", &in_class(USERS_HANDLER));
        assert_eq!(
            sorted_triples(&m),
            vec![(
                "/users".to_string(),
                "GET".to_string(),
                Some("listUsers".to_string())
            )]
        );
        assert!(m.refusals.is_empty(), "{:?}", m.refusals);
    }

    /// A prefix that cannot be resolved to a literal — a constant reference, a
    /// qualified constant, a concatenation, in either the positional or the
    /// named form — yields **no** path at all, and the registration is
    /// reported `path-not-composed` ([FR-WS-05], [NFR-RA-05]). Promoting
    /// `/users` here would advertise a provider at an address the service does
    /// not serve.
    ///
    /// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    #[test]
    fn a_non_literal_prefix_refuses_the_route_instead_of_promoting_a_partial_path() {
        for arguments in [
            "BASE",
            "Paths.V1",
            r#"BASE + "/v1""#,
            "value = BASE",
            "path = Paths.V1",
            r#"value = BASE + "/v1""#,
            "value = {BASE}",
        ] {
            let m = scan_lang("java", &in_prefixed_class(arguments, USERS_HANDLER));
            assert!(m.routes.is_empty(), "{arguments}: {:?}", m.routes);
            assert_eq!(
                m.refusals,
                [RouteRefusal::PathNotComposed],
                "{arguments} must report path-not-composed"
            );
        }
    }

    /// A property placeholder is a written literal but not a resolvable
    /// address, and resolving it is out of scope ([CR-101] §3.3) — so as a
    /// *prefix* it is refused rather than joined onto. Asserted alongside the
    /// unchanged method-path rule (promoted verbatim, S-328) so the asymmetry
    /// is a decision on the record: a path written whole is recorded as
    /// written, a prefix has to survive being joined.
    ///
    /// [CR-101]: ../../../docs/requests/CR-101-jvm-spring-route-extraction.md
    #[test]
    fn a_property_placeholder_prefix_is_refused_while_a_placeholder_path_is_not() {
        let prefixed = scan_lang(
            "java",
            &in_prefixed_class(r#""${api.base}""#, USERS_HANDLER),
        );
        assert!(prefixed.routes.is_empty(), "{:?}", prefixed.routes);
        assert_eq!(prefixed.refusals, [RouteRefusal::PathNotComposed]);

        let unprefixed = java_routes(&in_class(
            r#"    @GetMapping(value = "${api.base}/users")
    public String listUsers() { return ""; }"#,
        ));
        assert_eq!(
            unprefixed,
            vec![(
                "${api.base}/users".to_string(),
                "GET".to_string(),
                Some("listUsers".to_string())
            )]
        );
    }

    /// The contract-first shape: the **interface** declares the prefix and the
    /// mappings, and composition treats an interface exactly as a class
    /// ([FR-FW-05]).
    #[test]
    fn an_interface_level_prefix_composes_like_a_class_one() {
        let got = java_routes(
            r#"
@RequestMapping("/v1")
public interface UserApi {
    @RequestMapping(method = RequestMethod.GET, value = "/users", produces = "application/json")
    String listUsers();

    @GetMapping(path = {"/users/{id}", "/users/by-id/{id}"})
    String getUser(String id);
}
"#,
        );
        assert_eq!(
            got,
            vec![
                (
                    "/v1/users".to_string(),
                    "ANY".to_string(),
                    Some("listUsers".to_string())
                ),
                (
                    "/v1/users/by-id/{id}".to_string(),
                    "GET".to_string(),
                    Some("getUser".to_string())
                ),
                (
                    "/v1/users/{id}".to_string(),
                    "GET".to_string(),
                    Some("getUser".to_string())
                ),
            ]
        );
    }

    /// A list-valued class prefix serves the type at every base, so each
    /// handler registers once per base.
    #[test]
    fn a_list_valued_class_prefix_registers_the_handler_at_each_base() {
        let got = java_routes(&in_prefixed_class(
            r#"value = {"/v1", "/v2"}"#,
            USERS_HANDLER,
        ));
        assert_eq!(
            got,
            vec![
                (
                    "/v1/users".to_string(),
                    "GET".to_string(),
                    Some("listUsers".to_string())
                ),
                (
                    "/v2/users".to_string(),
                    "GET".to_string(),
                    Some("listUsers".to_string())
                ),
            ]
        );
    }

    /// A prefixed nested type takes **its own** prefix, not its enclosing
    /// type's — Spring binds a handler to its declaring type. The innermost
    /// containing scope wins.
    /// A **positional** array-valued class prefix — `@RequestMapping({"/a"})`,
    /// which several codegen templates emit — composes exactly like the named
    /// form. Before the positional patterns learned the array shape it matched
    /// no prefix pattern at all, so the type read as *unprefixed* and every
    /// handler was silently promoted at its bare method path: a wrong address,
    /// not an absent one ([NFR-RA-05]).
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    /// Composition runs **before** dedup, so two method paths that differ only
    /// before composition converge afterwards and collapse to one route. Pinned
    /// so the consequence is on the record rather than a surprise: `/users` and
    /// `users` under `/v1` are the same endpoint, Spring would reject the pair
    /// as an ambiguous mapping, and the surviving route keeps a proven handler.
    #[test]
    fn two_method_paths_that_converge_after_composition_collapse_to_one_route() {
        let m = scan_lang(
            "java",
            &in_prefixed_class(
                r#""/v1""#,
                r#"    @GetMapping(value = "/users")
    public String listUsers() { return ""; }

    @GetMapping(value = "users")
    public String listUsersAgain() { return ""; }"#,
            ),
        );
        let got = sorted_triples(&m);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].0, "/v1/users");
        assert_eq!(got[0].1, "GET");
        assert!(got[0].2.is_some(), "the survivor keeps a proven handler");
        assert!(m.refusals.is_empty(), "{:?}", m.refusals);
    }

    #[test]
    fn a_positional_array_valued_class_prefix_composes() {
        let got = java_routes(&in_prefixed_class(r#"{"/v1", "/v2"}"#, USERS_HANDLER));
        assert_eq!(
            got,
            vec![
                (
                    "/v1/users".to_string(),
                    "GET".to_string(),
                    Some("listUsers".to_string())
                ),
                (
                    "/v2/users".to_string(),
                    "GET".to_string(),
                    Some("listUsers".to_string())
                ),
            ]
        );

        // ...and its non-literal twin refuses rather than promoting `/users`.
        let m = scan_lang("java", &in_prefixed_class("{BASE}", USERS_HANDLER));
        assert!(m.routes.is_empty(), "{:?}", m.routes);
        assert_eq!(m.refusals, [RouteRefusal::PathNotComposed]);
    }

    /// A method-level `@RequestMapping` must never be read as a prefix
    /// governing its own route. The regression this pins is real and was
    /// observed: expressing "a type declaration" as the grammar's
    /// `declaration` supertype also matches `method_declaration`, which
    /// composed every named mapping with itself into `/v1/x/v1/x`.
    #[test]
    fn a_method_level_mapping_is_never_its_own_prefix() {
        let got = java_routes(&in_class(
            r#"    @RequestMapping(method = RequestMethod.GET, value = "/v1/x")
    public String getX() { return ""; }"#,
        ));
        assert_eq!(
            got,
            vec![(
                "/v1/x".to_string(),
                "ANY".to_string(),
                Some("getX".to_string())
            )]
        );
    }

    /// An explicitly empty `value = {}` declares no prefix — Spring reads it
    /// that way — so the type's handlers keep their own paths and nothing is
    /// refused. The catch-all opaque pattern must not fire on it.
    #[test]
    fn an_empty_prefix_list_declares_no_prefix() {
        let m = scan_lang("java", &in_prefixed_class("value = {}", USERS_HANDLER));
        assert_eq!(
            sorted_triples(&m),
            vec![(
                "/users".to_string(),
                "GET".to_string(),
                Some("listUsers".to_string())
            )]
        );
        assert!(m.refusals.is_empty(), "{:?}", m.refusals);
    }

    /// A prefix that is the empty string supplies nothing: a handler with its
    /// own path keeps it, and a *pathless* handler is dropped rather than
    /// promoted as a route named `"GET "`.
    #[test]
    fn an_empty_string_prefix_never_manufactures_a_pathless_route() {
        let with_path = java_routes(&in_prefixed_class(r#""""#, USERS_HANDLER));
        assert_eq!(
            with_path,
            vec![(
                "/users".to_string(),
                "GET".to_string(),
                Some("listUsers".to_string())
            )]
        );

        let m = scan_lang(
            "java",
            &in_prefixed_class(
                r#""""#,
                "    @GetMapping\n    public String listUsers() { return \"\"; }",
            ),
        );
        assert!(m.routes.is_empty(), "{:?}", m.routes);
        assert!(m.refusals.is_empty(), "{:?}", m.refusals);
    }

    /// A SpEL prefix is refused exactly like a property placeholder, and a Java
    /// text block — which the unquoting heuristic cannot fully read — never
    /// composes a route name carrying stray quotes or a newline.
    #[test]
    fn a_spel_or_text_block_prefix_is_refused() {
        for arguments in [r##""#{cfg.base}""##, "\"\"\"\n/v1\"\"\""] {
            let m = scan_lang("java", &in_prefixed_class(arguments, USERS_HANDLER));
            assert!(m.routes.is_empty(), "{arguments}: {:?}", m.routes);
            assert_eq!(
                m.refusals,
                [RouteRefusal::PathNotComposed],
                "{arguments} must report path-not-composed"
            );
        }
    }

    /// One refused registration counts once, however many paths it wrote — the
    /// grain `routes_not_composed` documents.
    #[test]
    fn a_refused_list_valued_registration_counts_once() {
        let m = scan_lang(
            "java",
            &in_prefixed_class(
                "BASE",
                r#"    @GetMapping(value = {"/a", "/b"})
    public String list() { return ""; }"#,
            ),
        );
        assert!(m.routes.is_empty(), "{:?}", m.routes);
        assert_eq!(m.refusals, [RouteRefusal::PathNotComposed], "one annotation, one refusal");
    }

    /// A **positional** method path composes with the class prefix exactly as a
    /// named one does — the two forms S-328 established must both compose, and
    /// composition runs after precedence so a suppressed positional path is
    /// never composed and then dropped.
    #[test]
    fn a_positional_method_path_composes_with_the_class_prefix() {
        let got = java_routes(&in_prefixed_class(
            r#""/v1""#,
            r#"    @GetMapping("/users")
    public String listUsers() { return ""; }"#,
        ));
        assert_eq!(
            got,
            vec![(
                "/v1/users".to_string(),
                "GET".to_string(),
                Some("listUsers".to_string())
            )]
        );
    }

    /// The stereotype is written first in real Spring code; annotation order
    /// must not decide whether a prefix is found.
    #[test]
    fn the_prefix_is_found_whatever_order_the_annotations_are_written_in() {
        let got = java_routes(&format!(
            "@RestController\n@RequestMapping(\"/v1\")\npublic class C {{\n{USERS_HANDLER}\n}}\n"
        ));
        assert_eq!(
            got,
            vec![(
                "/v1/users".to_string(),
                "GET".to_string(),
                Some("listUsers".to_string())
            )]
        );
    }

    /// A record and an enum can carry a controller prefix too, and both
    /// compose. Leaving them out would not lose the route — the type-boundary
    /// pattern would still match, so the handler would be promoted at its bare
    /// method path, a wrong address rather than an absent one.
    #[test]
    fn a_record_or_enum_level_prefix_composes_like_a_class_one() {
        let record = java_routes(&format!(
            "@RequestMapping(\"/v1\")\npublic record R(String id) {{\n{USERS_HANDLER}\n}}\n"
        ));
        assert_eq!(
            record,
            vec![(
                "/v1/users".to_string(),
                "GET".to_string(),
                Some("listUsers".to_string())
            )]
        );

        let enumeration = java_routes(&format!(
            "@RequestMapping(\"/v1\")\npublic enum E {{\n    A;\n{USERS_HANDLER}\n}}\n"
        ));
        assert_eq!(
            enumeration,
            vec![(
                "/v1/users".to_string(),
                "GET".to_string(),
                Some("listUsers".to_string())
            )]
        );
    }

    #[test]
    fn a_nested_prefixed_class_takes_its_own_prefix() {
        let got = java_routes(
            r#"
@RequestMapping("/outer")
public class Outer {
    @GetMapping(value = "/a")
    public String outerHandler() { return ""; }

    @RequestMapping("/inner")
    public static class Inner {
        @GetMapping(value = "/b")
        public String innerHandler() { return ""; }
    }
}
"#,
        );
        assert_eq!(
            got,
            vec![
                (
                    "/inner/b".to_string(),
                    "GET".to_string(),
                    Some("innerHandler".to_string())
                ),
                (
                    "/outer/a".to_string(),
                    "GET".to_string(),
                    Some("outerHandler".to_string())
                ),
            ]
        );
    }

    /// An **unannotated** nested type is its own controller in Spring and
    /// inherits nothing: its handlers must NOT be promoted at the enclosing
    /// type's prefix. Without the type-boundary scope this composes
    /// `/outer/b`, a path the service never serves ([NFR-RA-05]).
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    #[test]
    fn an_unannotated_nested_class_does_not_inherit_the_outer_prefix() {
        let m = scan_lang(
            "java",
            r#"
@RequestMapping("/outer")
public class Outer {
    @GetMapping(value = "/a")
    public String outerHandler() { return ""; }

    public static class Inner {
        @GetMapping(value = "/b")
        public String innerHandler() { return ""; }
    }
}
"#,
        );
        assert_eq!(
            sorted_triples(&m),
            vec![
                (
                    "/b".to_string(),
                    "GET".to_string(),
                    Some("innerHandler".to_string())
                ),
                (
                    "/outer/a".to_string(),
                    "GET".to_string(),
                    Some("outerHandler".to_string())
                ),
            ]
        );
        assert!(m.refusals.is_empty(), "{:?}", m.refusals);
    }

    /// A class-level `@RequestMapping` that names no path argument declares no
    /// prefix: its handlers compose to their own paths and nothing is refused.
    /// Without this the `method =`-only form — legal, and common on a base
    /// controller — would silently refuse every route in the class.
    #[test]
    fn a_class_annotation_with_no_path_argument_is_not_a_prefix() {
        for arguments in [
            "method = RequestMethod.GET",
            r#"produces = "application/json""#,
            r#"consumes = {"application/json"}"#,
        ] {
            let m = scan_lang("java", &in_prefixed_class(arguments, USERS_HANDLER));
            assert_eq!(
                sorted_triples(&m),
                vec![(
                    "/users".to_string(),
                    "GET".to_string(),
                    Some("listUsers".to_string())
                )],
                "{arguments}"
            );
            assert!(m.refusals.is_empty(), "{arguments}: {:?}", m.refusals);
        }
    }

    /// Only `@RequestMapping` prefixes a type in Spring. A stereotype marker,
    /// and any other string-valued class annotation, must contribute no
    /// prefix — otherwise `@Validated("group")` would relocate every route in
    /// the class.
    #[test]
    fn only_request_mapping_prefixes_a_type() {
        for annotation in [
            "@RestController",
            r#"@Validated("group")"#,
            r#"@Profile(value = "prod")"#,
            r#"@GetMapping("/not-a-prefix")"#,
        ] {
            let source = format!("{annotation}\npublic class C {{\n{USERS_HANDLER}\n}}\n");
            let m = scan_lang("java", &source);
            assert_eq!(
                sorted_triples(&m),
                vec![(
                    "/users".to_string(),
                    "GET".to_string(),
                    Some("listUsers".to_string())
                )],
                "{annotation}"
            );
            assert!(
                m.prefixes
                    .iter()
                    .all(|s| s.literals.is_empty() && s.opaque.is_empty()),
                "{annotation} must declare no prefix: {:?}",
                m.prefixes
            );
        }
    }

    /// The `[framework_methods]` gate still runs first: `@Override` in a
    /// prefixed class is a marker annotation the pathless pattern matches, and
    /// it promotes nothing because its name is not in the table ([FR-FW-04]).
    /// The bare `@RestController` implementation of a prefixed interface is
    /// exactly this shape, so a regression here would double every
    /// contract-first route.
    ///
    /// [FR-FW-04]: ../../../docs/specs/requirements/FR-FW-04.md
    #[test]
    fn an_unmapped_marker_annotation_in_a_prefixed_class_promotes_nothing() {
        let m = scan_lang(
            "java",
            &in_prefixed_class(
                r#""/v1""#,
                "    @Override\n    public String listUsers() { return \"\"; }",
            ),
        );
        assert!(m.routes.is_empty(), "{:?}", m.routes);
        assert!(m.refusals.is_empty(), "{:?}", m.refusals);
    }

    /// The wiring composition depends on, asserted at the source level rather
    /// than on hand-built values (the S-328 pattern): the scope must span the
    /// whole declaration — annotation *and* body — or a handler's byte offset
    /// would fall outside it and every route would silently lose its prefix.
    #[test]
    fn a_prefix_scope_spans_the_whole_declaration_it_governs() {
        let source = in_prefixed_class(r#""/v1""#, USERS_HANDLER);
        let m = scan_lang("java", &source);
        // The prefix pattern and the bare type-boundary pattern both match the
        // one declaration, so both scopes carry its range; only one declares
        // the path, and `innermost_prefix` unions the tie.
        let scope = m
            .prefixes
            .iter()
            .find(|s| !s.literals.is_empty())
            .unwrap_or_else(|| panic!("a declared prefix: {:?}", m.prefixes));
        assert_eq!(
            scope.literals.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
            ["/v1"]
        );
        assert!(
            scope.literals.iter().all(|l| l.resolvable),
            "a literal prefix reads as an address"
        );
        assert!(
            m.prefixes
                .iter()
                .all(|s| s.start == scope.start && s.end == scope.end),
            "every scope here is the one declaration: {:?}",
            m.prefixes
        );
        // The whole class declaration: from its first annotation to its
        // closing brace.
        assert_eq!(scope.start, source.find('@').expect("annotation"));
        assert_eq!(scope.end, source.trim_end().len());
        // And the handler this scope must govern really is inside it.
        let handler = source.find("@GetMapping").expect("handler annotation");
        assert!(
            scope.start <= handler && handler < scope.end,
            "{scope:?} must contain byte {handler}"
        );
    }
}

// ── Kotlin Spring mapping annotations (S-330) ────────────────────────────────

/// Kotlin's Spring query is annotation-compatible with Java's but sits on a
/// different syntax tree, and before S-330 it paid for the difference twice: a
/// named `value = "/x"` argument was matched by an **unanchored** positional
/// pattern (so the query header's "deliberately NOT captured in v1" note was
/// false), and the same unanchored pattern read `produces = "application/json"`
/// as a URL. This module is the Java `java_spring` set mirrored onto Kotlin
/// source; `jvm_parity` below asserts the two languages agree fixture for
/// fixture.
#[cfg(feature = "lang-kotlin")]
mod kotlin_spring {
    use super::*;

    /// The sorted `(path, method, handler)` projection of a scanned Kotlin
    /// snippet — sorted so a test asserts the promoted *set*, not tree-sitter's
    /// match order.
    fn kotlin_routes(source: &str) -> Vec<(String, String, Option<String>)> {
        sorted_triples(&scan_lang("kt", source))
    }

    /// The same projection of an already-scanned file, for the tests that
    /// assert routes *and* refusals from one scan.
    fn sorted_triples(m: &FileMatches) -> Vec<(String, String, Option<String>)> {
        let mut got: Vec<(String, String, Option<String>)> = m
            .routes
            .iter()
            .map(|r| (r.path.clone(), r.method.clone(), r.handler.clone()))
            .collect();
        got.sort();
        got
    }

    /// A function wrapped in the minimal legal class body.
    fn in_class(members: &str) -> String {
        format!("class C {{\n{members}\n}}\n")
    }

    /// A handler wrapped in a class carrying `@RequestMapping(<arguments>)`.
    fn in_prefixed_class(arguments: &str, members: &str) -> String {
        format!("@RequestMapping({arguments})\n@RestController\nclass C {{\n{members}\n}}\n")
    }

    /// The canonical handler: a named-argument `@GetMapping` on `/users`.
    const USERS_HANDLER: &str = r#"    @GetMapping(value = "/users")
    fun listUsers(): String { return "" }"#;

    /// The `(path, method, handler)` triple `USERS_HANDLER` promotes unprefixed.
    fn users_route(path: &str) -> Vec<(String, String, Option<String>)> {
        vec![(
            path.to_string(),
            "GET".to_string(),
            Some("listUsers".to_string()),
        )]
    }

    // ── The named-argument form (the S-328 shape, on Kotlin's tree) ──────────

    #[test]
    fn named_value_argument_yields_the_route() {
        let got = kotlin_routes(&in_class(
            r#"    @RequestMapping(method = RequestMethod.GET, value = "/v1/x", produces = "application/json")
    fun getX(): String { return "" }"#,
        ));
        assert_eq!(
            got,
            vec![(
                "/v1/x".to_string(),
                "ANY".to_string(),
                Some("getX".to_string())
            )]
        );
    }

    #[test]
    fn named_path_argument_is_an_alias_for_value() {
        let got = kotlin_routes(&in_class(
            r#"    @GetMapping(path = "/v1/y")
    fun getY(): String { return "" }"#,
        ));
        assert_eq!(
            got,
            vec![(
                "/v1/y".to_string(),
                "GET".to_string(),
                Some("getY".to_string())
            )]
        );
    }

    /// Kotlin writes an annotation array as `[…]`, not Java's `{…}` — one route
    /// per element either way.
    #[test]
    fn list_valued_paths_yield_one_route_each() {
        let got = kotlin_routes(&in_class(
            r#"    @GetMapping(value = ["/a", "/b"])
    fun get(): String { return "" }"#,
        ));
        assert_eq!(
            got,
            vec![
                ("/a".to_string(), "GET".to_string(), Some("get".to_string())),
                ("/b".to_string(), "GET".to_string(), Some("get".to_string())),
            ]
        );
    }

    /// The regression guard on the pattern that already existed: adding the
    /// named form must not cost the positional one.
    #[test]
    fn positional_literal_form_is_unchanged() {
        let got = kotlin_routes(&in_class(
            r#"    @GetMapping("/users")
    fun listUsers(): String { return "" }"#,
        ));
        assert_eq!(got, users_route("/users"));
    }

    /// **The defect this story exists to fix.** Kotlin's `value_argument` holds
    /// the literal as a direct child whether or not a name precedes it, so the
    /// pre-S-330 pattern `(value_argument (string_literal))` matched every
    /// string-valued named argument — promoting `GET application/json` and
    /// `GET text/plain` as *route paths*, the approximate match [NFR-RA-05]
    /// forbids. Only `value`/`path` name a URL; the first-child anchor plus the
    /// key predicate is what keeps every other argument out.
    ///
    /// Mutation-checked: deleting the `.` from the positional pattern makes
    /// this fail with two fabricated media-type routes.
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    #[test]
    fn non_path_named_arguments_never_become_paths() {
        let m = scan_lang(
            "kt",
            &in_class(
                r#"    @GetMapping(produces = "application/json", consumes = "text/plain")
    fun getX(): String { return "" }"#,
            ),
        );
        assert!(m.routes.is_empty(), "{:?}", m.routes);
        assert!(m.pathless.is_empty(), "{:?}", m.pathless);
        assert!(m.refusals.is_empty(), "{:?}", m.refusals);

        // …and the same argument names on a *type* relocate nothing either:
        // a class-level `produces` is not a prefix, and reading it as one would
        // refuse every route in the controller instead of composing it.
        let prefixed = scan_lang(
            "kt",
            &in_prefixed_class(r#"produces = "application/json""#, USERS_HANDLER),
        );
        assert_eq!(sorted_triples(&prefixed), users_route("/users"));
        assert!(prefixed.refusals.is_empty(), "{:?}", prefixed.refusals);
    }

    /// The wiring the precedence pass depends on, asserted at the source level
    /// rather than on hand-built values: **both** path patterns must capture
    /// `@fw.route.anchor`, the `named` rank must follow the capture the path
    /// came from, and two annotations on one function must be two distinct
    /// sites. Without this the query could stop anchoring and the behavioural
    /// tests would stay green until a mixed-form annotation showed up.
    #[test]
    fn path_origins_record_the_annotation_site_and_the_named_rank() {
        let m = scan_lang(
            "kt",
            &in_class(
                r#"    @GetMapping("/read")
    @PostMapping(value = "/write")
    fun both(): String { return "" }"#,
            ),
        );
        let mut got: Vec<(&str, Option<usize>, bool)> = m
            .routes
            .iter()
            .map(|r| (r.path.as_str(), r.origin.site, r.origin.named))
            .collect();
        got.sort();
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[0].0, "/read");
        assert!(!got[0].2, "a positional path is not named: {got:?}");
        assert_eq!(got[1].0, "/write");
        assert!(got[1].2, "a `value =` path is named: {got:?}");
        let (read_site, write_site) = (got[0].1, got[1].1);
        assert!(
            read_site.is_some(),
            "the positional pattern must anchor: {got:?}"
        );
        assert!(
            write_site.is_some(),
            "the named pattern must anchor: {got:?}"
        );
        assert_ne!(
            read_site, write_site,
            "each annotation is its own site: {got:?}"
        );
    }

    /// Kotlin is the dialect [`drop_outranked_paths`] was built for (S-328).
    /// Its `value_arguments` is a homogeneous list, so `@RequestMapping("/a",
    /// value = "/b")` — illegal in Java, where the parser `ERROR`-wraps the
    /// leading argument — parses cleanly here and really does match both path
    /// patterns at one annotation. The *rank* therefore decides, and exactly
    /// one route survives; without `@fw.route.anchor` on the positional pattern
    /// both would be promoted and the service would advertise an endpoint it
    /// does not serve.
    ///
    /// Mutation-checked: removing `@fw.route.anchor` from either pattern makes
    /// this fail with two routes.
    #[test]
    fn a_named_path_outranks_a_positional_one_at_the_same_annotation() {
        let got = kotlin_routes(&in_class(
            r#"    @RequestMapping("/positional", value = "/named")
    fun get(): String { return "" }"#,
        ));
        assert_eq!(
            got,
            vec![(
                "/named".to_string(),
                "ANY".to_string(),
                Some("get".to_string())
            )]
        );

        // Written the other way round the rank still decides, which is where
        // Kotlin and Java legitimately diverge: Java's recovery keeps whichever
        // argument parsed cleanly (`/positional`), Kotlin's grammar parses both
        // and the named one wins. Neither form is code a Spring service should
        // contain — `value` is `@AliasFor` the positional argument — and the
        // outcome is pinned so the divergence is a decision on the record.
        let reversed = kotlin_routes(&in_class(
            r#"    @RequestMapping(value = "/named", "/positional")
    fun get(): String { return "" }"#,
        ));
        assert_eq!(
            reversed,
            vec![(
                "/named".to_string(),
                "ANY".to_string(),
                Some("get".to_string())
            )]
        );
    }

    #[test]
    fn a_named_path_on_one_annotation_never_suppresses_another() {
        // Precedence is per registration site: two annotations on one function
        // are two sites, so the positional one keeps its route.
        let got = kotlin_routes(&in_class(
            r#"    @GetMapping("/read")
    @PostMapping(value = "/write")
    fun both(): String { return "" }"#,
        ));
        assert_eq!(
            got,
            vec![
                (
                    "/read".to_string(),
                    "GET".to_string(),
                    Some("both".to_string())
                ),
                (
                    "/write".to_string(),
                    "POST".to_string(),
                    Some("both".to_string())
                ),
            ]
        );
    }

    #[test]
    fn both_alias_keys_on_one_annotation_yield_a_route_each() {
        // `value` and `path` on one annotation is a Spring `@AliasFor` conflict
        // the application would reject at startup. The query promotes each as
        // written rather than adjudicating it, exactly as Java's does.
        let got = kotlin_routes(&in_class(
            r#"    @GetMapping(value = "/x", path = "/y")
    fun get(): String { return "" }"#,
        ));
        assert_eq!(
            got,
            vec![
                ("/x".to_string(), "GET".to_string(), Some("get".to_string())),
                ("/y".to_string(), "GET".to_string(), Some("get".to_string())),
            ]
        );
    }

    #[test]
    fn non_literal_named_paths_promote_nothing() {
        // A constant reference or a concatenation leaves no literal, so nothing
        // is promoted — never a guessed path (NFR-RA-05).
        for arguments in [
            r#"value = BASE + "/x""#,
            "value = BASE",
            "value = Paths.USERS",
            r#"["/a", "/b"]"#, // a *positional* array, uncaptured as in Java
        ] {
            let m = scan_lang(
                "kt",
                &in_class(&format!(
                    "    @GetMapping({arguments})\n    fun get(): String {{ return \"\" }}"
                )),
            );
            assert!(m.routes.is_empty(), "{arguments}: {:?}", m.routes);
        }
    }

    #[test]
    fn a_mixed_list_promotes_only_its_literal_elements() {
        let got = kotlin_routes(&in_class(
            r#"    @GetMapping(value = ["/a", BASE + "/b"])
    fun get(): String { return "" }"#,
        ));
        assert_eq!(
            got,
            vec![("/a".to_string(), "GET".to_string(), Some("get".to_string()))]
        );
    }

    /// A path written *whole* is recorded as written, whether the indirection
    /// is a Spring property placeholder or a Kotlin string template: the node
    /// states the registration as the source does, and
    /// [`route_template`](crate::resolve::route_template) refuses to bind it
    /// one layer down. As a **prefix** the same text is refused instead —
    /// `a_string_template_prefix_is_refused_while_a_template_path_is_not`
    /// pins the asymmetry.
    #[test]
    fn placeholder_and_template_paths_are_promoted_verbatim() {
        for path in ["${api.base}/users", "$BASE/users"] {
            let got = kotlin_routes(&in_class(&format!(
                "    @GetMapping(value = \"{path}\")\n    fun listUsers(): String {{ return \"\" }}"
            )));
            assert_eq!(
                got,
                vec![(
                    path.to_string(),
                    "GET".to_string(),
                    Some("listUsers".to_string())
                )],
                "{path}"
            );
        }
    }

    /// The contract-first shape: the interface declares the mapping and the
    /// implementation is a bare `@RestController`. Exactly one route — and in
    /// Kotlin the implementation carries the `override` **keyword** rather than
    /// an annotation, so it cannot match any pattern in the query at all.
    #[test]
    fn interface_declared_handler_yields_exactly_one_route() {
        let m = scan_lang(
            "kt",
            r#"
interface UserApi {
    @GetMapping(path = "/users")
    fun listUsers(): String
}

@RestController
class UserController : UserApi {
    override fun listUsers(): String { return "" }
}
"#,
        );
        assert_eq!(sorted_triples(&m), users_route("/users"));
        assert_eq!(
            m.components.iter().map(|c| &c.type_path).collect::<Vec<_>>(),
            ["UserController"],
            "the implementation is the stereotype, the interface is not"
        );
    }

    #[test]
    fn annotation_absent_from_the_method_table_promotes_nothing() {
        // The [framework_methods] gate (FR-FW-04): an unmapped annotation
        // promotes nothing whatever arguments it carries — named, positional
        // or list-valued.
        let m = scan_lang(
            "kt",
            &in_class(
                r#"    @Operation(value = "/v1/x")
    @ApiResponse(path = ["/a", "/b"])
    @Deprecated("/positional")
    fun getX(): String { return "" }"#,
            ),
        );
        assert!(m.routes.is_empty(), "{:?}", m.routes);
        assert!(m.pathless.is_empty(), "{:?}", m.pathless);
    }

    // ── Composition, inherited from the shared interpreter (S-329) ───────────

    #[test]
    fn class_level_prefix_composes_with_the_method_path() {
        let got = kotlin_routes(&in_prefixed_class(r#""/api/v1""#, USERS_HANDLER));
        assert_eq!(got, users_route("/api/v1/users"));
    }

    /// Separator normalisation over real Kotlin source, not just the pure
    /// joiner: whichever side writes the slash, the promoted route carries
    /// exactly one. Inherited whole from `join_route_path` — the Kotlin query
    /// contributes no joining code.
    #[test]
    fn prefix_composition_normalises_the_separator_over_real_source() {
        for (prefix, path) in [
            (r#""/v1""#, "/users"),
            (r#""/v1""#, "users"),
            (r#""/v1/""#, "/users"),
            (r#""/v1/""#, "users"),
        ] {
            let got = kotlin_routes(&in_prefixed_class(
                prefix,
                &format!(
                    "    @GetMapping(value = \"{path}\")\n    fun listUsers(): String {{ return \"\" }}"
                ),
            ));
            assert_eq!(
                got,
                users_route("/v1/users"),
                "prefix {prefix}, path {path}"
            );
        }
    }

    /// A handler in a prefixed type whose own annotation carries no path takes
    /// the prefix as its full path — the marker-annotation shape, which in
    /// Kotlin is `(annotation (user_type …))` rather than Java's
    /// `marker_annotation` but is just as disjoint from the argument-bearing
    /// form, so it can never share a registration site with it.
    #[test]
    fn a_prefixed_handler_with_no_method_path_takes_the_prefix() {
        let got = kotlin_routes(&in_prefixed_class(
            r#""/v1/users""#,
            "    @GetMapping\n    fun listUsers(): String { return \"\" }",
        ));
        assert_eq!(got, users_route("/v1/users"));
    }

    /// The other half of the pathless rule: with no prefix in scope there is
    /// nothing to take, so the bare annotation promotes nothing — and it is
    /// **not** a composition failure, so it reports no reason ([BR-46]).
    ///
    /// [BR-46]: ../../../docs/specs/software-spec.md#310-framework-extraction
    #[test]
    fn a_pathless_annotation_with_no_prefix_promotes_nothing_and_is_not_refused() {
        let m = scan_lang(
            "kt",
            &in_class("    @GetMapping\n    fun listUsers(): String { return \"\" }"),
        );
        assert!(m.routes.is_empty(), "{:?}", m.routes);
        assert!(m.refusals.is_empty(), "{:?}", m.refusals);
    }

    /// [BR-46] directly: a handler with no prefix in scope composes to its
    /// method path alone and is never reported `path-not-composed`.
    ///
    /// [BR-46]: ../../../docs/specs/software-spec.md#310-framework-extraction
    #[test]
    fn an_unprefixed_handler_keeps_its_method_path_and_is_not_refused() {
        let m = scan_lang("kt", &in_class(USERS_HANDLER));
        assert_eq!(sorted_triples(&m), users_route("/users"));
        assert!(m.refusals.is_empty(), "{:?}", m.refusals);
    }

    /// A prefix that cannot be resolved to a literal yields **no** path at all
    /// and the registration is reported `path-not-composed` ([NFR-RA-05]).
    /// Kotlin adds three forms Java has no syntax for: a string template in
    /// either spelling, and a raw (`"""…"""`) literal, which the literal
    /// patterns deliberately do not read — the `(expression)` catch-all catches
    /// it as opaque so the type reads as *unreadable* rather than as
    /// *unprefixed*, which would silently promote `/users` at the wrong
    /// address.
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    #[test]
    fn a_non_literal_prefix_refuses_the_route_instead_of_promoting_a_partial_path() {
        for arguments in [
            "BASE",
            "Paths.V1",
            r#"BASE + "/v1""#,
            "value = BASE",
            "path = Paths.V1",
            r#"value = BASE + "/v1""#,
            "value = [BASE]",
            "[BASE]",
            r#""${api.base}""#,
            r##""#{cfg.base}""##,
            r#""$BASE/v1""#,
            r#""${BASE}/v1""#,
            "\"\"\"\n/v1\"\"\"",
        ] {
            let m = scan_lang("kt", &in_prefixed_class(arguments, USERS_HANDLER));
            assert!(m.routes.is_empty(), "{arguments}: {:?}", m.routes);
            assert_eq!(
                m.refusals,
                [RouteRefusal::PathNotComposed],
                "{arguments} must report path-not-composed"
            );
        }
    }

    /// The asymmetry, on the record for Kotlin as it is for Java: the *same*
    /// template text is refused as a prefix and promoted verbatim as a whole
    /// method path. A prefix is *joined*, so every handler under the type would
    /// inherit a fabricated address; a path written whole is the registration
    /// as the source states it.
    #[test]
    fn a_string_template_prefix_is_refused_while_a_template_path_is_not() {
        let prefixed = scan_lang("kt", &in_prefixed_class(r#""$BASE""#, USERS_HANDLER));
        assert!(prefixed.routes.is_empty(), "{:?}", prefixed.routes);
        assert_eq!(prefixed.refusals, [RouteRefusal::PathNotComposed]);

        let unprefixed = kotlin_routes(&in_class(
            r#"    @GetMapping(value = "$BASE/users")
    fun listUsers(): String { return "" }"#,
        ));
        assert_eq!(unprefixed, users_route("$BASE/users"));
    }

    /// A dollar sign that introduces no reference is still a path character:
    /// the refusal rule targets `${…}` / `$name`, not `$`.
    #[test]
    fn a_dollar_sign_that_names_nothing_still_composes() {
        let m = scan_lang("kt", &in_prefixed_class(r#""/price$""#, USERS_HANDLER));
        assert_eq!(sorted_triples(&m), users_route("/price$/users"));
        assert!(m.refusals.is_empty(), "{:?}", m.refusals);
    }

    /// The contract-first shape: the **interface** declares the prefix and the
    /// mappings. In Kotlin an interface is a `class_declaration` with a
    /// `class_body`, so it is the same node kind a class is and composition
    /// treats the two identically without the query naming either.
    #[test]
    fn an_interface_level_prefix_composes_like_a_class_one() {
        let got = kotlin_routes(
            r#"
@RequestMapping("/v1")
interface UserApi {
    @RequestMapping(method = RequestMethod.GET, value = "/users", produces = "application/json")
    fun listUsers(): String

    @GetMapping(path = ["/users/{id}", "/users/by-id/{id}"])
    fun getUser(id: String): String
}
"#,
        );
        assert_eq!(
            got,
            vec![
                (
                    "/v1/users".to_string(),
                    "ANY".to_string(),
                    Some("listUsers".to_string())
                ),
                (
                    "/v1/users/by-id/{id}".to_string(),
                    "GET".to_string(),
                    Some("getUser".to_string())
                ),
                (
                    "/v1/users/{id}".to_string(),
                    "GET".to_string(),
                    Some("getUser".to_string())
                ),
            ]
        );
    }

    /// A list-valued class prefix serves the type at every base, in both the
    /// named and the positional spelling, so each handler registers once per
    /// base.
    #[test]
    fn a_list_valued_class_prefix_registers_the_handler_at_each_base() {
        for arguments in [r#"value = ["/v1", "/v2"]"#, r#"["/v1", "/v2"]"#] {
            let got = kotlin_routes(&in_prefixed_class(arguments, USERS_HANDLER));
            assert_eq!(
                got,
                vec![
                    (
                        "/v1/users".to_string(),
                        "GET".to_string(),
                        Some("listUsers".to_string())
                    ),
                    (
                        "/v2/users".to_string(),
                        "GET".to_string(),
                        Some("listUsers".to_string())
                    ),
                ],
                "{arguments}"
            );
        }
    }

    /// In a mixed list the readable element wins and the rest is dropped: the
    /// opaque capture overlaps only the non-literal element, so `/v1` still
    /// composes rather than the whole scope being refused.
    #[test]
    fn a_mixed_list_class_prefix_composes_on_its_literal() {
        let m = scan_lang(
            "kt",
            &in_prefixed_class(r#"value = ["/v1", BASE]"#, USERS_HANDLER),
        );
        assert_eq!(sorted_triples(&m), users_route("/v1/users"));
        assert!(m.refusals.is_empty(), "{:?}", m.refusals);
    }

    /// An explicitly empty `value = []` declares no prefix — Spring reads it
    /// that way — so the type's handlers keep their own paths and nothing is
    /// refused. This grammar cannot represent an empty `collection_literal`
    /// (its element list is `commaSep1`) and error-recovers with a zero-width
    /// `MISSING` node, which the opaque catch-all would otherwise read as a
    /// written-but-unreadable prefix and refuse the whole controller for. The
    /// query's `(#not-eq? @fw.route.prefix.opaque "")` guard is what keeps a
    /// parse artefact from becoming a refusal.
    #[test]
    fn an_empty_prefix_list_declares_no_prefix() {
        for arguments in ["value = []", "[]"] {
            // Both the stereotyped shape real controllers carry and the bare
            // one: an empty `collection_literal` is error-recovered, so the two
            // do not parse alike and only the bare form exposes the `MISSING`
            // node the guard exists for.
            for source in [
                in_prefixed_class(arguments, USERS_HANDLER),
                format!("@RequestMapping({arguments})\nclass C {{\n{USERS_HANDLER}\n}}\n"),
            ] {
                let m = scan_lang("kt", &source);
                assert_eq!(sorted_triples(&m), users_route("/users"), "{source}");
                assert!(m.refusals.is_empty(), "{source}: {:?}", m.refusals);
            }
        }
    }

    /// A prefix that is the empty string supplies nothing: a handler with its
    /// own path keeps it, and a *pathless* handler is dropped rather than
    /// promoted as a route named `"GET "`.
    #[test]
    fn an_empty_string_prefix_never_manufactures_a_pathless_route() {
        let with_path = kotlin_routes(&in_prefixed_class(r#""""#, USERS_HANDLER));
        assert_eq!(with_path, users_route("/users"));

        let m = scan_lang(
            "kt",
            &in_prefixed_class(
                r#""""#,
                "    @GetMapping\n    fun listUsers(): String { return \"\" }",
            ),
        );
        assert!(m.routes.is_empty(), "{:?}", m.routes);
        assert!(m.refusals.is_empty(), "{:?}", m.refusals);
    }

    #[test]
    fn a_nested_prefixed_class_takes_its_own_prefix() {
        let got = kotlin_routes(
            r#"
@RequestMapping("/outer")
class Outer {
    @GetMapping(value = "/a")
    fun outerHandler(): String { return "" }

    @RequestMapping("/inner")
    class Inner {
        @GetMapping(value = "/b")
        fun innerHandler(): String { return "" }
    }
}
"#,
        );
        assert_eq!(
            got,
            vec![
                (
                    "/inner/b".to_string(),
                    "GET".to_string(),
                    Some("innerHandler".to_string())
                ),
                (
                    "/outer/a".to_string(),
                    "GET".to_string(),
                    Some("outerHandler".to_string())
                ),
            ]
        );
    }

    /// An **unannotated** nested type is its own controller in Spring and
    /// inherits nothing: its handlers must NOT be promoted at the enclosing
    /// type's prefix. Without the bare type-boundary pattern this composes
    /// `/outer/b`, a path the service never serves ([NFR-RA-05]).
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    #[test]
    fn an_unannotated_nested_class_does_not_inherit_the_outer_prefix() {
        let m = scan_lang(
            "kt",
            r#"
@RequestMapping("/outer")
class Outer {
    @GetMapping(value = "/a")
    fun outerHandler(): String { return "" }

    class Inner {
        @GetMapping(value = "/b")
        fun innerHandler(): String { return "" }
    }
}
"#,
        );
        assert_eq!(
            sorted_triples(&m),
            vec![
                (
                    "/b".to_string(),
                    "GET".to_string(),
                    Some("innerHandler".to_string())
                ),
                (
                    "/outer/a".to_string(),
                    "GET".to_string(),
                    Some("outerHandler".to_string())
                ),
            ]
        );
        assert!(m.refusals.is_empty(), "{:?}", m.refusals);
    }

    /// A class-level `@RequestMapping` that names no path argument declares no
    /// prefix: its handlers compose to their own paths and nothing is refused.
    /// Without this the `method =`-only form — legal, and common on a base
    /// controller — would silently refuse every route in the class.
    #[test]
    fn a_class_annotation_with_no_path_argument_is_not_a_prefix() {
        for arguments in [
            "method = RequestMethod.GET",
            r#"produces = "application/json""#,
            r#"consumes = ["application/json"]"#,
        ] {
            let m = scan_lang("kt", &in_prefixed_class(arguments, USERS_HANDLER));
            assert_eq!(sorted_triples(&m), users_route("/users"), "{arguments}");
            assert!(m.refusals.is_empty(), "{arguments}: {:?}", m.refusals);
        }
    }

    /// Only `@RequestMapping` prefixes a type in Spring. A stereotype marker,
    /// and any other string-valued class annotation, must contribute no prefix
    /// — otherwise `@Validated("group")` would relocate every route in the
    /// class.
    #[test]
    fn only_request_mapping_prefixes_a_type() {
        for annotation in [
            "@RestController",
            r#"@Validated("group")"#,
            r#"@Profile(value = "prod")"#,
            r#"@GetMapping("/not-a-prefix")"#,
        ] {
            let m = scan_lang(
                "kt",
                &format!("{annotation}\nclass C {{\n{USERS_HANDLER}\n}}\n"),
            );
            assert_eq!(sorted_triples(&m), users_route("/users"), "{annotation}");
            assert!(
                m.prefixes
                    .iter()
                    .all(|s| s.literals.is_empty() && s.opaque.is_empty()),
                "{annotation} must declare no prefix: {:?}",
                m.prefixes
            );
        }
    }

    /// An `object` declaration is a type declaration too, and Kotlin's is a
    /// distinct node kind from a class's. Leaving it out would not lose the
    /// route safely: the handler would be promoted at its bare method path, a
    /// wrong address rather than an absent one ([NFR-RA-05]).
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    #[test]
    fn an_object_declaration_prefixes_like_a_class() {
        let got = kotlin_routes(&format!(
            "@RequestMapping(\"/v1\")\nobject O {{\n{USERS_HANDLER}\n}}\n"
        ));
        assert_eq!(got, users_route("/v1/users"));
    }

    /// A method-level `@RequestMapping` must never be read as a prefix
    /// governing its own route — the `/v1/x/v1/x` self-composition S-329
    /// observed in Java when "a type declaration" was spelled as a supertype
    /// that also matched methods. Kotlin's spelling (a wildcard constrained by
    /// a type-body child) excludes `function_declaration`, whose body is a
    /// `function_body`.
    #[test]
    fn a_method_level_mapping_is_never_its_own_prefix() {
        let got = kotlin_routes(&in_class(
            r#"    @RequestMapping(method = RequestMethod.GET, value = "/v1/x")
    fun getX(): String { return "" }"#,
        ));
        assert_eq!(
            got,
            vec![(
                "/v1/x".to_string(),
                "ANY".to_string(),
                Some("getX".to_string())
            )]
        );
    }

    /// The `[framework_methods]` gate still runs first: a marker annotation the
    /// pathless pattern matches promotes nothing when its name is not in the
    /// table ([FR-FW-04]). Kotlin's bare implementation of a prefixed interface
    /// is *even* safer than Java's — it carries the `override` keyword, not an
    /// annotation — but a plain `@Autowired` handler is the shape that would
    /// double a route if the gate moved.
    ///
    /// [FR-FW-04]: ../../../docs/specs/requirements/FR-FW-04.md
    #[test]
    fn an_unmapped_marker_annotation_in_a_prefixed_class_promotes_nothing() {
        let m = scan_lang(
            "kt",
            &in_prefixed_class(
                r#""/v1""#,
                "    @Autowired\n    fun listUsers(): String { return \"\" }",
            ),
        );
        assert!(m.routes.is_empty(), "{:?}", m.routes);
        assert!(m.refusals.is_empty(), "{:?}", m.refusals);
    }

    /// The wiring composition depends on, asserted at the source level rather
    /// than on hand-built values: the scope must span the whole declaration —
    /// annotation *and* body — or a handler's byte offset would fall outside it
    /// and every route would silently lose its prefix. Kotlin puts a
    /// declaration's annotations inside its `modifiers` child, so
    /// `class_declaration` already covers them; this test is what would notice
    /// if that stopped being true.
    #[test]
    fn a_prefix_scope_spans_the_whole_declaration_it_governs() {
        let source = in_prefixed_class(r#""/v1""#, USERS_HANDLER);
        let m = scan_lang("kt", &source);
        let scope = m
            .prefixes
            .iter()
            .find(|s| !s.literals.is_empty())
            .unwrap_or_else(|| panic!("a declared prefix: {:?}", m.prefixes));
        assert_eq!(
            scope
                .literals
                .iter()
                .map(|l| l.text.as_str())
                .collect::<Vec<_>>(),
            ["/v1"]
        );
        assert!(
            scope.literals.iter().all(|l| l.resolvable),
            "a literal prefix reads as an address"
        );
        // The whole class declaration: from its first annotation to its closing
        // brace.
        assert_eq!(scope.start, source.find('@').expect("annotation"));
        assert_eq!(scope.end, source.trim_end().len());
        // And the handler this scope must govern really is inside it.
        let handler = source.find("@GetMapping").expect("handler annotation");
        assert!(
            scope.start <= handler && handler < scope.end,
            "{scope:?} must contain byte {handler}"
        );
    }

    /// The Kotlin query names each shared composition capture as its own
    /// capture, not merely as a prefix of a longer one — asserted by
    /// exact-name counting, the same guard the Java query carries, so a rename
    /// fails here rather than passing vacuously.
    /// `every_framework_query_naming_a_prefix_also_names_its_scope` covers the
    /// pairing rule across every shipped query.
    #[test]
    fn the_kotlin_query_delegates_composition_by_naming_the_shared_captures() {
        let query = include_str!("../../../plugins/kotlin/queries/frameworks.scm");
        for capture in [
            "fw.route.prefix",
            "fw.route.prefix.scope",
            "fw.route.prefix.opaque",
            "fw.route.path",
            "fw.route.path.named",
            "fw.route.anchor",
        ] {
            assert!(
                capture_occurrences(query, capture) > 0,
                "the Kotlin query must name @{capture}"
            );
        }
        // Both path patterns anchor (see
        // `path_origins_record_the_annotation_site_and_the_named_rank` for the
        // behavioural half): a path with no anchor competes with nothing and
        // always survives, so anchoring one pattern and not the other silently
        // promotes both at a mixed-form annotation.
        assert!(
            capture_occurrences(query, "fw.route.anchor") >= 3,
            "every registration pattern must anchor: positional, named, pathless"
        );
        // The stale claim this story corrected: the header must no longer say
        // named arguments are unsupported, because they never were merely
        // absent — the unanchored positional pattern was matching them.
        assert!(
            !query.contains("Deliberately NOT captured in v1"),
            "the header's exclusion note must be corrected, not carried over"
        );
    }
}

// ── Java/Kotlin parity (S-330) ───────────────────────────────────────────────

/// The story's own acceptance criterion, asserted rather than argued: every
/// [S-328]/[S-329] behaviour holds for the equivalent Kotlin fixture, with
/// *identical* resulting routes. The table is paired source — the same Spring
/// annotation written in each language's syntax — and the assertion compares
/// the promoted route set, the refusal count and the promoted components, so a
/// divergence in any of the three fails here rather than in one language's own
/// module.
#[cfg(all(feature = "lang-java", feature = "lang-kotlin"))]
mod jvm_parity {
    use super::*;

    /// Everything the scan established, in a form two languages can be
    /// compared on: the sorted route set, how many registrations were refused,
    /// and the promoted component names.
    fn promoted(ext: &str, source: &str) -> (Vec<String>, usize, Vec<String>) {
        let m = scan_lang(ext, source);
        let mut routes: Vec<String> = m
            .routes
            .iter()
            .map(|r| format!("{} {} -> {:?}", r.method, r.path, r.handler))
            .collect();
        routes.sort();
        let mut components: Vec<String> =
            m.components.iter().map(|c| c.type_path.clone()).collect();
        components.sort();
        (routes, m.refusals.len(), components)
    }

    #[test]
    fn kotlin_and_java_fixtures_promote_identical_routes() {
        for (label, kotlin, java) in PAIRED_FIXTURES {
            assert_eq!(
                promoted("kt", kotlin),
                promoted("java", java),
                "{label}: Kotlin and Java must promote the same routes"
            );
        }
        assert!(
            PAIRED_FIXTURES.len() >= 20,
            "the parity table must cover the fixture set, not a sample"
        );
    }

    /// The candidacy gate and the method table are *shared*, not merely
    /// similar: a route promoted from Kotlin has cleared the same
    /// `org::springframework` ledger fingerprint and been named by the same
    /// annotation→verb mapping as its Java twin ([FR-FW-04]). Asserted both
    /// semantically (the loaded descriptors are equal) and byte-for-byte on the
    /// `[framework_methods]` rows, so a row added to one descriptor and not the
    /// other fails here.
    ///
    /// [FR-FW-04]: ../../../docs/specs/requirements/FR-FW-04.md
    #[test]
    fn the_candidacy_gate_and_method_table_are_shared_between_java_and_kotlin() {
        let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
        let java = registry.for_extension("java").expect("java plugin");
        let kotlin = registry.for_extension("kt").expect("kotlin plugin");

        assert_eq!(
            java.semantics().framework_detectors,
            kotlin.semantics().framework_detectors,
            "the ledger candidacy gate is one fingerprint for both languages"
        );
        assert_eq!(
            java.semantics().framework_detectors,
            ["org::springframework"],
            "…and it is Spring's package prefix"
        );
        assert_eq!(
            java.semantics().framework_methods,
            kotlin.semantics().framework_methods,
            "the annotation→verb table is one mapping for both languages"
        );

        // Byte-identical, not just equal once parsed: the rows as written.
        let rows = |descriptor: &str| -> Vec<String> {
            descriptor
                .lines()
                .skip_while(|line| line.trim() != "[framework_methods]")
                .skip(1)
                .take_while(|line| !line.trim_start().starts_with('['))
                .map(|line| line.to_string())
                .collect()
        };
        let java_rows = rows(include_str!("../../../plugins/java/plugin.toml"));
        assert!(!java_rows.is_empty(), "the Java table must be found");
        assert_eq!(
            java_rows,
            rows(include_str!("../../../plugins/kotlin/plugin.toml")),
            "the [framework_methods] rows must be byte-identical"
        );
    }

    /// The non-duplication criterion, proved structurally rather than by
    /// reading the diff: the shared interpreter names **no** JVM annotation
    /// node kind in its code, so neither language can have a composition
    /// implementation of its own. Comments are stripped first — the module
    /// documents these node kinds on purpose, to explain what the queries
    /// carry.
    ///
    /// A second copy of composition would have to name at least one of these
    /// (there is no other way to walk an annotation's arguments), which is what
    /// makes the absence a real check rather than a stylistic one.
    #[test]
    fn no_language_specific_composition_code_exists() {
        let code: String = include_str!("../framework.rs")
            .lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        for node_kind in [
            // Kotlin
            "value_argument",
            "constructor_invocation",
            "collection_literal",
            "user_type",
            "simple_identifier",
            "class_body",
            "object_declaration",
            "interpolation",
            // Java
            "element_value_pair",
            "element_value_array_initializer",
            "annotation_argument_list",
            "marker_annotation",
            "interface_body",
            "method_declaration",
        ] {
            assert!(
                !code.contains(node_kind),
                "the shared interpreter must not name the {node_kind} node kind"
            );
        }
    }

    /// The same Spring annotation written in each language's syntax. Kotlin
    /// arrays are `[…]` where Java's are `{…}`, Kotlin's implementation
    /// override is a keyword where Java's is `@Override`, and Kotlin's
    /// interfaces and objects are the same node kind as its classes — none of
    /// which may change the promoted route.
    const PAIRED_FIXTURES: &[(&str, &str, &str)] = &[
        (
            "named value argument",
            "class C {\n    @RequestMapping(method = RequestMethod.GET, value = \"/v1/x\", produces = \"application/json\")\n    fun getX(): String { return \"\" }\n}\n",
            "public class C {\n    @RequestMapping(method = RequestMethod.GET, value = \"/v1/x\", produces = \"application/json\")\n    public String getX() { return \"\"; }\n}\n",
        ),
        (
            "named path alias",
            "class C {\n    @GetMapping(path = \"/v1/y\")\n    fun getY(): String { return \"\" }\n}\n",
            "public class C {\n    @GetMapping(path = \"/v1/y\")\n    public String getY() { return \"\"; }\n}\n",
        ),
        (
            "list-valued method path",
            "class C {\n    @GetMapping(value = [\"/a\", \"/b\"])\n    fun get(): String { return \"\" }\n}\n",
            "public class C {\n    @GetMapping(value = {\"/a\", \"/b\"})\n    public String get() { return \"\"; }\n}\n",
        ),
        (
            "positional method path",
            "class C {\n    @GetMapping(\"/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "public class C {\n    @GetMapping(\"/users\")\n    public String listUsers() { return \"\"; }\n}\n",
        ),
        (
            "produces/consumes only",
            "class C {\n    @GetMapping(produces = \"application/json\", consumes = \"text/plain\")\n    fun getX(): String { return \"\" }\n}\n",
            "public class C {\n    @GetMapping(produces = \"application/json\", consumes = \"text/plain\")\n    public String getX() { return \"\"; }\n}\n",
        ),
        (
            "prefixed interface and bare implementation",
            "@RequestMapping(\"/v1\")\ninterface UserApi {\n    @RequestMapping(method = RequestMethod.GET, value = \"/users\", produces = \"application/json\")\n    fun listUsers(): String\n\n    @GetMapping(path = [\"/users/{id}\", \"/users/by-id/{id}\"])\n    fun getUser(id: String): String\n}\n\n@RestController\nclass UserController : UserApi {\n    override fun listUsers(): String { return \"\" }\n    override fun getUser(id: String): String { return \"\" }\n}\n",
            "@RequestMapping(\"/v1\")\npublic interface UserApi {\n    @RequestMapping(method = RequestMethod.GET, value = \"/users\", produces = \"application/json\")\n    String listUsers();\n\n    @GetMapping(path = {\"/users/{id}\", \"/users/by-id/{id}\"})\n    String getUser(String id);\n}\n\n@RestController\nclass UserController implements UserApi {\n    @Override\n    public String listUsers() { return \"\"; }\n    @Override\n    public String getUser(String id) { return \"\"; }\n}\n",
        ),
        (
            "pathless handler in a prefixed class",
            "@RequestMapping(\"/v1/users\")\n@RestController\nclass C {\n    @GetMapping\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RequestMapping(\"/v1/users\")\n@RestController\npublic class C {\n    @GetMapping\n    public String listUsers() { return \"\"; }\n}\n",
        ),
        (
            "pathless handler with no prefix",
            "class C {\n    @GetMapping\n    fun listUsers(): String { return \"\" }\n}\n",
            "public class C {\n    @GetMapping\n    public String listUsers() { return \"\"; }\n}\n",
        ),
        (
            "unannotated nested type inherits nothing",
            "@RequestMapping(\"/outer\")\nclass Outer {\n    @GetMapping(value = \"/a\")\n    fun outerHandler(): String { return \"\" }\n\n    class Inner {\n        @GetMapping(value = \"/b\")\n        fun innerHandler(): String { return \"\" }\n    }\n}\n",
            "@RequestMapping(\"/outer\")\npublic class Outer {\n    @GetMapping(value = \"/a\")\n    public String outerHandler() { return \"\"; }\n\n    public static class Inner {\n        @GetMapping(value = \"/b\")\n        public String innerHandler() { return \"\"; }\n    }\n}\n",
        ),
        (
            "nested prefixed type takes its own prefix",
            "@RequestMapping(\"/outer\")\nclass Outer {\n    @GetMapping(value = \"/a\")\n    fun outerHandler(): String { return \"\" }\n\n    @RequestMapping(\"/inner\")\n    class Inner {\n        @GetMapping(value = \"/b\")\n        fun innerHandler(): String { return \"\" }\n    }\n}\n",
            "@RequestMapping(\"/outer\")\npublic class Outer {\n    @GetMapping(value = \"/a\")\n    public String outerHandler() { return \"\"; }\n\n    @RequestMapping(\"/inner\")\n    public static class Inner {\n        @GetMapping(value = \"/b\")\n        public String innerHandler() { return \"\"; }\n    }\n}\n",
        ),
        (
            "positional non-literal prefix is refused",
            "@RequestMapping(BASE)\nclass C {\n    @GetMapping(value = \"/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RequestMapping(BASE)\npublic class C {\n    @GetMapping(value = \"/users\")\n    public String listUsers() { return \"\"; }\n}\n",
        ),
        (
            "named non-literal prefix is refused",
            "@RequestMapping(value = BASE)\nclass C {\n    @GetMapping(value = \"/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RequestMapping(value = BASE)\npublic class C {\n    @GetMapping(value = \"/users\")\n    public String listUsers() { return \"\"; }\n}\n",
        ),
        (
            "empty prefix list declares no prefix",
            "@RequestMapping(value = [])\nclass C {\n    @GetMapping(value = \"/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RequestMapping(value = {})\npublic class C {\n    @GetMapping(value = \"/users\")\n    public String listUsers() { return \"\"; }\n}\n",
        ),
        (
            "class annotation with no path argument",
            "@RequestMapping(produces = \"application/json\")\nclass C {\n    @GetMapping(value = \"/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RequestMapping(produces = \"application/json\")\npublic class C {\n    @GetMapping(value = \"/users\")\n    public String listUsers() { return \"\"; }\n}\n",
        ),
        (
            "class annotation with only a method argument",
            "@RequestMapping(method = RequestMethod.GET)\nclass C {\n    @GetMapping(value = \"/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RequestMapping(method = RequestMethod.GET)\npublic class C {\n    @GetMapping(value = \"/users\")\n    public String listUsers() { return \"\"; }\n}\n",
        ),
        (
            "named list-valued class prefix",
            "@RequestMapping(value = [\"/v1\", \"/v2\"])\nclass C {\n    @GetMapping(value = \"/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RequestMapping(value = {\"/v1\", \"/v2\"})\npublic class C {\n    @GetMapping(value = \"/users\")\n    public String listUsers() { return \"\"; }\n}\n",
        ),
        (
            "positional list-valued class prefix",
            "@RequestMapping([\"/v1\", \"/v2\"])\nclass C {\n    @GetMapping(value = \"/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RequestMapping({\"/v1\", \"/v2\"})\npublic class C {\n    @GetMapping(value = \"/users\")\n    public String listUsers() { return \"\"; }\n}\n",
        ),
        (
            "mixed class prefix list composes on its literal",
            "@RequestMapping(value = [\"/v1\", BASE])\nclass C {\n    @GetMapping(value = \"/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RequestMapping(value = {\"/v1\", BASE})\npublic class C {\n    @GetMapping(value = \"/users\")\n    public String listUsers() { return \"\"; }\n}\n",
        ),
        (
            "property-placeholder prefix is refused",
            "@RequestMapping(\"${api.base}\")\nclass C {\n    @GetMapping(value = \"/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RequestMapping(\"${api.base}\")\npublic class C {\n    @GetMapping(value = \"/users\")\n    public String listUsers() { return \"\"; }\n}\n",
        ),
        (
            "only RequestMapping prefixes a type",
            "@Validated(\"group\")\nclass C {\n    @GetMapping(value = \"/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "@Validated(\"group\")\npublic class C {\n    @GetMapping(value = \"/users\")\n    public String listUsers() { return \"\"; }\n}\n",
        ),
        (
            "separator normalisation",
            "@RequestMapping(\"/v1/\")\nclass C {\n    @GetMapping(value = \"users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RequestMapping(\"/v1/\")\npublic class C {\n    @GetMapping(value = \"users\")\n    public String listUsers() { return \"\"; }\n}\n",
        ),
        (
            "unmapped annotation carrying arguments",
            "class C {\n    @Operation(value = \"/v1/x\")\n    @ApiResponse(path = [\"/a\", \"/b\"])\n    fun getX(): String { return \"\" }\n}\n",
            "public class C {\n    @Operation(value = \"/v1/x\")\n    @ApiResponse(path = {\"/a\", \"/b\"})\n    public String getX() { return \"\"; }\n}\n",
        ),
        (
            "two annotations on one handler",
            "class C {\n    @GetMapping(\"/read\")\n    @PostMapping(value = \"/write\")\n    fun both(): String { return \"\" }\n}\n",
            "public class C {\n    @GetMapping(\"/read\")\n    @PostMapping(value = \"/write\")\n    public String both() { return \"\"; }\n}\n",
        ),
        (
            "both alias keys on one annotation",
            "class C {\n    @GetMapping(value = \"/x\", path = \"/y\")\n    fun get(): String { return \"\" }\n}\n",
            "public class C {\n    @GetMapping(value = \"/x\", path = \"/y\")\n    public String get() { return \"\"; }\n}\n",
        ),
        (
            "placeholder method path promoted verbatim",
            "class C {\n    @GetMapping(value = \"${api.base}/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "public class C {\n    @GetMapping(value = \"${api.base}/users\")\n    public String listUsers() { return \"\"; }\n}\n",
        ),
        (
            "empty-string prefix over a pathless handler",
            "@RequestMapping(\"\")\nclass C {\n    @GetMapping\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RequestMapping(\"\")\npublic class C {\n    @GetMapping\n    public String listUsers() { return \"\"; }\n}\n",
        ),
        (
            "stereotype component",
            "@RestController\nclass UserController {\n    @GetMapping(\"/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RestController\npublic class UserController {\n    @GetMapping(\"/users\")\n    public String listUsers() { return \"\"; }\n}\n",
        ),
        (
            "positional array method path is captured by neither language",
            "class C {\n    @GetMapping([\"/a\", \"/b\"])\n    fun get(): String { return \"\" }\n}\n",
            "public class C {\n    @GetMapping({\"/a\", \"/b\"})\n    public String get() { return \"\"; }\n}\n",
        ),
        (
            "non-literal named method paths promote nothing",
            "class C {\n    @GetMapping(value = BASE)\n    fun a(): String { return \"\" }\n    @GetMapping(value = BASE + \"/x\")\n    fun b(): String { return \"\" }\n    @GetMapping(value = Paths.USERS)\n    fun c(): String { return \"\" }\n}\n",
            "public class C {\n    @GetMapping(value = BASE)\n    public String a() { return \"\"; }\n    @GetMapping(value = BASE + \"/x\")\n    public String b() { return \"\"; }\n    @GetMapping(value = Paths.USERS)\n    public String c() { return \"\"; }\n}\n",
        ),
        (
            "mixed list method path",
            "class C {\n    @GetMapping(value = [\"/a\", BASE + \"/b\"])\n    fun get(): String { return \"\" }\n}\n",
            "public class C {\n    @GetMapping(value = {\"/a\", BASE + \"/b\"})\n    public String get() { return \"\"; }\n}\n",
        ),
        (
            "paths converging after composition collapse",
            "@RequestMapping(\"/v1\")\nclass C {\n    @GetMapping(value = \"/users\")\n    fun listUsers(): String { return \"\" }\n    @GetMapping(value = \"users\")\n    fun listUsersAgain(): String { return \"\" }\n}\n",
            "@RequestMapping(\"/v1\")\npublic class C {\n    @GetMapping(value = \"/users\")\n    public String listUsers() { return \"\"; }\n    @GetMapping(value = \"users\")\n    public String listUsersAgain() { return \"\"; }\n}\n",
        ),
        (
            "method-level mapping is not its own prefix",
            "class C {\n    @RequestMapping(method = RequestMethod.GET, value = \"/v1/x\")\n    fun getX(): String { return \"\" }\n}\n",
            "public class C {\n    @RequestMapping(method = RequestMethod.GET, value = \"/v1/x\")\n    public String getX() { return \"\"; }\n}\n",
        ),
        (
            "annotation order does not decide the prefix",
            "@RestController\n@RequestMapping(\"/v1\")\nclass C {\n    @GetMapping(value = \"/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RestController\n@RequestMapping(\"/v1\")\npublic class C {\n    @GetMapping(value = \"/users\")\n    public String listUsers() { return \"\"; }\n}\n",
        ),
        (
            "explicitly empty method path under a prefix",
            "@RequestMapping(\"/v1\")\nclass C {\n    @GetMapping(\"\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RequestMapping(\"/v1\")\npublic class C {\n    @GetMapping(\"\")\n    public String listUsers() { return \"\"; }\n}\n",
        ),
        (
            "unmapped marker annotation in a prefixed class",
            "@RequestMapping(\"/v1\")\nclass C {\n    @Autowired\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RequestMapping(\"/v1\")\npublic class C {\n    @Autowired\n    public String listUsers() { return \"\"; }\n}\n",
        ),
    ];
}
