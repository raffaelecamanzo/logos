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
        let mut got = route_triples(scan_lang("java", source));
        got.sort();
        got
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

    #[test]
    fn class_level_prefixes_are_not_composed_yet() {
        // S-329 owns prefix composition; this task promotes the method path
        // verbatim and the class-level annotation promotes nothing on its own.
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
                "/users".to_string(),
                "GET".to_string(),
                Some("listUsers".to_string())
            )]
        );
    }
}
