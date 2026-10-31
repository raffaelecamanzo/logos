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
    scan_source(&mut parser, plugin, &format!("src/Fixture.{ext}"), source, None)
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

/// The **sorted** `(path, method, handler)` projection of an already-scanned
/// file — for the tests that assert routes *and* refusals from one scan, and
/// that want the promoted *set* rather than tree-sitter's match order.
///
/// Feature-gated because it has no caller in a build with neither JVM grammar,
/// and `--all-targets -- -D warnings` treats an unused test helper as an error.
/// `route_triples` above cannot simply sort instead: the Axum/Actix tests assert
/// their chains in order.
#[cfg(any(feature = "lang-java", feature = "lang-kotlin"))]
fn sorted_triples(m: &FileMatches) -> Vec<(String, String, Option<String>)> {
    let mut got: Vec<(String, String, Option<String>)> = m
        .routes
        .iter()
        .map(|r| (r.path.clone(), r.method.clone(), r.handler.clone()))
        .collect();
    got.sort();
    got
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
    // An empty literal reads as an empty path — which is no path at all, so
    // CR-110's guard promotes no route for it. Reading and promoting are
    // separate questions: `an_empty_method_path_still_composes_onto_its_prefix`
    // proves the literal is still read, by composing it onto a prefix.
    let empty = routes("fn f() { let _ = r.route(\"\", get(h)); }");
    assert!(empty.is_empty(), "{empty:?}");
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
        folded_from: Vec::new(),
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
        folded_from: Vec::new(),
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
        folded_from: Vec::new(),
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
                folded_from: Vec::new(),
            })
            .collect(),
        opaque: opaque.to_vec(),
        nested: false,
        absorbs: false,
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
        folded_from: Vec::new(),
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
/// Kotlin `"${BASE}/v1"` string template exposes an `interpolation` child that
/// sits *inside* the literal, so a query can name it directly.
///
/// The literal text here is deliberately one that
/// [`is_resolvable_prefix`] **accepts**, so the disqualification can only come
/// from the overlap. It used to read `"$BASE/v1"`, which S-330's template rule
/// now rejects on its text as well — leaving the test green while no longer
/// exercising the mechanism it names.
#[test]
fn an_opaque_capture_disqualifies_only_the_literals_it_overlaps() {
    // A fragment INSIDE a literal: the literal spans 10..20, the unreadable
    // fragment 11..16, so the literal cannot be read as a whole address.
    let mut inside = FileMatches {
        routes: vec![route_at("/users", 50)],
        prefixes: vec![scope(0, 100, &[("/base/v1", 10, 20)], &[(11, 16)])],
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

/// Occurrences of `@<name>` in a query's **patterns**, counting only the
/// capture whose name is exactly `name` — `@fw.route.prefix.scope` is not an
/// occurrence of `@fw.route.prefix`.
///
/// Comment lines are dropped first, and that is load-bearing rather than tidy.
/// These query files document their own capture vocabulary in prose, so a
/// whole-file text count is satisfied by the header alone: measured before this
/// filter, the Python query "named" `@fw.route.prefix` and
/// `@fw.route.prefix.scope` twice and once respectively while capturing neither,
/// which passed `every_framework_query_naming_a_prefix_also_names_its_scope`
/// vacuously — the exact failure that guard exists to prevent. Counting only
/// pattern text also stops a *floor* assertion from being inflated by a comment
/// mentioning the capture it pins.
#[cfg(test)]
fn capture_occurrences(source: &str, name: &str) -> usize {
    let patterns = query_patterns(source);
    let needle = format!("@{name}");
    patterns
        .match_indices(&needle)
        .filter(|(at, _)| {
            patterns[at + needle.len()..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_alphanumeric() && c != '.' && c != '_' && c != '-')
        })
        .count()
}

/// A `.scm` query with its comment lines removed — tree-sitter query comments
/// run from a `;` to the end of the line, and every comment in the shipped
/// queries occupies a whole line.
#[cfg(test)]
fn query_patterns(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with(';'))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Assert `query` names each of `captures` as its own capture in its patterns,
/// not merely as a prefix of a longer name and not merely in its header prose.
#[cfg(test)]
fn names_every_capture(query: &str, label: &str, captures: &[&str]) {
    for capture in captures {
        assert!(
            capture_occurrences(query, capture) > 0,
            "the {label} query must name @{capture} in a pattern"
        );
    }
}

/// Assert every registration pattern in `query` carries `@fw.route.anchor`.
///
/// Expressed as an **equality against the method count** rather than a floor:
/// a floor equal to today's pattern count cannot notice a fourth registration
/// pattern added without an anchor, and per S-328 a path with no anchor
/// competes with nothing in [`drop_outranked_paths`] and always survives — so
/// the omission silently promotes two routes for one registration. Every
/// registration pattern in both JVM queries names its method
/// (`@fw.route.method`) — the annotation or the DSL call — which makes the
/// method count the surface this invariant has to scale with. (It was the
/// handler count until S-561: a functional registration written as a lambda
/// names no handler.)
#[cfg(test)]
fn every_registration_pattern_anchors(query: &str, label: &str) {
    // A predicate line (`(#match? @fw.route.method …)`) names the capture
    // without capturing it, so it is not a registration pattern.
    let captured: String = query
        .lines()
        .filter(|line| !line.trim_start().starts_with("(#"))
        .collect::<Vec<_>>()
        .join("\n");
    let methods = capture_occurrences(&captured, "fw.route.method");
    assert!(methods > 0, "{label}: no registration pattern found");
    assert_eq!(
        capture_occurrences(&captured, "fw.route.anchor"),
        methods,
        "{label}: every registration pattern must anchor"
    );
}

/// The Java query names each shared capture as its own capture, not merely as a
/// prefix of a longer one and not merely in its header — asserted by exact-name
/// counting over pattern text, so a rename fails here rather than passing
/// vacuously.
///
/// Kept beside its Kotlin twin (S-330) and asserting the same set: the two
/// queries describe one framework through one capture vocabulary, and an
/// invariant pinned for one language and not the other is how the pair drifts.
#[test]
fn the_java_query_delegates_composition_by_naming_the_shared_captures() {
    let query = include_str!("../../../plugins/java/queries/frameworks.scm");
    names_every_capture(query, "Java", JVM_SPRING_CAPTURES);
    every_registration_pattern_anchors(query, "Java");
}

/// The Kotlin query names the same shared captures and anchors the same way
/// (S-330) — the parity claim at the level of the query's own text, which is
/// what `kotlin_and_java_fixtures_promote_identical_routes` cannot see.
#[test]
fn the_kotlin_query_delegates_composition_by_naming_the_shared_captures() {
    let query = include_str!("../../../plugins/kotlin/queries/frameworks.scm");
    names_every_capture(query, "Kotlin", JVM_SPRING_CAPTURES);
    every_registration_pattern_anchors(query, "Kotlin");
    // The stale claim S-330 corrected: the header must no longer say named
    // arguments are unsupported, because they never were merely absent — the
    // unanchored positional pattern was already matching them.
    assert!(
        !query.contains("Deliberately NOT captured in v1"),
        "the header's exclusion note must be corrected, not carried over"
    );
}

/// The capture vocabulary both JVM queries must name: the S-328 path forms, the
/// S-328 precedence anchor, and the three S-329 composition captures.
#[cfg(test)]
const JVM_SPRING_CAPTURES: &[&str] = &[
    "fw.route.path",
    "fw.route.path.named",
    "fw.route.anchor",
    "fw.route.handler",
    "fw.route.method",
    "fw.route.prefix",
    "fw.route.prefix.scope",
    "fw.route.prefix.opaque",
];

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
        "src/UserController.kt",
        r#"
@RequestMapping("/v1/")
class UserController {
    @GetMapping("/users")
    fun listUsers(): String { return "" }
}
"#,
        None,
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

// ── CR-110: a promoted route's path has to be a path ─────────────────────────

/// The guard's rule, stated directly on the predicate rather than through a
/// grammar.
///
/// A URL path carries the `/` that makes it one — absolutely (`/users`) or
/// relatively, which a Django URLconf and an ASP.NET Core attribute both
/// register by design. A bare single token is a property key in every ratified
/// dialect and a path in none, which is the whole of CR-110's observed
/// population. An opening template reference is exempt: the separators may live
/// inside the property, and [FR-FW-05] promotes those verbatim.
///
/// [FR-FW-05]: ../../../docs/specs/requirements/FR-FW-05.md
#[test]
fn only_a_slash_bearing_or_reference_opening_path_is_a_url_path() {
    for path in [
        "/",
        "/users",
        "/users/{id}",
        // Django's URLconf and ASP.NET Core's attribute routes, relative by
        // design — the shapes a leading-`/` rule would have deleted.
        "users/",
        "api/users",
        "${api.base}/users",
        "${api.base}",
        "$BASE/x",
    ] {
        assert!(is_a_url_path(path), "{path} is a URL path");
    }
    // Every one of CR-110's 37 observed false positives is one of these: a bare
    // token, with no separator and no reference.
    for not in ["", "active", "year", "codGestPEC", "mce_marker", "__caret", "$"] {
        assert!(!is_a_url_path(not), "{not} is not a URL path");
    }
    // The exemption is a *reference*, not a bare sigil: `$` alone, or followed
    // by anything that cannot open an identifier, names nothing.
    for not_a_reference in ["$", "$ x", "$1", "price$"] {
        assert!(
            !opens_with_template_reference(not_a_reference),
            "{not_a_reference} opens no reference"
        );
    }
}

/// A relative registration from a framework that registers relatively survives
/// the guard, end to end through its own query.
///
/// This is the conformance edge CR-110 §3.2's leading-`/` wording missed:
/// [FR-FW-03] ratifies Django, whose URLconf paths take their prefix from the
/// root `include()` and are therefore written relative. Deleting them would
/// breach [FR-FW-01]'s existing criterion for a ratified framework.
///
/// [FR-FW-01]: ../../../docs/specs/requirements/FR-FW-01.md
/// [FR-FW-03]: ../../../docs/specs/requirements/FR-FW-03.md
#[test]
#[cfg(feature = "lang-python")]
fn a_relative_django_urlconf_path_survives_the_guard() {
    let m = scan_lang(
        "py",
        "from django.urls import path\n\ndef index(request):\n    return None\n\nurlpatterns = [path(\"users/\", index)]\n",
    );
    assert_eq!(
        route_triples(m),
        vec![(
            "users/".to_string(),
            "ANY".to_string(),
            Some("index".to_string())
        )]
    );
}

/// A dropped candidate leaves **nothing** — no route, and no refusal.
/// [FR-FW-05]'s refusal accounting is for an endpoint Logos could not address;
/// a property lookup was never an endpoint, and recording one would
/// manufacture a coverage denominator out of ordinary code (CR-110 §10).
///
/// [FR-FW-05]: ../../../docs/specs/requirements/FR-FW-05.md
#[test]
fn a_path_that_is_no_path_is_dropped_without_a_coverage_reason() {
    let m = scan("fn f() { let _ = r.route(\"active\", get(h)); }");
    assert!(m.routes.is_empty(), "{:?}", m.routes);
    assert!(m.refusals.is_empty(), "{:?}", m.refusals);
}

/// The guard is **shared**, so it cannot be a query's private business: no
/// shipped `frameworks.scm` may constrain `@fw.route.path`'s shape with a
/// predicate of its own.
///
/// This is the testable face of "every plugin inherits it". A per-query
/// `(#match? @fw.route.path "^/")` would satisfy one language's fixtures while
/// leaving the same latent defect in the other nine, and would drift the moment
/// the shared rule changed — which is exactly the failure CR-110 §10 rejected
/// when it put the rule in the pass instead of the TypeScript query.
#[test]
fn no_frameworks_query_constrains_the_path_shape_itself() {
    let mut checked = 0;
    for entry in crate::plugin::grammars::compiled() {
        for query in entry.embedded_queries {
            if !query.relative_path.ends_with("frameworks.scm") {
                continue;
            }
            for line in query_patterns(query.source).lines() {
                let predicate =
                    line.contains("#match?") || line.contains("#any-of?") || line.contains("#eq?");
                assert!(
                    !(predicate && line.contains("@fw.route.path")),
                    "{}: the path-shape rule belongs to the shared pass, not this query: {line}",
                    query.label
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 0, "no frameworks.scm was checked");
}

/// One guard, three capture dialects: a bare-token path promotes nothing
/// whether it arrived through Rust's legacy structural anchor, Go's
/// selector-call pattern, or TypeScript's Express call pattern. Unrelated
/// queries, one outcome — which is what "the rule lives in the pass" means in
/// practice, and the closest a test can get to CR-110 AC5's "in **every**
/// plugin shipping `frameworks`".
///
/// Java stopped being a fourth when S-561 declared Spring's rooting: Spring
/// serves `@GetMapping("users")` from `/users`, so the Java dialect states the
/// slash it left out (`@fw.route.slashed`) and the path is no bare token by the
/// time the guard reads it. The guard itself is untouched — the assertion below
/// pins the rooted route, and the Rust, Go and TypeScript halves pin the rule.
///
/// Go is here by name because AC8 names it: `hermodr-mirror`'s 23 Go routes are
/// the workspace's control group, so the dialect that must be *unaffected* is
/// also the one whose bare-token behaviour is worth pinning.
#[test]
#[cfg(all(
    feature = "lang-java",
    feature = "lang-go",
    feature = "lang-typescript"
))]
fn a_bare_token_path_promotes_nothing_in_any_capture_dialect_but_springs_rooted_one() {
    let rust = scan("fn f() { let _ = r.route(\"users\", get(h)); }");
    assert!(rust.routes.is_empty(), "rust: {:?}", rust.routes);

    let java = scan_lang(
        "java",
        "@RestController\npublic class C {\n    @GetMapping(value = \"users\")\n    public String listUsers() { return \"\"; }\n}\n",
    );
    let java_paths: Vec<&str> = java.routes.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(java_paths, ["/users"], "java: Spring roots what it was written without");
    assert!(java.refusals.is_empty(), "java: {:?}", java.refusals);

    let go = scan_lang("go", "func main() { r.GET(\"users\", listUsers) }\n");
    assert!(go.routes.is_empty(), "go: {:?}", go.routes);
    assert!(go.refusals.is_empty(), "go: {:?}", go.refusals);

    let ts = scan_lang("ts", "router.get(\"users\", listUsers);");
    assert!(ts.routes.is_empty(), "ts: {:?}", ts.routes);
}

/// …and the same Go dialect still promotes an absolute route with its handler —
/// the control half of CR-110 AC8, in miniature: the guard must cost
/// `hermodr-mirror`'s 23 legitimate `net/http`/Gin routes nothing.
#[test]
#[cfg(feature = "lang-go")]
fn an_absolute_go_route_is_untouched_by_the_guard() {
    assert_eq!(
        route_triples(scan_lang(
            "go",
            "func main() { r.GET(\"/users\", listUsers) }\n"
        )),
        vec![(
            "/users".to_string(),
            "GET".to_string(),
            Some("listUsers".to_string())
        )]
    );
}

/// A bare-token method path under a resolvable prefix is a route — the guard
/// reads the **composed** path, so Spring's `@RequestMapping("/api")` +
/// `@GetMapping("users")` survives it, and an empty method path still composes
/// to its prefix alone (the read this asserts is what
/// `raw_and_empty_string_literals_are_read_correctly` defers to).
#[test]
#[cfg(feature = "lang-java")]
fn an_empty_method_path_still_composes_onto_its_prefix() {
    for (written, expected) in [("users", "/api/users"), ("", "/api")] {
        let m = scan_lang(
            "java",
            &format!(
                "@RequestMapping(\"/api\")\n@RestController\npublic class C {{\n    @GetMapping(\"{written}\")\n    public String h() {{ return \"\"; }}\n}}\n"
            ),
        );
        assert_eq!(
            route_triples(m),
            vec![(
                expected.to_string(),
                "GET".to_string(),
                Some("h".to_string())
            )],
            "written {written:?}"
        );
    }
}

/// The reference exemption, exercised end-to-end rather than only as a
/// predicate: a method path that is *nothing but* an unresolved property
/// placeholder still promotes, verbatim.
///
/// Worth its own scan because every other placeholder fixture in this file
/// writes `${api.base}/users` — which carries a `/` and so clears
/// [`is_a_url_path`] through the ordinary branch, leaving the exemption itself
/// unproven. Here the `/` can only be inside the property, which is exactly the
/// case [FR-FW-05] says to promote as written.
///
/// [FR-FW-05]: ../../../docs/specs/requirements/FR-FW-05.md
#[test]
#[cfg(feature = "lang-java")]
fn a_slash_free_placeholder_path_still_promotes_verbatim() {
    let m = scan_lang(
        "java",
        "@RestController\npublic class C {\n    @GetMapping(value = \"${api.base}\")\n    public String h() { return \"\"; }\n}\n",
    );
    assert_eq!(
        route_triples(m),
        vec![(
            "${api.base}".to_string(),
            "GET".to_string(),
            Some("h".to_string())
        )]
    );
}

// ── CR-110: Express receiver scoping (TypeScript / TSX) ──────────────────────

/// Both TS grammars ship the *same* framework query and must stay in step —
/// `.tsx` files (and JavaScript's, which runs the `tsx` copy, S-538) parse with
/// the sibling plugin, so a rule landed in one and not the other leaves half the
/// TypeScript surface unfixed.
#[cfg(feature = "lang-typescript")]
const TS_EXTENSIONS: [&str; 2] = ["ts", "tsx"];

/// The two shapes CR-110 was filed for, asserted as **absences**: [FR-FW-01]'s
/// acceptance criterion only ever said what must be promoted, and that silence
/// is what let `<expr>.get("string")` match an Angular control lookup and a
/// vendored bundle's property read.
///
/// [FR-FW-01]: ../../../docs/specs/requirements/FR-FW-01.md
#[test]
#[cfg(feature = "lang-typescript")]
fn a_property_lookup_shaped_like_a_registration_promotes_no_route() {
    for source in [
        // The `pec-agid-statistics-informer` shape: an Angular `ParamMap` read.
        "const year = params.get('year');",
        // …and its reactive-form sibling, named in CR-110 §6.
        "const y = formGroup.get('year');",
        // The `styleguide` shape: 32 of these, from a vendored minified editor.
        "if (self.state.get('active')) { run(); }",
        "return this.state.get('checked');",
    ] {
        for ext in TS_EXTENSIONS {
            let m = scan_lang(ext, source);
            assert!(m.routes.is_empty(), "{ext} {source}: {:?}", m.routes);
            assert!(m.refusals.is_empty(), "{ext} {source}: {:?}", m.refusals);
        }
    }
}

/// The receiver rule earns its keep on the case the shared path guard cannot
/// see: a lookup keyed on a `/`-bearing string. `cache.get("/cache/key")` is a
/// URL path by shape and still not a route.
#[test]
#[cfg(feature = "lang-typescript")]
fn a_path_shaped_key_on_a_non_router_receiver_promotes_no_route() {
    for ext in TS_EXTENSIONS {
        let m = scan_lang(ext, "const hit = cache.get(\"/cache/key\");");
        assert!(m.routes.is_empty(), "{ext}: {:?}", m.routes);
    }
}

/// A **bare** `this` receiver is not a router for the handler-less pattern,
/// and that exclusion is deliberate rather than an oversight in the name rule.
///
/// `this.get("active")` is textually indistinguishable from the property getter
/// every class writes, which is the very shape CR-110 was filed about — so the
/// receiver alternation names no bare `this` node, and the name rule could not
/// match one anyway. The qualified spellings (`this.app`, `this.router`) are
/// admitted, and are covered by
/// `a_handler_less_registration_on_a_router_receiver_promotes_a_route_with_no_handler`.
///
/// Scoped to the handler-less pattern on purpose: `this.get("/x", handler)`
/// names a handler, so it matches the handler-bearing pattern, which is
/// unscoped by design and still promotes — asserted here so the boundary
/// between the two patterns is pinned rather than assumed.
#[test]
#[cfg(feature = "lang-typescript")]
fn a_bare_this_receiver_is_deliberately_not_a_router() {
    for ext in TS_EXTENSIONS {
        let handler_less = scan_lang(ext, "this.get(\"/x\");");
        assert!(
            handler_less.routes.is_empty(),
            "{ext}: {:?}",
            handler_less.routes
        );
        assert_eq!(
            route_triples(scan_lang(ext, "this.get(\"/x\", handler);")),
            vec![(
                "/x".to_string(),
                "GET".to_string(),
                Some("handler".to_string())
            )],
            "{ext}: the handler-bearing pattern is unscoped by design"
        );
    }
}

/// [FR-FW-01]'s positive case, unchanged: a registration naming its handler
/// still promotes a route with the edge to it.
///
/// [FR-FW-01]: ../../../docs/specs/requirements/FR-FW-01.md
#[test]
#[cfg(feature = "lang-typescript")]
fn an_express_registration_with_a_handler_still_promotes_a_route() {
    for ext in TS_EXTENSIONS {
        assert_eq!(
            route_triples(scan_lang(ext, "app.get(\"/users\", listUsers);")),
            vec![(
                "/users".to_string(),
                "GET".to_string(),
                Some("listUsers".to_string())
            )],
            "{ext}"
        );
    }
}

/// The incremental gate's detector list (S-541): a prefix is import-only only
/// when every loaded language declaring it says so, and the receiver
/// parameters are the union of every language's — so the footprint probe is
/// never narrower than any language's candidacy.
#[test]
#[cfg(all(feature = "lang-typescript", feature = "lang-rust"))]
fn the_gate_detectors_are_import_only_only_where_every_declaring_language_agrees() {
    let import_only = |reg: &LanguageRegistry, prefix: &str| {
        footprint_keys(reg)
            .0
            .into_iter()
            .find(|d| d.prefix == prefix)
            .map(|d| d.import_only)
    };
    let shipped = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
    // JavaScript matches `next` on imports alone; TypeScript on every kind.
    assert_eq!(import_only(&shipped, "next"), Some(false));
    assert_eq!(import_only(&shipped, "axum"), Some(false));
    let receivers = footprint_keys(&shipped).1;
    assert_eq!(receivers.names, ["app", "router"]);
    assert_eq!(receivers.extensions, ["cjs", "js", "jsx", "mjs"], "JavaScript's alone");

    let toy = |name: &'static str, toml: &'static str| crate::plugin::grammars::GrammarEntry {
        manifest_label: name,
        manifest_toml: toml,
        language: tree_sitter_rust::LANGUAGE,
        embedded_queries: &[],
    };
    let load = |entries: &[crate::plugin::grammars::GrammarEntry]| {
        LanguageRegistry::load_from(entries, crate::plugin::AbiRange::runtime(), None, &mut |_| {})
            .expect("a data-only grammar loads")
    };
    let mut entries = crate::plugin::grammars::compiled();
    entries.push(toy(
        "toyone/plugin.toml",
        r#"
            name = "toyone"
            extensions = ["toyone"]
            module_separator = "."
            abi_version = 15
            capabilities = []
            framework_detectors = ["toyweb"]
            [framework_candidacy]
            detectors_import_only = true
            receiver_parameters = ["server"]
        "#,
    ));
    let alone = load(&entries);
    assert_eq!(import_only(&alone, "toyweb"), Some(true), "its only declarer agrees");
    let receivers = footprint_keys(&alone).1;
    assert_eq!(receivers.names, ["app", "router", "server"]);
    assert_eq!(receivers.extensions, ["cjs", "js", "jsx", "mjs", "toyone"]);

    // One more language naming `toyweb` on any reference kind widens it.
    entries.push(toy(
        "toytwo/plugin.toml",
        r#"
            name = "toytwo"
            extensions = ["toytwo"]
            module_separator = "."
            abi_version = 15
            capabilities = []
            framework_detectors = ["toyweb"]
        "#,
    ));
    assert_eq!(import_only(&load(&entries), "toyweb"), Some(false));
}

/// A JavaScript template-literal path is read verbatim and put to the shared
/// path rule like any other (S-541, [FR-FW-03]): a template whose static tail
/// starts with `/` is a route, its leading interpolation kept as a prefix this
/// pass does not resolve; a template opening with a reference is kept as a
/// Spring `${api.base}` placeholder is; one that is neither is no path.
///
/// [FR-FW-03]: ../../../docs/specs/requirements/FR-FW-03.md
#[test]
#[cfg(feature = "lang-typescript")]
fn a_javascript_template_path_is_read_verbatim_under_the_shared_path_rule() {
    let route = |path: &str| ("GET".to_string(), path.to_string());
    for (written, expected) in [
        ("`${relativePath}/ping`", Some(route("${relativePath}/ping"))),
        ("`/api/${name}/config`", Some(route("/api/${name}/config"))),
        ("`${a}/x/${b}/y`", Some(route("${a}/x/${b}/y"))),
        ("`/users`", Some(route("/users"))),
        ("`/api${name}`", Some(route("/api${name}"))),
        // The separators may live inside the reference (`is_a_url_path`).
        ("`${base}`", Some(route("${base}"))),
        // Neither a `/` nor an opening reference: a property key, not a path.
        ("`users`", None),
        ("`user${id}`", None),
    ] {
        for source in [
            format!("app.get({written}, handler);"),
            format!("router.get({written}, (req, res) => res.send(1));"),
        ] {
            let got: Vec<(String, String)> = scan_lang("js", &source)
                .routes
                .into_iter()
                .map(|r| (r.method, r.path))
                .collect();
            assert_eq!(got, expected.clone().into_iter().collect::<Vec<_>>(), "{source}");
        }
    }
}

/// The template patterns keep the shared Express patterns' rules — a
/// router-named receiver, never a capitalised method — and apply the receiver
/// rule to the handler-bearing form too: a template is how a client call spells
/// an interpolated URL, and its callback is no route handler. And they are
/// JavaScript's own layer, so a TypeScript file reads a template path as before.
#[test]
#[cfg(feature = "lang-typescript")]
fn a_javascript_template_path_keeps_the_express_receiver_and_method_rules() {
    for source in [
        "cache.get(`${base}/key`);",
        "app.Get(`${base}/users`, handler);",
        "axios.get(`/api/items/${item.id}`, config);",
        "axios.post(`${API_URL}/items`, item);",
        "https.get(`${base}/health`, onResponse);",
    ] {
        assert!(scan_lang("js", source).routes.is_empty(), "{source}");
    }
    for ext in TS_EXTENSIONS {
        assert!(
            scan_lang(ext, "app.get(`${base}/ping`, handler);").routes.is_empty(),
            "{ext}: the template layer is JavaScript's alone"
        );
    }
}

/// …and the handler-less form still promotes a route with **no** fabricated
/// handler edge ([NFR-RA-05]) — the reason the unscoped pattern exists at all.
/// Asserted across the receiver spellings the name rule admits, so a narrowing
/// of the regex cannot quietly drop a conventional Express app.
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[test]
#[cfg(feature = "lang-typescript")]
fn a_handler_less_registration_on_a_router_receiver_promotes_a_route_with_no_handler() {
    for receiver in [
        "app",
        "router",
        "server",
        "apiRouter",
        "adminApp",
        // The `…Server` suffix branch of the regex, which the other two
        // suffix spellings do not cover.
        "apiServer",
        "this.app",
    ] {
        for ext in TS_EXTENSIONS {
            let source = format!("{receiver}.get(\"/users\", (req, res) => res.send(1));");
            assert_eq!(
                route_triples(scan_lang(ext, &source)),
                vec![("/users".to_string(), "GET".to_string(), None)],
                "{ext} {receiver}"
            );
        }
    }
}

/// The mounting verbs are subject to the same rule as the method verbs.
///
/// `use` and `all` map to `ANY` in the TypeScript descriptor's
/// `[framework_methods]` table, and `router.use("/api", …)` is the single most
/// common Express composition — so the receiver rule has to admit it on a
/// router and refuse it on anything else, exactly as it does for `get`.
/// Untested, a regex that happened to admit only the method verbs would leave
/// the noisiest registration shape in Express unguarded.
#[test]
#[cfg(feature = "lang-typescript")]
fn the_any_mapped_mounting_verbs_obey_the_same_receiver_rule() {
    for verb in ["use", "all"] {
        for ext in TS_EXTENSIONS {
            assert_eq!(
                route_triples(scan_lang(ext, &format!("router.{verb}(\"/api\");"))),
                vec![("/api".to_string(), "ANY".to_string(), None)],
                "{ext} router.{verb}"
            );
            let refused = scan_lang(ext, &format!("registry.{verb}(\"/api\");"));
            assert!(refused.routes.is_empty(), "{ext} registry.{verb}: {:?}", refused.routes);
        }
    }
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
        // that does not fold leaves no literal, so nothing is promoted — never
        // a guessed path (NFR-RA-05). None of these names a constant the file
        // declares; that each is *counted* is `java_constant_fold`'s to pin.
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
        // non-literal promotes what it can establish. The refused element is
        // counted since S-469 (`java_constant_fold`).
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
    /// method path alone and is never refused.
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
    /// named form — yields **no** path at all, and the registration is refused
    /// and counted in the run's `routes_not_composed` statistic ([FR-FW-05],
    /// [NFR-RA-05]). Promoting `/users` here would advertise a provider at an
    /// address the service does not serve.
    ///
    /// [FR-FW-05]: ../../../docs/specs/requirements/FR-FW-05.md
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

    /// The Java half of a rule S-330 widened in the **shared** interpreter, so
    /// the changed language carries its own pin rather than inheriting one.
    ///
    /// `is_resolvable_prefix` refused `${…}` before; it now also refuses a
    /// brace-less `$name`, because a Kotlin string template writes the same
    /// unresolved indirection that way and this grammar exposes no node for it.
    /// The consequence for Java is a **narrower** recall: a prefix literal that
    /// happens to contain `$name` — an OData `$metadata` route base is the
    /// realistic case — is refused where it previously composed. That is a false
    /// refusal, not a false route, so it errs the way [NFR-RA-05] and
    /// [FR-FW-05] ask ("never a guessed path"); the alternative, gating the rule
    /// on a per-language descriptor flag, is recorded in the review as a design
    /// question rather than assumed here.
    ///
    /// A `$` that introduces nothing still composes, in Java as in Kotlin.
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    /// [FR-FW-05]: ../../../docs/specs/requirements/FR-FW-05.md
    #[test]
    fn a_dollar_reference_in_a_java_prefix_is_refused_and_a_lone_dollar_is_not() {
        for refused in [r#""/odata/$metadata""#, r#""$BASE/v1""#, r#""${api.base}""#] {
            let m = scan_lang("java", &in_prefixed_class(refused, USERS_HANDLER));
            assert!(m.routes.is_empty(), "{refused}: {:?}", m.routes);
            assert_eq!(
                m.refusals,
                [RouteRefusal::PathNotComposed],
                "{refused} must report path-not-composed"
            );
        }

        let m = scan_lang("java", &in_prefixed_class(r#""/price$""#, USERS_HANDLER));
        assert_eq!(
            sorted_triples(&m),
            vec![(
                "/price$/users".to_string(),
                "GET".to_string(),
                Some("listUsers".to_string())
            )]
        );
        assert!(m.refusals.is_empty(), "{:?}", m.refusals);
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

// ── Java Spring paths folded from `String` constants (S-469) ─────────────────

/// A mapping path written as a `+` chain of literals and same-type `String`
/// constants folds to one literal and composes exactly as a written one; any
/// other non-literal path is refused and **counted** rather than dropped
/// ([FR-FW-05], [NFR-RA-05], [CR-151]).
///
/// [FR-FW-05]: ../../../docs/specs/requirements/FR-FW-05.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// [CR-151]: ../../../docs/requests/CR-151-provider-routes-composed-from-string-constants.md
#[cfg(feature = "lang-java")]
mod java_constant_fold {
    use super::*;

    /// The path every fold fixture below is built to reach.
    const FOLDED: &str = "/v1/users/{userId}/x";

    /// One promoted route as `(path, method, handler)`.
    type Triple = (String, String, Option<String>);

    /// `(path, method, handler)` of each promoted route, plus the refusals.
    fn fold_scan(source: &str) -> (Vec<Triple>, Vec<RouteRefusal>) {
        let m = scan_lang("java", source);
        (sorted_triples(&m), m.refusals)
    }

    fn get(path: &str, handler: &str) -> Triple {
        (path.to_string(), "GET".to_string(), Some(handler.to_string()))
    }

    /// One fixture per path form, each over a class `static final` constant,
    /// under the type's literal `@RequestMapping` prefix.
    #[test]
    fn every_path_form_folds_a_same_class_constant_and_composes_with_the_prefix() {
        for arguments in [
            r#""/users/{" + USER_ID + "}/x""#,
            r#"value = "/users/{" + USER_ID + "}/x""#,
            r#"path = "/users/{" + USER_ID + "}/x""#,
            r#"value = {"/users/{" + USER_ID + "}/x"}"#,
        ] {
            let (routes, refusals) = fold_scan(&format!(
                "@RequestMapping(\"/v1\")\n@RestController\npublic class C {{\n    \
                 private static final String USER_ID = \"userId\";\n\n    \
                 @GetMapping({arguments})\n    public String get() {{ return \"\"; }}\n}}\n"
            ));
            assert_eq!(routes, vec![get(FOLDED, "get")], "{arguments}");
            assert!(refusals.is_empty(), "{arguments}: {refusals:?}");
        }
    }

    /// The estate's same-type shape (`mailbox-api`'s `MailboxOperationApiV1`):
    /// an interface whose `String` field is implicitly `static final`, used by
    /// a mapping declared on the interface itself — in every path form.
    #[test]
    fn every_path_form_folds_an_interface_constant() {
        for arguments in [
            r#""/users/{" + USER_ID + "}/x""#,
            r#"value = "/users/{" + USER_ID + "}/x""#,
            r#"path = "/users/{" + USER_ID + "}/x""#,
            r#"value = {"/users/{" + USER_ID + "}/x"}"#,
        ] {
            let (routes, refusals) = fold_scan(&format!(
                "@RequestMapping(\"/v1\")\npublic interface Api {{\n    \
                 String USER_ID = \"userId\";\n\n    \
                 @GetMapping({arguments})\n    String get();\n}}\n"
            ));
            assert_eq!(routes, vec![get(FOLDED, "get")], "{arguments}");
            assert!(refusals.is_empty(), "{arguments}: {refusals:?}");
        }
    }

    /// `final static` is as legal as `static final`, and a constant may be
    /// built from another constant of the same type.
    #[test]
    fn modifier_order_and_a_constant_built_from_constants_both_fold() {
        let (routes, refusals) = fold_scan(
            r#"public class C {
    static final String USERS = "/v1/users";
    final static String USER = USERS + "/{" + "userId" + "}";

    @GetMapping(USER + "/x")
    public String get() { return ""; }
}
"#,
        );
        assert_eq!(routes, vec![get(FOLDED, "get")]);
        assert!(refusals.is_empty(), "{refusals:?}");
    }

    /// Every refusal shape the acceptance criteria name promotes nothing and
    /// counts exactly one — never a partial path.
    #[test]
    fn a_method_path_that_does_not_fold_is_refused_and_counted_once() {
        for (label, members) in [
            ("a method call", r#"    @GetMapping(value = Paths.users())"#),
            (
                "a non-final field",
                "    static String USERS = \"/users\";\n    @GetMapping(value = USERS)",
            ),
            (
                "a non-static final field",
                "    final String USERS = \"/users\";\n    @GetMapping(value = USERS)",
            ),
            (
                "a non-String constant",
                "    static final int USERS = 1;\n    @GetMapping(value = \"/users/\" + USERS)",
            ),
            ("an out-of-type constant", "    @GetMapping(value = Paths.USERS)"),
            ("an undeclared name", r#"    @GetMapping(value = "/users/{" + ID + "}")"#),
            (
                "a constant whose initializer does not fold",
                "    static final String USERS = Paths.BASE + \"/users\";\n    @GetMapping(USERS)",
            ),
            (
                "a cyclic pair of constants",
                "    static final String A = B;\n    static final String B = A;\n    @GetMapping(A)",
            ),
            // A valid tree four constants wide and eight deep: unbounded, it
            // folds 65,536 copies of "/a" into a 131,072-character path, so
            // only the operand budget refuses it (a wider one would hang the
            // indexer).
            (
                "a constant tree past the operand budget",
                "    static final String A0 = \"/a\";\n    static final String A1 = A0 + A0 + A0 + A0;\n    \
                 static final String A2 = A1 + A1 + A1 + A1;\n    static final String A3 = A2 + A2 + A2 + A2;\n    \
                 static final String A4 = A3 + A3 + A3 + A3;\n    static final String A5 = A4 + A4 + A4 + A4;\n    \
                 static final String A6 = A5 + A5 + A5 + A5;\n    static final String A7 = A6 + A6 + A6 + A6;\n    \
                 static final String A8 = A7 + A7 + A7 + A7;\n    @GetMapping(A8)",
            ),
            // Another type in the same file is still another type — reaching it
            // is S-470's shape, not this story's.
            (
                "another type of the same file",
                "    @GetMapping(\"/{\" + Paths.ID + \"}\")",
            ),
            // A field named like the type obscures it (JLS §6.4.2): `C.ID`
            // would read `ID` off the String `C`, so it is not `C`'s constant.
            (
                "a qualifier obscured by a same-named field",
                "    static final String C = \"/c\";\n    static final String ID = \"id\";\n    @GetMapping(\"/{\" + C.ID + \"}\")",
            ),
        ] {
            let (routes, refusals) = fold_scan(&format!(
                "class Paths {{\n    static final String ID = \"id\";\n}}\n\n\
                 public class C {{\n{members}\n    public String get() {{ return \"\"; }}\n}}\n"
            ));
            assert!(routes.is_empty(), "{label}: {routes:?}");
            assert_eq!(refusals, [RouteRefusal::PathNotFolded], "{label}");
        }
    }

    /// A written literal belongs to the literal patterns even where the fold
    /// could not read it: an escape sequence is promoted verbatim, as before
    /// S-469, and is **not** also counted as a refused fold. The opaque
    /// patterns match it too (a literal is an `expression`), which is what the
    /// identity rule in `fold_constants` exists for.
    #[test]
    fn a_written_literal_the_fold_cannot_read_is_promoted_and_not_counted() {
        let (routes, refusals) = fold_scan(
            "public class C {\n    @GetMapping(value = {\"/a\\tb\", \"/c\"})\n    \
             public String get() { return \"\"; }\n}\n",
        );
        assert_eq!(routes, vec![get("/a\\tb", "get"), get("/c", "get")]);
        assert!(refusals.is_empty(), "{refusals:?}");
    }

    /// A mixed list promotes its literal and folded elements and counts each
    /// refused one — a refused path element is its own unstated address.
    #[test]
    fn a_mixed_list_promotes_what_folds_and_counts_each_refused_element() {
        let (routes, refusals) = fold_scan(
            r#"public class C {
    static final String B = "/b";

    @GetMapping(value = {"/a", B, Paths.C, OTHER + "/d"})
    public String get() { return ""; }
}
"#,
        );
        assert_eq!(routes, vec![get("/a", "get"), get("/b", "get")]);
        assert_eq!(
            refusals,
            [RouteRefusal::PathNotFolded, RouteRefusal::PathNotFolded]
        );
    }

    /// Java scoping decides what a name binds to, and the fold refuses wherever
    /// that answer is not provable from the file.
    #[test]
    fn a_name_the_file_cannot_prove_is_refused() {
        for (label, source) in [
            // A supertype may declare `ID`, and an inherited field would hide
            // the enclosing type's — so the enclosing one is not the answer.
            (
                "inherited from a superclass",
                "public class Outer {\n    static final String ID = \"id\";\n    \
                 public static class C extends Base {\n        \
                 @GetMapping(\"/{\" + ID + \"}\")\n        public String get() { return \"\"; }\n    }\n}\n",
            ),
            (
                "inherited from an interface",
                "public class Outer {\n    static final String ID = \"id\";\n    \
                 public static class C implements Api {\n        \
                 @GetMapping(\"/{\" + ID + \"}\")\n        public String get() { return \"\"; }\n    }\n}\n",
            ),
            // The other type kinds that may inherit a name, one row each —
            // every `@fw.const.scope.opaque` pattern is pinned by its own row.
            (
                "an interface extending another",
                "public class Outer {\n    static final String ID = \"id\";\n    \
                 public interface Api extends Base {\n        \
                 @GetMapping(\"/{\" + ID + \"}\")\n        String get();\n    }\n}\n",
            ),
            (
                "a record implementing an interface",
                "public class Outer {\n    static final String ID = \"id\";\n    \
                 public record R() implements Api {\n        \
                 @GetMapping(\"/{\" + ID + \"}\")\n        public String get() { return \"\"; }\n    }\n}\n",
            ),
            (
                "an enum",
                "public class Outer {\n    static final String ID = \"id\";\n    \
                 public enum E {\n        A;\n        \
                 @GetMapping(\"/{\" + ID + \"}\")\n        public String get() { return \"\"; }\n    }\n}\n",
            ),
            (
                "an anonymous class",
                "public class Outer {\n    static final String ID = \"id\";\n    \
                 Object o = new Base() {\n        \
                 @GetMapping(\"/{\" + ID + \"}\")\n        public String get() { return \"\"; }\n    };\n}\n",
            ),
            // A non-constant field of the inner type shadows the outer
            // constant; skipping past it would fold the wrong declaration.
            (
                "shadowed by a non-constant field",
                "public class Outer {\n    static final String ID = \"id\";\n    \
                 public static class C {\n        String ID;\n        \
                 @GetMapping(\"/{\" + ID + \"}\")\n        public String get() { return \"\"; }\n    }\n}\n",
            ),
            // A type's own annotation is resolved outside its body (JLS §6.3),
            // so a top-level class cannot name its own constant unqualified.
            (
                "a type annotation naming its own member unqualified",
                "@RequestMapping(BASE)\npublic class C {\n    static final String BASE = \"/v1\";\n    \
                 @GetMapping(\"/users\")\n    public String get() { return \"\"; }\n}\n",
            ),
        ] {
            let (routes, refusals) = fold_scan(source);
            assert!(routes.is_empty(), "{label}: {routes:?}");
            assert_eq!(refusals.len(), 1, "{label}: {refusals:?}");
        }
    }

    /// A constant declared inside a nested `@interface` is that annotation
    /// type's member, not the enclosing class's (review of S-469: it was filed
    /// under the class and folded). Java binds `ID` here to the enclosing
    /// class's constant, or — beside an inherited `BASE` — to the supertype's.
    #[test]
    fn a_nested_annotation_types_constant_is_not_the_enclosing_classs() {
        let (routes, refusals) = fold_scan(
            "public class Outer {\n    static final String ID = \"outer\";\n    \
             public static class C {\n        @interface Marker { String ID = \"m\"; }\n        \
             @GetMapping(\"/{\" + ID + \"}\")\n        public String get() { return \"\"; }\n    }\n}\n",
        );
        assert_eq!(routes, vec![get("/{outer}", "get")]);
        assert!(refusals.is_empty(), "{refusals:?}");

        // Beside a supertype, the annotation type's constant must not stand
        // in for the inherited one: refused, never `/fabricated`.
        let (routes, refusals) = fold_scan(
            "public class C extends Base {\n    @interface Marker { String BASE = \"/fabricated\"; }\n    \
             @GetMapping(BASE)\n    public String get() { return \"\"; }\n    \
             @GetMapping(C.BASE)\n    public String get2() { return \"\"; }\n}\n",
        );
        assert!(routes.is_empty(), "{routes:?}");
        assert_eq!(refusals, [RouteRefusal::PathNotFolded, RouteRefusal::PathNotFolded]);
    }

    /// A path the parser only recovered from an error is not read: a Unicode
    /// escape in an identifier (`BASEASE` is `BASEASE` to javac) parses as
    /// the identifier `BASE` beside an `ERROR` node, and folding that identifier
    /// would promote a different constant's value (review of S-469). The
    /// registration is refused and counted instead, for a method path and for a
    /// class prefix alike.
    #[test]
    fn a_path_recovered_from_a_parse_error_is_refused_not_folded() {
        let constants = "    static final String BASE = \"/fab\";\n    static final String BASEASE = \"/real\";\n";
        let (routes, refusals) = fold_scan(&format!(
            "public class C {{\n{constants}    @GetMapping(BASE\\u0041SE)\n    \
             public String get() {{ return \"\"; }}\n}}\n"
        ));
        assert!(routes.is_empty(), "{routes:?}");
        assert_eq!(refusals, [RouteRefusal::PathNotFolded]);

        let (routes, refusals) = fold_scan(&format!(
            "public class Outer {{\n{constants}    @RequestMapping(BASE\\u0041SE)\n    public static class C {{\n        \
             @GetMapping(\"/users\")\n        public String get() {{ return \"\"; }}\n    }}\n}}\n"
        ));
        assert!(routes.iter().all(|r| !r.0.starts_with("/fab")), "{routes:?}");
        assert!(routes.is_empty(), "{routes:?}");
        assert_eq!(refusals, [RouteRefusal::PathNotComposed]);

        // A constant whose initializer holds the escape has no value the fold
        // may read: `A` is `BASEASE`'s value to javac, never `BASE`'s.
        let (routes, refusals) = fold_scan(&format!(
            "public class C {{\n{constants}    static final String A = BASE\\u0041SE;\n    \
             @GetMapping(A)\n    public String get() {{ return \"\"; }}\n}}\n"
        ));
        assert!(routes.is_empty(), "{routes:?}");
        assert_eq!(refusals, [RouteRefusal::PathNotFolded]);
    }

    /// The positive twins of the scoping refusals: a type declaring the
    /// constant itself wins over anything it inherits, and an enclosing type's
    /// constant is visible to a nested type that inherits nothing.
    #[test]
    fn an_own_or_enclosing_constant_folds() {
        for (label, source, path) in [
            (
                "own constant beside a supertype",
                "public class C extends Base {\n    static final String ID = \"id\";\n    \
                 @GetMapping(\"/{\" + ID + \"}\")\n    public String get() { return \"\"; }\n}\n",
                "/{id}",
            ),
            (
                "enclosing type's constant",
                "public class Outer {\n    static final String ID = \"id\";\n    \
                 public static class C {\n        @GetMapping(\"/{\" + ID + \"}\")\n        \
                 public String get() { return \"\"; }\n    }\n}\n",
                "/{id}",
            ),
            (
                "qualified by its own type",
                "public class C {\n    static final String ID = \"id\";\n    \
                 @GetMapping(\"/{\" + C.ID + \"}\")\n    public String get() { return \"\"; }\n}\n",
                "/{id}",
            ),
        ] {
            let (routes, refusals) = fold_scan(source);
            assert_eq!(routes, vec![get(path, "get")], "{label}");
            assert!(refusals.is_empty(), "{label}: {refusals:?}");
        }
    }

    /// A class-level prefix built from same-type constants composes — through
    /// the qualified form a type's own annotation needs, or from an enclosing
    /// type — while an unfoldable prefix is still refused as `PathNotComposed`.
    #[test]
    fn a_prefix_built_from_same_type_constants_composes() {
        for (label, source) in [
            (
                "qualified own constant",
                "@RequestMapping(C.BASE + \"/users\")\npublic class C {\n    \
                 static final String BASE = \"/v1\";\n    \
                 @GetMapping(\"/{\" + \"userId\" + \"}/x\")\n    public String get() { return \"\"; }\n}\n",
            ),
            (
                "enclosing type's constant, named form",
                "public class Outer {\n    static final String BASE = \"/v1/users\";\n    \
                 @RequestMapping(value = BASE)\n    public static class C {\n        \
                 @GetMapping(\"/{userId}/x\")\n        public String get() { return \"\"; }\n    }\n}\n",
            ),
        ] {
            let (routes, refusals) = fold_scan(source);
            assert_eq!(routes, vec![get(FOLDED, "get")], "{label}");
            assert!(refusals.is_empty(), "{label}: {refusals:?}");
        }

        let (routes, refusals) = fold_scan(
            "@RequestMapping(ApiPaths.USERS + \"/v1\")\npublic class C {\n    \
             @GetMapping(\"/users\")\n    public String get() { return \"\"; }\n}\n",
        );
        assert!(routes.is_empty(), "{routes:?}");
        assert_eq!(refusals, [RouteRefusal::PathNotComposed]);
    }

    /// A folded prefix is judged exactly like a written one: folding a
    /// placeholder does not make it a joinable address.
    #[test]
    fn a_folded_prefix_is_still_judged_as_an_address() {
        let (routes, refusals) = fold_scan(
            "@RequestMapping(C.BASE)\npublic class C {\n    \
             static final String BASE = \"${api.base}\";\n    \
             @GetMapping(\"/users\")\n    public String get() { return \"\"; }\n}\n",
        );
        assert!(routes.is_empty(), "{routes:?}");
        assert_eq!(refusals, [RouteRefusal::PathNotComposed]);
    }

    /// The depth bound is sixteen constants on one chain: `A15` (sixteen,
    /// counting `A0`) folds and `A16` is refused. The refusal is about the
    /// asking chain, not the constants on it — a path that meets `A16` first
    /// does not stop a later path folding `A15` through the very same
    /// constants.
    #[test]
    fn a_chain_through_more_than_sixteen_constants_is_refused() {
        let mut members = String::from("    static final String A0 = \"/a\";\n");
        for k in 1..=16 {
            members.push_str(&format!("    static final String A{k} = A{};\n", k - 1));
        }
        let (routes, refusals) = fold_scan(&format!(
            "public class C {{\n{members}    @GetMapping(A16)\n    public String deep() {{ return \"\"; }}\n    \
             @GetMapping(A15)\n    public String bounded() {{ return \"\"; }}\n}}\n"
        ));
        assert_eq!(routes, vec![get("/a", "bounded")]);
        assert_eq!(refusals, [RouteRefusal::PathNotFolded]);
    }

    /// A folded path is trimmed exactly as a written one is, prefix or not: the
    /// route is named `GET /users/{id}`, never with the blanks the literals
    /// carried, which no consumer would match.
    #[test]
    fn a_folded_path_is_trimmed_like_a_written_one() {
        let (routes, refusals) = fold_scan(
            "public class C {\n    static final String ID = \"id\";\n    \
             @GetMapping(\" /users/{\" + ID + \"} \")\n    public String get() { return \"\"; }\n}\n",
        );
        assert_eq!(routes, vec![get("/users/{id}", "get")]);
        assert!(refusals.is_empty(), "{refusals:?}");
    }

    /// Provenance: a folded route names every constant it used and the file
    /// declaring it, the prefix's included; a written route names none.
    #[test]
    fn a_folded_route_records_its_constants_and_a_literal_route_none() {
        let m = scan_lang(
            "java",
            "@RequestMapping(C.BASE)\npublic class C {\n    \
             static final String BASE = \"/v1\";\n    \
             static final String USER_ID = \"userId\";\n    \
             @GetMapping(\"/users/{\" + USER_ID + \"}/x\")\n    public String folded() { return \"\"; }\n    \
             @GetMapping(\"/users\")\n    public String written() { return \"\"; }\n}\n",
        );
        let constant = |name: &str| FoldedConstant {
            name: name.to_string(),
            file: "src/Fixture.java".to_string(),
        };
        let by_path: std::collections::BTreeMap<&str, &Vec<FoldedConstant>> = m
            .routes
            .iter()
            .map(|r| (r.path.as_str(), &r.folded_from))
            .collect();
        assert_eq!(
            by_path.get(FOLDED).map(|v| v.as_slice()),
            Some([constant("USER_ID"), constant("BASE")].as_slice()),
            "{by_path:?}"
        );
        // Composed with the folded prefix, so it carries BASE — and nothing of
        // its own.
        assert_eq!(
            by_path.get("/v1/users").map(|v| v.as_slice()),
            Some([constant("BASE")].as_slice()),
            "{by_path:?}"
        );

        let literal = scan_lang(
            "java",
            "@RequestMapping(\"/v1\")\npublic class C {\n    \
             @GetMapping(\"/users\")\n    public String written() { return \"\"; }\n}\n",
        );
        assert_eq!(literal.routes.len(), 1, "{:?}", literal.routes);
        assert!(literal.routes[0].folded_from.is_empty(), "{:?}", literal.routes);
    }

    /// The store-free projection carries the provenance too — it is what the
    /// reference-estate measurement ([S-470]) reads.
    ///
    /// [S-470]: ../../../docs/planning/journal.md#s-470-a-static-imported-same-member-constant-folds-measured-on-the-reference-estate
    #[test]
    fn the_projection_carries_the_provenance() {
        let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
        let plugin = registry.for_extension("java").expect("java plugin");
        let routes = routes_in_source(
            plugin,
            "src/Api.java",
            "public interface Api {\n    String ID = \"id\";\n    \
             @GetMapping(\"/v1/{\" + ID + \"}\")\n    String get();\n}\n",
        );
        assert_eq!(routes.len(), 1, "{routes:?}");
        assert_eq!(routes[0].path, "/v1/{id}");
        assert_eq!(
            routes[0].folded_from,
            [FoldedConstant { name: "ID".to_string(), file: "src/Api.java".to_string() }]
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
            // Not just "no route": no *pathless* candidate either, and no
            // refusal. A pathless candidate is the dangerous residue — under a
            // prefixed type it is promoted AT the prefix, a wrong address that
            // an unprefixed fixture cannot see.
            assert!(m.pathless.is_empty(), "{arguments}: {:?}", m.pathless);
            assert!(m.refusals.is_empty(), "{arguments}: {:?}", m.refusals);

            let prefixed = scan_lang(
                "kt",
                &in_prefixed_class(
                    r#""/v1""#,
                    &format!(
                        "    @GetMapping({arguments})\n    fun get(): String {{ return \"\" }}"
                    ),
                ),
            );
            assert!(prefixed.routes.is_empty(), "{arguments}: {:?}", prefixed.routes);
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
        assert!(m.refusals.is_empty(), "{:?}", m.refusals);
    }

    /// A backtick-escaped argument name is legal Kotlin, and the key node's text
    /// carries the backticks — so a raw `#any-of? "value" "path"` misses it and
    /// the prefix reads as **absent** rather than opaque. That fails *open*: the
    /// handler is promoted at its bare method path, a wrong address rather than
    /// a refused one ([NFR-RA-05]). Found in review; the key predicates now
    /// accept both spellings, on the type and on the method.
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    #[test]
    fn a_backtick_escaped_argument_key_is_still_a_path_argument() {
        let prefixed = scan_lang(
            "kt",
            &in_prefixed_class(r#"`value` = "/v1""#, USERS_HANDLER),
        );
        assert_eq!(sorted_triples(&prefixed), users_route("/v1/users"));
        assert!(prefixed.refusals.is_empty(), "{:?}", prefixed.refusals);

        let method = kotlin_routes(&in_class(
            r#"    @GetMapping(`path` = "/users")
    fun listUsers(): String { return "" }"#,
        ));
        assert_eq!(method, users_route("/users"));

        // …and the escaping does not smuggle a non-path argument through.
        let produces = scan_lang(
            "kt",
            &in_class(
                r#"    @GetMapping(`produces` = "application/json")
    fun getX(): String { return "" }"#,
            ),
        );
        assert!(produces.routes.is_empty(), "{:?}", produces.routes);
    }

    /// Kotlin's bracket form puts several **independent** registrations under
    /// one `annotation` node. Anchoring precedence there made the named path
    /// outrank the positional one *across annotations*, silently dropping a
    /// route the service really serves; the anchor is the individual
    /// `constructor_invocation` for exactly this reason.
    ///
    /// Mutation-checked: moving `@fw.route.anchor` back onto the `annotation`
    /// node fails this test with one route instead of two.
    #[test]
    fn a_bracketed_multi_annotation_registers_each_of_its_mappings() {
        let got = kotlin_routes(&in_class(
            r#"    @[GetMapping(value = "/a") PostMapping("/b")]
    fun h(): String { return "" }"#,
        ));
        assert_eq!(
            got,
            vec![
                ("/a".to_string(), "GET".to_string(), Some("h".to_string())),
                ("/b".to_string(), "POST".to_string(), Some("h".to_string())),
            ]
        );

        // Written separately the two annotations already worked — pinned
        // alongside so the bracket form is asserted to *match* it rather than
        // merely to promote something.
        let separate = kotlin_routes(&in_class(
            r#"    @GetMapping(value = "/a")
    @PostMapping("/b")
    fun h(): String { return "" }"#,
        ));
        assert_eq!(got, separate, "both spellings are the same two registrations");
    }

    /// One refused registration counts once, however many paths it wrote — the
    /// grain [`FrameworkStats::routes_not_composed`] documents. Kotlin reaches
    /// multi-candidate registrations through a different shape than Java (list
    /// elements of a `collection_literal`), so the grain needs its own fixture
    /// here rather than inheriting Java's.
    ///
    /// [`FrameworkStats::routes_not_composed`]: crate::models::pipeline::FrameworkStats::routes_not_composed
    #[test]
    fn a_refused_list_valued_registration_counts_once() {
        let m = scan_lang(
            "kt",
            &in_prefixed_class(
                "BASE",
                r#"    @GetMapping(value = ["/a", "/b"])
    fun list(): String { return "" }"#,
            ),
        );
        assert!(m.routes.is_empty(), "{:?}", m.routes);
        assert_eq!(
            m.refusals,
            [RouteRefusal::PathNotComposed],
            "one annotation, one refusal"
        );
    }

    /// **An upstream grammar defect, pinned rather than fixed.**
    ///
    /// `tree-sitter-kotlin-ng` 1.1 does not parse a type declaration carrying
    /// two or more annotations when another top-level declaration follows it —
    /// the ordinary `@RestController @RequestMapping(…) class C { … }` plus a
    /// DTO or a service interface. The declaration is not a `class_declaration`
    /// at all (its modifiers are reparsed as a file-level expression) and
    /// `has_error()` stays **false**, so neither a declaration pattern nor a
    /// parse-error gate can see it. Review's suggested `has_error()` gate was
    /// measured and does not close it; the file's partial-extraction warning
    /// names the expression instead (S-559, `kotlin_declarations.rs`).
    ///
    /// This is **not** introduced by composition: the pre-S-330 query loses the
    /// same route and the same stereotype component on the same input. What
    /// composition adds is that where the handler survives the misparse it is
    /// promoted at its bare method path rather than the composed one — an
    /// under-composed address rather than an absent one.
    ///
    /// The test pins what the grammar actually does, so a grammar upgrade (the
    /// real fix, escalated as its own change) fails here and gets noticed. The
    /// single-annotation form is asserted alongside to show the trigger really
    /// is the second annotation and not the trailing declaration.
    #[test]
    fn an_upstream_misparse_of_a_multiply_annotated_type_loses_its_prefix() {
        let trailing = "\ninterface S {\n    fun all(): String\n}\n";
        let handler = "    @GetMapping(\"/x\")\n    fun x(): String { return \"\" }";

        // One annotation: parses, composes, promotes the component-free route.
        let one = scan_lang(
            "kt",
            &format!("@RequestMapping(\"/v1\")\nclass C {{\n{handler}\n}}\n{trailing}"),
        );
        assert_eq!(
            sorted_triples(&one),
            vec![("/v1/x".to_string(), "GET".to_string(), Some("x".to_string()))],
            "a singly-annotated type is unaffected"
        );

        // Two annotations and no trailing declaration: also fine, and the
        // stereotype is promoted.
        let two = scan_lang(
            "kt",
            &format!(
                "@RestController\n@RequestMapping(\"/v1\")\nclass C {{\n{handler}\n}}\n"
            ),
        );
        assert_eq!(
            sorted_triples(&two),
            vec![("/v1/x".to_string(), "GET".to_string(), Some("x".to_string()))]
        );
        assert_eq!(two.components.len(), 1);

        // Two annotations AND a trailing declaration: the whole controller is
        // swallowed — no route, no component, and no syntax error reported.
        let misparsed = scan_lang(
            "kt",
            &format!(
                "@RestController\n@RequestMapping(\"/v1\")\nclass C {{\n{handler}\n}}\n{trailing}"
            ),
        );
        assert!(
            misparsed.routes.is_empty(),
            "grammar behaviour changed — re-check the escalated defect: {:?}",
            misparsed.routes
        );
        assert!(misparsed.components.is_empty(), "{:?}", misparsed.components);
        assert!(misparsed.refusals.is_empty(), "{:?}", misparsed.refusals);
    }

    /// The same defect, in the shape review found first: an **annotated**
    /// `enum class` is swallowed whole, so the named-argument handler inside it
    /// is promoted at its bare method path rather than the composed one. The
    /// impl notes originally claimed this failed safe with zero routes; it does
    /// not, and the honest behaviour is pinned here.
    ///
    /// A second modifier is enough to make the same declaration parse, which is
    /// what identifies this as the grammar's annotation handling rather than
    /// anything about enums.
    #[test]
    fn an_annotated_enum_class_is_swallowed_while_a_modified_one_composes() {
        let body = "    A;\n    @GetMapping(value = \"/users\")\n    fun listUsers(): String { return \"\" }";

        let swallowed = scan_lang(
            "kt",
            &format!("@RequestMapping(\"/e\")\nenum class E {{\n{body}\n}}\n"),
        );
        assert_eq!(
            sorted_triples(&swallowed),
            users_route("/users"),
            "grammar behaviour changed — re-check the escalated defect"
        );
        assert!(
            swallowed.prefixes.is_empty(),
            "the prefix is not seen at all: {:?}",
            swallowed.prefixes
        );

        let parsed = scan_lang(
            "kt",
            &format!("@RequestMapping(\"/e\")\ninternal enum class E {{\n{body}\n}}\n"),
        );
        assert_eq!(
            sorted_triples(&parsed),
            users_route("/e/users"),
            "with a second modifier the same declaration parses and composes"
        );
    }

    /// Two shapes this query does not capture, both parity-consistent with Java
    /// and both failing **safe** — a missing route, never a wrong one. Asserted
    /// so the exclusions the header claims are the exclusions that exist, and so
    /// adding either capture later is a deliberate change rather than a
    /// surprise.
    #[test]
    fn the_headers_documented_capture_gaps_are_real_and_promote_nothing() {
        // A positional array method path. Java reads `{…}` since S-469; the
        // divergence is pinned by
        // `a_positional_list_method_path_is_read_in_java_and_not_yet_in_kotlin`.
        let positional_list = scan_lang(
            "kt",
            &in_prefixed_class(
                r#""/v1""#,
                r#"    @GetMapping(["/a", "/b"])
    fun get(): String { return "" }"#,
            ),
        );
        assert!(positional_list.routes.is_empty(), "{:?}", positional_list.routes);
        assert!(positional_list.refusals.is_empty(), "{:?}", positional_list.refusals);

        // A mapping annotation whose only arguments are non-path: it is a
        // `constructor_invocation`, so the marker pattern cannot reach it and
        // its route (the prefix alone) is not promoted.
        let produces_only = scan_lang(
            "kt",
            &in_prefixed_class(
                r#""/v1""#,
                r#"    @GetMapping(produces = ["text/plain"])
    fun noPath(): String { return "" }"#,
            ),
        );
        assert!(produces_only.routes.is_empty(), "{:?}", produces_only.routes);
        assert!(produces_only.pathless.is_empty(), "{:?}", produces_only.pathless);
        assert!(produces_only.refusals.is_empty(), "{:?}", produces_only.refusals);
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
    /// method path alone and is never refused.
    ///
    /// [BR-46]: ../../../docs/specs/software-spec.md#310-framework-extraction
    #[test]
    fn an_unprefixed_handler_keeps_its_method_path_and_is_not_refused() {
        let m = scan_lang("kt", &in_class(USERS_HANDLER));
        assert_eq!(sorted_triples(&m), users_route("/users"));
        assert!(m.refusals.is_empty(), "{:?}", m.refusals);
    }

    /// A prefix that cannot be resolved to a literal yields **no** path at all
    /// and the registration is refused, counted in the run's
    /// `routes_not_composed` statistic ([NFR-RA-05]).
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
            // The **bare** shape is the one that exercises the guard: the empty
            // `collection_literal` error-recovers into a zero-width `MISSING`
            // node, which the opaque catch-all would otherwise read as a
            // written-but-unreadable prefix and refuse the whole controller for.
            let bare = scan_lang(
                "kt",
                &format!("@RequestMapping({arguments})\nclass C {{\n{USERS_HANDLER}\n}}\n"),
            );
            assert_eq!(sorted_triples(&bare), users_route("/users"), "{arguments}");
            assert!(bare.refusals.is_empty(), "{arguments}: {:?}", bare.refusals);
            assert!(
                bare.prefixes.iter().all(|scope| scope.literals.is_empty()),
                "{arguments} declares no prefix: {:?}",
                bare.prefixes
            );

            // The **stereotyped** shape carries a second annotation, which puts
            // it in the reach of the upstream misparse
            // `an_upstream_misparse_of_a_multiply_annotated_type_loses_its_prefix`
            // pins: the declaration is swallowed, so its `@RestController`
            // component is lost too. Asserted rather than left unnoticed — the
            // route being right here is not evidence that the file was read.
            let stereotyped = scan_lang("kt", &in_prefixed_class(arguments, USERS_HANDLER));
            assert_eq!(
                sorted_triples(&stereotyped),
                users_route("/users"),
                "{arguments}"
            );
            assert!(stereotyped.refusals.is_empty(), "{arguments}: {:?}", stereotyped.refusals);
            assert!(
                stereotyped.components.is_empty(),
                "{arguments}: the misparse loses the stereotype — if this now \
                 promotes, the grammar was upgraded: {:?}",
                stereotyped.components
            );
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

    /// Everything the scan established, in a form two languages can be compared
    /// on: the sorted route set, how many registrations were refused, the
    /// promoted component names, and how many registrations were left pathless.
    ///
    /// `pathless` is in the tuple because [`compose_prefixes`] drains that list
    /// and a divergence confined to it would otherwise be invisible: one
    /// language capturing a pathless candidate the other does not is a real
    /// difference in what the queries see, even when both end up promoting the
    /// same routes.
    fn promoted(ext: &str, source: &str) -> (Vec<String>, usize, Vec<String>, usize) {
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
        (routes, m.refusals.len(), components, m.pathless.len())
    }

    /// Equality between two languages is only half an assertion: a row where
    /// **both** sides promote nothing compares `([], 0, [], 0)` against itself
    /// and would survive a mutation that made both languages fabricate the same
    /// wrong route. Review measured that 7 of these rows were in exactly that
    /// state.
    ///
    /// So each row declares whether it **establishes** anything — a route, a
    /// refusal or a component — and the two cases are asserted differently: an
    /// establishing row must produce a non-trivial outcome on both sides, and a
    /// silent row must produce the *fully* empty outcome, not merely the same
    /// one. A row that flips from establishing to silent (or back) now fails
    /// here instead of quietly becoming an identity comparison. The floor counts
    /// rows that promote actual routes, which is what "covers the fixture set"
    /// was meant to mean.
    #[test]
    fn kotlin_and_java_fixtures_promote_identical_routes() {
        // Nothing promoted, nothing refused, nothing left pathless.
        let silent = (Vec::new(), 0usize, Vec::new(), 0usize);
        let mut promoting = 0;
        for &(label, kotlin, java, establishes) in PAIRED_FIXTURES {
            let (kt, java_side) = (promoted("kt", kotlin), promoted("java", java));
            assert_eq!(
                kt, java_side,
                "{label}: Kotlin and Java must promote the same routes"
            );
            if establishes {
                assert_ne!(
                    kt, silent,
                    "{label} is declared to establish something but the comparison \
                     is empty on both sides, so it asserts nothing"
                );
            } else {
                assert_eq!(
                    kt, silent,
                    "{label} is declared silent but established something"
                );
            }
            if !kt.0.is_empty() {
                promoting += 1;
            }
        }
        assert!(
            promoting >= 20,
            "the parity table must cover the fixture set, not a sample: \
             {promoting} rows promote a route"
        );
    }

    /// The one place the two JVM dialects **deliberately** part (S-469,
    /// [CR-151] §3.3). A method path written as an expression is captured,
    /// folded or refused and **counted** in Java; Kotlin's query captures no
    /// opaque method path and no `@fw.const.*` fact, so there it still promotes
    /// nothing and counts nothing, as before. Both halves are asserted here —
    /// the routes still agree, the refusal counts do not — so the divergence
    /// is a pinned decision rather than a row quietly dropped from
    /// `PAIRED_FIXTURES`. When a Kotlin CR brings `const val` and templates
    /// into scope, this test is the one to invert.
    ///
    /// [CR-151]: ../../../docs/requests/CR-151-provider-routes-composed-from-string-constants.md
    #[test]
    fn a_non_literal_method_path_is_counted_in_java_and_not_yet_in_kotlin() {
        // (label, kotlin, java, promoted routes on both sides, Java refusals)
        let rows: &[(&str, &str, &str, usize, usize)] = &[
            (
                "non-literal named method paths",
                "class C {\n    @GetMapping(value = BASE)\n    fun a(): String { return \"\" }\n    @GetMapping(value = BASE + \"/x\")\n    fun b(): String { return \"\" }\n    @GetMapping(value = Paths.USERS)\n    fun c(): String { return \"\" }\n}\n",
                "public class C {\n    @GetMapping(value = BASE)\n    public String a() { return \"\"; }\n    @GetMapping(value = BASE + \"/x\")\n    public String b() { return \"\"; }\n    @GetMapping(value = Paths.USERS)\n    public String c() { return \"\"; }\n}\n",
                0,
                3,
            ),
            (
                "mixed list method path",
                "class C {\n    @GetMapping(value = [\"/a\", BASE + \"/b\"])\n    fun get(): String { return \"\" }\n}\n",
                "public class C {\n    @GetMapping(value = {\"/a\", BASE + \"/b\"})\n    public String get() { return \"\"; }\n}\n",
                1,
                1,
            ),
        ];
        for &(label, kotlin, java, routes, java_refusals) in rows {
            let (kt, java_side) = (promoted("kt", kotlin), promoted("java", java));
            assert_eq!(kt.0, java_side.0, "{label}: the promoted routes still agree");
            assert_eq!(kt.0.len(), routes, "{label}: {:?}", kt.0);
            assert_eq!(kt.1, 0, "{label}: Kotlin counts nothing yet");
            assert_eq!(java_side.1, java_refusals, "{label}: Java counts each refused path");
            assert_eq!((kt.2, kt.3), (java_side.2, java_side.3), "{label}");
        }
    }

    /// A positional list method path is read in Java since S-469 (review of
    /// S-469: its non-literal elements were dropped with no trace, which
    /// [FR-FW-05] forbids) and not yet in Kotlin, whose query captures neither
    /// `["/a", "/b"]` nor its elements. The literal list promotes one route per
    /// element in Java and nothing in Kotlin; a mixed list promotes the literal
    /// element and counts the refused one in Java.
    ///
    /// [FR-FW-05]: ../../../docs/specs/requirements/FR-FW-05.md
    #[test]
    fn a_positional_list_method_path_is_read_in_java_and_not_yet_in_kotlin() {
        let literal = promoted(
            "java",
            "public class C {\n    @GetMapping({\"/a\", \"/b\"})\n    public String get() { return \"\"; }\n}\n",
        );
        assert_eq!(
            literal.0,
            ["GET /a -> Some(\"get\")", "GET /b -> Some(\"get\")"],
            "{literal:?}"
        );
        assert_eq!(literal.1, 0, "{literal:?}");
        let kotlin = promoted(
            "kt",
            "class C {\n    @GetMapping([\"/a\", \"/b\"])\n    fun get(): String { return \"\" }\n}\n",
        );
        assert_eq!(kotlin, (Vec::new(), 0, Vec::new(), 0));

        let mixed = promoted(
            "java",
            "public class C {\n    static final String B = \"/b\";\n    \
             @GetMapping({\"/a\", B + \"/x\", OTHER})\n    public String get() { return \"\"; }\n}\n",
        );
        assert_eq!(
            mixed.0,
            ["GET /a -> Some(\"get\")", "GET /b/x -> Some(\"get\")"],
            "{mixed:?}"
        );
        assert_eq!(mixed.1, 1, "OTHER is counted: {mixed:?}");
    }

    /// The fold is opted into by the query, not by the language: a prefix of
    /// literals only (`"/v1" + "/x"`) is foldable text in either syntax, yet
    /// Kotlin — whose query names no `@fw.const.*` capture — still refuses it,
    /// exactly as before S-469, while Java composes it.
    #[test]
    fn only_a_dialect_that_captures_constants_folds_a_prefix() {
        let kt = scan_lang(
            "kt",
            "@RequestMapping(\"/v1\" + \"/x\")\nclass C {\n    @GetMapping(value = \"/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
        );
        assert!(kt.routes.is_empty(), "{:?}", kt.routes);
        assert_eq!(kt.refusals, [RouteRefusal::PathNotComposed]);

        let java = scan_lang(
            "java",
            "@RequestMapping(\"/v1\" + \"/x\")\npublic class C {\n    @GetMapping(value = \"/users\")\n    public String listUsers() { return \"\"; }\n}\n",
        );
        let paths: Vec<&str> = java.routes.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, ["/v1/x/users"]);
        assert!(java.refusals.is_empty(), "{:?}", java.refusals);
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

        // Kotlin's gate is Java's plus Ktor's own package (S-561): the Spring
        // fingerprint is shared, and the one addition is the DSL's.
        assert_eq!(
            java.semantics().framework_detectors,
            ["org::springframework"],
            "the shared fingerprint is Spring's package prefix"
        );
        assert_eq!(
            kotlin.semantics().framework_detectors,
            ["org::springframework", "io::ktor"],
            "Kotlin adds Ktor's package to the shared Spring fingerprint"
        );
        // The annotation→verb mapping is one table for both languages; Kotlin's
        // extra rows are the functional DSLs' call names (S-561), which carry no
        // annotation.
        let annotation_rows = |table: &std::collections::BTreeMap<String, String>| {
            table
                .iter()
                .filter(|(name, _)| name.ends_with("Mapping"))
                .map(|(name, verb)| (name.clone(), verb.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            annotation_rows(&java.semantics().framework_methods),
            annotation_rows(&kotlin.semantics().framework_methods),
            "the annotation→verb table is one mapping for both languages"
        );
        assert_eq!(
            java.semantics().framework_methods.len(),
            6,
            "Java's table is the six annotation rows"
        );

        // Byte-identical, not just equal once parsed: the annotation rows as
        // written. Blank lines and comments are dropped first —
        // `[framework_methods]` is the last section in both descriptors, so the
        // slice runs to EOF and a benign explanatory comment or trailing newline
        // under one file's last row would otherwise fail with a misleading "not
        // byte-identical".
        let rows = |descriptor: &str| -> Vec<String> {
            descriptor
                .lines()
                .skip_while(|line| line.trim() != "[framework_methods]")
                .skip(1)
                .take_while(|line| !line.trim_start().starts_with('['))
                .filter(|line| {
                    let trimmed = line.trim();
                    !trimmed.is_empty() && !trimmed.starts_with('#')
                })
                .map(|line| line.to_string())
                .collect()
        };
        let java_rows = rows(include_str!("../../../plugins/java/plugin.toml"));
        assert_eq!(
            java_rows.len(),
            6,
            "the table's shape is pinned too, so a dropped row cannot pass by \
             being dropped from both: {java_rows:?}"
        );
        let kotlin_rows = rows(include_str!("../../../plugins/kotlin/plugin.toml"));
        assert_eq!(
            kotlin_rows[..java_rows.len()],
            java_rows[..],
            "the annotation rows must be byte-identical, and lead Kotlin's table"
        );
        assert_eq!(
            kotlin_rows.len(),
            java_rows.len() + 14,
            "Kotlin's table adds the seven upper-case and seven lower-case DSL rows: {kotlin_rows:?}"
        );
    }

    /// The non-duplication criterion, proved structurally rather than by reading
    /// the diff: the shared interpreter names **no** JVM annotation node kind in
    /// its code, so neither language can have a composition implementation of
    /// its own. A second copy would have to walk an annotation's arguments, and
    /// there is no way to do that without naming one of those kinds.
    ///
    /// **Inverted, and derived from the grammars rather than from a hand-written
    /// list.** The first version of this test listed the kinds it forbade, and
    /// review demonstrated the consequence by mutation: a complete Kotlin-only
    /// composition function walking `class_declaration` → `modifiers` →
    /// `annotation` passed, because none of those three was on the list (and one
    /// entry, `simple_identifier`, was not even a node kind in the pinned
    /// grammar). A closed list cannot notice what it does not name. So the check
    /// now enumerates **every named node kind of both JVM grammars** — 248 of
    /// them, straight from the loaded `Language` — and requires any that the
    /// interpreter's code names to be on a short, deliberate allowlist. A new
    /// grammar release cannot leave it behind, and adding a JVM node kind to the
    /// interpreter is a decision someone has to write down here.
    ///
    /// Scope is every `.rs` file under `src/resolve/`, not one file: a future
    /// `resolve/framework/kotlin.rs` would be invisible to a single
    /// `include_str!`, and `route_template.rs` is on the same promotion path.
    #[test]
    fn no_language_specific_composition_code_exists() {
        let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
        let jvm_kinds = jvm_node_kinds(&registry);
        assert!(
            jvm_kinds.contains("annotation") && jvm_kinds.contains("element_value_pair"),
            "the derived kind set must really come from the JVM grammars"
        );

        for (file, code) in resolver_sources() {
            for literal in quoted_identifiers(&code) {
                if !jvm_kinds.contains(literal.as_str()) {
                    continue;
                }
                assert!(
                    RESOLVER_KIND_ALLOWLIST.contains(&literal.as_str())
                        || FILE_KIND_ALLOWLIST.contains(&(file.as_str(), literal.as_str())),
                    "{file} names the JVM node kind {literal:?}; the shared \
                     interpreter must stay language-neutral. If this is a Rust \
                     (or other) grammar kind that merely collides with a JVM \
                     one, add it to RESOLVER_KIND_ALLOWLIST with a reason."
                );
            }
            // Language-specific logic need not name a node kind at all — a
            // `plugin.id == "kotlin"` branch would do. The guard would miss
            // that, so it is asserted directly.
            for id in ["\"kotlin\"", "\"java\"", "\"kt\"", "\"scala\""] {
                assert!(
                    !code.contains(id),
                    "{file} branches on the language id {id}; the shared \
                     interpreter must be driven by capture names and descriptor \
                     data, never by which language it is looking at"
                );
            }
        }
    }

    /// Every named node kind of both JVM grammars, read from the loaded
    /// `Language` so it cannot fall behind a grammar upgrade.
    fn jvm_node_kinds(registry: &LanguageRegistry) -> std::collections::BTreeSet<String> {
        let mut kinds = std::collections::BTreeSet::new();
        for ext in ["java", "kt"] {
            let language = registry
                .for_extension(ext)
                .unwrap_or_else(|| panic!("{ext} plugin"))
                .language();
            for id in 0..language.node_kind_count() {
                let id = id as u16;
                if language.node_kind_is_named(id) {
                    if let Some(kind) = language.node_kind_for_id(id) {
                        kinds.insert(kind.to_string());
                    }
                }
            }
        }
        kinds
    }

    /// Every `.rs` file under `src/resolve/` except the test modules, verbatim.
    ///
    /// **No comment stripping, deliberately.** The first version of this test
    /// split each line at the first `//` to skip the prose, and review defeated
    /// that twice: `node.kind() == "http://value_argument"` named a guarded kind
    /// and passed because the cut landed inside the string literal, and a
    /// hand-written scanner then mis-lexed Rust's `'"'` char literal and stopped
    /// seeing code at all. Both failures come from deciding what is a comment
    /// without knowing what is a string.
    ///
    /// The raw source needs no such decision, because the guard only looks at
    /// **double-quoted identifiers** and these modules quote node kinds in
    /// `backticks` when they discuss them in prose. Measured: the set of
    /// JVM-colliding quoted identifiers is identical with and without comment
    /// stripping. If a future comment does double-quote a node kind the guard
    /// will flag it, and the fix is to write it in backticks like its
    /// neighbours.
    fn resolver_sources() -> Vec<(String, String)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
            let entries = std::fs::read_dir(dir).expect("resolve/ is readable");
            for entry in entries {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs")
                    && path.file_name().is_some_and(|f| f != "tests.rs")
                {
                    let source = std::fs::read_to_string(&path).expect("source is readable");
                    out.push((
                        path.file_name().expect("file name").to_string_lossy().into_owned(),
                        source,
                    ));
                }
            }
        }
        let mut out = Vec::new();
        walk(
            std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src/resolve")),
            &mut out,
        );
        assert!(out.len() > 1, "resolve/ must contribute several files: {out:?}");
        out
    }

    /// Every `"identifier"` appearing in `code` — the only shape a `node.kind()`
    /// comparison can take.
    ///
    /// Each `"` is tried as an opening quote **independently**, and the scan
    /// advances one byte at a time rather than skipping past the literal it just
    /// read. That matters: pairing quotes off against each other goes out of
    /// phase on the first `'"'` char literal or `\"` escape in the file — which
    /// `framework.rs` has — and everything after it is then read inside-out. An
    /// occurrence test does not need to know where literals begin, only that the
    /// three-token shape appears somewhere, so it does not pay for a lexer it
    /// cannot get right.
    fn quoted_identifiers(code: &str) -> Vec<String> {
        let bytes = code.as_bytes();
        let mut found = Vec::new();
        for (at, _) in code.match_indices('"') {
            let start = at + 1;
            let mut end = start;
            while end < bytes.len()
                && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_')
            {
                end += 1;
            }
            if end > start
                && !bytes[start].is_ascii_digit()
                && bytes.get(end) == Some(&b'"')
            {
                found.push(code[start..end].to_string());
            }
        }
        found
    }

    /// Node kinds the resolver legitimately names that happen to exist in the
    /// JVM grammars too. Every one is a **Rust** grammar kind used by the legacy
    /// Axum/Actix structural walkers (or by the dispatch/binder passes), which is
    /// why the collision is a coincidence of naming rather than JVM knowledge.
    /// Measured: these are the complete intersection today.
    const RESOLVER_KIND_ALLOWLIST: &[&str] = &[
        "block_comment",
        "call_expression",
        "generic_type",
        "identifier",
        "line_comment",
        "scoped_identifier",
        "scoped_type_identifier",
        "string_content",
        "string_literal",
        "super",
        "type",
        "type_arguments",
        "type_identifier",
    ];

    /// Literals that spell a JVM node kind in one file only, and are no node kind
    /// there — exempt in that file alone, so the same literal elsewhere in the
    /// resolver is still caught. `ts_config` reads JSON and walks no tree:
    /// `import` is a `package.json` `exports` condition (S-531).
    const FILE_KIND_ALLOWLIST: &[(&str, &str)] = &[("ts_config.rs", "import"), ("ts_config_tests.rs", "import")];

    /// The same Spring annotation written in each language's syntax. Kotlin
    /// arrays are `[…]` where Java's are `{…}`, Kotlin's implementation
    /// override is a keyword where Java's is `@Override`, and Kotlin's
    /// interfaces and objects are the same node kind as its classes — none of
    /// which may change the promoted route.
    /// `(label, kotlin, java, establishes)` — `establishes` is `false` for a row
    /// asserting a shape that promotes, refuses and defers **nothing** in either
    /// language.
    const PAIRED_FIXTURES: &[(&str, &str, &str, bool)] = &[
        (
            "named value argument",
            "class C {\n    @RequestMapping(method = RequestMethod.GET, value = \"/v1/x\", produces = \"application/json\")\n    fun getX(): String { return \"\" }\n}\n",
            "public class C {\n    @RequestMapping(method = RequestMethod.GET, value = \"/v1/x\", produces = \"application/json\")\n    public String getX() { return \"\"; }\n}\n",
            true,
        ),
        (
            "named path alias",
            "class C {\n    @GetMapping(path = \"/v1/y\")\n    fun getY(): String { return \"\" }\n}\n",
            "public class C {\n    @GetMapping(path = \"/v1/y\")\n    public String getY() { return \"\"; }\n}\n",
            true,
        ),
        (
            "list-valued method path",
            "class C {\n    @GetMapping(value = [\"/a\", \"/b\"])\n    fun get(): String { return \"\" }\n}\n",
            "public class C {\n    @GetMapping(value = {\"/a\", \"/b\"})\n    public String get() { return \"\"; }\n}\n",
            true,
        ),
        (
            "positional method path",
            "class C {\n    @GetMapping(\"/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "public class C {\n    @GetMapping(\"/users\")\n    public String listUsers() { return \"\"; }\n}\n",
            true,
        ),
        (
            "produces/consumes only",
            "class C {\n    @GetMapping(produces = \"application/json\", consumes = \"text/plain\")\n    fun getX(): String { return \"\" }\n}\n",
            "public class C {\n    @GetMapping(produces = \"application/json\", consumes = \"text/plain\")\n    public String getX() { return \"\"; }\n}\n",
            false,
        ),
        (
            "prefixed interface and bare implementation",
            "@RequestMapping(\"/v1\")\ninterface UserApi {\n    @RequestMapping(method = RequestMethod.GET, value = \"/users\", produces = \"application/json\")\n    fun listUsers(): String\n\n    @GetMapping(path = [\"/users/{id}\", \"/users/by-id/{id}\"])\n    fun getUser(id: String): String\n}\n\n@RestController\nclass UserController : UserApi {\n    override fun listUsers(): String { return \"\" }\n    override fun getUser(id: String): String { return \"\" }\n}\n",
            "@RequestMapping(\"/v1\")\npublic interface UserApi {\n    @RequestMapping(method = RequestMethod.GET, value = \"/users\", produces = \"application/json\")\n    String listUsers();\n\n    @GetMapping(path = {\"/users/{id}\", \"/users/by-id/{id}\"})\n    String getUser(String id);\n}\n\n@RestController\nclass UserController implements UserApi {\n    @Override\n    public String listUsers() { return \"\"; }\n    @Override\n    public String getUser(String id) { return \"\"; }\n}\n",
            true,
        ),
        (
            "pathless handler in a prefixed class",
            "@RequestMapping(\"/v1/users\")\n@RestController\nclass C {\n    @GetMapping\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RequestMapping(\"/v1/users\")\n@RestController\npublic class C {\n    @GetMapping\n    public String listUsers() { return \"\"; }\n}\n",
            true,
        ),
        (
            "pathless handler with no prefix",
            "class C {\n    @GetMapping\n    fun listUsers(): String { return \"\" }\n}\n",
            "public class C {\n    @GetMapping\n    public String listUsers() { return \"\"; }\n}\n",
            false,
        ),
        (
            "unannotated nested type inherits nothing",
            "@RequestMapping(\"/outer\")\nclass Outer {\n    @GetMapping(value = \"/a\")\n    fun outerHandler(): String { return \"\" }\n\n    class Inner {\n        @GetMapping(value = \"/b\")\n        fun innerHandler(): String { return \"\" }\n    }\n}\n",
            "@RequestMapping(\"/outer\")\npublic class Outer {\n    @GetMapping(value = \"/a\")\n    public String outerHandler() { return \"\"; }\n\n    public static class Inner {\n        @GetMapping(value = \"/b\")\n        public String innerHandler() { return \"\"; }\n    }\n}\n",
            true,
        ),
        (
            "nested prefixed type takes its own prefix",
            "@RequestMapping(\"/outer\")\nclass Outer {\n    @GetMapping(value = \"/a\")\n    fun outerHandler(): String { return \"\" }\n\n    @RequestMapping(\"/inner\")\n    class Inner {\n        @GetMapping(value = \"/b\")\n        fun innerHandler(): String { return \"\" }\n    }\n}\n",
            "@RequestMapping(\"/outer\")\npublic class Outer {\n    @GetMapping(value = \"/a\")\n    public String outerHandler() { return \"\"; }\n\n    @RequestMapping(\"/inner\")\n    public static class Inner {\n        @GetMapping(value = \"/b\")\n        public String innerHandler() { return \"\"; }\n    }\n}\n",
            true,
        ),
        (
            "positional non-literal prefix is refused",
            "@RequestMapping(BASE)\nclass C {\n    @GetMapping(value = \"/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RequestMapping(BASE)\npublic class C {\n    @GetMapping(value = \"/users\")\n    public String listUsers() { return \"\"; }\n}\n",
            true,
        ),
        (
            "named non-literal prefix is refused",
            "@RequestMapping(value = BASE)\nclass C {\n    @GetMapping(value = \"/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RequestMapping(value = BASE)\npublic class C {\n    @GetMapping(value = \"/users\")\n    public String listUsers() { return \"\"; }\n}\n",
            true,
        ),
        (
            "empty prefix list declares no prefix",
            "@RequestMapping(value = [])\nclass C {\n    @GetMapping(value = \"/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RequestMapping(value = {})\npublic class C {\n    @GetMapping(value = \"/users\")\n    public String listUsers() { return \"\"; }\n}\n",
            true,
        ),
        (
            "class annotation with no path argument",
            "@RequestMapping(produces = \"application/json\")\nclass C {\n    @GetMapping(value = \"/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RequestMapping(produces = \"application/json\")\npublic class C {\n    @GetMapping(value = \"/users\")\n    public String listUsers() { return \"\"; }\n}\n",
            true,
        ),
        (
            "class annotation with only a method argument",
            "@RequestMapping(method = RequestMethod.GET)\nclass C {\n    @GetMapping(value = \"/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RequestMapping(method = RequestMethod.GET)\npublic class C {\n    @GetMapping(value = \"/users\")\n    public String listUsers() { return \"\"; }\n}\n",
            true,
        ),
        (
            "named list-valued class prefix",
            "@RequestMapping(value = [\"/v1\", \"/v2\"])\nclass C {\n    @GetMapping(value = \"/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RequestMapping(value = {\"/v1\", \"/v2\"})\npublic class C {\n    @GetMapping(value = \"/users\")\n    public String listUsers() { return \"\"; }\n}\n",
            true,
        ),
        (
            "positional list-valued class prefix",
            "@RequestMapping([\"/v1\", \"/v2\"])\nclass C {\n    @GetMapping(value = \"/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RequestMapping({\"/v1\", \"/v2\"})\npublic class C {\n    @GetMapping(value = \"/users\")\n    public String listUsers() { return \"\"; }\n}\n",
            true,
        ),
        (
            "mixed class prefix list composes on its literal",
            "@RequestMapping(value = [\"/v1\", BASE])\nclass C {\n    @GetMapping(value = \"/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RequestMapping(value = {\"/v1\", BASE})\npublic class C {\n    @GetMapping(value = \"/users\")\n    public String listUsers() { return \"\"; }\n}\n",
            true,
        ),
        (
            "property-placeholder prefix is refused",
            "@RequestMapping(\"${api.base}\")\nclass C {\n    @GetMapping(value = \"/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RequestMapping(\"${api.base}\")\npublic class C {\n    @GetMapping(value = \"/users\")\n    public String listUsers() { return \"\"; }\n}\n",
            true,
        ),
        (
            "only RequestMapping prefixes a type",
            "@Validated(\"group\")\nclass C {\n    @GetMapping(value = \"/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "@Validated(\"group\")\npublic class C {\n    @GetMapping(value = \"/users\")\n    public String listUsers() { return \"\"; }\n}\n",
            true,
        ),
        (
            "separator normalisation",
            "@RequestMapping(\"/v1/\")\nclass C {\n    @GetMapping(value = \"users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RequestMapping(\"/v1/\")\npublic class C {\n    @GetMapping(value = \"users\")\n    public String listUsers() { return \"\"; }\n}\n",
            true,
        ),
        (
            "unmapped annotation carrying arguments",
            "class C {\n    @Operation(value = \"/v1/x\")\n    @ApiResponse(path = [\"/a\", \"/b\"])\n    fun getX(): String { return \"\" }\n}\n",
            "public class C {\n    @Operation(value = \"/v1/x\")\n    @ApiResponse(path = {\"/a\", \"/b\"})\n    public String getX() { return \"\"; }\n}\n",
            false,
        ),
        (
            "two annotations on one handler",
            "class C {\n    @GetMapping(\"/read\")\n    @PostMapping(value = \"/write\")\n    fun both(): String { return \"\" }\n}\n",
            "public class C {\n    @GetMapping(\"/read\")\n    @PostMapping(value = \"/write\")\n    public String both() { return \"\"; }\n}\n",
            true,
        ),
        (
            "both alias keys on one annotation",
            "class C {\n    @GetMapping(value = \"/x\", path = \"/y\")\n    fun get(): String { return \"\" }\n}\n",
            "public class C {\n    @GetMapping(value = \"/x\", path = \"/y\")\n    public String get() { return \"\"; }\n}\n",
            true,
        ),
        (
            "placeholder method path promoted verbatim",
            "class C {\n    @GetMapping(value = \"${api.base}/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "public class C {\n    @GetMapping(value = \"${api.base}/users\")\n    public String listUsers() { return \"\"; }\n}\n",
            true,
        ),
        (
            "empty-string prefix over a pathless handler",
            "@RequestMapping(\"\")\nclass C {\n    @GetMapping\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RequestMapping(\"\")\npublic class C {\n    @GetMapping\n    public String listUsers() { return \"\"; }\n}\n",
            false,
        ),
        (
            "stereotype component",
            "@RestController\nclass UserController {\n    @GetMapping(\"/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RestController\npublic class UserController {\n    @GetMapping(\"/users\")\n    public String listUsers() { return \"\"; }\n}\n",
            true,
        ),
        (
            "paths converging after composition collapse",
            "@RequestMapping(\"/v1\")\nclass C {\n    @GetMapping(value = \"/users\")\n    fun listUsers(): String { return \"\" }\n    @GetMapping(value = \"users\")\n    fun listUsersAgain(): String { return \"\" }\n}\n",
            "@RequestMapping(\"/v1\")\npublic class C {\n    @GetMapping(value = \"/users\")\n    public String listUsers() { return \"\"; }\n    @GetMapping(value = \"users\")\n    public String listUsersAgain() { return \"\"; }\n}\n",
            true,
        ),
        (
            "method-level mapping is not its own prefix",
            "class C {\n    @RequestMapping(method = RequestMethod.GET, value = \"/v1/x\")\n    fun getX(): String { return \"\" }\n}\n",
            "public class C {\n    @RequestMapping(method = RequestMethod.GET, value = \"/v1/x\")\n    public String getX() { return \"\"; }\n}\n",
            true,
        ),
        (
            "annotation order does not decide the prefix",
            "@RestController\n@RequestMapping(\"/v1\")\nclass C {\n    @GetMapping(value = \"/users\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RestController\n@RequestMapping(\"/v1\")\npublic class C {\n    @GetMapping(value = \"/users\")\n    public String listUsers() { return \"\"; }\n}\n",
            true,
        ),
        (
            "explicitly empty method path under a prefix",
            "@RequestMapping(\"/v1\")\nclass C {\n    @GetMapping(\"\")\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RequestMapping(\"/v1\")\npublic class C {\n    @GetMapping(\"\")\n    public String listUsers() { return \"\"; }\n}\n",
            true,
        ),
        (
            "a refused list-valued registration counts once",
            "@RequestMapping(BASE)\nclass C {\n    @GetMapping(value = [\"/a\", \"/b\"])\n    fun list(): String { return \"\" }\n}\n",
            "@RequestMapping(BASE)\npublic class C {\n    @GetMapping(value = {\"/a\", \"/b\"})\n    public String list() { return \"\"; }\n}\n",
            true,
        ),
        (
            "unmapped marker annotation in a prefixed class",
            "@RequestMapping(\"/v1\")\nclass C {\n    @Autowired\n    fun listUsers(): String { return \"\" }\n}\n",
            "@RequestMapping(\"/v1\")\npublic class C {\n    @Autowired\n    public String listUsers() { return \"\"; }\n}\n",
            false,
        ),
    ];
}

/// [`routes_in_source`] must be a projection of [`scan_source`], not a second
/// reading of the query — that is its whole justification for existing. These
/// pin the two together on the shapes the reference-workspace measurement
/// (S-384) depends on.
mod measurement_seam {
    use super::*;

    const SPRING_CONTROLLER: &str = r#"
@RestController
@RequestMapping("/v1")
class MailboxApiV1 {
    @GetMapping("/users/{userId}/mailboxes/{mailboxId}")
    ResponseEntity<X> get(String userId, String mailboxId) { return null; }

    @PostMapping("/users/{userId}/mailboxes")
    ResponseEntity<X> create(String userId) { return null; }
}
"#;

    #[test]
    fn the_projection_agrees_with_the_scan_it_projects() {
        let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
        let plugin = registry.for_extension("java").expect("java plugin");
        let mut parser = Parser::new();
        // Every projected field is compared, `line` included: a field the
        // projection carries but no assertion reads can be silently zeroed.
        let scanned: Vec<(String, String, u32)> = scan_source(&mut parser, plugin, "src/MailboxApiV1.java", SPRING_CONTROLLER, None)
            .routes
            .into_iter()
            .map(|r| (r.method, r.path, r.start_line))
            .collect();
        let projected: Vec<(String, String, u32)> = routes_in_source(plugin, "src/MailboxApiV1.java", SPRING_CONTROLLER)
            .into_iter()
            .map(|r| (r.method, r.path, r.line))
            .collect();
        assert_eq!(projected, scanned);
        assert!(!projected.is_empty(), "the fixture must produce routes for this to mean anything");
    }

    #[test]
    fn the_projection_carries_the_class_level_prefix_composition() {
        // FR-FW-05. A projection that bypassed `compose_prefixes` would report
        // `/users/{userId}/mailboxes` and the measurement would match the wrong
        // provider — or none.
        let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
        let plugin = registry.for_extension("java").expect("java plugin");
        let paths: Vec<String> =
            routes_in_source(plugin, "src/MailboxApiV1.java", SPRING_CONTROLLER).into_iter().map(|r| r.path).collect();
        assert!(
            paths.contains(&"/v1/users/{userId}/mailboxes/{mailboxId}".to_string()),
            "composed paths: {paths:?}",
        );
        assert!(
            paths.contains(&"/v1/users/{userId}/mailboxes".to_string()),
            "composed paths: {paths:?}",
        );
    }

    #[test]
    fn a_concatenated_path_is_dropped_without_costing_its_literal_sibling() {
        // The provider-side gap S-384 measured: `pecserver-facade` registers
        // `value = "/mailboxes/{" + EMAIL_ADDRESS_PARAMETER_NAME + "}/size"`.
        // The constant reaches that file through a static import, which the
        // fold does not resolve yet (S-470), so the route is still absent —
        // refused and counted since S-469, where before it was invisible.
        //
        // The fixture carries a LITERAL sibling beside the concatenated one,
        // and the assertion is an exact equality rather than a negative. That
        // is the whole point: an earlier version asserted only
        // `!paths.contains("size")` over a concat-only fixture, which produces
        // an EMPTY vector — so the assertion was true by emptiness and survived
        // `routes_in_source` being replaced with `Vec::new()`. A negative
        // assertion needs a positive control or it pins nothing.
        let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
        let plugin = registry.for_extension("java").expect("java plugin");
        let source = r#"
@RestController
@RequestMapping("/v1")
class Facade {
    @GetMapping(value = "/mailboxes/{" + EMAIL + "}/size")
    ResponseEntity<X> size() { return null; }

    @GetMapping("/mailboxes/count")
    ResponseEntity<X> count() { return null; }
}
"#;
        let paths: Vec<String> =
            routes_in_source(plugin, "src/Facade.java", source).into_iter().map(|r| r.path).collect();
        assert_eq!(
            paths,
            vec!["/v1/mailboxes/count".to_string()],
            "the literal sibling must survive and the concatenated one must be absent",
        );
    }
}

/// A stored path is read only relative to the engine root (NFR-SE-04): an
/// absolute one — which `Path::join` would let replace the root — is refused
/// rather than read, for a declaring file exactly as for a candidate (S-470).
#[test]
fn a_member_file_is_read_only_by_a_relative_path() {
    let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
    let root = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(root.path().join("x.rs"), "fn x() {}\n").expect("write");
    let absolute = root.path().join("x.rs");
    let absolute = absolute.to_str().expect("utf-8 path");
    assert!(read_member_file(&registry, root.path(), "x.rs").is_some());
    assert!(read_member_file(&registry, root.path(), absolute).is_none());
}

// ── PHP: Laravel route groups and Symfony `#[Route]` attributes (S-549) ───────

/// Laravel and Symfony routes through the PHP query ([FR-FW-03], [FR-FW-05],
/// S-549): Laravel paths are rooted and take their groups' prefixes, nested
/// groups accumulate, and Symfony's attribute routes compose their class prefix
/// and name their method as the handler.
///
/// [FR-FW-03]: ../../../docs/specs/requirements/FR-FW-03.md
/// [FR-FW-05]: ../../../docs/specs/requirements/FR-FW-05.md
#[cfg(feature = "lang-php")]
mod php_routes {
    use super::*;

    /// The sorted `(METHOD path, handler)` set of a PHP snippet's routes.
    fn php_routes(source: &str) -> Vec<(String, Option<String>)> {
        let mut got: Vec<(String, Option<String>)> = scan_lang("php", source)
            .routes
            .into_iter()
            .map(|r| (format!("{} {}", r.method, r.path), r.handler))
            .collect();
        got.sort();
        got
    }

    /// The sorted route names alone.
    fn names(source: &str) -> Vec<String> {
        php_routes(source).into_iter().map(|(name, _)| name).collect()
    }

    fn owned(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).to_string()).collect()
    }

    /// Laravel serves `Route::get('me')` at `/me`: the path is rooted, so it
    /// can match an OpenAPI `/me`, and a single segment is a path rather than
    /// the property key the shared path rule drops.
    #[test]
    fn laravel_paths_are_rooted_and_single_segments_are_kept() {
        let got = names(
            "<?php\nRoute::get('ping', fn () => null);\nRoute::post('me', X::class);\n\
             Route::get('/users', [U::class, 'index']);\nRoute::get('/', $show);\n",
        );
        assert_eq!(got, owned(&["GET /", "GET /ping", "GET /users", "POST /me"]));
    }

    /// `prefix()` before `group()`, directly or with other registrar calls in
    /// between, on either side; and the `['prefix' => …]` array form.
    #[test]
    fn a_prefix_group_prefixes_the_routes_in_its_closure() {
        let got = names(
            "<?php
Route::prefix('a')->group(function () { Route::get('one', X::class); });
Route::prefix('b')->middleware('api')->group(static function (): void { Route::get('two', X::class); });
Route::middleware('os.auth')->prefix('os/s3')->group(function () { Route::post('song', X::class); });
Route::prefix('d')->middleware('x')->name('d.')->group(function () { Route::get('three', X::class); });
Route::prefix('e')->middleware('x')->name('e.')->where([])->group(function () { Route::get('four', X::class); });
Route::group(['middleware' => 'x', 'prefix' => 'radio'], function () { Route::get('stations', X::class); });
Route::prefix('arrow')->group(fn () => Route::get('five', X::class));
Route::get('outside', X::class);
",
        );
        assert_eq!(
            got,
            owned(&[
                "GET /a/one",
                "GET /arrow/five",
                "GET /b/two",
                "GET /d/three",
                "GET /e/four",
                "GET /outside",
                "GET /radio/stations",
                "POST /os/s3/song",
            ])
        );
    }

    /// koel's shape: groups nest, a prefixed group inside a prefixed group
    /// appends to it, and a group with no prefix passes the outer one through —
    /// to the routes after an inner group closes as well as to those inside it.
    #[test]
    fn nested_groups_accumulate_and_a_prefixless_group_passes_through() {
        let got = names(
            "<?php
Route::prefix('api')->middleware('api')->group(static function (): void {
    Route::get('ping', fn () => null);
    Route::middleware('auth')->group(static function (): void {
        Route::get('me', [P::class, 'show']);
        Route::group(['prefix' => 'radio'], static function (): void {
            Route::get('stations/{s}/now-playing', N::class);
        });
        Route::get('data', D::class);
    });
    Route::middleware('os.auth')->prefix('os/s3')->group(static function (): void {
        Route::post('song', [S::class, 'put']);
        Route::prefix('/deep/')->group(fn () => Route::delete('/x', S::class));
    });
});
",
        );
        assert_eq!(
            got,
            owned(&[
                "DELETE /api/os/s3/deep/x",
                "GET /api/data",
                "GET /api/me",
                "GET /api/ping",
                "GET /api/radio/stations/{s}/now-playing",
                "POST /api/os/s3/song",
            ])
        );
    }

    /// A prefix the source does not state as a literal refuses every route
    /// under it — including those of a group nested inside it — rather than
    /// promoting them at a partial address ([NFR-RA-05]).
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    #[test]
    fn an_opaque_prefix_refuses_its_routes_and_those_nested_under_it() {
        let m = scan_lang(
            "php",
            "<?php
Route::prefix($base)->group(function () {
    Route::get('a', X::class);
    Route::prefix('inner')->group(function () { Route::get('b', X::class); });
});
Route::group(['prefix' => config('app.prefix')], function () { Route::get('c', X::class); });
Route::prefix(\"v{$n}\")->group(function () { Route::get('d', X::class); });
Route::get('kept', X::class);
",
        );
        let got: Vec<String> = m.routes.iter().map(|r| format!("{} {}", r.method, r.path)).collect();
        assert_eq!(got, ["GET /kept"]);
        assert_eq!(m.refusals, vec![RouteRefusal::PathNotComposed; 4]);
    }

    /// Resource routes are not expanded (CR-176 §3.3), and near misses of the
    /// facade and of the `prefix` key promote or prefix nothing.
    #[test]
    fn resource_routes_and_near_misses_promote_nothing() {
        let got = names(
            "<?php
Route::apiResource('albums', AlbumController::class);
Route::resource('photos', PhotoController::class)->except('show');
Router::get('not-the-facade', X::class);
Route::prefixes('p')->group(function () { Route::get('a', X::class); });
Route::group(['prefix_' => 'q', 'as' => 'r'], function () { Route::get('b', X::class); });
Route::prefix('s')->groups(function () { Route::get('c', X::class); });
",
        );
        assert_eq!(got, owned(&["GET /a", "GET /b", "GET /c"]));
    }

    /// `Route::match('get', '/x', …)` takes its verb as the first argument, so
    /// the pattern that reads the first string as the path would read `get` —
    /// rooted, a route at `/get` the application never serves. `match` maps to
    /// no verb, so the registration promotes nothing.
    #[test]
    fn a_string_verb_match_is_not_read_as_a_path() {
        assert_eq!(
            names("<?php\nRoute::match('get', '/x', X::class);\nRoute::match(['get', 'post'], '/y', X::class);\n"),
            Vec::<String>::new()
        );
    }

    /// A double-quoted PHP string is its own node kind (`encapsed_string`), so
    /// every literal position reads it too: a `"prefix"` key, a prefix, a path,
    /// a verb. One that interpolates (`"users/$id"`) is not a literal — a prefix
    /// refuses its routes, a Symfony path is refused and counted, a Laravel path
    /// promotes nothing.
    #[test]
    fn double_quoted_literals_read_like_single_quoted_ones() {
        let laravel = scan_lang(
            "php",
            "<?php
Route::group([\"prefix\" => \"dq\"], function () { Route::get(\"a\", X::class); });
Route::prefix(\"api\")->group(function () { Route::get('b', X::class); });
Route::get(\"users/$id\", X::class);
Route::get(\"users/{$id}/x\", X::class);
",
        );
        let got: Vec<String> = laravel.routes.iter().map(|r| format!("{} {}", r.method, r.path)).collect();
        assert_eq!(got, ["GET /dq/a", "GET /api/b"]);
        assert!(laravel.refusals.is_empty(), "{:?}", laravel.refusals);

        let symfony = scan_lang(
            "php",
            "<?php
#[Route(\"/blog\")]
class A {
    #[Route(\"/x\", methods: [\"GET\"])] public function x() {}
    #[Route(path: \"/y\", methods: \"PUT\")] public function y() {}
    #[Route(\"/z/$id\")] public function z() {}
}
",
        );
        let mut got: Vec<String> = symfony.routes.iter().map(|r| format!("{} {}", r.method, r.path)).collect();
        got.sort();
        assert_eq!(got, ["GET /blog/x", "PUT /blog/y"]);
        assert_eq!(symfony.refusals, [RouteRefusal::PathNotFolded]);
    }

    /// Symfony's own verb constants (`Request::METHOD_GET`) are verbs, alone or
    /// in a list, and an empty `methods:` list answers every verb as Symfony
    /// reads it. A constant of another class, or a variable, names no verb this
    /// pass can prove, and promotes nothing.
    #[test]
    fn symfony_verb_constants_and_an_empty_methods_list() {
        assert_eq!(
            names(
                "<?php
class A {
    #[Route('/c1', methods: Request::METHOD_GET)] public function c1() {}
    #[Route('/c2', methods: [Request::METHOD_POST, Request::METHOD_PUT])] public function c2() {}
    #[Route(path: '/c3', methods: [Request::METHOD_DELETE])] public function c3() {}
    #[Route(methods: [Request::METHOD_PATCH], path: '/c4')] public function c4() {}
    #[Route('/e', methods: [])] public function e() {}
    #[Route(path: '/e2', methods : [ ])] public function e2() {}
    #[Route('/o', methods: [Other::METHOD_GET])] public function o() {}
    #[Route('/v', methods: $verbs)] public function v() {}
}
"
            ),
            [
                "ANY /e",
                "ANY /e2",
                "DELETE /c3",
                "GET /c1",
                "PATCH /c4",
                "POST /c2",
                "PUT /c2",
            ]
        );
    }

    /// A registration is refused once per path element it could not read, not
    /// once per verb it lists: `methods: ['GET', 'POST']` is one match per verb
    /// over the same opaque path, and the count is of addresses. A refused
    /// prefix over a multi-verb registration likewise counts the registration
    /// once (its `@fw.route.anchor` is the site).
    #[test]
    fn a_multi_verb_registration_is_refused_once() {
        let m = scan_lang(
            "php",
            "<?php\nclass A { #[Route(self::P, methods: ['GET', 'POST'])] public function a() {} }\n\
             #[Route(self::BASE)]\nclass C { #[Route('/c', methods: ['GET', 'POST'])] public function c() {} }\n",
        );
        assert!(m.routes.is_empty(), "{:?}", m.routes);
        assert_eq!(m.refusals, [RouteRefusal::PathNotFolded, RouteRefusal::PathNotComposed]);
    }

    /// A query alternative is written once per pattern, so each copy needs its
    /// own fixture: the arrow closure and the mid-chain `prefix()` at every
    /// depth, the array form's arrow closure, and the single-string verb in each
    /// Symfony pattern that reads `methods:`.
    #[test]
    fn every_copy_of_an_alternative_is_read() {
        assert_eq!(
            names(
                "<?php
Route::prefix('a')->middleware('x')->group(fn () => Route::get('one', X::class));
Route::group(['prefix' => 'b'], fn () => Route::get('two', X::class));
Route::middleware('x')->prefix('c')->name('c.')->group(function () { Route::get('three', X::class); });
Route::middleware('x')->prefix('d')->name('d.')->where([])->group(function () { Route::get('four', X::class); });
Route::middleware('x')->prefix('e')->as('e.')->name('e.')->where([])->group(function () { Route::get('five', X::class); });
Route::prefix('f')->middleware('x')->name('f.')->group(fn () => Route::get('six', X::class));
Route::prefix('g')->middleware('x')->name('g.')->where([])->group(fn () => Route::get('seven', X::class));
class A {
    #[Route('/one', methods: 'PUT')] public function one() {}
    #[Route(methods: 'PATCH', path: '/two')] public function two() {}
}
"
            ),
            [
                "GET /a/one",
                "GET /b/two",
                "GET /c/three",
                "GET /d/four",
                "GET /e/five",
                "GET /f/six",
                "GET /g/seven",
                "PATCH /two",
                "PUT /one",
            ]
        );
    }

    /// Each pattern's own predicates, probed one name away: the array form's
    /// facade and `group` names, `prefix` at the deepest chain position, and
    /// `group` after a mid-chain `prefix()`. None of them is a prefix group, so
    /// the routes inside keep their written address.
    #[test]
    fn near_misses_of_each_patterns_predicates_prefix_nothing() {
        assert_eq!(
            names(
                "<?php
Router::group(['prefix' => 'n1'], function () { Route::get('one', X::class); });
Route::when(['prefix' => 'n2'], function () { Route::get('two', X::class); });
Route::middleware('n3')->name('x.')->where([])->domain('d')->group(function () { Route::get('three', X::class); });
Route::prefix('n4')->middleware('x')->groups(function () { Route::get('four', X::class); });
"
            ),
            ["GET /four", "GET /one", "GET /three", "GET /two"]
        );
    }

    /// Symfony roots a prefix and a path written without the leading slash, in
    /// the patterns that read `methods:` as in those that do not; whitespace
    /// before the `:` of `methods :` does not reopen the every-verb route; and
    /// every verb row — lower- and upper-case, and each `Request::METHOD_*` —
    /// maps.
    #[test]
    fn symfony_rooting_spacing_and_every_verb_row() {
        assert_eq!(
            names(
                "<?php
#[Route('api')]
class A {
    #[Route('x')] public function x() {}
}
class B {
    #[Route('health', methods: ['GET'])] public function health() {}
    #[Route('/ws', methods : ['GET'])] public function ws() {}
    #[Route('/s', methods: ['head'])] public function s() {}
    #[Route('/m', methods: ['PUT', 'PATCH', 'OPTIONS', 'DELETE', 'POST'])] public function m() {}
    #[Route('/c', methods: [Request::METHOD_OPTIONS, Request::METHOD_HEAD])] public function c() {}
}
"
            ),
            [
                "ANY /api/x",
                "DELETE /m",
                "GET /health",
                "GET /ws",
                "HEAD /c",
                "HEAD /s",
                "OPTIONS /c",
                "OPTIONS /m",
                "PATCH /m",
                "POST /m",
                "PUT /m",
            ]
        );
    }

    const SYMFONY: &str = "<?php
namespace App\\Controller;

use Symfony\\Component\\Routing\\Attribute\\Route;

#[Route('/blog', name: 'blog_')]
class BlogController extends AbstractController
{
    #[Route('/', name: 'index')]
    public function index(): Response {}

    #[Route('/{slug}', name: 'show', methods: ['GET', 'HEAD'])]
    public function show(string $slug): Response {}

    #[Route(path: '/{slug}/edit', methods: 'POST')]
    #[IsGranted('ROLE_ADMIN')]
    public function edit(string $slug): Response {}

    #[Route(methods: ['DELETE'], name: 'delete', path: 'remove/{id}')]
    public function remove(int $id): Response {}

    #[IsGranted('ROLE_USER')]
    #[Route(path: '/feed')]
    public function feed(): Response {}

    #[Cache(maxage: 60)]
    public function notARoute(): Response {}
}
";

    /// The class prefix composes onto every method path, the `methods:` list
    /// fans out (one route per verb, a missing list is ANY), the path may be
    /// positional or named and come before or after `methods:`, and the
    /// handler is the method the attribute decorates.
    #[test]
    fn symfony_attribute_routes_compose_their_class_prefix() {
        let h = |name: &str| Some(name.to_string());
        assert_eq!(
            php_routes(SYMFONY),
            vec![
                ("ANY /blog/".to_string(), h("index")),
                ("ANY /blog/feed".to_string(), h("feed")),
                ("DELETE /blog/remove/{id}".to_string(), h("remove")),
                ("GET /blog/{slug}".to_string(), h("show")),
                ("HEAD /blog/{slug}".to_string(), h("show")),
                ("POST /blog/{slug}/edit".to_string(), h("edit")),
            ]
        );
        // Every path above is a written literal, so nothing is refused — a
        // named argument's `path` name is never read as an opaque path.
        let refusals = scan_lang("php", SYMFONY).refusals;
        assert!(refusals.is_empty(), "{refusals:?}");
    }

    /// Without a class `#[Route]` the method paths stand alone (rooted); a named
    /// `path:` prefix composes like a positional one; a non-literal prefix
    /// refuses its routes; a non-literal method path is refused and counted.
    #[test]
    fn symfony_prefix_forms_and_opaque_paths() {
        assert_eq!(
            names(
                "<?php\nclass A { #[Route('health')] public function h() {} }\n\
                 #[Route(name: 'x_', path: '/api')]\nclass B { #[Route('/b', methods: ['get'])] public function b() {} }\n"
            ),
            owned(&["ANY /health", "GET /api/b"])
        );

        let m = scan_lang(
            "php",
            "<?php\n#[Route(self::BASE)]\nclass C { #[Route('/c')] public function c() {} }\n\
             class D { #[Route(self::PATH)] public function d() {} #[Route('/e')] public function e() {} }\n",
        );
        let got: Vec<String> = m.routes.iter().map(|r| format!("{} {}", r.method, r.path)).collect();
        assert_eq!(got, ["ANY /e"]);
        assert_eq!(m.refusals, [RouteRefusal::PathNotFolded, RouteRefusal::PathNotComposed]);
    }

    /// An attribute that is not `Route` — a near miss by one letter included —
    /// is no route, and a `name:` string is never read as the path.
    #[test]
    fn symfony_near_miss_attributes_are_not_routes() {
        assert_eq!(
            names(
                "<?php\nclass A {\n#[Routes('/a')] public function a() {}\n\
                 #[Router('/b', methods: ['GET'])] public function b() {}\n\
                 #[Route(name: 'only_a_name')] public function c() {}\n}\n"
            ),
            Vec::<String>::new()
        );
    }

    /// The nesting rule at the interpreter grain: a nested scope accumulates
    /// across every enclosing level, inherits an enclosing prefix when it states
    /// none, and becomes unreadable under an unreadable parent; a scope that
    /// does not opt in keeps Spring's innermost-only rule.
    #[test]
    fn nest_scopes_accumulates_only_for_scopes_that_opt_in() {
        let nested = |mut s: PrefixScope| {
            s.nested = true;
            s
        };
        let mut scopes = merge_scopes(&[
            scope(0, 100, &[("/api", 1, 2)], &[]),
            nested(scope(10, 90, &[("/v1", 11, 12)], &[])),
            nested(scope(20, 80, &[("/radio", 21, 22)], &[])),
            nested(scope(30, 40, &[], &[])),
            scope(50, 60, &[("/own", 51, 52)], &[]),
            scope(200, 300, &[], &[(201, 202)]),
            nested(scope(210, 290, &[("/x", 211, 212)], &[])),
        ]);
        nest_scopes(&mut scopes);
        let at = |byte: usize| innermost_prefix(&scopes, byte).map(|s| (s.paths.clone(), s.opaque));
        assert_eq!(at(15), Some((vec!["/api/v1".to_string()], false)));
        assert_eq!(at(25), Some((vec!["/api/v1/radio".to_string()], false)));
        assert_eq!(at(35), Some((vec!["/api/v1/radio".to_string()], false)));
        assert_eq!(at(55), Some((vec!["/own".to_string()], false)));
        assert_eq!(at(215), Some((Vec::new(), true)));
    }
}

// ── Functional routing, Ktor and rooted paths (S-561, CR-178) ────────────────

/// Spring's functional DSL and Ktor's `routing { }` on Kotlin's call tree: a
/// registration is a route only inside a router block, every `nest` / `route`
/// around it adds its prefix, and a handler reference written on a typed
/// property reads that property's declared type ([FR-FW-03], [NFR-RA-05]).
///
/// [FR-FW-03]: ../../../docs/specs/requirements/FR-FW-03.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[cfg(feature = "lang-kotlin")]
mod kotlin_dsl {
    use super::*;

    fn scanned(source: &str) -> FileMatches {
        scan_lang("kt", source)
    }

    /// The sorted `(method path)` names of a scan — the handler column is
    /// asserted where a test is about it.
    fn names(m: &FileMatches) -> Vec<String> {
        let mut got: Vec<String> = m.routes.iter().map(|r| format!("{} {}", r.method, r.path)).collect();
        got.sort();
        got
    }

    fn owned(rows: &[&str]) -> Vec<String> {
        rows.iter().map(|r| r.to_string()).collect()
    }

    #[test]
    fn a_co_router_registration_is_a_route_linked_to_its_typed_handler() {
        let m = scanned(
            r#"
class Routes(private val talkHandler: TalkHandler, val blog: BlogHandler) {
    @Bean
    fun router() = coRouter {
        GET("/talk", talkHandler::findAll)
        POST("/blog/{id}", blog::save)
    }
}
"#,
        );
        assert_eq!(
            sorted_triples(&m),
            [
                ("/blog/{id}".to_string(), "POST".to_string(), Some("BlogHandler::save".to_string())),
                ("/talk".to_string(), "GET".to_string(), Some("TalkHandler::findAll".to_string())),
            ]
        );
        assert!(m.refusals.is_empty(), "{:?}", m.refusals);
    }

    #[test]
    fn the_handler_forms_a_registration_can_carry() {
        let m = scanned(
            r#"
class Routes(private val h: H, plain: P) {
    fun router() = router {
        GET("/ref", h::m)
        GET("/top", ::topLevel)
        GET("/typed", Foo::bar)
        GET("/lambda") { ok().build() }
        GET("/param", plain::m)
        GET("/read", h.m)
    }
}
"#,
        );
        let handler = |path: &str| {
            m.routes
                .iter()
                .find(|r| r.path == path)
                .unwrap_or_else(|| panic!("{path} is promoted"))
                .handler
                .clone()
        };
        assert_eq!(handler("/ref").as_deref(), Some("H::m"), "a property's declared type");
        assert_eq!(handler("/top").as_deref(), Some("topLevel"), "a bare function reference");
        assert_eq!(handler("/typed").as_deref(), Some("Foo::bar"), "a type's member is written as one");
        assert_eq!(handler("/lambda"), None, "a lambda names no handler");
        // Near misses: a plain constructor parameter is no property, and `h.m`
        // is a property READ — neither is a callable reference.
        assert_eq!(handler("/param").as_deref(), Some("plain::m"), "no declared property: the written head");
        assert_eq!(handler("/read"), None, "`.` is not `::`");
    }

    #[test]
    fn nests_compose_outermost_first_and_a_path_free_predicate_adds_nothing() {
        let m = scanned(
            r#"
class Routes(private val h: H) {
    fun api() = coRouter {
        (accept(APPLICATION_JSON) and "/api").nest {
            "/admin".nest {
                GET("/{year}/talk", h::byYear)
            }
            GET("/faq", h::faq)
            accept(TEXT_HTML).nest { GET("/html") }
        }
        ("/legacy" and accept(APPLICATION_JSON)).nest { GET("/old") }
        accept(TEXT_HTML).nest { GET("/page") }
        GET("/") { ok().build() }
    }
}
"#,
        );
        assert_eq!(
            names(&m),
            owned(&[
                "GET /",
                "GET /api/admin/{year}/talk",
                "GET /api/faq",
                "GET /api/html",
                "GET /legacy/old",
                "GET /page",
            ])
        );
        assert!(m.refusals.is_empty(), "{:?}", m.refusals);
    }

    #[test]
    fn a_prefix_written_without_a_slash_is_rooted() {
        let m = scanned(
            r#"
fun routes() = router {
    "admin".nest { GET("users") { ok().build() } }
    GET("plain") { ok().build() }
}
"#,
        );
        assert_eq!(names(&m), owned(&["GET /admin/users", "GET /plain"]));
    }

    #[test]
    fn an_unreadable_nest_prefix_refuses_its_registrations_and_counts_them() {
        let m = scanned(
            r#"
fun routes() = router {
    BASE.nest { GET("/a") { ok().build() } }
    (accept(APPLICATION_JSON) and BASE).nest { GET("/b") { ok().build() } }
    path("/p").nest { GET("/c") { ok().build() } }
    (accept(JSON).and(path("/q"))).nest { GET("/d") { ok().build() } }
    "/${base}".nest { GET("/e") { ok().build() } }
    GET("/ok") { ok().build() }
}
"#,
        );
        assert_eq!(names(&m), owned(&["GET /ok"]), "only the readable registration is promoted");
        assert_eq!(m.refusals, [RouteRefusal::PathNotComposed; 5], "each refused registration is counted");
    }

    #[test]
    fn a_registration_outside_a_router_block_is_not_a_route() {
        let m = scanned(
            r#"
class T(private val mockMvc: MockMvc) {
    fun test() {
        mockMvc.get("/vets") { accept = APPLICATION_JSON }
        get("/loose") { 1 }
        GET("/also-loose") { 2 }
        post("/x/y")
        get(url) { 3 }
        GET(base + "/x") { 4 }
        cache.get("/cache/key")
    }
}
"#,
        );
        assert!(m.routes.is_empty(), "{:?}", m.routes);
        assert!(m.refusals.is_empty(), "nothing outside a router block is counted: {:?}", m.refusals);
    }

    #[test]
    fn a_call_inside_a_handler_body_is_not_a_registration() {
        let m = scanned(
            r#"
fun Application.module() {
    routing {
        get("/h") { call.respond(buildJsonObject { put("status", "ok") }) }
        get("/home") { call.respondHtml { head { title("x") } } }
        route("/p") {
            get("/x") { val v = with(call.parameters) { get("id") }; put(KEY) { 1 } }
            get { put("k") { 2 } }
            delete { }
        }
    }
}
fun spring() = router {
    GET("/s") { ok().body(JSONObject().apply { put("status", 1) }) }
    (GET("/a") or GET("/b")) { put("k") { 3 } }
}
"#,
        );
        assert_eq!(
            names(&m),
            owned(&["DELETE /p", "GET /a", "GET /b", "GET /h", "GET /home", "GET /p", "GET /p/x", "GET /s"]),
            "the registrations are promoted; the calls in their handler bodies are not"
        );
        assert!(m.refusals.is_empty(), "a handler's own call is not a refused path: {:?}", m.refusals);
    }

    #[test]
    fn every_dsl_verb_maps_to_its_own_method() {
        let m = scanned(
            r#"
fun spring() = router {
    GET("/g") { 1 }
    POST("/po") { 1 }
    PUT("/pu") { 1 }
    DELETE("/d") { 1 }
    PATCH("/pa") { 1 }
    HEAD("/h") { 1 }
    OPTIONS("/o") { 1 }
}
fun Application.ktor() = routing {
    get("/kg") { 1 }
    post("/kpo") { 1 }
    put("/kpu") { 1 }
    delete("/kd") { 1 }
    patch("/kpa") { 1 }
    head("/kh") { 1 }
    options("/ko") { 1 }
}
"#,
        );
        assert_eq!(
            names(&m),
            owned(&[
                "DELETE /d", "DELETE /kd", "GET /g", "GET /kg", "HEAD /h", "HEAD /kh", "OPTIONS /ko",
                "OPTIONS /o", "PATCH /kpa", "PATCH /pa", "POST /kpo", "POST /po", "PUT /kpu", "PUT /pu",
            ])
        );
    }

    #[test]
    fn every_path_free_predicate_adds_no_segment_and_a_literal_beside_one_is_the_prefix() {
        let m = scanned(
            r#"
fun routes() = router {
    (contentType(X) and "/a").nest { GET("/1") { 1 } }
    ("/b" and headers(X)).nest { GET("/2") { 1 } }
    (method(X) and "/c").nest { GET("/3") { 1 } }
    method(X).nest { GET("/4") { 1 } }
    headers(X).nest { GET("/5") { 1 } }
    contentType(X).nest { GET("/6") { 1 } }
}
"#,
        );
        assert_eq!(names(&m), owned(&["GET /4", "GET /5", "GET /6", "GET /a/1", "GET /b/2", "GET /c/3"]));
        assert!(m.refusals.is_empty(), "{:?}", m.refusals);
    }

    #[test]
    fn a_handler_head_reads_the_property_the_enclosing_class_declares() {
        let m = scanned(
            r#"
class Outer(private val h: A, private val g: Foo<Bar>, val q: a.B, val n: N?) {
    private val member: M = M()
    fun r() = router {
        GET("/own", h::m)
        GET("/member", member::m)
        GET("/generic", g::m)
        GET("/qualified", q::m)
        GET("/nullable", n::m)
    }

    inner class Inner(val h: B) {
        fun r() = router { GET("/inner", h::m) }
    }
}

class Other(val h: C) {
    fun r() = router { GET("/other", h::m) }
}
"#,
        );
        let handler = |path: &str| m.routes.iter().find(|r| r.path == path).and_then(|r| r.handler.clone());
        assert_eq!(handler("/own").as_deref(), Some("A::m"));
        assert_eq!(handler("/member").as_deref(), Some("M::m"), "a member property that states its type");
        // A type that is not one bare name is not read: the written head stays,
        // and binds nothing.
        assert_eq!(handler("/generic").as_deref(), Some("g::m"));
        assert_eq!(handler("/qualified").as_deref(), Some("q::m"));
        assert_eq!(handler("/nullable").as_deref(), Some("n::m"));
        // The innermost declaration of a name wins, and a class sees only its own.
        assert_eq!(handler("/inner").as_deref(), Some("B::m"));
        assert_eq!(handler("/other").as_deref(), Some("C::m"));
    }

    #[test]
    fn an_unreadable_nest_refuses_each_registration_once_however_many_patterns_read_it() {
        let m = scanned(
            r#"
class R(private val h: H) {
    fun r() = router {
        BASE.nest {
            GET("/x", h::m)
            GET("/y", ::top)
            GET("/z") { 1 }
        }
    }
}
"#,
        );
        assert!(m.routes.is_empty(), "{:?}", m.routes);
        assert_eq!(m.refusals, [RouteRefusal::PathNotComposed; 3]);
    }

    #[test]
    fn a_verb_named_annotation_is_not_a_mapping_annotation() {
        // Retrofit's `@GET("users")` shares its name with the DSL verb the table
        // now maps, but only a `…Mapping` annotation is a Spring registration.
        let m = scanned(
            r#"
@RequestMapping("/v1")
interface Api {
    @GET("users") fun list(): Call
    @GET fun bare(): Call
    @get:JsonProperty("x") fun other(): Call
    @GetMapping("ok") fun real(): String
}
"#,
        );
        assert_eq!(names(&m), owned(&["GET /v1/ok"]));
    }

    #[test]
    fn a_path_that_is_not_a_literal_is_counted_never_guessed() {
        let m = scanned(
            r#"
fun routes() = router {
    GET(PATH) { ok().build() }
    (2012..2024).forEach { year -> GET("/$year/") { ok().build() } }
    GET("/${a}/b") { ok().build() }
    GET("/price$") { ok().build() }
}
"#,
        );
        assert_eq!(names(&m), owned(&["GET /price$"]), "a lone trailing dollar is a literal");
        assert_eq!(m.refusals, [RouteRefusal::PathNotFolded; 3]);
    }

    #[test]
    fn ktor_routes_carry_their_route_prefixes() {
        let m = scanned(
            r#"
import io.ktor.server.routing.*

fun Application.module() {
    routing {
        get { call.respondText("index") }
        get("/health") { call.respondText("ok") }
        route("/p") {
            get("/x") { call.respondText("x") }
            get { call.respondText("root") }
            route("/q", HttpMethod.Post) {
                post("/y") { }
                authenticate("auth") { delete("/z") { } }
            }
        }
        route("api") { put("v1/items") { } }
    }
}
"#,
        );
        assert_eq!(
            names(&m),
            owned(&[
                "DELETE /p/q/z",
                "GET /",
                "GET /health",
                "GET /p",
                "GET /p/x",
                "POST /p/q/y",
                "PUT /api/v1/items",
            ])
        );
        assert!(m.refusals.is_empty(), "{:?}", m.refusals);
    }

    #[test]
    fn a_ktor_route_prefix_that_is_not_a_literal_refuses_what_is_under_it() {
        let m = scanned(
            r#"
fun Application.module() {
    routing {
        route(BASE) { get("/x") { } }
        route("/ok") { get("/y") { } }
    }
}
"#,
        );
        assert_eq!(names(&m), owned(&["GET /ok/y"]));
        assert_eq!(m.refusals, [RouteRefusal::PathNotComposed]);
    }

    #[test]
    fn a_ktor_routing_extension_is_mounted_where_this_file_cannot_see() {
        let m = scanned(
            r#"
fun Route.userRoutes() {
    get("/users") { }
    route("/admin") { get("/stats") { } }
}
private fun Routing.other(): Unit { post("/o") { } }
fun Application.module() { routing { userRoutes() } }
fun plain(): Route { return r }
"#,
        );
        assert!(m.routes.is_empty(), "no route is promoted at a bare path: {:?}", m.routes);
        assert_eq!(m.refusals.len(), 3, "each registration is counted: {:?}", m.refusals);
    }

    #[test]
    fn spring_and_ktor_dsls_do_not_read_each_others_call_names() {
        // `GET` is Spring's, `get` Ktor's: a near miss in case is no verb.
        let m = scanned("fun r() = router { Get(\"/x\") { 1 }\n gets(\"/y\") { 2 }\n GET_(\"/z\") { 3 } }\n");
        assert!(m.routes.is_empty(), "{:?}", m.routes);
        assert!(m.refusals.is_empty(), "{:?}", m.refusals);
    }

    #[test]
    fn the_annotation_forms_are_unchanged_beside_the_dsl() {
        let m = scanned(
            r#"
@RestController
class C {
    @GetMapping("/users")
    fun list(): String { return "" }
}
"#,
        );
        assert_eq!(names(&m), owned(&["GET /users"]));
    }
}

/// Spring serves a mapping written without a leading slash from the root
/// (S-561, CR-178): `@GetMapping("vets.xml")` is `/vets.xml`, in Java and in
/// Kotlin alike — and every path that is not Spring's is left as written.
#[cfg(all(feature = "lang-java", feature = "lang-kotlin"))]
mod slash_less_mappings {
    use super::*;

    fn paths(ext: &str, source: &str) -> Vec<String> {
        let mut got: Vec<String> = scan_lang(ext, source).routes.into_iter().map(|r| r.path).collect();
        got.sort();
        got
    }

    /// A controller class `C` carrying `prefix` (a class-level annotation, or
    /// nothing) around `members` — the same text in either language.
    fn controller(members: &str, prefix: &str) -> String {
        format!("{prefix}\n@RestController\nclass C {{\n{members}\n}}\n")
    }

    #[test]
    fn a_method_path_without_a_slash_is_rooted_in_both_languages() {
        for (members, expected) in [
            (r#"@GetMapping("vets.xml") public String vets() { return ""; }"#, "/vets.xml"),
            (r#"@GetMapping(value = "owners") public String owners() { return ""; }"#, "/owners"),
            (r#"@GetMapping(path = {"a", "/b"}) public String ab() { return ""; }"#, "/a"),
        ] {
            let got = paths("java", &controller(members, ""));
            assert!(got.contains(&expected.to_string()), "java {members}: {got:?}");
        }
        let got = paths("kt", &controller(r#"@GetMapping("vets.xml") fun vets(): String { return "" }"#, ""));
        assert_eq!(got, ["/vets.xml"]);
        let got = paths("kt", &controller(r#"@GetMapping(value = ["a", "/b"]) fun ab(): String { return "" }"#, ""));
        assert_eq!(got, ["/a", "/b"]);
    }

    #[test]
    fn a_prefix_without_a_slash_is_rooted_and_joined_with_one_separator() {
        let in_java = paths("java", &controller(r#"@GetMapping("users") public String u() { return ""; }"#, r#"@RequestMapping("v1")"#));
        assert_eq!(in_java, ["/v1/users"]);
        let in_kotlin = paths("kt", &controller(r#"@GetMapping("users") fun u(): String { return "" }"#, r#"@RequestMapping("v1")"#));
        assert_eq!(in_kotlin, ["/v1/users"]);
    }

    #[test]
    fn every_prefix_form_without_a_slash_is_rooted() {
        for (prefix, expected) in [
            (r#"@RequestMapping(value = "v1")"#, "/v1/u"),
            (r#"@RequestMapping(path = C.BASE)"#, "/b/u"),
            (r#"@RequestMapping({"v1", "/v2"})"#, "/v1/u"),
        ] {
            let members = r#"@GetMapping("u") public String u() { return ""; }
    static final String BASE = "b";"#;
            let got = paths("java", &controller(members, prefix));
            assert!(got.contains(&expected.to_string()), "java {prefix}: {got:?}");
        }
        let members = r#"@GetMapping("u") fun u(): String { return "" }"#;
        assert_eq!(paths("kt", &controller(members, r#"@RequestMapping(value = "v1")"#)), ["/v1/u"]);
        assert_eq!(
            paths("kt", &controller(members, r#"@RequestMapping(["v1", "/v2"])"#)),
            ["/v1/u", "/v2/u"]
        );
    }

    #[test]
    fn a_folded_path_or_prefix_without_a_slash_is_rooted_too() {
        // Folded from constants, the path is judged as a written one is: Spring
        // serves `"api" + "/" + "users"` from `/api/users`, folded or written.
        let written = controller(
            r#"@GetMapping(value = ROOT + "/users") public String u() { return ""; }
    static final String ROOT = "api";"#,
            "",
        );
        assert_eq!(paths("java", &written), ["/api/users"]);
        let prefixed = controller(
            r#"@GetMapping("users") public String u() { return ""; }
    static final String BASE = "v";"#,
            r#"@RequestMapping(C.BASE + "2")"#,
        );
        assert_eq!(paths("java", &prefixed), ["/v2/users"]);
    }

    #[test]
    fn a_path_that_is_already_rooted_or_names_a_placeholder_is_left_as_written() {
        assert_eq!(
            paths("java", &controller(r#"@GetMapping("/users") public String u() { return ""; }"#, "")),
            ["/users"]
        );
        // The property may supply the slash: no leading one is guessed.
        assert_eq!(
            paths("java", &controller(r#"@GetMapping("${api.base}/x") public String u() { return ""; }"#, "")),
            ["${api.base}/x"]
        );
        assert_eq!(
            paths("kt", &controller(r#"@GetMapping("${api.base}/x") fun u(): String { return "" }"#, "")),
            ["${api.base}/x"]
        );
    }

    #[test]
    #[cfg(feature = "lang-python")]
    fn a_non_spring_relative_path_is_not_rooted() {
        // Django's URLconf is relative by design: the root URLconf's `include()`
        // supplies the prefix, so nothing here may give it a slash.
        let m = scan_lang("py", "from django.urls import path\nurlpatterns = [path(\"users/\", index)]\n");
        let got: Vec<&str> = m.routes.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(got, ["users/"]);
    }

    #[test]
    #[cfg(feature = "lang-python")]
    fn a_single_word_that_is_not_a_slash_less_path_in_any_other_position_stays_unpromoted() {
        // The CR-110 rule is untouched: a bare token the dialect does not root
        // is a property key, not a route.
        let m = scan_lang("py", "from fastapi import FastAPI\napp = FastAPI()\nx = state.get(\"active\")\n");
        assert!(m.routes.is_empty(), "{:?}", m.routes);
    }
}

// ── Rails routes: `draw`-scoped, `=>`/`to:` forms, prefixes, actions (S-567) ──

/// The sorted `(path, method, handler)` projection of a scanned Ruby file.
#[cfg(feature = "lang-ruby")]
fn rails_routes(source: &str) -> (Vec<(String, String, Option<String>)>, usize) {
    let m = scan_lang("rb", source);
    let mut got = route_triples_of(&m);
    got.sort();
    (got, m.refusals.len())
}

#[cfg(feature = "lang-ruby")]
fn route_triples_of(m: &FileMatches) -> Vec<(String, String, Option<String>)> {
    m.routes
        .iter()
        .map(|r| (r.path.clone(), r.method.clone(), r.handler.clone()))
        .collect()
}

#[cfg(feature = "lang-ruby")]
fn triple(path: &str, method: &str, handler: Option<&str>) -> (String, String, Option<String>) {
    (path.to_string(), method.to_string(), handler.map(str::to_string))
}

/// rack's three false routes were a test client's `get("/cgi/test", …)` calls:
/// a verb call is a route only inside a `routes.draw` block (S-567).
#[cfg(feature = "lang-ruby")]
#[test]
fn a_rails_verb_call_outside_a_draw_block_is_no_route() {
    let (got, refused) = rails_routes(
        r#"class FilesTest < Minitest::Test
  def test_cgi
    get("/cgi/test", 'HTTP_IF_MODIFIED_SINCE' => "x")
    post "/", to: "home#index"
  end
end
"#,
    );
    assert_eq!(got, vec![], "a test client's request is no route");
    assert_eq!(refused, 0, "and no refused one either");
}

/// lobsters' dominant shape is `get "/x" => "c#a"` (122 of its ~178 lines); the
/// `to:` and `:to =>` forms name the handler the same way, and a `redirect(…)`
/// handler is a route with no action (S-567).
#[cfg(feature = "lang-ruby")]
#[test]
fn rails_hash_rocket_and_to_forms_are_routes_naming_their_action() {
    let (got, refused) = rails_routes(
        r#"Rails.application.routes.draw do
  get "/rss" => "home#index", :format => "rss"
  post "/login" => "login#login"
  get "/page/:page", to: "home#index"
  get "/hottest", :to => "home#hottest"
  get("/c/:id", to: "comments#redirect_from_short_id", as: "c")
  patch "/x", as: "x", to: "story_urls#latest"
  get "/upvoted", to: redirect("/upvoted/stories")
  delete "/plain"
end
"#,
    );
    assert_eq!(
        got,
        vec![
            triple("/c/:id", "GET", Some("CommentsController::redirect_from_short_id")),
            triple("/hottest", "GET", Some("HomeController::hottest")),
            triple("/login", "POST", Some("LoginController::login")),
            triple("/page/:page", "GET", Some("HomeController::index")),
            triple("/plain", "DELETE", None),
            triple("/rss", "GET", Some("HomeController::index")),
            triple("/upvoted", "GET", None),
            triple("/x", "PATCH", Some("StoryUrlsController::latest")),
        ]
    );
    assert_eq!(refused, 0);
}

/// `root` is `GET` at the path the blocks around it give it — `/` directly
/// under `draw`, the namespace's path inside one — and its handler is written
/// as `to:` or positionally; the positional `"c#a"` is never a path (S-567).
#[cfg(feature = "lang-ruby")]
#[test]
fn rails_root_is_get_at_the_enclosing_path() {
    let (got, _) = rails_routes(
        r#"Rails.application.routes.draw do
  root to: "home#index", as: "root"
  namespace :admin do
    root "dashboard#show"
  end
end
"#,
    );
    assert_eq!(
        got,
        vec![
            triple("/", "GET", Some("HomeController::index")),
            triple("/admin", "GET", Some("Admin::DashboardController::show")),
        ]
    );
}

/// `namespace` prefixes both the path and the controller's module; `scope`
/// prefixes the path alone, and its `module:` the controller alone; a
/// controller written `a/b#c` names its own module (S-567).
#[cfg(feature = "lang-ruby")]
#[test]
fn rails_namespace_and_scope_prefix_paths_and_qualify_controllers() {
    let (got, refused) = rails_routes(
        r#"Rails.application.routes.draw do
  namespace :mod do
    get "flagged_stories/:period" => "flagged#flagged_stories"
    get "/" => "activities#index"
    get "admins" => "/admin/users#index"
    namespace "reports" do
      get "weekly" => "summaries#weekly"
    end
  end
  scope "/api", module: :v1 do
    post "items" => "items#create"
  end
  scope path: "/p" do
    get "q" => "admin/users#index"
  end
  scope module: "legacy" do
    get "/old" => "pages#show"
  end
  constraints id: /x/ do
    get "/domains/:id" => "home#for_domain"
  end
  namespace :staff_area, module: "staff" do
    get "shifts" => "rotas#index"
  end
end
"#,
    );
    assert_eq!(
        got,
        vec![
            triple("/api/items", "POST", Some("V1::ItemsController::create")),
            triple("/domains/:id", "GET", Some("HomeController::for_domain")),
            triple("/mod", "GET", Some("Mod::ActivitiesController::index")),
            triple("/mod/admins", "GET", Some("Admin::UsersController::index")),
            triple("/mod/flagged_stories/:period", "GET", Some("Mod::FlaggedController::flagged_stories")),
            triple("/mod/reports/weekly", "GET", Some("Mod::Reports::SummariesController::weekly")),
            triple("/old", "GET", Some("Legacy::PagesController::show")),
            triple("/p/q", "GET", Some("Admin::UsersController::index")),
            triple("/staff_area/shifts", "GET", Some("Staff::RotasController::index")),
        ]
    );
    assert_eq!(refused, 0);
}

/// `resources` stays unexpanded: the macro promotes nothing, and a
/// registration inside its block sits under the resource's unexpanded path, so
/// it is refused and counted rather than promoted at a guessed address (S-567,
/// NFR-RA-05).
#[cfg(feature = "lang-ruby")]
#[test]
fn rails_resources_stay_unexpanded_and_their_member_routes_are_refused() {
    let (got, refused) = rails_routes(
        r#"Rails.application.routes.draw do
  resources :stories, except: [:index] do
    post "upvote"
    get "/stories/:short_id", to: redirect("/s/%{short_id}")
  end
  resources :comments do
    member do
      get "/comments/:id" => "comments#redirect_from_short_id"
    end
  end
  resource :session
  get "/about" => "about#about"
end
"#,
    );
    assert_eq!(got, vec![triple("/about", "GET", Some("AboutController::about"))]);
    assert_eq!(refused, 3, "one per registration under an unexpanded resource");
}

/// A handler string that is not `controller#action` names no action — the
/// route keeps its node and no handler is guessed (S-567, NFR-RA-05).
#[cfg(feature = "lang-ruby")]
#[test]
fn a_rails_handler_that_is_not_controller_hash_action_names_nothing() {
    let (got, _) = rails_routes(
        r##"Rails.application.routes.draw do
  get "/a" => "home"
  get "/b" => "home#"
  get "/c" => "#index"
  get "/d" => "home#in dex"
  get "/e" => "home#index#x"
end
"##,
    );
    assert!(got.iter().all(|(_, _, handler)| handler.is_none()), "{got:?}");
    assert_eq!(got.len(), 5, "every route keeps its node: {got:?}");
}

/// A `namespace`, `scope` or `resources` block is a router prefix only inside
/// `routes.draw`: outside it — a routes file another one `draw`s, a Grape API,
/// a test — it establishes no route, refused or otherwise, and never promotes a
/// path missing its root (S-567).
#[cfg(feature = "lang-ruby")]
#[test]
fn a_rails_prefix_block_outside_a_draw_block_establishes_no_route() {
    let (got, refused) = rails_routes(
        r#"namespace :admin do
  get "stats" => "stats#index"
end
class FilesTest < Minitest::Test
  def test_cgi
    scope "/x" do
      get "/cgi" => "users#show"
    end
    resources :stories do
      post "upvote"
    end
  end
end
"#,
    );
    assert_eq!(got, vec![], "no `draw` encloses any of them");
    assert_eq!(refused, 0, "and nothing is refused either");
}

/// A `namespace` or `scope` whose first argument this file cannot read — a
/// quoted symbol, an `ENV` fallback, an element reference — is an opaque
/// prefix: its registrations are refused and counted, never promoted under the
/// enclosing path, and a namespace's handlers are never read without its module
/// (S-567, FR-FW-05, NFR-RA-05). An option first (`path:`, `module:`) is no
/// such argument.
#[cfg(feature = "lang-ruby")]
#[test]
fn a_rails_prefix_this_file_cannot_read_refuses_its_routes() {
    let (got, refused) = rails_routes(
        r#"Rails.application.routes.draw do
  namespace :"api/v1" do
    get "x" => "users#index"
  end
  scope ENV["RAILS_RELATIVE_URL_ROOT"] || "/" do
    get "y" => "users#index"
  end
  scope ENV["P"] do
    get "z" => "users#index"
  end
  namespace :admin, path: "secret" do
    get "a" => "users#index"
  end
  scope path: "/p", module: :v1 do
    get "b" => "users#index"
  end
  namespace :"ops/v2", path: "ops" do
    get "c" => "users#index"
  end
end
"#,
    );
    assert_eq!(
        got,
        vec![
            triple("/ops/c", "GET", None),
            triple("/p/b", "GET", Some("V1::UsersController::index")),
            triple("/secret/a", "GET", Some("Admin::UsersController::index")),
        ]
    );
    assert_eq!(refused, 3, "one per registration under an unreadable prefix");
}

/// A `path:` or `module:` option written as a value rather than a literal is
/// unreadable: the path refuses its registrations, and the module leaves every
/// handler under it naming nothing rather than a top-level controller (S-567,
/// FR-FW-05, NFR-RA-05).
#[cfg(feature = "lang-ruby")]
#[test]
fn a_rails_path_or_module_option_written_as_a_value_is_unreadable() {
    let (got, refused) = rails_routes(
        r#"Rails.application.routes.draw do
  scope Foo do
    get "a" => "users#index"
  end
  scope path: some_var do
    get "b" => "users#index"
  end
  scope module: mod_name do
    get "/c" => "users#index"
  end
end
"#,
    );
    assert_eq!(got, vec![triple("/c", "GET", None)]);
    assert_eq!(refused, 2, "the constant prefix and the `path:` value");
}

// ── A path of exactly `/` under a prefix (sprint-109 review) ──────────────────

/// The sorted `METHOD path` names of a snippet's routes, scanned through the
/// plugin registered for `ext`.
fn route_names_of(ext: &str, source: &str) -> Vec<String> {
    let mut got: Vec<String> = scan_lang(ext, source)
        .routes
        .into_iter()
        .map(|r| format!("{} {}", r.method, r.path))
        .collect();
    got.sort();
    got
}

/// Laravel trims both sides of a group join, so `Route::get('/')` in a
/// `users` group is served at `/users` — the key an OpenAPI operation or an
/// HTTP-client call on `/users` joins — whichever way the prefix was written
/// and however deep the group nests (`@fw.route.prefix.absorbs`).
#[cfg(feature = "lang-php")]
#[test]
fn a_laravel_root_route_in_a_group_is_the_group_prefix() {
    let got = route_names_of(
        "php",
        "<?php
Route::prefix('users')->group(function () {
    Route::get('/', [UserController::class, 'index']);
    Route::get('{id}', [UserController::class, 'show']);
});
Route::prefix('/admin/')->group(function () {
    Route::get('/', [AdminController::class, 'index']);
    Route::prefix('audit')->group(fn () => Route::get('/', [AuditController::class, 'index']));
});
Route::group(['prefix' => 'teams'], function () {
    Route::post('/', [TeamController::class, 'store']);
});
",
    );
    assert_eq!(got, ["GET /admin", "GET /admin/audit", "GET /users", "GET /users/{id}", "POST /teams"]);
}

/// The control in the same language: Symfony does not trim, so a method
/// `#[Route('/')]` under a class `#[Route('/blog')]` keeps the `/blog/` Symfony
/// serves — the PHP opt-in is Laravel's groups, not the language's.
#[cfg(feature = "lang-php")]
#[test]
fn a_symfony_root_route_under_a_class_prefix_keeps_its_trailing_slash() {
    let got = route_names_of(
        "php",
        "<?php
use Symfony\\Component\\Routing\\Attribute\\Route;

#[Route('/blog')]
class BlogController extends AbstractController
{
    #[Route('/', methods: ['GET'])]
    public function index(): Response {}
}
",
    );
    assert_eq!(got, ["GET /blog/"]);
}

/// NestJS strips the end slash of every non-root path, so `@Get('/')` and a
/// bare `@Get()` under `@Controller('users')` are one endpoint and one key —
/// also under a prefix written `'teams/'` — in both TypeScript grammars and in
/// JavaScript (which runs the `tsx` query). Each form is scanned in a file of its
/// own: one file promotes one route per key.
#[cfg(feature = "lang-typescript")]
#[test]
fn a_nestjs_root_route_and_a_pathless_one_name_the_same_endpoint() {
    let controller = |prefix: &str, decorator: &str| {
        format!(
            "import {{ Controller, Get }} from '@nestjs/common';
@Controller({prefix})
export class UsersController {{
  {decorator}
  list() {{}}
}}
"
        )
    };
    for ext in ["ts", "tsx", "js"] {
        for (prefix, want) in [("'users'", "GET /users"), ("'teams/'", "GET /teams"), ("", "GET /")] {
            for decorator in ["@Get('/')", "@Get()"] {
                assert_eq!(
                    route_names_of(ext, &controller(prefix, decorator)),
                    [want],
                    "{ext}: @Controller({prefix}) {decorator}"
                );
            }
        }
    }
}

/// Rails' `normalize_path` drops a trailing slash, so `get '/'` inside a
/// `scope` or a `namespace` is served at the block's own path.
#[cfg(feature = "lang-ruby")]
#[test]
fn a_rails_root_route_in_a_scope_or_namespace_is_the_block_path() {
    let got = route_names_of(
        "rb",
        r#"Rails.application.routes.draw do
  get "/" => "home#index"
  scope '/admin' do
    get '/', to: 'dash#index'
    namespace :reports do
      get "/" => "reports#index"
    end
  end
  namespace :mod do
    get "/" => "activities#index"
    get "x" => "activities#x"
  end
end
"#,
    );
    assert_eq!(got, ["GET /", "GET /admin", "GET /admin/reports", "GET /mod", "GET /mod/x"]);
}

/// The control: Spring serves `@GetMapping("/")` under `@RequestMapping("/v1")`
/// at `/v1/`, a different endpoint from the bare `@GetMapping`'s `/v1`, so the
/// Java query states no absorption and both keys stay.
#[cfg(feature = "lang-java")]
#[test]
fn a_spring_root_route_under_a_class_prefix_keeps_its_trailing_slash() {
    let got = route_names_of(
        "java",
        r#"@RestController
@RequestMapping("/v1")
class V1Controller {
    @GetMapping("/")
    String slash() { return ""; }
    @GetMapping
    String bare() { return ""; }
}
"#,
    );
    assert_eq!(got, ["GET /v1", "GET /v1/"]);
}
