//! ASP.NET Core routes (S-554, [CR-177], [FR-FW-03], [FR-FW-01], [FR-FW-05],
//! [NFR-RA-05]) — end to end through the real [`Engine`] over the embedded C#
//! grammar, the C# framework query and its descriptor.
//!
//! What a C# file yields, by acceptance criterion:
//!
//! 1. **minimal APIs** — `app.MapGet("/p", Handler)` is a route, and one
//!    registered on a `MapGroup` variable (or straight on the call) is the
//!    group's prefix joined to its path, through any chain of builder calls
//!    (`.HasApiVersion(1.0)`) and nested groups;
//! 2. **controllers** — a class `[Route("api/[controller]")]` prefix is
//!    composed onto each action's `[HttpGet]` path, `[controller]` and
//!    `[action]` expanded, an absolute action path (`[HttpGet("/x")]`)
//!    ignoring it;
//! 3. **admission** — a file that relies on a `global using`, or on the
//!    implicit usings of the SDK's web project, is a candidate with no
//!    `using Microsoft.AspNetCore…` of its own;
//! 4. **never fabricated** — a registration on a group whose prefix is not
//!    written, or on two groups of one scope, never promotes a partial path.
//!
//! [CR-177]: ../../docs/requests/CR-177-csharp-calls-members-routes-partials-and-typed-receivers-resolve.md
//! [FR-FW-01]: ../../docs/specs/requirements/FR-FW-01.md
//! [FR-FW-03]: ../../docs/specs/requirements/FR-FW-03.md
//! [FR-FW-05]: ../../docs/specs/requirements/FR-FW-05.md
//! [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
#![cfg(feature = "lang-c-sharp")]

use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId, NodeKind};
use logos_core::{Engine, Runtime};

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    fs::write(path, contents).expect("write fixture");
}

/// Index `files` and return the engine with its framework stats.
fn index(files: &[(&str, &str)]) -> (tempfile::TempDir, Engine, logos_core::models::pipeline::FrameworkStats) {
    let tmp = tempfile::tempdir().unwrap();
    for (rel, contents) in files {
        write(tmp.path(), rel, contents);
    }
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let stats = engine.index().framework;
    (tmp, engine, stats)
}

fn nodes_of(rt: &Runtime, kind: NodeKind) -> Vec<(NodeId, String)> {
    rt.submit_read(move |store| {
        Ok(store
            .all_nodes()?
            .into_iter()
            .filter(|n| n.kind == kind)
            .map(|n| (n.id, n.name))
            .collect())
    })
    .expect("read runs")
}

/// Every promoted route's name, sorted.
fn route_names(rt: &Runtime) -> Vec<String> {
    let mut names: Vec<String> = nodes_of(rt, NodeKind::Route).into_iter().map(|(_, n)| n).collect();
    names.sort();
    names
}

/// Every `RoutesTo` edge as `(route name, handler name)`, sorted.
fn route_handlers(rt: &Runtime) -> Vec<(String, String)> {
    let names: std::collections::HashMap<NodeId, String> = nodes_of(rt, NodeKind::Route)
        .into_iter()
        .chain(nodes_of(rt, NodeKind::Method))
        .collect();
    let mut pairs: Vec<(String, String)> = rt
        .submit_read(|store| {
            Ok(store
                .all_edges()?
                .into_iter()
                .filter(|e| e.kind == EdgeKind::RoutesTo)
                .map(|e| (e.source, e.target))
                .collect::<Vec<_>>())
        })
        .expect("read runs")
        .into_iter()
        .map(|(route, handler)| (names[&route].clone(), names[&handler].clone()))
        .collect();
    pairs.sort();
    pairs
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

// ── Minimal APIs ─────────────────────────────────────────────────────────────

/// eShop's own shape (`Catalog.API/Apis/CatalogApi.cs`): groups whose prefix is
/// written without a leading slash, a builder chain after `MapGroup`, a nested
/// group, handlers as method groups, a lambda handler and an ungrouped route.
const CATALOG_API: &str = r#"
using Microsoft.AspNetCore.Http.HttpResults;

public static class CatalogApi
{
    public static IEndpointRouteBuilder MapCatalogApi(this IEndpointRouteBuilder app)
    {
        var vApi = app.NewVersionedApi("Catalog");
        var api = vApi.MapGroup("api/catalog").HasApiVersion(1, 0).HasApiVersion(2, 0);
        var v1 = vApi.MapGroup("api/catalog").HasApiVersion(1, 0);
        var admin = api.MapGroup("/admin");

        v1.MapGet("/items", GetAllItemsV1);
        api.MapGet("/items/{id:int}", GetItemById);
        api.MapPost("/items", CreateItem);
        admin.MapDelete("/items/{id}", DeleteItem);
        admin.MapGet("/", () => "admin root");
        app.MapGet("/", () => "catalog");
        return app;
    }

    public static string GetAllItemsV1() => "";
    public static string GetItemById(int id) => "";
    public static string CreateItem() => "";
    public static string DeleteItem(int id) => "";
}
"#;

#[test]
fn minimal_api_registrations_become_routes_with_their_group_prefixes() {
    let (_tmp, engine, stats) = index(&[("Apis/CatalogApi.cs", CATALOG_API)]);
    let rt = engine.runtime().unwrap();

    assert_eq!(
        route_names(rt),
        names(&[
            "DELETE /api/catalog/admin/items/{id}",
            "GET /",
            "GET /api/catalog/admin",
            "GET /api/catalog/items",
            "GET /api/catalog/items/{id:int}",
            "POST /api/catalog/items",
        ]),
        "every registration is a route at its group's prefix; a path written without \
         its leading slash is rooted, as ASP.NET Core serves it, and a group's own root \
         (`\"/\"`) is the group, not a trailing slash after it"
    );
    assert_eq!(stats.routes, 6);
    assert_eq!(stats.routes_not_composed, 0);

    // The handler is the method group, proven by the binder; a lambda has none.
    let handlers = route_handlers(rt);
    assert!(handlers.contains(&("GET /api/catalog/items".to_string(), "GetAllItemsV1".to_string())));
    assert!(handlers.contains(&("POST /api/catalog/items".to_string(), "CreateItem".to_string())));
    assert!(!handlers.iter().any(|(route, _)| route == "GET /"));
}

#[test]
fn a_route_registered_on_a_group_call_directly_takes_its_prefix() {
    let (_tmp, engine, stats) = index(&[(
        "Program.cs",
        r#"
using Microsoft.AspNetCore.Builder;

var app = WebApplication.Create(args);
app.MapGroup("/api").MapGet("/ping", Ping);
app.MapGroup("/api").RequireAuthorization().MapPost("/echo", Echo);
app.Run();

static string Ping() => "pong";
static string Echo() => "";
"#,
    )]);
    assert_eq!(route_names(engine.runtime().unwrap()), names(&["GET /api/ping", "POST /api/echo"]));
    assert_eq!(stats.routes_not_composed, 0);
}

#[test]
fn two_groups_of_one_scope_never_lend_each_other_their_prefix() {
    let (_tmp, engine, _) = index(&[(
        "Apis.cs",
        r#"
using Microsoft.AspNetCore.Builder;

public static class Apis
{
    public static void Map(WebApplication app)
    {
        var a = app.MapGroup("/a");
        var b = app.MapGroup("/b");
        a.MapGet("/x", X);
        b.MapGet("/y", Y);
    }
    static string X() => "";
    static string Y() => "";
}
"#,
    )]);
    assert_eq!(route_names(engine.runtime().unwrap()), names(&["GET /a/x", "GET /b/y"]));
}

#[test]
fn a_group_whose_prefix_is_not_written_promotes_nothing_and_is_counted() {
    let (_tmp, engine, stats) = index(&[(
        "Apis.cs",
        r#"
using Microsoft.AspNetCore.Builder;

public static class Apis
{
    // A group handed in: its prefix is the caller's, not stated here.
    public static void FromParameter(RouteGroupBuilder group)
    {
        group.MapGet("/x", X);
    }
    // A group whose prefix is a constant the pass does not fold in a minimal API.
    public static void Computed(WebApplication app, string prefix)
    {
        var g = app.MapGroup(prefix);
        g.MapGet("/y", X);
    }
    // A group a helper builds.
    public static void Helper(WebApplication app)
    {
        var g = Build(app);
        g.MapGet("/z", X);
    }
    // The one registration whose address is stated.
    public static void Stated(WebApplication app)
    {
        app.MapGet("/ok", X);
    }
    static string X() => "";
    static RouteGroupBuilder Build(WebApplication app) => app.MapGroup("/h");
}
"#,
    )]);
    assert_eq!(route_names(engine.runtime().unwrap()), names(&["GET /ok"]));
    assert_eq!(stats.routes_not_composed, 3, "each refused registration is counted once");
}

/// One method body in a candidate file; the routes it promotes and the
/// registrations it refuses.
fn body_routes(body: &str) -> (Vec<String>, u64) {
    let source = format!(
        "using Microsoft.AspNetCore.Builder;\n\npublic static class Apis\n{{\n    \
         static RouteGroupBuilder Field;\n    \
         public static void M(WebApplication app, object o, int k, System.Collections.Generic.IEnumerable<RouteGroupBuilder> gs)\n    {{\n{body}\n    }}\n}}\n"
    );
    let (_tmp, engine, stats) = index(&[("Apis.cs", &source)]);
    (route_names(engine.runtime().unwrap()), stats.routes_not_composed)
}

/// A receiver nothing binds is not a root: every other way C# introduces a name
/// (a member, a pattern variable, a loop variable, a `using`, an `out` variable,
/// a switch section, a declaration with no initializer) is refused and counted,
/// never promoted at the bare path.
#[test]
fn a_receiver_the_query_does_not_bind_is_refused_never_promoted_bare() {
    for body in [
        "Field.MapGet(\"/x\", () => 1);",
        "switch (k) { case 1: var g = app.MapGroup(\"/a\"); g.MapGet(\"/x\", () => 1); break; }",
        "using (var g = app.MapGroup(\"/a\")) g.MapGet(\"/x\", () => 1);",
        "RouteGroupBuilder g; g = app.MapGroup(\"/a\"); g.MapGet(\"/x\", () => 1);",
        "foreach (var g in gs) g.MapGet(\"/x\", () => 1);",
        "if (o is RouteGroupBuilder g) g.MapGet(\"/x\", () => 1);",
        "var g = app.MapOrders(); g.MapGet(\"/{id}\", () => 1);",
        "var g = Groups.Orders(app); g.MapGet(\"/{id}\", () => 1);",
        "app.MapOrders().MapGet(\"/{id}\", () => 1);",
        "var g = app.MapGroup(\"/a\"); g = app.MapGroup(\"/b\"); g.MapGet(\"/x\", () => 1);",
        "var g = app.MapGroup(\"/a\"); if (k > 0) { g = app.MapGroup(\"/b\"); } g.MapGet(\"/x\", () => 1);",
    ] {
        let (routes, refused) = body_routes(body);
        assert!(routes.is_empty(), "{body}: promoted {routes:?}");
        assert_eq!(refused, 1, "{body}: refused");
    }
}

/// The same call shapes where the receiver IS provably a plain builder, so the
/// refusals above are the rule and not a blanket.
#[test]
fn a_receiver_the_query_proves_a_plain_builder_still_promotes() {
    let (routes, refused) = body_routes("app.MapGet(\"/x\", () => 1); var b = WebApplication.CreateBuilder(); b.Build().MapGet(\"/y\", () => 1);");
    assert_eq!(routes, names(&["GET /x", "GET /y"]));
    assert_eq!(refused, 0);
}

/// A group kept in a field of a type declared after top-level statements does not
/// see the top-level local of the same name — the field is declared after its use,
/// and a member is in scope across the whole type.
#[test]
fn a_top_level_local_does_not_leak_into_a_later_type() {
    let (_tmp, engine, stats) = index(&[(
        "Program.cs",
        r#"
using Microsoft.AspNetCore.Builder;

var app = WebApplication.Create(args);
var api = app.MapGroup("/a");
api.MapGet("/x", () => 1);

public class Later
{
    void M() { api.MapGet("/y", () => 1); }
    RouteGroupBuilder api;
}
"#,
    )]);
    assert_eq!(route_names(engine.runtime().unwrap()), names(&["GET /a/x"]));
    assert_eq!(stats.routes_not_composed, 1);
}

#[test]
fn a_map_call_on_something_that_is_not_an_endpoint_builder_is_not_a_route() {
    // `Map` with a path-shaped string on an unrelated receiver, in a file that
    // is not an ASP.NET Core candidate at all.
    let (_tmp, engine, stats) = index(&[(
        "Mapper.cs",
        r#"
using System.Collections.Generic;

public class Mapper
{
    private readonly Registry _registry = new();

    public void Run()
    {
        _registry.MapGet("/not/a/route", Handle);
    }
    void Handle() {}
}
"#,
    )]);
    assert!(route_names(engine.runtime().unwrap()).is_empty());
    assert_eq!(stats.routes, 0);
    assert_eq!(stats.routes_not_composed, 0, "the file is no candidate: nothing is scanned, so nothing is refused");
}

// ── Controllers ──────────────────────────────────────────────────────────────

const CATALOG_CONTROLLER: &str = r#"
using Microsoft.AspNetCore.Mvc;

[ApiController]
[Route("api/[controller]")]
public class CatalogController : ControllerBase
{
    [HttpGet]
    public string List() => "";

    [HttpGet("{id}")]
    public string Get(int id) => "";

    [HttpPost("[action]")]
    public string Create() => "";

    [HttpGet("/health")]
    public string Health() => "";

    [HttpDelete("items/{id:int}")]
    public string Remove(int id) => "";
}

[Route("plain")]
public class StatusController : ControllerBase
{
    [HttpGet("up")]
    public string Up() => "";
}

public class OrphanController : ControllerBase
{
    [HttpGet("/orphan")]
    public string Orphan() => "";
}
"#;

#[test]
fn controller_routes_compose_their_class_route_prefix() {
    let (_tmp, engine, stats) = index(&[("Controllers/CatalogController.cs", CATALOG_CONTROLLER)]);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        route_names(rt),
        names(&[
            "DELETE /api/Catalog/items/{id:int}",
            "GET /api/Catalog",
            "GET /api/Catalog/{id}",
            "GET /health",
            "GET /orphan",
            "GET /plain/up",
            "POST /api/Catalog/Create",
        ]),
        "[controller] is the class name without its suffix, [action] the method's; an \
         absolute action path ignores the class prefix; a bare [HttpGet] is the prefix"
    );
    assert_eq!(stats.routes_not_composed, 0);
    let handlers = route_handlers(rt);
    assert!(handlers.contains(&("GET /api/Catalog".to_string(), "List".to_string())));
    assert!(handlers.contains(&("GET /plain/up".to_string(), "Up".to_string())));
}

/// The default MVC template `api/[controller]/[action]`: `[action]` is each
/// action's own name (`Async` trimmed), the class prefix being shared.
#[test]
fn an_action_token_in_the_class_prefix_is_each_actions_own_name() {
    let (_tmp, engine, stats) = index(&[(
        "Controllers/CatalogController.cs",
        r#"
using Microsoft.AspNetCore.Mvc;

[Route("api/[controller]/[action]")]
public class CatalogController : ControllerBase
{
    [HttpGet("{id}")]
    public string Get(int id) => "";

    [HttpGet]
    public string ListAsync() => "";
}
"#,
    )]);
    assert_eq!(
        route_names(engine.runtime().unwrap()),
        names(&["GET /api/Catalog/Get/{id}", "GET /api/Catalog/List"])
    );
    assert_eq!(stats.routes_not_composed, 0);
}

/// A token this pass cannot expand is no URL path: the route is refused and
/// counted, never promoted with the bracketed text in its name.
#[test]
fn a_token_left_unexpanded_or_an_action_renamed_refuses_the_route() {
    let (_tmp, engine, stats) = index(&[(
        "Controllers/QController.cs",
        r#"
using Microsoft.AspNetCore.Mvc;

[Route("api/[area]/[controller]")]
public class QController : ControllerBase
{
    [HttpGet("x")]
    public string A() => "";
}

[Route("api")]
public class RController : ControllerBase
{
    [HttpGet("[controller]/x")]
    public string B() => "";

    [HttpGet("[action]")]
    [ActionName("Other")]
    public string Foo() => "";

    [HttpGet("fine")]
    public string Fine() => "";
}
"#,
    )]);
    assert_eq!(route_names(engine.runtime().unwrap()), names(&["GET /api/fine"]));
    assert_eq!(stats.routes_not_composed, 3);
}

/// A verb attribute beside a method-level `[Route]` is addressed by both; the
/// verb's own (absent) path is not the route. A `[Route]` alone is still one.
#[test]
fn a_verb_beside_a_method_level_route_is_refused_and_a_route_alone_is_not() {
    let (_tmp, engine, stats) = index(&[(
        "Controllers/MixedController.cs",
        r#"
using Microsoft.AspNetCore.Mvc;

[Route("api/mixed")]
public class MixedController : ControllerBase
{
    [HttpGet]
    [Route("{id}")]
    public string Separate(int id) => "";

    [Route("{id}/a")]
    [HttpGet]
    public string Reversed(int id) => "";

    [HttpGet, Route("{id}/b")]
    public string SameList(int id) => "";

    [Route("{id}/c"), HttpGet]
    public string SameListReversed(int id) => "";

    [Route("only")]
    public string Alone() => "";

    [HttpGet("plain")]
    public string Plain() => "";
}
"#,
    )]);
    assert_eq!(
        route_names(engine.runtime().unwrap()),
        names(&["ANY /api/mixed/only", "GET /api/mixed/plain"])
    );
    // Each of the four methods registers twice (the verb and the `[Route]`), and
    // both are refused.
    assert_eq!(stats.routes_not_composed, 8);
}

/// `[Route("~/api/…")]` on the class is served from the root, not under a `~`.
#[test]
fn a_class_prefix_written_from_the_application_root_drops_its_tilde() {
    let (_tmp, engine, _) = index(&[(
        "Controllers/CatalogController.cs",
        r#"
using Microsoft.AspNetCore.Mvc;

[Route("~/api/[controller]")]
public class CatalogController : ControllerBase
{
    [HttpGet("x")]
    public string A() => "";
}
"#,
    )]);
    assert_eq!(route_names(engine.runtime().unwrap()), names(&["GET /api/Catalog/x"]));
}

#[test]
fn a_class_route_prefix_that_is_not_written_refuses_its_actions() {
    let (_tmp, engine, stats) = index(&[(
        "Controllers/XController.cs",
        r#"
using Microsoft.AspNetCore.Mvc;

[Route(Routes.Base)]
public class XController : ControllerBase
{
    [HttpGet("list")]
    public string List() => "";

    [HttpGet("/abs")]
    public string Abs() => "";
}
"#,
    )]);
    // The relative action's address is `<unknown>/list`: never promoted. The
    // absolute one does not depend on the prefix.
    assert_eq!(route_names(engine.runtime().unwrap()), names(&["GET /abs"]));
    assert_eq!(stats.routes_not_composed, 1);
}

// ── Detector admission ───────────────────────────────────────────────────────

const BARE_CONTROLLER: &str = r#"
[ApiController]
[Route("api/[controller]")]
public class PingController : ControllerBase
{
    [HttpGet("one")]
    public string One() => "";
}
"#;

#[test]
fn a_file_under_a_global_using_is_admitted_without_a_using_of_its_own() {
    let (_tmp, engine, _) = index(&[
        ("Web/GlobalUsings.cs", "global using Microsoft.AspNetCore.Mvc;\n"),
        ("Web/Controllers/PingController.cs", BARE_CONTROLLER),
    ]);
    assert_eq!(route_names(engine.runtime().unwrap()), names(&["GET /api/Ping/one"]));
}

#[test]
fn a_global_using_admits_only_the_files_under_its_directory() {
    let (_tmp, engine, _) = index(&[
        ("Web/GlobalUsings.cs", "global using Microsoft.AspNetCore.Mvc;\n"),
        ("Other/Controllers/PingController.cs", BARE_CONTROLLER),
    ]);
    assert!(
        route_names(engine.runtime().unwrap()).is_empty(),
        "a global using reaches the files beneath it, never a sibling project"
    );
}

#[test]
fn a_global_using_of_a_namespace_that_only_resembles_a_detector_admits_nothing() {
    let (_tmp, engine, _) = index(&[
        ("Web/GlobalUsings.cs", "global using Microsoft.AspNetCoreExtras.Mvc;\n"),
        ("Web/Controllers/PingController.cs", BARE_CONTROLLER),
    ]);
    assert!(route_names(engine.runtime().unwrap()).is_empty());
}

#[test]
fn a_non_global_using_in_another_file_admits_nobody_else() {
    let (_tmp, engine, _) = index(&[
        ("Web/Other.cs", "using Microsoft.AspNetCore.Mvc;\npublic class Other {}\n"),
        ("Web/Controllers/PingController.cs", BARE_CONTROLLER),
    ]);
    assert!(route_names(engine.runtime().unwrap()).is_empty());
}

#[test]
fn a_file_naming_an_implicit_using_endpoint_type_is_admitted() {
    // The web SDK's implicit usings bring `Microsoft.AspNetCore.Builder` and
    // `.Routing` in, so the file spells no `using` for the types it names.
    let (_tmp, engine, _) = index(&[(
        "Endpoints.cs",
        r#"
public static class Endpoints
{
    public static IEndpointRouteBuilder Map(this IEndpointRouteBuilder app)
    {
        app.MapPost("/logout", Logout);
        return app;
    }
    static string Logout() => "";
}
"#,
    )]);
    assert_eq!(route_names(engine.runtime().unwrap()), names(&["POST /logout"]));
}

#[test]
fn a_top_level_program_with_no_using_at_all_is_admitted_by_its_endpoint_type() {
    // The default template: top-level statements, every namespace implicit.
    let (_tmp, engine, _) = index(&[(
        "Program.cs",
        r#"
var builder = WebApplication.CreateBuilder(args);
var app = builder.Build();
var v1 = app.MapGroup("/v1");
v1.MapGet("/ping", () => "pong");
app.MapPost("/echo", Echo);
app.Run();

static string Echo() => "";
"#,
    )]);
    assert_eq!(route_names(engine.runtime().unwrap()), names(&["GET /v1/ping", "POST /echo"]));
}

#[test]
fn a_lambda_parameter_may_be_a_group_so_its_registrations_are_refused() {
    let (_tmp, engine, stats) = index(&[(
        "Program.cs",
        r#"
using Microsoft.AspNetCore.Builder;

public static class Wiring
{
    public static void Map(WebApplication app)
    {
        app.MapGroup("/g", group => { group.MapGet("/x", () => "x"); });
        app.MapGet("/ok", () => "ok");
    }
}
"#,
    )]);
    assert_eq!(route_names(engine.runtime().unwrap()), names(&["GET /ok"]));
    assert_eq!(stats.routes_not_composed, 1);
}

#[test]
fn only_the_first_positional_attribute_argument_is_the_path() {
    let (_tmp, engine, _) = index(&[(
        "Controllers/NamedController.cs",
        r#"
using Microsoft.AspNetCore.Mvc;

[Route("api/named")]
public class NamedController : ControllerBase
{
    [HttpGet("one", Name = "GetOne")]
    public string One() => "";

    [HttpGet(Name = "Root")]
    public string Root() => "";

    [HttpGet("~/tilde")]
    public string Tilde() => "";
}
"#,
    )]);
    assert_eq!(
        route_names(engine.runtime().unwrap()),
        names(&["GET /api/named/one", "GET /tilde"]),
        "`Name = …` is not a second route, and a `~/` path is from the root, not the class prefix"
    );
}

#[test]
fn a_map_call_with_a_path_that_is_not_a_literal_is_not_a_route() {
    let (_tmp, engine, _) = index(&[(
        "Apis.cs",
        r#"
using Microsoft.AspNetCore.Builder;

public static class Apis
{
    public static void Map(WebApplication app, string path)
    {
        app.MapGet(path, () => "x");
        app.MapGet("/lit", () => "x");
    }
}
"#,
    )]);
    assert_eq!(route_names(engine.runtime().unwrap()), names(&["GET /lit"]));
}

// ── Sync ≡ reindex ───────────────────────────────────────────────────────────

/// The routes indexed from `before`, those a sync of `changes` then leaves, and
/// those a fresh index of the final tree has: the incremental pass is a pure
/// function of the same ledger and promotes the same set (NFR-RA-06).
fn sync_matches_reindex(
    before: &[(&str, &str)],
    changes: &[(&str, Option<&str>)],
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let (tmp, engine, _) = index(before);
    let indexed = route_names(engine.runtime().unwrap());
    let mut touched = Vec::new();
    for (rel, contents) in changes {
        match contents {
            Some(contents) => write(tmp.path(), rel, contents),
            None => fs::remove_file(tmp.path().join(rel)).expect("remove"),
        }
        touched.push(std::path::PathBuf::from(rel));
    }
    let _ = engine.sync(&touched);
    let synced = route_names(engine.runtime().unwrap());

    let fresh_tmp = tempfile::tempdir().unwrap();
    for (rel, contents) in before {
        if !changes.iter().any(|(c, _)| c == rel) {
            write(fresh_tmp.path(), rel, contents);
        }
    }
    for (rel, contents) in changes {
        if let Some(contents) = contents {
            write(fresh_tmp.path(), rel, contents);
        }
    }
    let fresh = Engine::start(fresh_tmp.path()).expect("engine starts");
    fresh.index();
    (indexed, synced, route_names(fresh.runtime().unwrap()))
}

#[test]
fn a_global_using_added_by_sync_admits_the_files_it_governs_like_a_reindex() {
    let (indexed, synced, fresh) = sync_matches_reindex(
        &[
            ("Web/GlobalUsings.cs", "// nothing yet\n"),
            ("Web/Controllers/PingController.cs", BARE_CONTROLLER),
        ],
        &[("Web/GlobalUsings.cs", Some("global using Microsoft.AspNetCore.Mvc;\n"))],
    );
    assert!(indexed.is_empty());
    assert_eq!(fresh, names(&["GET /api/Ping/one"]));
    assert_eq!(synced, fresh);
}

#[test]
fn a_global_using_removed_by_sync_retires_the_routes_it_admitted_like_a_reindex() {
    let (indexed, synced, fresh) = sync_matches_reindex(
        &[
            ("Web/GlobalUsings.cs", "global using Microsoft.AspNetCore.Mvc;\n"),
            ("Web/Controllers/PingController.cs", BARE_CONTROLLER),
        ],
        &[("Web/GlobalUsings.cs", None)],
    );
    assert_eq!(indexed, names(&["GET /api/Ping/one"]));
    assert!(fresh.is_empty());
    assert_eq!(synced, fresh);
}

#[test]
fn a_minimal_api_edited_by_sync_promotes_like_a_reindex() {
    let (indexed, synced, fresh) = sync_matches_reindex(
        &[("Apis/CatalogApi.cs", CATALOG_API)],
        &[("Apis/CatalogApi.cs", Some(&CATALOG_API.replace("api/catalog", "api/shop")))],
    );
    assert!(indexed.contains(&"GET /api/catalog/items".to_string()));
    assert!(fresh.contains(&"GET /api/shop/items".to_string()));
    assert_eq!(synced, fresh);
}

// ── Coverage the review's mutation sweep asked for ───────────────────────────

#[test]
fn every_map_verb_is_its_own_method_and_binds_its_method_group_handler() {
    let (_tmp, engine, _) = index(&[(
        "Apis/VerbsApi.cs",
        r#"
using Microsoft.AspNetCore.Builder;

public static class Handlers
{
    public static string Get() => "";
}

public static class VerbsApi
{
    public static void Map(WebApplication app)
    {
        app.MapGet("/g", Handlers.Get);
        app.MapPost("/po", Post);
        app.MapPut("/pu", Put);
        app.MapPatch("/pa", Patch);
        app.MapDelete("/d", Delete);
    }
    static string Post() => "";
    static string Put() => "";
    static string Patch() => "";
    static string Delete() => "";
}
"#,
    )]);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        route_names(rt),
        names(&["DELETE /d", "GET /g", "PATCH /pa", "POST /po", "PUT /pu"])
    );
    let handlers = route_handlers(rt);
    for pair in [("DELETE /d", "Delete"), ("PATCH /pa", "Patch"), ("POST /po", "Post"), ("PUT /pu", "Put"), ("GET /g", "Get")] {
        assert!(handlers.contains(&(pair.0.to_string(), pair.1.to_string())), "{pair:?} in {handlers:?}");
    }
}

#[test]
fn registrations_this_file_does_not_read_promote_nothing() {
    let (_tmp, engine, _) = index(&[(
        "Apis/Other.cs",
        r#"
using Microsoft.AspNetCore.Builder;

public static class Other
{
    public static void Map(WebApplication app)
    {
        app.MapMethods("/check", new[] { "OPTIONS" }, () => 1);
        app.MapFallback("/fallback", () => 1);
        app.MapControllerRoute("default", "{controller}/{action}");
    }
}
"#,
    )]);
    assert!(route_names(engine.runtime().unwrap()).is_empty());
}

#[test]
fn only_a_route_attribute_is_a_class_prefix() {
    let (_tmp, engine, stats) = index(&[(
        "Controllers/AttrController.cs",
        r#"
using Microsoft.AspNetCore.Mvc;

[Produces("application/json")]
[Route("api/a")]
public class AController : ControllerBase
{
    [HttpGet("x")]
    [Authorize("Policy")]
    public string X() => "";
}

[Produces("application/json")]
[ApiVersion("1.0")]
public class BController : ControllerBase
{
    [HttpGet("y")]
    public string Y() => "";

    [HttpGet]
    public string Bare() => "";
}

[Route("one")]
[Route("two")]
public class CController : ControllerBase
{
    [HttpGet("z")]
    public string Z() => "";
}
"#,
    )]);
    assert_eq!(
        route_names(engine.runtime().unwrap()),
        names(&["GET /api/a/x", "GET /one/z", "GET /two/z", "GET /y"]),
        "[Produces]/[ApiVersion] are no prefix, a bare verb with no prefix is no route, two [Route]s are two prefixes"
    );
    assert_eq!(stats.routes_not_composed, 0);
}

#[test]
fn a_lambda_group_parameter_of_any_form_is_refused() {
    for lambda in ["(a, b) => { a.MapGet(\"/y\", () => 1); }", "(RouteGroupBuilder a) => { a.MapGet(\"/y\", () => 1); }"] {
        let (routes, refused) = body_routes(&format!("app.MapGroup(\"/h\", {lambda});"));
        assert!(routes.is_empty(), "{lambda}: {routes:?}");
        assert_eq!(refused, 1, "{lambda}");
    }
}

#[test]
fn a_named_group_argument_is_not_read_as_the_prefix() {
    let (routes, refused) = body_routes("var g = app.MapGroup(pattern: \"/g\"); g.MapGet(\"/x\", () => 1);");
    assert!(routes.is_empty(), "{routes:?}");
    assert_eq!(refused, 1);
}

#[test]
fn a_parameter_that_is_a_plain_builder_shadows_an_outer_group_of_its_name() {
    let (_tmp, engine, stats) = index(&[(
        "Program.cs",
        r#"
using Microsoft.AspNetCore.Builder;

var app = WebApplication.Create(args);
var g = app.MapGroup("/g");
g.MapGet("/in", () => 1);

static void Shadow(WebApplication g) { g.MapGet("/x", () => 1); }
"#,
    )]);
    assert_eq!(route_names(engine.runtime().unwrap()), names(&["GET /g/in", "GET /x"]));
    assert_eq!(stats.routes_not_composed, 0);
}

#[test]
fn a_minimal_api_inside_a_prefixed_class_takes_no_class_prefix() {
    let (_tmp, engine, _) = index(&[(
        "Controllers/AController.cs",
        r#"
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Mvc;

[Route("api/[controller]")]
public class AController : ControllerBase
{
    public void Wire(WebApplication app) { app.MapGet("/m", () => 1); }

    [HttpGet("a")]
    public string A() => "";
}
"#,
    )]);
    assert_eq!(route_names(engine.runtime().unwrap()), names(&["GET /api/A/a", "GET /m"]));
}

#[test]
fn a_global_using_reaches_its_directory_exactly_and_by_extension_case() {
    // At the root, it reaches everything below.
    let (_t, engine, _) = index(&[
        ("GlobalUsings.cs", "global using Microsoft.AspNetCore.Mvc;\n"),
        ("Controllers/PingController.cs", BARE_CONTROLLER),
    ]);
    assert_eq!(route_names(engine.runtime().unwrap()), names(&["GET /api/Ping/one"]), "root-level global using");

    // `Web2` is not under `Web`.
    let (_t, engine, _) = index(&[
        ("Web/GlobalUsings.cs", "global using Microsoft.AspNetCore.Mvc;\n"),
        ("Web2/PingController.cs", BARE_CONTROLLER),
    ]);
    assert!(route_names(engine.runtime().unwrap()).is_empty(), "a sibling whose name starts with the directory's");

    // The extension compares case-insensitively, as the binder's does.
    let (_t, engine, _) = index(&[
        ("Web/GlobalUsings.CS", "global using Microsoft.AspNetCore.Mvc;\n"),
        ("Web/PingController.cs", BARE_CONTROLLER),
    ]);
    assert_eq!(route_names(engine.runtime().unwrap()), names(&["GET /api/Ping/one"]), "mixed-case extension");
}

#[test]
fn a_file_admitted_by_an_endpoint_type_alone_is_promoted_by_sync_like_a_reindex() {
    let endpoints = "\
public static class Endpoints
{
    public static void Map(this IEndpointRouteBuilder app) { app.MapPost(\"/logout\", () => 1); }
}
";
    let (indexed, synced, fresh) =
        sync_matches_reindex(&[("Endpoints.cs", "// empty\n")], &[("Endpoints.cs", Some(endpoints))]);
    assert!(indexed.is_empty());
    assert_eq!(fresh, names(&["POST /logout"]));
    assert_eq!(synced, fresh);
}

// ── OpenAPI route references ─────────────────────────────────────────────────

#[cfg(feature = "lang-yaml")]
#[test]
fn an_openapi_operation_binds_a_minimal_api_route_where_the_shapes_match() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "openapi.yaml",
        "\
openapi: 3.0.3
info:
  title: Catalog
  version: 1.0.0
paths:
  /api/catalog/items:
    get:
      summary: list
  /api/catalog/items/{itemId}:
    delete:
      summary: remove
  /api/catalog/missing:
    get:
      summary: nobody serves this
",
    );
    write(
        tmp.path(),
        "Apis/CatalogApi.cs",
        r#"
using Microsoft.AspNetCore.Http.HttpResults;

public static class CatalogApi
{
    public static void Map(this IEndpointRouteBuilder app)
    {
        var api = app.MapGroup("api/catalog");
        api.MapGet("/items", List);
        api.MapDelete("/items/{id}", Remove);
    }
    static string List() => "";
    static string Remove(int id) => "";
}
"#,
    );
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    // The operation references were unbindable until the routes existed; a
    // no-op sync re-runs resolution over the full ledger (FR-RS-03).
    let _ = engine.sync(&[] as &[std::path::PathBuf]);
    let rt = engine.runtime().unwrap();

    let bound = rt
        .submit_read(|store| {
            let mut bound = Vec::new();
            for route in store.all_nodes()?.into_iter().filter(|n| n.kind == NodeKind::Route) {
                let into = store
                    .neighbours_in(route.id)?
                    .into_iter()
                    .filter(|(kind, src)| *kind == EdgeKind::ArtifactBinding && src.kind == NodeKind::ApiOperation)
                    .count();
                bound.push((route.name, into));
            }
            bound.sort();
            Ok(bound)
        })
        .expect("read runs");
    assert_eq!(
        bound,
        vec![
            ("DELETE /api/catalog/items/{id}".to_string(), 1),
            ("GET /api/catalog/items".to_string(), 1),
        ],
        "the two operations bind the routes their composed paths match; the third stays unresolved"
    );
}
