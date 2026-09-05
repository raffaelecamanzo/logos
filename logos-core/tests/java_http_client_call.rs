//! Java HTTP client-call capture (S-341, [CR-108], [FR-WS-08], [ADR-54],
//! [NFR-RA-05]).
//!
//! Drives the **real** pipeline — the compiled `tree-sitter-java` grammar, the
//! plugin's `invocations` query, the generic dispatch's HTTP-verb and
//! static-literal decisions, and `resolve::http_client_call`'s normalizer — over
//! one fixture per idiom in [FR-WS-08]'s normative Java row, so a query that
//! compiles but captures nothing cannot pass ([CR-101]'s lesson).
//!
//! It pins three things a reader has to be able to trust:
//!
//! 1. **What is captured** — the four-idiom matrix, each rendering exactly one
//!    `"METHOD /template"` reference keyed through the same `route_key` the
//!    provider side reduces to ([FR-CG-09]).
//! 2. **What is refused** — [FR-WS-08]'s shared negative-case fixture contract
//!    (S-340), referenced rather than re-invented here.
//! 3. **What is a stated ceiling** — the Java idioms the arm's capture
//!    vocabulary provably cannot express, asserted as *zero* references so the
//!    ceiling is pinned in a test rather than only claimed in prose ([ADR-54]).
//!
//! Gated on the Java grammar so a build excluding it does not run it.
//!
//! [CR-101]: ../../docs/requests/CR-101-jvm-spring-route-extraction.md
//! [CR-108]: ../../docs/requests/CR-108-per-language-http-client-call-capture.md
//! [FR-CG-09]: ../../docs/specs/requirements/FR-CG-09.md
//! [FR-WS-08]: ../../docs/specs/requirements/FR-WS-08.md
//! [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
//! [ADR-54]: ../../docs/specs/architecture/decisions/ADR-54.md
#![cfg(feature = "lang-java")]

use std::fs;

use logos_core::Engine;

/// The `import`s that make a file a client-call candidate under the arm's
/// ledger gate (this plugin's own `http_client_detectors` descriptor rows) — the
/// consumer-side twin of [FR-FW-04]'s framework candidacy. Prepended to every
/// positive fixture; deliberately **absent** from the negative-case-1 fixture.
const CLIENT_IMPORTS: &str = r#"
import org.springframework.web.client.RestClient;
import org.springframework.web.client.RestTemplate;
import org.springframework.web.reactive.function.client.WebClient;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.URI;
"#;

/// Index `body` as a single Java file and return every `http-client-call`
/// reference target the arm wrote to the ledger, sorted.
fn client_calls(body: &str) -> Vec<String> {
    client_calls_raw(&format!("package com.example;\n{CLIENT_IMPORTS}\n{body}"))
}

/// As [`client_calls`], but the caller supplies the whole compilation unit —
/// used by the negative case that must ship *without* the client imports.
fn client_calls_raw(source: &str) -> Vec<String> {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(tmp.path().join("src")).expect("mkdir");
    fs::write(tmp.path().join("src/Calls.java"), source).expect("write fixture");

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

// ── 1. The four-idiom matrix ([FR-WS-08]'s normative Java row) ───────────────

/// `RestClient` — the fluent chain whose verb and path sit on **different**
/// links. Captured, not refused: this is the S-341 fluent-chain decision, and
/// the dominant real-world Spring shape.
#[test]
fn a_rest_client_fluent_chain_yields_one_reference() {
    assert_eq!(
        client_calls(
            r#"
public class Calls {
    private RestClient restClient;
    String user(String id) {
        return restClient.get().uri("/users/{id}").retrieve().body(String.class);
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
public class Calls {
    private WebClient webClient;
    Object order(String id) {
        return webClient.post().uri("/orders/{id}").retrieve().bodyToMono(String.class).block();
    }
}
"#
        ),
        ["POST /orders/{id}"]
    );
}

/// `RestTemplate` — the plain receiver-method idiom (the shape the Rust arm's
/// single pattern captures), here on the bare-verb methods.
///
/// Its verb-suffixed siblings (`getForObject`, `exchange`, …) are a **stated
/// ceiling**, pinned by
/// [`rest_template_verb_suffixed_methods_are_a_stated_ceiling`].
#[test]
fn a_rest_template_receiver_call_yields_one_reference() {
    assert_eq!(
        client_calls(
            r#"
public class Calls {
    private RestTemplate restTemplate;
    void drop(String id) {
        restTemplate.delete("/carts/{id}");
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
public class Calls {
    private HttpClient httpClient;
    HttpRequest uriThenVerb() {
        return HttpRequest.newBuilder().uri(URI.create("/items/{id}")).GET().build();
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
public class Calls {
    private HttpClient httpClient;
    HttpRequest verbThenUri() {
        return HttpRequest.newBuilder().POST(body).uri(URI.create("/items")).build();
    }
}
"#
        ),
        ["POST /items"],
        "the reversed `.POST(…).uri(URI.create(…))` order"
    );
}

// ── 2. The shared negative-case fixture contract (S-340, [FR-WS-08]) ─────────

/// Shared negative case **1** — a same-shaped non-HTTP receiver call. Defined
/// once in [FR-WS-08]'s "Shared negative-case fixture contract" section; this
/// story only proves Java's half of it: the file carries **no** HTTP-client
/// import, so the arm's ledger gate never scans it and `cache.get("k")` cannot
/// fabricate anything.
#[test]
fn a_non_client_file_is_never_scanned_for_client_calls() {
    assert!(
        client_calls_raw(
            r#"package com.example;
import java.util.Map;

public class Calls {
    private Map<String, String> cache;
    String lookup() {
        return cache.get("k");
    }
    String routeShapedKey() {
        return cache.get("/admin/users");
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
/// Inherited verbatim from the Rust arm, whose `HTTP_METHODS` rustdoc already
/// records it ("a genuine HTTP-client file that also does an incidental
/// `/`-keyed collection `.get` is a documented accuracy ceiling") — no query
/// can distinguish `perms.get("/admin/users")` from `client.get("/admin/users")`
/// without receiver typing. Pinned here rather than left to prose so that
/// narrowing it later is a deliberate change, and so a reader of the test above
/// cannot mistake the file-level gate for a general guarantee.
///
/// This is the consumer-side twin of the provider-side false positives
/// [S-350](../../docs/planning/journal.md) removed; whether to close it is a
/// sprint-review decision, recorded in the S-341 implementation notes.
#[test]
fn a_route_shaped_collection_get_inside_a_client_file_is_a_stated_ceiling() {
    assert_eq!(
        client_calls(
            r#"
public class Calls {
    private RestClient restClient;
    private java.util.Map<String, String> perms;
    String notACall() {
        return perms.get("/admin/users");
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
/// Java's query fills the interpreter's slots such that the refusal fires.
#[test]
fn a_runtime_composed_path_emits_no_reference() {
    assert!(
        client_calls(
            r#"
public class Calls {
    private RestClient restClient;
    private String baseUrl;
    String bareVariable(String path) {
        return restClient.get().uri(path).retrieve().body(String.class);
    }
    String composed(String id) {
        return restClient.get().uri(baseUrl + "/users/" + id).retrieve().body(String.class);
    }
    String relative() {
        return restClient.get().uri("users/me").retrieve().body(String.class);
    }
    String propertyPlaceholder() {
        return restClient.get().uri("${users.service.url}/users").retrieve().body(String.class);
    }
    String builderLambda() {
        return restClient.get().uri(b -> b.path("/users").build()).retrieve().body(String.class);
    }
    String helperCall(String id) {
        return restClient.get().uri(buildUserUrl(id)).retrieve().body(String.class);
    }
}
"#
        )
        .is_empty(),
        "a bare variable, a concatenation, a relative literal, a `$` \
         placeholder literal, a builder lambda and a helper-method call are \
         each \
         base-url-runtime — no reference, no ledger entry, no approximate bind"
    );
}

/// The three shapes above are not hypothetical: they are what the real
/// `pec-services` estate is made of. Measured across its 2 447 Java files,
/// **0 of 124** `.uri(…)` arguments is a string literal of any kind — 68 are
/// bare variables or constant fields, 39 are helper-method calls and 13 are
/// builder lambdas. Pinning the dominant shapes here is what stops a later
/// story "improving" one of them into a fabricated bind ([ADR-54] keeps
/// base-URL composition out of scope precisely because composing it would be
/// fabrication).
///
/// An absolute literal carrying a `${…}` segment takes the *other* refusal
/// path — it is absolute, so it reaches the template normalizer and is refused
/// as `path-not-composed` rather than `base-url-runtime`. Both refuse; the
/// distinction matters only to the coverage vocabulary.
#[test]
fn an_interpolated_segment_in_an_absolute_path_is_also_refused() {
    assert!(
        client_calls(
            r#"
public class Calls {
    private RestClient restClient;
    String interpolatedSegment() {
        return restClient.get().uri("/api/${version}/users").retrieve().body(String.class);
    }
}
"#
        )
        .is_empty(),
        "an absolute literal whose segment is interpolated does not normalize"
    );
}

/// Shared negative case **3** — `path-not-composed`. A static, absolute literal
/// that does not positionally normalize is refused rather than approximated.
#[test]
fn an_absolute_but_non_normalizing_path_emits_no_reference() {
    assert!(
        client_calls(
            r#"
public class Calls {
    private RestClient restClient;
    String catchAll() {
        return restClient.get().uri("/files/**").retrieve().body(String.class);
    }
}
"#
        )
        .is_empty(),
        "a catch-all template is honestly unbound, never approximately matched"
    );
}

/// A receiver-less or class-qualified verb call is **never** an outbound call
/// ([NFR-RA-05]). Java's `method_invocation` makes `object:` optional, so the
/// plain receiver-method pattern would otherwise match the static-import idioms
/// that dominate Java routing and test DSLs — inside a file that passes the
/// ledger gate, which these routinely do (a client test stubs the downstream it
/// calls; a WebFlux BFF holds both a `WebClient` and functional routes).
///
/// Two of these invert direction outright: `RouterFunctions`/`RequestPredicates`
/// declare a **provider** route, so capturing them would record a service as
/// calling its own endpoint and — via the `invocation` intake — seed a false
/// app-wide reachability root ([FR-WS-12]).
#[test]
fn receiver_less_and_class_qualified_verb_calls_are_never_captured() {
    for (label, body) in [
        (
            "RouterFunctions route declaration (a provider, not a call)",
            r#"Object routes() { return RouterFunctions.route(GET("/users"), handler::all); }"#,
        ),
        (
            "RequestPredicates, class-qualified",
            r#"Object routes() { return route(RequestPredicates.GET("/users"), handler::all); }"#,
        ),
        (
            "MockMvcRequestBuilders static import",
            r#"void t() throws Exception { mockMvc.perform(get("/api/users")); }"#,
        ),
        (
            "WireMock stubFor",
            r#"void t() { stubFor(get("/api/users").willReturn(ok())); }"#,
        ),
        (
            "Apache Camel rest DSL (a chained, non-identifier receiver)",
            r#"void t() { rest("/api").get("/{id}").to("direct:x"); }"#,
        ),
    ] {
        // `restClient.get().uri("/probe")` is the positive control: it proves
        // the file WAS scanned, so each shape's absence is pattern 4's receiver
        // rule refusing it and not a closed ledger gate. Without it a later
        // tightening of `http_client_detectors` would leave this test green
        // while proving nothing — the class the Go and C# arms closed in this
        // same sprint (`go_invocations.rs`, `c_sharp_invocations.rs`).
        let calls = client_calls(&format!(
            "public class Calls {{ private RestClient restClient; {body} \
             String probe() {{ return restClient.get().uri(\"/probe\").retrieve().body(String.class); }} }}"
        ));
        assert_eq!(
            calls,
            ["GET /probe"],
            "{label} must emit no outbound call, and the file was genuinely \
             scanned: {calls:?}"
        );
    }
}

/// `URI.create(…)` is anchored on its **receiver type**, so an unrelated
/// `create` factory does not smuggle a path into the JDK-builder patterns.
#[test]
fn only_uri_create_unwraps_a_builder_path() {
    assert!(
        client_calls(
            r#"
public class Calls {
    HttpRequest a() {
        return HttpRequest.newBuilder().GET().uri(MyFactory.create("/internal/{id}"));
    }
}
"#
        )
        .is_empty(),
        "`MyFactory.create(…)` is not `URI.create(…)` — no path is unwrapped"
    );
}

// ── 3. Stated coverage ceilings ([ADR-54]) ──────────────────────────────────

/// **Ceiling.** `RestTemplate`'s verb-suffixed methods encode the HTTP verb in
/// the *method name* (`getForObject`), and `exchange` puts it in a **second**
/// argument (`HttpMethod.GET`). The arm's `@invoke.http.method` slot needs a node
/// whose text is literally an HTTP verb, so both are dropped by the generic
/// dispatch's `is_http_method` check.
///
/// Asserted as zero rather than left to prose: if a later change starts
/// capturing these, this test fails and the ceiling is re-decided deliberately.
#[test]
fn rest_template_verb_suffixed_methods_are_a_stated_ceiling() {
    assert!(
        client_calls(
            r#"
public class Calls {
    private RestTemplate restTemplate;
    String forObject(String id) {
        return restTemplate.getForObject("/carts/{id}", String.class);
    }
    Object exchange(String id) {
        return restTemplate.exchange("/carts/{id}", HttpMethod.GET, null, String.class);
    }
}
"#
        )
        .is_empty(),
        "`getForObject` is not an HTTP verb and `exchange` carries its verb in a \
         second argument — both need a descriptor-level method-alias table the \
         arm does not have (CR-108 CRA-05), so they stay honestly uncaptured"
    );
}

/// **Ceiling.** A Java *text block* (`"""…"""`) path literal is statically
/// present but not captured: `static_string_literal` accepts only
/// `string_content` / `string_fragment` / `escape_sequence` children, and a text
/// block's `multiline_string_fragment` falls through to the dynamic branch. The
/// site is therefore refused as `base-url-runtime` — an honest refusal, but for
/// the wrong stated reason, since the path is fully static. Under-capture is
/// safe ([NFR-RA-05]); recorded here so it is a known ceiling rather than a
/// silent one.
#[test]
fn a_text_block_path_literal_is_a_stated_ceiling() {
    assert!(
        client_calls(
            "\npublic class Calls {\n    private RestClient restClient;\n    String a() {\n        return restClient.get().uri(\"\"\"\n/users/{id}\"\"\").retrieve().body(String.class);\n    }\n}\n"
        )
        .is_empty(),
        "a text-block literal is not recognised as a static string"
    );
}

/// **Ceiling.** OpenFeign declares the path on an *annotated interface method*
/// with no call-site receiver: the parse tree is
/// `(interface_declaration … (method_declaration (modifiers (annotation
/// name: (identifier) arguments: (annotation_argument_list (string_literal))))))`
/// — a declaration, not a `method_invocation`, and the verb is the annotation
/// name `GetMapping`. Neither the anchor nor the verb is expressible in the
/// arm's capture vocabulary, so it is recorded as a ceiling rather than worked
/// around ([ADR-54]). `pec-services` contains zero `@FeignClient`, so this is
/// not on the measured critical path.
#[test]
fn openfeign_interfaces_are_a_stated_ceiling() {
    assert!(
        client_calls_raw(
            r#"package com.example;
import org.springframework.cloud.openfeign.FeignClient;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.client.RestClient;

@FeignClient(name = "users", url = "http://users")
public interface UserApi {
    @GetMapping("/users/{id}")
    String byId(String id);
}
"#
        )
        .is_empty(),
        "the verb is an annotation name on a method_declaration — there is no \
         method_invocation to anchor on, in a file that passes the ledger gate"
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
public class Calls {
    HttpRequest separated() {
        return HttpRequest.newBuilder().uri(URI.create("/hdr")).header("a", "b").GET().build();
    }
    HttpRequest ctor() {
        return HttpRequest.newBuilder(URI.create("/ctor")).GET().build();
    }
}
"#
        )
        .is_empty(),
        "refused whole, never partially — no verb-less or path-less half is emitted"
    );
}
