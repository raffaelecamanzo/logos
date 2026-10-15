//! Readers of the `logos` binary's user-visible surface, shared by the suites
//! that assert it: `surface_snapshots.rs` (the default and `agents` builds against
//! the Sprint 96 baseline, S-639) and `no_extension_surface.rs` (the build with no
//! extension, S-645). One reader per surface, so the two suites can never disagree
//! about what a route, a tool or a help line is.
//!
//! Each reader takes the [`ExtensionHost`] to compose, so a suite reads the very
//! host its build's `main` composes (`cli/src/composition.rs`).

// Each suite includes this file as a module of its own and calls a subset.
#![allow(dead_code)]

use std::process::Command;
use std::sync::Arc;

use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
};
use http_body_util::BodyExt;
use logos_core::federation::{Backing, EngineRegistry, Federation, RegistryMode};
use logos_core::Engine;
use logos_ext::web::WebContext;
use logos_ext::ExtensionHost;
use mcp::LogosMcp;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

/// The web adapter's source, for the route-table scan ([`core_route_census`]).
const WEB_LIB: &str = include_str!("../../../web/src/lib.rs");

// ── JSON rendering ─────────────────────────────────────────────────────────

/// `value` with every object's keys sorted, recursively. Whether `serde_json`
/// keeps insertion order depends on which crates the feature resolver unified
/// into this build, so a snapshot of raw output would differ between `cargo
/// test -p logos` and `cargo test --workspace` for no surface reason.
pub fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            Value::Object(keys.into_iter().map(|k| (k.clone(), canonical(&map[k]))).collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

pub fn pretty(value: &Value) -> String {
    let mut out = serde_json::to_string_pretty(&canonical(value)).expect("serialisable");
    out.push('\n');
    out
}

// ── The web route census and POST allow-list ───────────────────────────────

/// One mounted route: its path as the router declares it, and its methods.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RouteEntry {
    pub path: String,
    pub methods: Vec<&'static str>,
}

/// Non-comment production code of `source`: everything before its unit-test
/// module, with `//` comments stripped (the web crate's own route-table scan
/// reads it the same way).
fn production_code(source: &str) -> String {
    let code = source
        .split_once("\n#[cfg(test)]\nmod tests {")
        .map_or(source, |(before, _)| before);
    code.lines()
        .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The argument list of the call whose opening parenthesis ends `rest`'s prefix.
fn call_args(rest: &str) -> &str {
    let mut depth = 0usize;
    let end = rest
        .char_indices()
        .find(|&(_, c)| match c {
            '(' => {
                depth += 1;
                false
            }
            ')' if depth == 0 => true,
            ')' => {
                depth -= 1;
                false
            }
            _ => false,
        })
        .map(|(i, _)| i)
        .expect("every `.route(` call closes");
    &rest[..end]
}

/// The route table `build_router` declares — the same in every build.
///
/// Read from the web adapter's source: the router answers requests but cannot
/// list its routes. `build_router` carried an `agents`-only block until the wiki
/// generator's route — its last entry — became the `wiki` extension's (S-644);
/// every feature-dependent route now reaches the census through the composed
/// host instead ([`composed_route_census`]). A `#[cfg(` back inside the body would
/// make this scan count a route some build does not compile, so it is refused.
pub fn core_route_census() -> Vec<RouteEntry> {
    let code = production_code(WEB_LIB);
    let body_at = code.find("fn build_router(").expect("web declares build_router");
    let body = &code[body_at..];
    let body = &body[..body.find("\n}\n").expect("build_router closes")];
    assert!(
        !body.contains("#[cfg("),
        "build_router compiles a feature-gated route again — this census counts every \
         `.route(` in it, so it would claim a route some build does not have"
    );
    // The census reads `.route(` calls only, so a route mounted any other way would be
    // served yet never counted (S-645 review). Every other way in is refused: axum's
    // merging/nesting calls, and a helper handed the router — except
    // `mount_extension_routes`, whose routes are the host's, counted below.
    for refused in [".merge(", ".nest(", ".nest_service(", ".route_service(", "Router::route("] {
        assert!(!body.contains(refused), "build_router mounts routes through `{refused}`, which this census cannot read");
    }
    for (at, marker) in body.match_indices("(router") {
        if body[at + marker.len()..].chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let callee: String =
            body[..at].chars().rev().take_while(|c| c.is_alphanumeric() || *c == '_').collect::<Vec<_>>().into_iter().rev().collect();
        assert!(
            callee.is_empty() || callee == "mount_extension_routes",
            "build_router hands the router to `{callee}`, whose routes this census cannot read"
        );
    }

    let mut routes = Vec::new();
    for (at, marker) in body.match_indices(".route(") {
        let args = call_args(&body[at + marker.len()..]);
        let first = args.split(',').next().expect("a path argument").trim();
        let path = match first.strip_prefix('"') {
            Some(literal) => literal.trim_end_matches('"').to_string(),
            None => {
                let decl = format!("const {first}: &str = \"");
                let from = code
                    .find(&decl)
                    .unwrap_or_else(|| panic!("route constant `{first}` is declared in web"));
                let tail = &code[from + decl.len()..];
                tail[..tail.find('"').expect("a closing quote")].to_string()
            }
        };
        let handler = &args[first.len()..];
        let mut methods = Vec::new();
        if handler.contains("get(") {
            methods.push("GET");
        }
        if handler.contains("post(") {
            methods.push("POST");
        }
        assert!(!methods.is_empty(), "route `{path}` mounts neither get nor post: {args}");
        routes.push(RouteEntry { path, methods });
    }
    assert!(routes.len() > 30, "the route scan found only {} routes", routes.len());
    routes.sort();
    routes
}

/// A concrete request path for a route pattern: `:param` → `1`, `*rest` → `x`.
pub fn probe_path(pattern: &str) -> String {
    pattern
        .split('/')
        .map(|seg| match seg.chars().next() {
            Some(':') => "1",
            Some('*') => "x",
            _ => seg,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Does the router's method guard admit a `POST` to `path`? Asked through the
/// real middleware stack with no `Origin`: an admitted `POST` reaches the
/// intent guard and is refused `403` as cross-origin before any handler runs,
/// and a refused one is answered `405` by the method guard itself. So the probe
/// writes nothing and calls no provider.
pub async fn post_admitted(router: &axum::Router, path: &str) -> bool {
    let request = Request::builder()
        .method(Method::POST)
        .uri(path)
        .body(Body::empty())
        .expect("a request");
    let status = router.clone().oneshot(request).await.expect("the router answers").status();
    match status {
        StatusCode::FORBIDDEN => true,
        StatusCode::METHOD_NOT_ALLOWED => false,
        other => panic!("POST {path} answered {other}: neither the method nor the intent guard"),
    }
}

/// The route table a router built over `host` serves: the core table, plus each
/// active extension's routes, a path's methods merged.
pub fn composed_route_census(host: &ExtensionHost, engine: &Arc<Engine>) -> Vec<RouteEntry> {
    let mut routes = core_route_census();
    let context = WebContext {
        engine: Arc::clone(engine),
        backing: Arc::new(Backing::Single(Arc::clone(engine))),
    };
    for route in host.web_routes(&context) {
        let method = route.method().as_str();
        match routes.iter_mut().find(|r| r.path == route.path()) {
            Some(entry) => entry.methods.push(method),
            None => routes.push(RouteEntry { path: route.path().to_string(), methods: vec![method] }),
        }
    }
    for route in &mut routes {
        route.methods.sort_unstable();
    }
    routes.sort();
    routes
}

/// The census and the `POST` allow-list a router over `host` serves, the
/// allow-list observed through the router's own guards for every census path.
pub async fn composed_web_surface(host: &ExtensionHost) -> (Vec<RouteEntry>, Vec<String>) {
    let (_dir, engine) = bare_engine();
    let census = composed_route_census(host, &engine);
    let backing = Arc::new(Backing::Single(Arc::clone(&engine)));
    let router = web::hosted_router(backing, web::IntentToken::generate(), host)
        .expect("the router builds");
    let mut admitted = Vec::new();
    for route in &census {
        if post_admitted(&router, &probe_path(&route.path)).await {
            admitted.push(route.path.clone());
        }
    }
    // Every POST-mounted route is admitted, and nothing else is: the allow-list
    // is a property of the composed router, observed rather than restated.
    let mounted_posts: Vec<&String> =
        census.iter().filter(|r| r.methods.contains(&"POST")).map(|r| &r.path).collect();
    assert_eq!(admitted.iter().collect::<Vec<_>>(), mounted_posts);
    (census, admitted)
}

pub fn render_census(routes: &[RouteEntry]) -> String {
    let mut out = String::new();
    for route in routes {
        out.push_str(&format!("{:<9}{}\n", route.methods.join(" "), route.path));
    }
    out
}

/// A started engine over an empty, un-indexed project — enough for every surface
/// here, none of which reads the graph.
pub fn bare_engine() -> (TempDir, Arc<Engine>) {
    let dir = TempDir::new().expect("a temp project");
    let engine = Arc::new(Engine::start(dir.path()).expect("the engine starts"));
    (dir, engine)
}

// ── MCP tools/list ──────────────────────────────────────────────────────────

/// The roster as `tools/list` serialises it, sorted by name. Generic over the
/// tool type so this crate names no MCP SDK version of its own.
pub fn render_tools<T: serde::Serialize>(tools: Vec<T>) -> String {
    let mut tools: Vec<Value> =
        tools.into_iter().map(|t| serde_json::to_value(t).expect("a tool serialises")).collect();
    tools.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    pretty(&Value::Array(tools))
}

/// `tools/list` of an MCP server composed over `host`, rendered: single-root, then
/// federated (over an empty workspace — the roster does not depend on members).
pub fn mcp_rosters(host: &ExtensionHost) -> (String, String) {
    let (_dir, engine) = bare_engine();
    let single = render_tools(LogosMcp::new(engine, host).list_tools());

    let root = TempDir::new().expect("a temp workspace");
    let federation = Federation {
        name: "snapshot".to_string(),
        root: root.path().to_path_buf(),
        members: Vec::new(),
        default: None,
        links: Vec::new(),
        governance: Default::default(),
        warm_concurrency: None,
        member_kinds: Default::default(),
        exclusion: Default::default(),
    };
    let registry = EngineRegistry::<Engine>::new(federation, RegistryMode::Lazy);
    let federated = render_tools(LogosMcp::federated(registry, host).list_tools());
    (single, federated)
}

// ── CLI help ────────────────────────────────────────────────────────────────

pub fn help(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_logos"))
        .args(args)
        .output()
        .expect("the logos binary runs");
    assert!(out.status.success(), "logos {args:?} exits 0");
    String::from_utf8(out.stdout).expect("help is UTF-8")
}

// ── The config read-model ───────────────────────────────────────────────────

/// A project whose policy carries a `[chat]` and a `[wiki]` table and whose
/// secrets carry the chat key — the tables CR-213 gives to extensions, so the
/// read-model snapshot covers exactly what that move must not change.
///
/// The files are written here rather than by `logos init`, whose starter
/// templates the read-model echoes verbatim: a reworded template comment is not
/// a surface change this baseline should fail on.
pub fn configured_project() -> TempDir {
    let dir = TempDir::new().expect("a temp project");
    let logos = dir.path().join(".logos");
    std::fs::create_dir_all(&logos).expect("create .logos");
    std::fs::write(
        logos.join("config.toml"),
        "# surface-snapshot fixture\n\n[chat]\nmodel = \"snapshot-chat-model\"\n\n\
         [wiki]\nmodel = \"snapshot-wiki-model\"\n",
    )
    .expect("write the config");
    std::fs::write(logos.join("rules.toml"), "[constraints]\nmax_cycles = 0\n")
        .expect("write the rules");
    std::fs::write(logos.join("secrets.toml"), "[chat]\napi_key = \"sk-snapshot-key-0123\"\n")
        .expect("write the secrets");
    dir
}

/// `GET <uri>` on a router composed over `host` for the project at `root`: the
/// status and the body as JSON, with the project's location replaced by `<ROOT>`
/// (a read-model may name it; a snapshot must not).
pub async fn get_json(host: &ExtensionHost, root: &std::path::Path, uri: &str) -> (StatusCode, Value) {
    let engine = Arc::new(Engine::start(root).expect("the engine starts"));
    let backing = Arc::new(Backing::Single(engine));
    let response = web::hosted_router(backing, web::IntentToken::generate(), host)
        .expect("the router builds")
        .oneshot(Request::get(uri).body(Body::empty()).expect("a request"))
        .await
        .expect("the router answers");
    let status = response.status();
    let bytes = response.into_body().collect().await.expect("a body").to_bytes();
    let mut text = String::from_utf8(bytes.to_vec()).expect("UTF-8 JSON");
    for root in [root.to_path_buf(), root.canonicalize().expect("canonical root")] {
        text = text.replace(&root.display().to_string(), "<ROOT>");
    }
    (status, serde_json::from_str(&text).expect("the body is JSON"))
}

