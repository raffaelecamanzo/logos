//! Kotlin HTTP client-call capture (S-342, [CR-108], [FR-WS-08], [FR-PL-07],
//! [ADR-54], [NFR-RA-05]).
//!
//! Drives the **real** pipeline — the compiled `tree-sitter-kotlin-ng` grammar,
//! the plugin's `invocations` query, the generic dispatch's HTTP-verb and
//! static-literal decisions, and `resolve::http_client_call`'s normalizer — over
//! one fixture per idiom in [FR-WS-08]'s normative Kotlin row, so a query that
//! compiles but captures nothing cannot pass ([CR-101]'s lesson).
//!
//! Kotlin binds the same JVM client APIs as Java, so §1's four-idiom matrix is
//! the Java matrix and its emitted keys are **byte-identical** for equivalent
//! source (S-342 AC1). What differs is the grammar, and §2 is where that shows:
//! the Kotlin-specific call shapes ([FR-PL-07] flags this crate as the
//! second-highest-risk grammar of the set, so every one of them is pinned
//! against a parse tree established under the pinned version before the query
//! was written, not adapted by eye from Java's).
//!
//! It pins four things a reader has to be able to trust:
//!
//! 1. **What is captured** — the four-idiom matrix, byte-identical to Java's.
//! 2. **What the Kotlin grammar does to those idioms** — trailing lambda, named
//!    argument, extension-function receiver, string template.
//! 3. **What is refused** — [FR-WS-08]'s shared negative-case fixture contract
//!    (S-340), referenced rather than re-invented here, plus the Kotlin route
//!    **registration** shapes, which are syntactically client calls (S-345's
//!    mandatory re-check: a phantom outbound call binds another workspace
//!    member's route and fabricates a cross-service edge, [NFR-RA-05]).
//! 4. **What is a stated ceiling** — asserted as *zero* references so the
//!    ceiling is pinned in a test rather than only claimed in prose ([ADR-54]).
//!
//! Gated on the Kotlin grammar so a build excluding it does not run it.
//!
//! [CR-101]: ../../docs/requests/CR-101-jvm-spring-route-extraction.md
//! [CR-108]: ../../docs/requests/CR-108-per-language-http-client-call-capture.md
//! [FR-PL-07]: ../../docs/specs/requirements/FR-PL-07.md
//! [FR-WS-08]: ../../docs/specs/requirements/FR-WS-08.md
//! [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
//! [ADR-54]: ../../docs/specs/architecture/decisions/ADR-54.md
#![cfg(feature = "lang-kotlin")]

use std::fs;

use logos_core::Engine;

/// The `import`s that make a file a client-call candidate under the arm's
/// ledger gate (this plugin's own `http_client_detectors` descriptor rows) — the
/// consumer-side twin of [FR-FW-04]'s framework candidacy. Prepended to every
/// positive fixture; deliberately **absent** from the negative-case-1 fixture.
///
/// Byte-identical package set to `java_http_client_call.rs`'s, because the rows
/// gating them are byte-identical: two languages compiling against the same
/// JARs must gate on the same packages.
const CLIENT_IMPORTS: &str = r#"
import org.springframework.web.client.RestClient
import org.springframework.web.client.RestTemplate
import org.springframework.web.reactive.function.client.WebClient
import java.net.http.HttpClient
import java.net.http.HttpRequest
import java.net.URI
"#;

/// A genuine, captured client call appended to every never-fabricate fixture as
/// its **positive control** — it renders `"GET /probe"`, so a fixture that
/// asserts exactly that has proved the ledger gate was open and the query live.
///
/// Without one, a `is_empty()` assertion over an in-client-file fixture is
/// satisfied just as well by a closed gate as by the discriminator under test,
/// so a later tightening of `http_client_detectors` would leave these tests
/// green while proving nothing. Go and C# closed that class in this same sprint
/// (`go_invocations.rs`, `c_sharp_invocations.rs`); Kotlin — the arm where a
/// detached predicate already proved that "a rule with no test cannot notice
/// that it stopped applying" — is held to the same standard here.
///
/// Its shape is the fluent pattern 1 anchor, so it is independent of pattern 4's
/// receiver, arity and position rules: no fixture's own discriminator can
/// suppress the control.
const PROBE: &str = r#"
fun probe(restClient: RestClient): String =
    restClient.get().uri("/probe").retrieve().body(String::class.java)
"#;

/// Index `body` as a single Kotlin file and return **every** `http-client-call`
/// row the arm wrote to the ledger, sorted — refusals included, as the empty
/// string that sorts first.
///
/// **Deliberately unfiltered (S-374)**, the discipline every other per-language
/// suite keeps: a keyless refusal row is a real ledger row, and a reader that
/// hid it would silently retire this file's "nothing at all was captured"
/// assertions — a spurious keyless row turns `["GET /probe"]` into
/// `["", "GET /probe"]` and fails the test. The split lives in
/// [`client_call_rows`].
fn client_calls(body: &str) -> Vec<String> {
    client_calls_raw(&format!("package com.example\n{CLIENT_IMPORTS}\n{body}"))
}

/// [`client_calls`]'s two populations: the arm's `(references, refusal rows)`.
///
/// The only reader that filters, and it hands back the count it removed — so a
/// test whose contract is about a refusal pins both halves and neither can hide
/// the other. One index run per call, because this suite indexes a whole engine
/// per fixture.
fn client_call_rows(body: &str) -> (Vec<String>, usize) {
    let targets = client_calls(body);
    let refusals = targets.iter().filter(|t| t.is_empty()).count();
    (
        targets.into_iter().filter(|t| !t.is_empty()).collect(),
        refusals,
    )
}

/// As [`client_calls`], but the caller supplies the whole compilation unit —
/// used by the negative case that must ship *without* the client imports.
/// Unfiltered, for the reason given on [`client_calls`].
fn client_calls_raw(source: &str) -> Vec<String> {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(tmp.path().join("src")).expect("mkdir");
    fs::write(tmp.path().join("src/Calls.kt"), source).expect("write fixture");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().expect("runtime");
    engine.index();

    let mut targets: Vec<String> = rt
        .submit_read(|store| {
            Ok(store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| r.payload.as_deref() == Some("http-client-call"))
                .map(|r| r.target)
                .collect())
        })
        .expect("read runs");
    targets.sort();
    targets
}

// ── 1. The four-idiom matrix ([FR-WS-08]'s normative Kotlin row) ─────────────
//
// Byte-identical keys to `java_http_client_call.rs` §1 for equivalent source
// (S-342 AC1): "GET /users/{id}", "POST /orders/{id}", "DELETE /carts/{id}",
// "GET /items/{id}", "POST /items".

/// `RestClient` — the fluent chain whose verb and path sit on **different**
/// links. Kotlin's parse tree for it is
/// `(call_expression (navigation_expression (call_expression (navigation_expression
/// (identifier) (identifier)) (value_arguments)) (identifier)) (value_arguments …))`
/// — no fields, so the query's constraints are positional rather than Java's
/// `object:` / `name:`.
#[test]
fn a_rest_client_fluent_chain_yields_one_reference() {
    assert_eq!(
        client_calls(
            r#"
class Calls(private val restClient: RestClient) {
    fun user(id: String): String {
        return restClient.get().uri("/users/{id}").retrieve().body(String::class.java)
    }
}
"#
        ),
        ["GET /users/{id}"],
        "the verb comes from the `.get()` link and the path from the adjacent \
         `.uri(…)` link — one reference, rendered \"METHOD /template\""
    );
}

/// `WebClient` — the same fluent shape on the reactive client, proving the
/// anchor is the adjacent link pair and not a `RestClient`-specific receiver.
#[test]
fn a_web_client_fluent_chain_yields_one_reference() {
    assert_eq!(
        client_calls(
            r#"
class Calls(private val webClient: WebClient) {
    fun order(id: String): Any? {
        return webClient.post().uri("/orders/{id}").retrieve().bodyToMono(String::class.java).block()
    }
}
"#
        ),
        ["POST /orders/{id}"]
    );
}

/// `RestTemplate` — the plain receiver-method idiom, on the bare-verb methods.
///
/// Its verb-suffixed siblings (`getForObject`, `exchange`, …) are a **stated
/// ceiling**, pinned by
/// [`rest_template_verb_suffixed_methods_are_a_stated_ceiling`].
#[test]
fn a_rest_template_receiver_call_yields_one_reference() {
    assert_eq!(
        client_calls(
            r#"
class Calls(private val restTemplate: RestTemplate) {
    fun drop(id: String) {
        restTemplate.delete("/carts/{id}")
    }
}
"#
        ),
        ["DELETE /carts/{id}"]
    );
}

/// `java.net.http.HttpClient` — the JDK builder, whose path is wrapped in
/// `URI.create(…)` rather than passed as a bare literal. Captured in **both**
/// builder orders, since `HttpRequest.Builder` methods are order-free.
#[test]
fn a_jdk_http_client_builder_yields_one_reference_in_either_order() {
    assert_eq!(
        client_calls(
            r#"
class Calls {
    fun uriThenVerb(): HttpRequest {
        return HttpRequest.newBuilder().uri(URI.create("/items/{id}")).GET().build()
    }
}
"#
        ),
        ["GET /items/{id}"],
        "the canonical `.uri(URI.create(…)).GET()` order"
    );
    assert_eq!(
        client_calls(
            r#"
class Calls {
    fun verbThenUri(body: Any): HttpRequest {
        return HttpRequest.newBuilder().POST(body).uri(URI.create("/items")).build()
    }
}
"#
        ),
        ["POST /items"],
        "the reversed `.POST(…).uri(URI.create(…))` order"
    );
}

// ── 2. The Kotlin-specific call shapes (S-342 AC2, AC4) ─────────────────────

/// **Named argument — CAPTURED.** `f(url = "/p")` parses as
/// `(value_argument (identifier) (string_literal))` where `f("/p")` parses as
/// `(value_argument (string_literal))`. Binding `@invoke.http.arg` to the
/// value_argument's *last* child covers both with one pattern, so the named form
/// needs no pattern of its own and emits the key the positional form emits.
#[test]
fn a_named_argument_call_is_captured_identically_to_the_positional_form() {
    assert_eq!(
        client_calls(
            r#"
class Calls(private val restTemplate: RestTemplate) {
    fun drop(id: String) {
        restTemplate.delete(url = "/carts/{id}")
    }
}
"#
        ),
        ["DELETE /carts/{id}"],
        "the named form binds the argument's VALUE, not the parameter name — \
         the same key the positional fixture above emits"
    );
    assert_eq!(
        client_calls(
            r#"
class Calls(private val restClient: RestClient) {
    fun user(id: String): String {
        return restClient.get().uri(uri = "/users/{id}").retrieve().body(String::class.java)
    }
}
"#
        ),
        ["GET /users/{id}"],
        "and the same holds on the fluent chain's `.uri(…)` link"
    );
}

/// **Every position and receiver form pattern 4 spells out, pinned positively.**
///
/// Pattern 4 is written as four near-identical branches because the discriminator
/// against a trailing-lambda route registration IS the parent node, and a
/// tree-sitter query can only state a parent positively. That makes each branch
/// load-bearing by the query's own account — so each one needs a fixture that
/// would stop capturing if the branch were deleted. The same goes for the
/// `this.`-qualified receiver alternative repeated inside all four: the only
/// other `this.` fixture in this file goes through pattern 1, whose receiver slot
/// is an unconstrained `(_)`, so it would not notice.
#[test]
fn each_receiver_method_position_and_receiver_form_is_captured() {
    for (label, body, expected) in [
        (
            "block statement",
            r#"fun drop(id: String) { restTemplate.delete("/carts/{id}") }"#,
            "DELETE /carts/{id}",
        ),
        (
            "expression body",
            r#"fun drop(id: String) = restTemplate.delete("/sessions/{id}")"#,
            "DELETE /sessions/{id}",
        ),
        (
            "return position",
            r#"fun drop(id: String) { return restTemplate.delete("/baskets/{id}") }"#,
            "DELETE /baskets/{id}",
        ),
        (
            "property initialiser",
            r#"fun drop(id: String) { val ignored = restTemplate.delete("/orders/{id}") }"#,
            "DELETE /orders/{id}",
        ),
        (
            "`this.`-qualified receiver",
            r#"fun drop(id: String) { this.restTemplate.delete("/quotes/{id}") }"#,
            "DELETE /quotes/{id}",
        ),
    ] {
        assert_eq!(
            client_calls(&format!(
                "class Calls(private val restTemplate: RestTemplate) {{\n    {body}\n}}"
            )),
            [expected],
            "{label} is one of pattern 4's stated positions and must capture"
        );
    }
}

/// **Extension-function receiver — CAPTURED when the receiver is spelled,
/// REFUSED when it is implicit**, and the two halves of that split are recorded
/// here together because the refusal is not an oversight: an implicit-receiver
/// call and a receiver-less route-DSL registration (`GET("/users")`,
/// `get("/users") { … }`) have the *same* parse tree, so any pattern that
/// captured the first would capture the second and fabricate an outbound call
/// from a provider's own route ([NFR-RA-05]).
#[test]
fn an_extension_function_receiver_is_captured_when_spelled_and_refused_when_implicit() {
    assert_eq!(
        client_calls(
            r#"
fun RestClient.byId(id: String): String {
    return this.get().uri("/users/{id}").retrieve().body(String::class.java)
}
"#
        ),
        ["GET /users/{id}"],
        "an explicit `this.` extension receiver parses as \
         `(navigation_expression (this_expression) (identifier))` — the fluent \
         pattern's receiver slot is `(_)`, so it binds"
    );
    assert!(
        client_calls(
            r#"
fun RestClient.byId(id: String): String {
    return get().uri("/users/{id}").retrieve().body(String::class.java)
}
"#
        )
        .is_empty(),
        "an implicit extension receiver parses as `(call_expression \
         (identifier) (value_arguments))` — indistinguishable from Ktor's and \
         WebFlux's receiver-less route DSLs, so it is refused with them"
    );
}

/// **Trailing lambda — REFUSED**, in both of the two shapes Kotlin's grammar
/// produces for it, and for two different structural reasons.
///
/// A trailing lambda in place of the argument list (`uri { … }`) leaves the call
/// with **no `value_arguments` node at all**, so no pattern can bind a path. A
/// trailing lambda *after* an argument list (`get("/p") { … }`) does not add a
/// child — it **wraps** the call:
/// `(call_expression (call_expression f (value_arguments …)) (annotated_lambda …))`
/// — leaving the inner call byte-identical to a plain `f("/p")`. That inner call
/// is what a Javalin route registration and Spring's MockMvc Kotlin DSL are made
/// of, so pattern 4 excludes it by *position*: a captured call must be a
/// statement / expression body / `return` / property initialiser, never the
/// callee of a lambda application.
#[test]
fn a_trailing_lambda_call_is_refused() {
    let no_arguments = client_calls(&format!(
        r#"
class Calls(private val webClient: WebClient) {{
    fun user(): Any? {{
        return webClient.get().uri {{ b -> b.path("/users").build() }}.retrieve()
    }}
}}
{PROBE}
"#
    ));
    assert_eq!(
        no_arguments,
        ["GET /probe"],
        "`uri {{ … }}` has no value_arguments — the path is built at runtime and \
         is refused whole, never partially; the probe proves the file was \
         genuinely scanned: {no_arguments:?}"
    );
    let lambda_applied = client_calls(&format!(
        r#"
class Calls(private val restClient: RestClient) {{
    fun lambdaApplied(client: RestTemplate) {{
        client.get("/users/{{id}}") {{ it }}
    }}
}}
{PROBE}
"#
    ));
    assert_eq!(
        lambda_applied,
        ["GET /probe"],
        "a lambda-applied `receiver.verb(\"/p\") {{ … }}` is the shape of a route \
         REGISTRATION, and is excluded by pattern 4's position rule; the probe \
         proves the file was genuinely scanned: {lambda_applied:?}"
    );
}

/// **String template — REFUSED as `base-url-runtime` (S-342 AC4).** Kotlin's
/// interpolation is `"$base/users"` / `"${base}/users"`, not Java's `${…}`
/// *property placeholder*, and it needs its own test because the grammar treats
/// the two spellings differently — and one of them dangerously.
///
/// All three fixtures are refused by the same mechanism: pattern 1's
/// `(#not-match? @invoke.http.arg "[$]")` guard rejects the literal, and pattern
/// 1b re-captures the argument node so the site still reaches the interpreter
/// carrying the dynamic-path marker. That the reason really is
/// `base-url-runtime` — rather than the site silently never existing — is
/// asserted at slot level by
/// `extract::tests::a_kotlin_string_template_reaches_the_interpreter_as_a_dynamic_path`,
/// which is where the refusal reason is observable.
///
/// `trailingSegment` is the fixture the guard exists for. `bare` and `braced`
/// would be refused even without it (the first reads back *relative*, the second
/// carries an `interpolation` node `static_string_literal` rejects), but
/// `"/users/$id/roles"` reads back **absolute** — kotlin-ng merely splits the
/// fragment at the `$` into plain `string_content` children — so before the
/// guard it bound the runtime-composed template `/users/$id/roles`.
#[test]
fn a_string_template_path_emits_no_reference() {
    let (references, refusals) = client_call_rows(
        r#"
class Calls(private val restClient: RestClient, private val base: String) {
    fun bare(): String {
        return restClient.get().uri("$base/users").retrieve().body(String::class.java)
    }
    fun braced(): String {
        return restClient.get().uri("${base}/users").retrieve().body(String::class.java)
    }
    fun trailingSegment(id: String): String {
        return restClient.get().uri("/users/$id/roles").retrieve().body(String::class.java)
    }
}
"#,
    );
    assert!(
        references.is_empty(),
        "a Kotlin string template composes its path at runtime — no reference \
         and no approximate bind: {references:?}"
    );
    // Since S-374 the refusal is also RECORDED, one keyless row per declining
    // function — so the doc comment above no longer has to send a reader to a
    // slot-level test to see that the reason is `base-url-runtime` rather than
    // the site never existing: the row is the observable.
    assert_eq!(refusals, 3);
}

// ── 3. The shared negative-case fixture contract (S-340, [FR-WS-08]) ─────────

/// Shared negative case **1** — a same-shaped non-HTTP receiver call. Defined
/// once in [FR-WS-08]'s "Shared negative-case fixture contract" section; this
/// story only proves Kotlin's half of it: the file carries **no** HTTP-client
/// import, so the arm's ledger gate never scans it and `cache.get("k")` cannot
/// fabricate anything.
#[test]
fn a_non_client_file_is_never_scanned_for_client_calls() {
    assert!(
        client_calls_raw(
            r#"package com.example

class Calls {
    private val cache: MutableMap<String, String> = mutableMapOf()
    fun lookup(): String? {
        return cache.get("k")
    }
    fun routeShapedKey(): String? {
        return cache.get("/admin/users")
    }
}
"#
        )
        .is_empty(),
        "no HTTP-client import ⇒ the file is never scanned, so neither the \
         plain key nor the route-shaped one becomes a cross-service edge"
    );
}

/// **Stated over-capture ceiling** — the ledger gate is *file*-grained, so the
/// negative case above holds only across files. Inside a file that already
/// references a client package, a same-shaped collection call with a
/// route-shaped key still captures.
///
/// Inherited from the Rust arm. Java carried the identical ceiling on the same
/// ported query until S-375 ([CR-120]) closed it with a receiver-NAME boundary
/// rule — so the claim this docstring used to make, that no query can
/// distinguish `perms.get("/admin/users")` from `client.get("/admin/users")`
/// without receiver typing, is retired. The same rule ports here; until it does,
/// this test pins the current behaviour so that narrowing it is a deliberate
/// change, and so a reader of the test above cannot mistake the file-level gate
/// for a general guarantee.
///
/// [CR-120]: ../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
#[test]
fn a_route_shaped_collection_get_inside_a_client_file_is_a_stated_ceiling() {
    assert_eq!(
        client_calls(
            r#"
class Calls(private val restClient: RestClient) {
    private val perms: MutableMap<String, String> = mutableMapOf()
    fun notACall(): String? {
        return perms.get("/admin/users")
    }
}
"#
        ),
        ["GET /admin/users"],
        "file-grained gate: a route-shaped collection key inside a client file \
         is still captured — the documented ADR-54 accuracy ceiling"
    );
}

/// Shared negative case **2** — `base-url-runtime`. A bare-variable path and a
/// base-URL-composed one each emit **no** reference. The classification itself
/// is generic and already fixture-pinned in
/// `resolve::http_client_call::classify_client_call`; what this asserts is that
/// Kotlin's query fills the interpreter's slots such that the refusal fires.
#[test]
fn a_runtime_composed_path_emits_no_reference() {
    let (references, refusals) = client_call_rows(
        r#"
class Calls(private val restClient: RestClient, private val baseUrl: String) {
    fun bareVariable(path: String): String {
        return restClient.get().uri(path).retrieve().body(String::class.java)
    }
    fun composed(id: String): String {
        return restClient.get().uri(baseUrl + "/users/" + id).retrieve().body(String::class.java)
    }
    fun relative(): String {
        return restClient.get().uri("users/me").retrieve().body(String::class.java)
    }
    fun propertyPlaceholder(): String {
        return restClient.get().uri("\${users.service.url}/users").retrieve().body(String::class.java)
    }
    fun helperCall(id: String): String {
        return restClient.get().uri(buildUserUrl(id)).retrieve().body(String::class.java)
    }
}
"#,
    );
    assert!(
        references.is_empty(),
        "a bare variable, a concatenation, a relative literal, an escaped \
         Spring `${{…}}` placeholder literal and a helper-method call are each \
         base-url-runtime — no reference and no approximate bind: {references:?}"
    );
    assert_eq!(
        refusals, 5,
        "and each declining FUNCTION leaves one keyless row (S-374)"
    );
}

/// Shared negative case **3** — `path-not-composed`. A static, absolute literal
/// that does not positionally normalize is refused rather than approximated.
#[test]
fn an_absolute_but_non_normalizing_path_emits_no_reference() {
    assert!(
        client_calls(
            r#"
class Calls(private val restClient: RestClient) {
    fun catchAll(): String {
        return restClient.get().uri("/files/**").retrieve().body(String::class.java)
    }
}
"#
        )
        .is_empty(),
        "a catch-all template is honestly unbound, never approximately matched — \
         no reference AND, deliberately, no recorded refusal (that reason needs a \
         non-keyless row, S-374)"
    );
}

/// **The wrapped non-normalizing literal: two patterns, one call, and neither
/// half may report the other's reason** (S-374, [NFR-CC-04]).
///
/// Kotlin inherits Java's `.uri(URI.create(…))` unwrap, so it inherits the same
/// two-pattern overlap: the fluent verb-then-`uri` pattern's operand is the
/// whole `URI.create(…)` expression (no static literal ⇒ `base-url-runtime`),
/// the unwrapping pattern's operand is the literal. When the literal keys, the
/// reference and the cancellation are pinned by
/// [`a_jdk_http_client_builder_yields_one_reference_in_either_order`]; here it
/// does not key, so the unwrapping pattern classifies it `path-not-composed` and
/// records nothing — and that judgement must still cancel the enclosing match,
/// or a static absolute literal is filed as a runtime-composed path.
#[test]
fn a_wrapped_non_normalizing_literal_records_neither_a_reference_nor_a_refusal() {
    let (references, refusals) = client_call_rows(
        r#"
class Calls {
    fun catchAll(): HttpRequest =
        HttpRequest.newBuilder().GET().uri(URI.create("/files/**")).build()
}
"#,
    );
    assert!(
        references.is_empty(),
        "a catch-all template is never approximately matched: {references:?}"
    );
    assert_eq!(
        refusals, 0,
        "and the inner literal's `path-not-composed` judgement cancels the outer \
         match's candidate — a static literal is not `base-url-runtime`"
    );
}

// ── 4. A route registration is not a client call (S-345's re-check) ──────────

/// **The phantom-edge class this arm must never enter** ([NFR-RA-05]).
///
/// S-345's first Go draft captured four phantom outbound calls from one Gin
/// provider's own route registrations; each would have bound another workspace
/// member's matching route and fabricated a cross-service edge. Kotlin's mirror
/// of that trap was hunted for deliberately and comes in four shapes, refused by
/// three structural rules — never by a text predicate on the receiver name,
/// which is the fragile heuristic [CR-110] is currently cleaning up after.
///
/// [CR-110]: ../../docs/requests/CR-110-framework-route-false-positives.md
#[test]
fn route_registrations_are_never_captured_as_outbound_calls() {
    for (label, body) in [
        (
            "Spring WebFlux Kotlin router DSL (a provider, not a call)",
            r#"fun routes() = router { GET("/users") { req -> ok() } }"#,
        ),
        (
            "Ktor routing DSL",
            r#"fun routes() { routing { get("/users") { call.respond(x) } } }"#,
        ),
        (
            "RouterFunctions (refused by verb-name and arity, NOT the receiver rule)",
            r#"fun routes() = RouterFunctions.route(GET("/users"), handler)"#,
        ),
        (
            "Javalin, handler as a positional argument",
            r#"fun routes(app: Javalin) { app.get("/users", handler) }"#,
        ),
        (
            "Javalin, handler as a trailing lambda",
            r#"fun routes(app: Javalin) { app.get("/users") { ctx -> ctx.json(x) } }"#,
        ),
        (
            "Spring MockMvc Kotlin DSL — a call to this service's OWN route",
            r#"fun t(mockMvc: MockMvc) { mockMvc.get("/api/users") { accept = JSON } }"#,
        ),
    ] {
        let calls = client_calls(&format!("class Calls(private val restClient: RestClient)\n{body}\n{PROBE}"));
        assert_eq!(
            calls,
            ["GET /probe"],
            "{label} must emit no outbound call, and the file was genuinely \
             scanned: {calls:?}"
        );
    }
}

/// **The lower-case-receiver rule, pinned in its own right.**
///
/// `route_registrations_are_never_captured_as_outbound_calls` above does *not*
/// exercise it: every fixture there is refused by a different rule
/// (`RouterFunctions.route(…)` because `route` is not an HTTP verb and it takes
/// two arguments; the DSL forms because they have no receiver). So the
/// `(#match? @_recv "^[a-z_]")` guard — the one that keeps a **type**-qualified
/// call from being read as an outbound call — had no test at all, and could be
/// deleted with the whole suite still green.
///
/// It is not a hypothetical. `RequestPredicates.GET("/users")` is a Spring
/// WebFlux **route declaration**: an upper-case receiver, an HTTP-verb member
/// name and a sole absolute string literal, in a plain statement position. It is
/// the exact shape pattern 4 matches, and capturing it would record a provider
/// as calling its own endpoint — binding another workspace member's real
/// `GET /users` and fabricating a cross-service edge ([NFR-RA-05]).
///
/// This test exists because that is precisely what happened: the guard was
/// written but not attached (its predicate sat outside the alternation, so
/// tree-sitter parsed it as a separate pattern), and nothing here noticed.
#[test]
fn a_class_qualified_receiver_is_never_captured() {
    for (label, body) in [
        (
            "RequestPredicates.GET as a statement",
            r#"fun routes() { RequestPredicates.GET("/users") }"#,
        ),
        (
            "RequestPredicates.GET as a property initialiser",
            r#"fun routes() { val p = RequestPredicates.GET("/users") }"#,
        ),
        (
            "a non-HTTP class-qualified call that merely looks like one",
            r#"fun f() { val p = Paths.get("/etc/hosts") }"#,
        ),
    ] {
        let calls = client_calls(&format!("class Calls(private val restTemplate: RestTemplate)\n{body}\n{PROBE}"));
        assert_eq!(
            calls,
            ["GET /probe"],
            "{label}: a receiver must be a value, never a type — and the file \
             was genuinely scanned: {calls:?}"
        );
    }
}

/// **The `$`-guard on the plain receiver-method idiom, pinned in its own right.**
///
/// `a_string_template_path_emits_no_reference` exercises only the *fluent*
/// `.uri(…)` form, whose guard lives on pattern 1. Pattern 4 carries its own
/// copy, and it too was unattached and untested: with it gone,
/// `static_string_literal` concatenates kotlin-ng's split `string_content`
/// children back into `/carts/$id`, `classify_client_call` sees an absolute
/// literal, and the arm emits `DELETE /carts/$id` — a runtime-composed path
/// bound as though it were static ([NFR-RA-05]).
#[test]
fn an_interpolated_path_on_the_receiver_method_idiom_emits_no_reference() {
    for (label, body) in [
        (
            "block statement",
            r#"fun drop(id: String) { restTemplate.delete("/carts/$id") }"#,
        ),
        (
            "expression body",
            r#"fun drop(id: String) = restTemplate.delete("/carts/$id")"#,
        ),
        (
            "return position",
            r#"fun drop(id: String) { return restTemplate.delete("/carts/$id") }"#,
        ),
    ] {
        let calls = client_calls(&format!(
            "class Calls(private val restTemplate: RestTemplate) {{\n    {body}\n}}\n{PROBE}"
        ));
        assert_eq!(
            calls,
            ["GET /probe"],
            "{label}: a Kotlin string template is composed at runtime and must \
             never be bound as a static template — and the file was genuinely \
             scanned: {calls:?}"
        );
    }
}

/// **Stated over-capture ceiling — a route registration whose handler is
/// CHAINED rather than passed.**
///
/// Pattern 4's position rule excludes a trailing-lambda registration
/// (`app.get("/x") { … }`), but not one that takes its handler on a *later*
/// link (`val route = router.get("/x"); route.handler { … }` — Vert.x, http4k)
/// nor a bare MockMvc DSL call with no lambda. Structurally these are
/// indistinguishable from `restTemplate.delete("/x")`: lower-case receiver, HTTP
/// verb, sole absolute literal, statement or property position. No query can
/// separate them without receiver typing.
///
/// This is the same class as
/// [`a_route_shaped_collection_get_inside_a_client_file_is_a_stated_ceiling`]
/// and is pinned the same way — asserting the *current* (over-capturing)
/// behaviour, so that narrowing it later is a deliberate change rather than an
/// accident, and so the ceilings list cannot claim more than it delivers.
///
/// Note the direct-chain spelling (`router.post("/orders").handler { … }`) is
/// correctly refused: there the call is the receiver of a further navigation,
/// which is not one of pattern 4's positions.
#[test]
fn a_chained_handler_route_registration_is_a_stated_over_capture_ceiling() {
    assert_eq!(
        client_calls(
            r#"
class Calls(private val restTemplate: RestTemplate) {
    fun register(router: Router) {
        val route = router.get("/vertx-users")
        route.handler { ctx -> ctx.end() }
    }
    fun probe(mockMvc: MockMvc) {
        mockMvc.get("/mockmvc-orders")
    }
    fun direct(router: Router) {
        router.post("/chained").handler { ctx -> ctx.end() }
    }
}
"#
        ),
        ["GET /mockmvc-orders", "GET /vertx-users"],
        "the chained-handler and bare-DSL registrations are over-captured (the \
         documented ceiling); the DIRECT chain is correctly refused, so no \
         `POST /chained` appears"
    );
}

/// `URI.create(…)` is anchored on its **receiver type**, so an unrelated
/// `create` factory does not smuggle a path into the JDK-builder patterns.
#[test]
fn only_uri_create_unwraps_a_builder_path() {
    let (references, refusals) = client_call_rows(
        r#"
class Calls {
    fun a(): HttpRequest {
        return HttpRequest.newBuilder().GET().uri(MyFactory.create("/internal/{id}"))
    }
}
"#,
    );
    assert!(
        references.is_empty(),
        "`MyFactory.create(…)` is not `URI.create(…)` — no path is unwrapped: \
         {references:?}"
    );
    // The `.uri(…)` link is still matched, so the site exists with a non-literal
    // operand: one recorded `base-url-runtime` refusal (S-374), not a silent drop.
    assert_eq!(refusals, 1);
}

// ── 5. Stated coverage ceilings ([ADR-54]) ──────────────────────────────────

/// **Ceiling.** `RestTemplate`'s verb-suffixed methods encode the HTTP verb in
/// the *method name* (`getForObject`), and `exchange` puts it in a **second**
/// argument (`HttpMethod.GET`). The arm's `@invoke.http.method` slot needs a
/// node whose text is literally an HTTP verb, so both are dropped by the generic
/// dispatch's `is_http_method` check — identical to Java's ceiling.
///
/// The method-alias table this used to defer to (CR-108 CRA-05) landed later in
/// the same sprint as S-346's `[invocation_methods]`, so `getForObject` is now
/// liftable by descriptor data alone; `exchange` still is not, its verb binding
/// to no capture. The query header states the filter-half trade that keeps the
/// row undeclared.
#[test]
fn rest_template_verb_suffixed_methods_are_a_stated_ceiling() {
    assert!(
        client_calls(
            r#"
class Calls(private val restTemplate: RestTemplate) {
    fun forObject(id: String): String? {
        return restTemplate.getForObject("/carts/{id}", String::class.java)
    }
    fun exchange(id: String): Any? {
        return restTemplate.exchange("/carts/{id}", HttpMethod.GET, null, String::class.java)
    }
}
"#
        )
        .is_empty(),
        "`getForObject` is not an HTTP verb and `exchange` carries its verb in a \
         second argument — both stay honestly uncaptured"
    );
}

/// **Ceiling — a deliberate divergence from Java.** A multi-argument
/// receiver-method call is not captured, where Java captures
/// `restTemplate.put("/carts/{id}", body)`.
///
/// The reason is a Kotlin-specific one: Kotlin has route-registration APIs of
/// exactly that shape (`app.get("/users", handler)`), and nothing structural
/// separates a request body from a handler — the only text available is the
/// receiver name. Refusing both is the [NFR-RA-05] direction, and it is Go's
/// argument-arity discriminator applied to the language that needs it.
#[test]
fn a_multi_argument_receiver_call_is_a_stated_ceiling() {
    assert!(
        client_calls(
            r#"
class Calls(private val restTemplate: RestTemplate) {
    fun replace(id: String, body: Any) {
        restTemplate.put("/carts/{id}", body)
    }
}
"#
        )
        .is_empty(),
        "the path must be the sole argument — a trailing argument is what a \
         route registration's handler looks like"
    );
}

/// **Ceiling.** A receiver-method call outside pattern 4's four positions.
///
/// The position rule is what excludes a trailing-lambda route registration, and
/// it costs recall in exchange: a call nested as an argument, extended by a
/// further `.also { … }`, or — the common one — written inside a **lambda body**
/// is not captured, because a lambda body is not a `block` node in this grammar.
/// Under-capture is the safe direction ([NFR-RA-05]), but it is a real gap and a
/// reader deserves to see its shape rather than infer it.
#[test]
fn a_receiver_method_call_outside_pattern_4s_positions_is_a_stated_ceiling() {
    for (label, body) in [
        (
            "inside a lambda body",
            r#"fun dropAll(ids: List<String>) { ids.forEach { restTemplate.delete("/carts/{id}") } }"#,
        ),
        (
            "nested as an argument",
            r#"fun drop(id: String) { log(restTemplate.delete("/carts/{id}")) }"#,
        ),
        (
            "extended by a further chained call",
            r#"fun drop(id: String) { restTemplate.delete("/carts/{id}").also { audit(it) } }"#,
        ),
    ] {
        assert!(
            client_calls(&format!(
                "class Calls(private val restTemplate: RestTemplate) {{\n    {body}\n}}"
            ))
            .is_empty(),
            "{label} is outside pattern 4's stated positions and is not captured"
        );
    }
}

/// **Not a ceiling — a Kotlin/Java divergence, in the good direction.** A Kotlin
/// *raw string* (`"""…"""`) path literal **is** captured, where Java's text
/// block is not.
///
/// The reason is a grammar one, established by dumping the tree rather than
/// assumed from Java: kotlin-ng renders `"""/users/{id}"""` as
/// `(multiline_string_literal (string_content))`, and `string_content` is one of
/// the child kinds `static_string_literal` accepts — whereas Java's text block
/// exposes a `multiline_string_fragment`, which it does not. Pinned here so the
/// divergence is a recorded fact rather than an accident nobody checked.
#[test]
fn a_raw_string_path_literal_is_captured() {
    assert_eq!(
        client_calls(
            "\nclass Calls(private val restClient: RestClient) {\n    fun a(): String {\n        return restClient.get().uri(\"\"\"/users/{id}\"\"\").retrieve().body(String::class.java)\n    }\n}\n"
        ),
        ["GET /users/{id}"],
        "kotlin-ng exposes a raw string's body as `string_content`, which \
         `static_string_literal` reads"
    );
}

/// **Ceiling.** The JDK builder patterns anchor on the *adjacent* `.uri(…)`/verb
/// link pair. An intervening builder call separates them, and
/// `HttpRequest.newBuilder(URI.create(…))` carries no verb link at all — both
/// refuse whole rather than emitting a verb-less or path-less half.
#[test]
fn a_separated_jdk_builder_chain_is_a_stated_ceiling() {
    assert!(
        client_calls(
            r#"
class Calls {
    fun separated(): HttpRequest {
        return HttpRequest.newBuilder().uri(URI.create("/hdr")).header("a", "b").GET().build()
    }
    fun ctor(): HttpRequest {
        return HttpRequest.newBuilder(URI.create("/ctor")).GET().build()
    }
}
"#
        )
        .is_empty(),
        "refused whole, never partially — no verb-less or path-less half is emitted"
    );
}

/// **Ceiling.** OpenFeign declares the path on an *annotated interface method*
/// with no call-site receiver: the verb is the annotation name `GetMapping` and
/// the parse tree is a declaration, not a `call_expression`. Neither the anchor
/// nor the verb is expressible in the arm's capture vocabulary, so it is
/// recorded as a ceiling rather than worked around ([ADR-54]) — Java's identical
/// finding, re-pinned here against the Kotlin grammar.
#[test]
fn openfeign_interfaces_are_a_stated_ceiling() {
    assert!(
        client_calls_raw(
            r#"package com.example

import org.springframework.cloud.openfeign.FeignClient
import org.springframework.web.bind.annotation.GetMapping
import org.springframework.web.client.RestClient

@FeignClient(name = "users", url = "http://users")
interface UserApi {
    @GetMapping("/users/{id}")
    fun byId(id: String): String
}
"#
        )
        .is_empty(),
        "the verb is an annotation name on a function declaration — there is no \
         call to anchor on, in a file that passes the ledger gate"
    );
}
