//! C# HTTP client-call capture (S-346, [CR-108], [FR-WS-08], [ADR-54],
//! [NFR-RA-05]).
//!
//! Two levels, mirroring the Go and Java arms:
//!
//! 1. **Capture** — `plugins/c-sharp/queries/invocations.scm` populates the
//!    interpreter's `invoke.http.method` / `invoke.http.arg` slots for each
//!    `HttpClient` idiom in [FR-WS-08]'s normative C# row, and the descriptor's
//!    `[invocation_methods]` table resolves the two verb spellings C# uses that
//!    `extract::is_http_method` cannot read on its own.
//! 2. **Cross-member bind** — those references bind an ASP.NET Core provider's
//!    routes in **another** workspace member through the real `Engine` +
//!    `ContractBridge` path.
//!
//! The three shared negative cases are [FR-WS-08]'s "Shared negative-case
//! fixture contract" (S-340) — referenced, not re-invented:
//!
//! - **case 1** (same-shaped non-HTTP receiver call) is per-language and gated by
//!   the descriptor's `http_client_detectors` row (`plugins/c-sharp/plugin.toml`,
//!   read by `extract::capture_http_client_call_arm`):
//!   [`a_route_shaped_get_outside_a_system_net_http_file_is_not_captured`];
//! - **case 2** (`base-url-runtime`): [`a_runtime_composed_path_is_not_captured`];
//! - **case 3** (`path-not-composed`):
//!   [`an_absolute_but_non_normalizing_path_is_not_captured`].
//!
//! Cases 2 and 3 are classified generically in
//! `resolve::http_client_call::classify_client_call` and fixture-pinned there;
//! what these prove is only that the C# query routes its idioms into the slots
//! that reach that classifier.
//!
//! [CR-108]: ../../docs/requests/CR-108-per-language-http-client-call-capture.md
//! [FR-WS-08]: ../../docs/specs/requirements/FR-WS-08.md
//! [ADR-54]: ../../docs/specs/architecture/decisions/ADR-54.md
//! [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
#![cfg(feature = "lang-c-sharp")]

use std::fs;
use std::path::{Path, PathBuf};

use logos_core::extract::{extract, Facts, FileInput, SymbolContext};
use logos_core::federation::{
    cross_service_coverage, ContractBridge, EngineRegistry, Federation, Member, RegistryMode,
};
use logos_core::model::ArtifactRelation;
use logos_core::plugin::LanguageRegistry;
use logos_core::Engine;

// ── Capture-level harness ────────────────────────────────────────────────────

/// Extract one in-memory C# source through the embedded C# plugin.
fn extract_cs(source: &str) -> Facts {
    let tmp = tempfile::tempdir().expect("tempdir");
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
    let plugin = reg.for_extension("cs").expect("c-sharp grammar");
    extract(
        &FileInput::new("UserClient.cs", source),
        plugin,
        &SymbolContext::default(),
    )
}

/// Wrap a method body in a `System.Net.Http`-importing compilation unit, so the
/// `http_client_detectors` ledger gate is open.
fn client_file(body: &str) -> String {
    format!(
        "using System.Net.Http;\nusing System.Net.Http.Json;\n\n\
         public class UserClient\n{{\n    private readonly HttpClient client;\n\
         \x20   public void Run()\n    {{\n{body}\n    }}\n}}\n"
    )
}

/// The `HttpClientCall` reference targets captured from a source, in ledger order.
fn client_call_targets(facts: &Facts) -> Vec<String> {
    facts
        .refs
        .iter()
        .filter(|r| r.relation == Some(ArtifactRelation::HttpClientCall))
        .map(|r| r.target.clone())
        .collect()
}

// ── AC1: the four `Async`-suffixed verbs ────────────────────────────────────

/// The AC's four verbs (`GetAsync`, `PostAsync`, `PutAsync`, `DeleteAsync`)
/// plus `PatchAsync` each yield exactly one reference with the `Async` suffix
/// stripped and the verb upper-cased — the whole point of the
/// `[invocation_methods]` normalizer, since `GetAsync` is not one of
/// `extract::is_http_method`'s bare verbs.
#[test]
fn the_async_suffixed_verbs_each_capture_with_the_suffix_stripped() {
    for (call, expected) in [
        (r#"client.GetAsync("/users");"#, "GET /users"),
        (r#"client.PostAsync("/users", content);"#, "POST /users"),
        (r#"client.PutAsync("/users/{id}", content);"#, "PUT /users/{id}"),
        (r#"client.DeleteAsync("/users/{id}");"#, "DELETE /users/{id}"),
        (r#"client.PatchAsync("/users/{id}", content);"#, "PATCH /users/{id}"),
    ] {
        let facts = extract_cs(&client_file(&format!("        {call}")));
        assert_eq!(
            client_call_targets(&facts),
            vec![expected.to_string()],
            "one reference, `Async` stripped and verb upper-cased, for {call:?}"
        );
    }
}

/// The idiomatic spelling is `await`ed and assigned. `await_expression` wraps the
/// invocation, so the anchor must sit on the `invocation_expression` — this test
/// is what fails if it is ever moved to the statement.
#[test]
fn an_awaited_and_bound_call_captures_identically() {
    let facts = extract_cs(
        "using System.Net.Http;\nusing System.Threading.Tasks;\n\n\
         public class UserClient\n{\n    private readonly HttpClient client;\n\
         \x20   public async Task Run()\n    {\n\
         \x20       var response = await client.GetAsync(\"/users\");\n    }\n}\n",
    );
    assert_eq!(client_call_targets(&facts), vec!["GET /users".to_string()]);
}

/// The `[invocation_methods]` lookup is an EXACT match, like its provider-side
/// sibling `[framework_methods]` (`resolve::framework`'s `methods.get(...)`).
/// C# is case-sensitive, so a row spells the token exactly as the API does and
/// there is no case variation to absorb — a spelling that is not a row captures
/// nothing rather than being folded into one.
#[test]
fn the_normalizer_lookup_is_an_exact_match() {
    // Not compilable C# — `HttpClient` exposes no `getasync` — which is the
    // point: the table is not a fuzzy matcher, and the positive control proves
    // the file was scanned.
    let facts = extract_cs(&client_file(
        "        client.getasync(\"/lower\");\n\
         \x20       client.GetAsync(\"/users\");",
    ));
    assert_eq!(
        client_call_targets(&facts),
        vec!["GET /users".to_string()],
        "only the exact `GetAsync` row resolves"
    );
}

/// The `System.Net.Http.Json` extensions are generic, so the member name is a
/// `generic_name` rather than a bare identifier — a second pattern, not a second
/// vocabulary. This is the dominant modern .NET spelling; without it the arm
/// would capture nothing for a codebase that uses it throughout.
#[test]
fn a_generic_json_extension_call_captures_through_the_generic_name_pattern() {
    for (call, expected) in [
        (r#"client.GetFromJsonAsync<User>("/users");"#, "GET /users"),
        (r#"client.PostAsJsonAsync<User>("/users", user);"#, "POST /users"),
    ] {
        let facts = extract_cs(&client_file(&format!("        {call}")));
        assert_eq!(
            client_call_targets(&facts),
            vec![expected.to_string()],
            "the generic member name carries the verb for {call:?}"
        );
    }
}

/// A verbatim string (`@"/users"`) and a C# 11 raw string (`"""/users"""`) are
/// static paths like any other. Both are C#-specific literal spellings that
/// `static_string_literal` had no handling for before this story, and an
/// unhandled literal reads as *dynamic* — a silent no-capture rather than a loud
/// failure, which is why they are pinned.
#[test]
fn verbatim_and_raw_string_paths_are_static_literals() {
    let verbatim = extract_cs(&client_file(r#"        client.GetAsync(@"/users");"#));
    assert_eq!(
        client_call_targets(&verbatim),
        vec!["GET /users".to_string()],
        "a verbatim literal is static"
    );

    let raw = extract_cs(&client_file(
        "        client.GetAsync(\"\"\"/users\"\"\");",
    ));
    assert_eq!(
        client_call_targets(&raw),
        vec!["GET /users".to_string()],
        "a raw literal's `\"\"\"` fences are delimiters, not content and not dynamism"
    );
}

/// The C# twin of `go_invocations.rs::a_path_literal_ending_in_a_trimmed_character_is_not_corrupted`,
/// and the language where it actually bites: `verbatim_string_literal` is a LEAF
/// (no named children), so `@"…"` is the one realistic C# path spelling that
/// takes `static_string_literal`'s no-children fallback — the branch whose trim
/// set this story extended. A greedy trim silently turned `@"/tag/#"` into
/// `/tag/`, a WRONG template rather than an absent one, which could bind a real
/// `/tag/` route ([NFR-RA-05]).
///
/// Both spellings of the same path must agree.
#[test]
fn a_verbatim_path_ending_in_a_trimmed_character_is_not_corrupted() {
    let verbatim = extract_cs(&client_file(r#"        client.GetAsync(@"/tag/#");"#));
    assert_eq!(
        client_call_targets(&verbatim),
        vec!["GET /tag/#".to_string()],
        "the trailing `#` survives the fallback's unwrap — a trimmed path would \
         be a different route"
    );

    let plain = extract_cs(&client_file(r#"        client.GetAsync("/tag/#");"#));
    assert_eq!(
        client_call_targets(&plain),
        client_call_targets(&verbatim),
        "the verbatim and plain spellings of one path must not disagree"
    );
}

/// **Every declared `[invocation_methods]` row is exercised.** The table is
/// declarative data, so both typo classes fail SILENTLY: a mistyped key
/// (`GetStrngAsync`) means those calls are never captured, and a mistyped value
/// (`"GTE"`) is dropped by `is_http_method` — the behaviour `plugin.toml` and
/// `PluginManifest::invocation_methods` both document and nothing asserted.
///
/// Driven off the loaded descriptor rather than a hardcoded list, so a row added
/// without a fixture fails here rather than shipping untested.
#[test]
fn every_declared_invocation_method_row_captures_its_verb() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
    let table = reg
        .for_extension("cs")
        .expect("c-sharp grammar")
        .semantics()
        .invocation_methods
        .clone();
    assert!(!table.is_empty(), "C# declares the table this story added");

    for (token, verb) in &table {
        // A dotted key is a named constant in the `HttpRequestMessage`
        // constructor; a bare key is a method name.
        let call = if token.contains('.') {
            format!("        client.SendAsync(new HttpRequestMessage({token}, \"/probe\"));")
        } else {
            format!("        client.{token}(\"/probe\");")
        };
        let facts = extract_cs(&client_file(&call));
        assert_eq!(
            client_call_targets(&facts),
            vec![format!("{verb} /probe")],
            "row `{token} = \"{verb}\"` must capture; a key or value typo is \
             otherwise silent"
        );
    }
}

/// `client?.GetAsync("/users")` — the null-conditional receiver is idiomatic C#
/// and parses as a `member_binding_expression`, a different node kind from the
/// `member_access_expression` the main pattern anchors on. Its own pattern, so
/// the shape is captured rather than silently missed.
#[test]
fn a_null_conditional_receiver_captures_identically() {
    let facts = extract_cs(&client_file(
        "        client?.GetAsync(\"/users\");\n\
         \x20       client?.GetFromJsonAsync<User>(\"/orders\");",
    ));
    assert_eq!(
        client_call_targets(&facts),
        vec!["GET /orders".to_string(), "GET /users".to_string()],
        "both null-conditional spellings capture"
    );
}

// ── AC2: the constructor-argument anchor (`SendAsync`) ──────────────────────

/// **The story's second decision, proven.** `SendAsync(new HttpRequestMessage(
/// HttpMethod.Get, "/users"))` yields exactly one `GET /users` reference. The
/// verb is a NAMED CONSTANT, the shape S-345 measured in Go and deferred to this
/// story's normalizer table: the only text available is `HttpMethod.Get`, and the
/// table maps it.
///
/// `SendAsync` itself carries no row and needs none — the anchor is the
/// constructor, not the send.
#[test]
fn a_send_async_constructor_argument_takes_its_verb_from_the_named_constant() {
    let facts = extract_cs(&client_file(
        r#"        client.SendAsync(new HttpRequestMessage(HttpMethod.Get, "/users"));"#,
    ));
    assert_eq!(
        client_call_targets(&facts),
        vec!["GET /users".to_string()],
        "the named-constant verb resolves through [invocation_methods]"
    );
}

/// The split spelling — build the request, then send it — captures through the
/// same pattern, because the anchor is the `object_creation_expression` wherever
/// it sits. `client.SendAsync(request)` contributes nothing on its own.
#[test]
fn the_split_request_then_send_spelling_captures_through_the_same_anchor() {
    let facts = extract_cs(&client_file(
        "        var request = new HttpRequestMessage(HttpMethod.Delete, \"/orders/{id}\");\n\
         \x20       client.SendAsync(request);",
    ));
    assert_eq!(
        client_call_targets(&facts),
        vec!["DELETE /orders/{id}".to_string()],
        "exactly one reference, from the constructor and not from the send"
    );
}

/// A construction of some other type with the same two-argument shape is not an
/// outbound request: the type is pinned by name, so nothing structural is being
/// relied on that a bound result or a value position could defeat (the
/// discriminator S-345 abandoned).
#[test]
fn a_same_shaped_construction_of_another_type_is_not_captured() {
    let facts = extract_cs(&client_file(
        "        var entry = new CacheEntry(HttpMethod.Get, \"/users\");\n\
         \x20       client.GetAsync(\"/probe\");",
    ));
    assert_eq!(
        client_call_targets(&facts),
        vec!["GET /probe".to_string()],
        "only `HttpRequestMessage` is an outbound request, and the file was \
         genuinely scanned: {:?}",
        client_call_targets(&facts)
    );
}

/// `new HttpMethod("GET")` is a **stated ceiling, not a bug**. Reaching it would
/// need bare-verb identity rows (`Get = "GET"`) in `[invocation_methods]`, and
/// those rows would simultaneously re-admit `_cache.Get("/health")` through the
/// method-name anchor — reopening the fabrication class the table exists to
/// close ([NFR-RA-05]). The trade is refused, and refused visibly.
#[test]
fn a_literal_verb_http_method_construction_is_a_stated_ceiling_not_a_capture() {
    let facts = extract_cs(&client_file(
        "        client.SendAsync(new HttpRequestMessage(new HttpMethod(\"GET\"), \"/users\"));\n\
         \x20       client.GetAsync(\"/probe\");",
    ));
    assert_eq!(
        client_call_targets(&facts),
        vec!["GET /probe".to_string()],
        "honestly uncaptured rather than bought at the price of bare-verb rows, \
         and the file was genuinely scanned: {:?}",
        client_call_targets(&facts)
    );
}

// ── AC3 / shared negative case 2: base-url-runtime ──────────────────────────

/// An interpolated string (`$"{baseUrl}/users"`), a bare variable and a
/// concatenation each emit **no** reference — the path is composed at runtime, so
/// there is nothing static to bind ([NFR-RA-05]). This is [FR-WS-08]'s shared
/// negative case 2; the `base-url-runtime` reason itself is classified generically
/// by `classify_client_call`.
#[test]
fn a_runtime_composed_path_is_not_captured() {
    for call in [
        r#"client.GetAsync($"{baseUrl}/users");"#,
        r#"client.GetAsync(baseUrl);"#,
        r#"client.GetAsync(baseUrl + "/users");"#,
        r#"client.GetAsync(string.Format("{0}/users", baseUrl));"#,
        r#"client.SendAsync(new HttpRequestMessage(HttpMethod.Get, $"{baseUrl}/users"));"#,
    ] {
        let facts = extract_cs(&client_file(&format!(
            "        var baseUrl = \"https://x\";\n        {call}"
        )));
        assert!(
            client_call_targets(&facts).is_empty(),
            "a runtime-composed path is base-url-runtime, got {:?} for {call:?}",
            client_call_targets(&facts)
        );
    }
}

/// A relative literal (`"users"`) has no absolute route prefix — its base URL is
/// composed elsewhere — so it is likewise refused, not approximated.
#[test]
fn a_relative_path_literal_is_not_captured() {
    let facts = extract_cs(&client_file(r#"        client.GetAsync("users/{id}");"#));
    assert!(
        client_call_targets(&facts).is_empty(),
        "a relative literal has no workspace-composable prefix: {:?}",
        client_call_targets(&facts)
    );
}

// ── Shared negative case 3: path-not-composed ───────────────────────────────

/// A static, absolute literal that does not positionally normalize (a mixed
/// literal/parameter segment) emits no reference — never approximately matched.
#[test]
fn an_absolute_but_non_normalizing_path_is_not_captured() {
    let facts = extract_cs(&client_file(r#"        client.GetAsync("/v{version}/users");"#));
    assert!(
        client_call_targets(&facts).is_empty(),
        "a mixed literal/parameter segment is path-not-composed: {:?}",
        client_call_targets(&facts)
    );
}

// ── Shared negative case 1: same-shaped non-HTTP receiver call ──────────────

/// A `/`-shaped-key `.GetAsync(...)` in a file that references **no** C#
/// HTTP-client namespace is never scanned for client calls at all, so it can
/// never fabricate a cross-service edge ([FR-WS-08] shared negative case 1,
/// [NFR-RA-05]).
#[test]
fn a_route_shaped_get_outside_a_system_net_http_file_is_not_captured() {
    const BODY: &str = "public class Authorizer\n{\n    private readonly Permissions cache;\n\
                        \x20   public void Authorize()\n    {\n\
                        \x20       cache.GetAsync(\"/admin/users\");\n    }\n}\n";

    let facts = extract_cs(&format!("using System;\n\n{BODY}"));
    assert!(
        client_call_targets(&facts).is_empty(),
        "a non-client file never emits an outbound-call ref: {:?}",
        client_call_targets(&facts)
    );

    // Positive control, and the ADR-54 ceiling in the same fixture. The source is
    // byte-identical but for the `using System.Net.Http;`, so the emptiness above
    // is attributable to `capture_http_client_call_arm`'s ledger gate and to
    // nothing else.
    //
    // What it captures is the documented ADR-54 accuracy ceiling, and C#'s is
    // NARROWER than Go's: Go's residual is any `/`-keyed `.get(...)`, while here
    // the receiver must expose a method that is itself an
    // `[invocation_methods]` row. `IDistributedCache.GetAsync(key)` is the real
    // instance of that, which is what this fixture spells.
    let gated = extract_cs(&format!("using System.Net.Http;\n\n{BODY}"));
    assert_eq!(
        client_call_targets(&gated),
        vec!["GET /admin/users".to_string()],
        "the same source with the client `using` DOES capture — the ledger gate is \
         the only difference between these two cases, and the residual is ADR-54's \
         stated file-grained ceiling"
    );
}

/// Inside a genuine `System.Net.Http` file, a same-shaped call whose method name
/// is not an `[invocation_methods]` row is still never captured — including a
/// BARE HTTP verb (`cache.Get("/health")`), which Go can only record as a
/// ceiling. The table's filter half is what closes it here — Java closes the
/// same class a third way, with a receiver-name rule (S-375); the two mechanisms
/// are complementary.
#[test]
fn a_bare_verb_method_call_inside_a_client_file_is_not_captured() {
    let facts = extract_cs(&client_file(
        "        cache.Get(\"/admin/users\");\n\
         \x20       cache.Fetch(\"/admin/roles\");\n\
         \x20       client.GetAsync(\"/health\");",
    ));
    assert_eq!(
        client_call_targets(&facts),
        vec!["GET /health".to_string()],
        "only the table's rows are verbs; a bare `Get` and a `Fetch` are not"
    );
}

// ── Provider registrations must never be read as outbound calls ─────────────

/// **The cross-story mandatory re-check (S-345's finding).** An ASP.NET Core
/// minimal-API service that also does outbound calls is a *provider*. Its own
/// route registrations — `app.MapGet("/users", Handler)` — parse to exactly the
/// tree of `client.PostAsync("/users", content)`, and capturing them would turn
/// the provider's own surface into phantom consumer calls binding other members'
/// routes ([NFR-RA-05]).
///
/// The discriminator is the `[invocation_methods]` name pin: the registration
/// vocabulary (`Map*`) and the client vocabulary (`*Async`) are disjoint
/// identifier sets in this language, so no arity or value-position test is
/// needed — and none would work, since the trees are identical.
#[test]
fn asp_net_core_route_registrations_are_not_read_as_outbound_calls() {
    let facts = extract_cs(
        "using System.Net.Http;\nusing Microsoft.AspNetCore.Builder;\n\n\
         public class Program\n{\n    private static HttpClient client;\n\
         \x20   public static void Main()\n    {\n\
         \x20       var app = WebApplication.Create();\n\
         \x20       app.MapGet(\"/users\", ListUsers);\n\
         \x20       app.MapPost(\"/users\", CreateUser);\n\
         \x20       app.MapPut(\"/users/{id}\", UpdateUser);\n\
         \x20       app.MapDelete(\"/users/{id}\", DeleteUser);\n\
         \x20       app.MapMethods(\"/users\", verbs, AnyUser);\n\
         \x20       client.GetAsync(\"/probe\");\n    }\n}\n",
    );
    // The `GetAsync` is the positive control: it proves the file WAS scanned, so
    // the five registrations above are absent because the name pin rejected them
    // — not because a later tightening of `http_client_detectors` quietly closed
    // the gate on the whole fixture.
    assert_eq!(
        client_call_targets(&facts),
        vec!["GET /probe".to_string()],
        "a provider's own registrations are never outbound calls, and the file was \
         genuinely scanned: {:?}",
        client_call_targets(&facts)
    );
}

/// The BOUND-RESULT twin of the test above, and the one an "a registration
/// discards its result" discriminator gets wrong: ASP.NET Core's `MapGet` returns
/// a `RouteHandlerBuilder` that is routinely chained or named, so a value-position
/// check alone would read every one of these as an outbound call ([NFR-RA-05]).
#[test]
fn a_bound_or_chained_registration_is_still_not_an_outbound_call() {
    let facts = extract_cs(
        "using System.Net.Http;\nusing Microsoft.AspNetCore.Builder;\n\n\
         public class Program\n{\n    private static HttpClient client;\n\
         \x20   public static void Main()\n    {\n\
         \x20       var app = WebApplication.Create();\n\
         \x20       var route = app.MapGet(\"/users\", ListUsers);\n\
         \x20       app.MapPost(\"/orders\", CreateOrder).RequireAuthorization();\n\
         \x20       client.GetAsync(\"/probe\");\n    }\n}\n",
    );
    assert_eq!(
        client_call_targets(&facts),
        vec!["GET /probe".to_string()],
        "naming or chaining a registration does not make it a call: {:?}",
        client_call_targets(&facts)
    );
}

/// The controller-attribute provider spelling (`[HttpGet("/users")]`) is an
/// `attribute`, not an `invocation_expression`, so it cannot reach this arm at
/// all — asserted rather than assumed, since `HttpGet` is one character away from
/// a plausible client-table row.
#[test]
fn a_controller_verb_attribute_is_not_an_outbound_call() {
    let facts = extract_cs(
        "using System.Net.Http;\nusing Microsoft.AspNetCore.Mvc;\n\n\
         [ApiController]\npublic class UsersController\n{\n\
         \x20   private HttpClient client;\n\
         \x20   [HttpGet(\"/users\")]\n    public void List() {}\n\
         \x20   [HttpPost(\"/users\")]\n    public void Create() {}\n\
         \x20   public void Probe() { client.GetAsync(\"/probe\"); }\n}\n",
    );
    assert_eq!(
        client_call_targets(&facts),
        vec!["GET /probe".to_string()],
        "a verb attribute is a provider declaration, not a call, and the file \
         was genuinely scanned: {:?}",
        client_call_targets(&facts)
    );
}

// ── Go's ceiling is not lifted by this story's table ────────────────────────

/// `[invocation_methods]` is per-plugin data, so a language that declares no rows
/// keeps the pass-through behaviour it had before this story. Asserted on the
/// descriptors directly, because the *cross-language* claim is the one
/// `go_invocations.rs::a_named_constant_verb_is_a_stated_ceiling_not_a_capture`
/// depends on, and a future edit to `plugins/go/plugin.toml` should fail here
/// with the reason rather than only there with a target mismatch.
#[test]
#[cfg(all(
    feature = "lang-go",
    feature = "lang-java",
    feature = "lang-rust",
    feature = "lang-typescript"
))]
fn only_c_sharp_declares_an_invocation_methods_table() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");

    // Every other language that ships `invocations` — each relies on the
    // empty-table pass-through.
    for ext in ["go", "java", "rs", "ts", "tsx"] {
        let plugin = reg.for_extension(ext).expect("grammar is compiled in");
        assert!(
            plugin.semantics().invocation_methods.is_empty(),
            "`{ext}` must keep the pass-through verb behaviour: declaring rows here \
             lifts that language's own named-constant ceiling, which is a decision \
             for that language's story, not a side effect of S-346"
        );
    }

    let c_sharp = reg.for_extension("cs").expect("c-sharp grammar");
    assert_eq!(
        c_sharp.semantics().invocation_methods.get("GetAsync").map(String::as_str),
        Some("GET"),
        "C# is the language that pays for the table"
    );
}

// ── Cross-member bind against an ASP.NET Core provider ──────────────────────

/// A C# consumer exercising both anchor shapes — the `Async`-suffixed method name
/// and the `HttpRequestMessage` constructor argument — plus one runtime-composed
/// call that must bind nothing even though a matching route exists.
const CS_CLIENT: &str = r#"using System.Net.Http;

namespace Acme.Client
{
    public class OrderClient
    {
        private readonly HttpClient client;
        private readonly string baseUrl;

        public void ListUsers()
        {
            client.GetAsync("/users");
        }

        public void GetOrder()
        {
            client.SendAsync(new HttpRequestMessage(HttpMethod.Get, "/orders/{id}"));
        }

        public void GetReportsDynamic()
        {
            client.GetAsync($"{baseUrl}/reports");
        }
    }
}
"#;

/// An ASP.NET Core controller registering exact-method routes. `[HttpGet]` maps
/// to `GET` through `[framework_methods]`; the provider's `{id}` and the
/// consumer's `{id}` agree here, and the positional `route_key` would erase the
/// drift in any case. `/reports` is registered too, so the runtime-composed call
/// has a provider it would bind to if the arm ever approximated — it must not.
const CS_PROVIDER: &str = r#"using Microsoft.AspNetCore.Mvc;

namespace Acme.Api
{
    [ApiController]
    public class OrdersController
    {
        [HttpGet("/users")]
        public void ListUsers() {}

        [HttpGet("/orders/{id}")]
        public void GetOrder() {}

        [HttpGet("/reports")]
        public void ListReports() {}
    }
}
"#;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    fs::write(path, contents).expect("write fixture");
}

/// Index a member repo's fixtures into its own `.logos/logos.db`, then drop the
/// engine so the store is closed before the registry re-opens it.
fn index_member(root: &Path) {
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let _ = engine.sync(&[] as &[PathBuf]);
}

fn member(name: &str, root: &Path) -> Member {
    Member {
        name: name.to_string(),
        root: root.to_path_buf(),
    }
}

fn federation(root: &Path, members: Vec<Member>) -> Federation {
    Federation {
        name: "acme".to_string(),
        root: root.to_path_buf(),
        members,
        default: None,
        links: Vec::new(),
        governance: Default::default(),
        warm_concurrency: None,
    }
}

/// End-to-end: both C# anchor shapes bind real ASP.NET Core routes in **another**
/// member, and the interpolated call binds nothing even though `/reports` exists
/// ([FR-WS-12], [NFR-RA-05]).
#[test]
fn c_sharp_client_calls_bind_asp_net_core_routes_in_another_member() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    let web = root.join("web");
    let api = root.join("api");
    write(&web, "OrderClient.cs", CS_CLIENT);
    write(&api, "OrdersController.cs", CS_PROVIDER);
    index_member(&web);
    index_member(&api);

    let registry = EngineRegistry::<Engine>::new(
        federation(root, vec![member("web", &web), member("api", &api)]),
        RegistryMode::Lazy,
    );

    let edges = ContractBridge::new().edges(&registry);
    assert_eq!(
        edges.len(),
        2,
        "both static C# calls bind, the interpolated one does not: {edges:?}"
    );
    for edge in edges.iter() {
        assert_eq!(edge.relation, "route");
        assert_eq!(edge.from.member, "web", "the call site is in member `web`");
        assert_eq!(edge.to.member, "api", "the route is in member `api`");
    }

    // Name WHICH routes bound: a count alone would pass if `/reports` bound
    // falsely while `/users` failed to bind — the two errors cancelling out.
    let bound: Vec<String> = edges
        .iter()
        .map(|e| e.to.symbol.as_str().to_string())
        .collect();
    assert!(
        bound.iter().any(|s| s.contains("/users")),
        "the `GetAsync` (verb-as-method-name) call bound its route: {bound:?}"
    );
    assert!(
        bound.iter().any(|s| s.contains("/orders")),
        "the `HttpRequestMessage` (verb-as-constructor-argument) call bound its \
         route: {bound:?}"
    );
    assert!(
        !bound.iter().any(|s| s.contains("reports")),
        "the `$`-interpolated call must bind NOTHING even though /reports exists \
         as a provider: {bound:?}"
    );

    let coverage = cross_service_coverage(&registry);
    assert_eq!(coverage.bound, 2, "both static calls are bound");
    assert_eq!(coverage.ambiguous, 0);
    // Pin the whole census, not just the bound bucket: a phantom third reference
    // landing in `unbound` (or a real one silently reclassified into
    // `no_provider_in_workspace`) would otherwise go unremarked while
    // `bound == 2` still held.
    assert_eq!(
        coverage.references.len(),
        2,
        "exactly two client-call references reach the bridge — the interpolated \
         call is refused before it becomes one: {:?}",
        coverage.references
    );
    assert_eq!(coverage.unbound, 0, "neither reference is left unbound");
    assert_eq!(
        coverage.no_provider_in_workspace, 0,
        "both routes exist in member `api`"
    );
}
