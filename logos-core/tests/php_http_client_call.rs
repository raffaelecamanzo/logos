//! PHP HTTP client-call capture (S-348, [CR-108], [FR-WS-08], [FR-PL-07],
//! [ADR-54], [NFR-RA-05]).
//!
//! Drives the **real** pipeline — the compiled `tree-sitter-php` grammar (its
//! **full** `php` grammar, so HTML-interleaved templates parse), the plugin's
//! `invocations` query, the generic dispatch's HTTP-verb and static-literal
//! decisions, and `resolve::http_client_call`'s normalizer — over one fixture
//! per idiom in [FR-WS-08]'s normative PHP row (Guzzle), so a query that
//! compiles but captures nothing cannot pass ([CR-101]'s lesson).
//!
//! It pins four things a reader has to be able to trust:
//!
//! 1. **What is captured** — `$client->get('/p')`, `$client->request('GET',
//!    '/p')` (verb-as-first-argument) and `requestAsync`, each rendering
//!    exactly one `"METHOD /template"` reference keyed through the same
//!    `route_key` the provider side reduces to ([FR-CG-09]).
//! 2. **What is refused** — [FR-WS-08]'s shared negative-case fixture contract
//!    (S-340), referenced rather than re-invented here, *and* the
//!    registration-vs-client trap this story's own acceptance criteria single
//!    out: Slim's `$app->get('/x', $handler)` and Laravel's `Route::get('/x',
//!    $handler)` must never be read as an outbound call.
//! 3. **HTML-interleaved parity** ([FR-PL-07]) — the acceptance criterion a
//!    query written only against pure-PHP fixtures silently misses: a Guzzle
//!    call inside an HTML-interleaved template must capture identically to
//!    the same call in a pure-PHP file.
//! 4. **What is a stated ceiling** — the PHP idioms the arm's capture
//!    vocabulary provably cannot express, asserted as *zero* references so the
//!    ceiling is pinned in a test rather than only claimed in prose ([ADR-54]).
//!
//! Gated on the PHP grammar so a build excluding it does not run it.
//!
//! [CR-101]: ../../docs/requests/CR-101-jvm-spring-route-extraction.md
//! [CR-108]: ../../docs/requests/CR-108-per-language-http-client-call-capture.md
//! [FR-CG-09]: ../../docs/specs/requirements/FR-CG-09.md
//! [FR-WS-08]: ../../docs/specs/requirements/FR-WS-08.md
//! [FR-PL-07]: ../../docs/specs/requirements/FR-PL-07.md
//! [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
//! [ADR-54]: ../../docs/specs/architecture/decisions/ADR-54.md
#![cfg(feature = "lang-php")]

use std::fs;

use logos_core::Engine;

/// The `use` clause that makes a file a client-call candidate under the arm's
/// ledger gate (this plugin's own `http_client_detectors` descriptor row) —
/// the consumer-side twin of [FR-FW-04]'s framework candidacy. Prepended to
/// every positive fixture; deliberately **absent** from the negative-case-1
/// fixture.
const CLIENT_IMPORT: &str = "use GuzzleHttp\\Client;\n";

/// Index `body` as a single pure-PHP file (with the `<?php` tag and the client
/// import prepended) and return every `http-client-call` reference target the
/// arm wrote to the ledger, sorted.
fn client_calls(body: &str) -> Vec<String> {
    client_calls_raw(&format!("<?php\n{CLIENT_IMPORT}{body}"))
}

/// As [`client_calls`], but the caller supplies the whole file body (including
/// its own `<?php` tag(s)) — used by the negative case that must ship
/// *without* the client import, and by the HTML-interleaved fixture.
fn client_calls_raw(source: &str) -> Vec<String> {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(tmp.path().join("src")).expect("mkdir");
    fs::write(tmp.path().join("src/calls.php"), source).expect("write fixture");

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

// ── 1. The required call-form matrix ([FR-WS-08]'s normative PHP row) ───────

/// `$client->get('/users')` — the verb-as-method-name form, captured through a
/// bare `$var` receiver.
#[test]
fn a_verb_method_call_on_a_bare_variable_yields_one_reference() {
    assert_eq!(
        client_calls(
            r#"
$client = new Client();
$client->get('/users');
"#
        ),
        ["GET /users"]
    );
}

/// The property-access receiver twin — `$this->httpClient->get('/p')`, the
/// ordinary DI-injected-client shape.
#[test]
fn a_verb_method_call_through_a_property_access_receiver_yields_one_reference() {
    assert_eq!(
        client_calls(
            r#"
class UserService {
    private $httpClient;
    function fetch() {
        return $this->httpClient->get('/users');
    }
}
"#
        ),
        ["GET /users"]
    );
}

/// `$client->request('GET', '/users')` — the verb-as-first-argument form
/// consumed from Go/[S-345]'s constructor-argument decision. The verb comes
/// from the FIRST argument, the path from the second.
///
/// [S-345]: ../../docs/planning/journal.md#s-345-go-http-client-call-capture
#[test]
fn a_request_call_takes_its_verb_from_the_first_argument() {
    assert_eq!(
        client_calls(
            r#"
$client = new Client();
$client->request('GET', '/users');
"#
        ),
        ["GET /users"]
    );
}

/// A trailing `array $options` third argument does not break the
/// constructor-argument anchor — the pattern anchors only the first two
/// (adjacent, leading) arguments, never the whole list, mirroring Go's
/// [S-345](../../docs/planning/journal.md#s-345-go-http-client-call-capture)
/// constructor-argument pattern.
#[test]
fn a_trailing_options_array_argument_does_not_break_the_request_anchor() {
    assert_eq!(
        client_calls(
            r#"
$client = new Client();
$client->request('GET', '/users', ['json' => $body]);
"#
        ),
        ["GET /users"]
    );
}

/// A double-quoted verb literal (`"GET"`, tree-sitter-php's `encapsed_string`
/// grammar even with no interpolation) captures identically to a
/// single-quoted one (`'GET'`, `string`) — the two PHP quoting styles must
/// not disagree on whether a static verb is recognised.
#[test]
fn a_double_quoted_verb_literal_captures_identically_to_single_quoted() {
    assert_eq!(
        client_calls(
            r#"
$client = new Client();
$client->request("GET", '/users');
"#
        ),
        ["GET /users"]
    );
}

/// A lower-cased verb argument normalizes to the same key as the upper-cased
/// spelling — `is_http_method` is case-insensitive, and the normalizer upper-
/// cases the stored verb.
#[test]
fn a_lower_cased_verb_argument_normalizes_to_the_same_key() {
    assert_eq!(
        client_calls(
            r#"
$client = new Client();
$client->request('get', '/users');
"#
        ),
        ["GET /users"]
    );
}

/// `requestAsync('GET', '/users')` captures identically to `request` — its
/// verb comes from the first ARGUMENT, never the method name, so the `Async`
/// suffix on the method name is irrelevant to this anchor. See the query
/// header's "stated ceiling" note for why this differs from `getAsync`.
#[test]
fn request_async_captures_identically_to_request() {
    assert_eq!(
        client_calls(
            r#"
$client = new Client();
$client->requestAsync('POST', '/users');
"#
        ),
        ["POST /users"]
    );
}

/// `getAsync`/`postAsync`/… variants are **explicitly refused**, per
/// [FR-WS-08]'s "capture … or explicitly refused with their reason recorded"
/// acceptance criterion. The reason: the verb is embedded in the
/// `Async`-suffixed METHOD NAME, and `is_http_method` recognises only the bare
/// verbs — capturing this needs a text normalizer, C#/[S-346]'s stated scope
/// (a `[framework_methods]`-style `getasync → GET` table), not invented here
/// ahead of that story (NFR-MA-01). The single-argument pattern still
/// structurally MATCHES this call (proving the file was scanned); the verb
/// check is what refuses it.
///
/// [S-346]: ../../docs/planning/journal.md#s-346-c-http-client-call-capture
#[test]
fn async_suffixed_verb_method_calls_are_explicitly_refused() {
    for method in [
        "getAsync",
        "postAsync",
        "putAsync",
        "deleteAsync",
        "patchAsync",
        "headAsync",
        "optionsAsync",
    ] {
        // `$client->get('/probe')` is the positive control: it proves the
        // file WAS scanned, so `{method}(...)`'s absence is the verb check
        // refusing a structurally-matched call — not a closed ledger gate.
        let facts = client_calls(&format!(
            r#"
$client = new Client();
$client->{method}('/users');
$client->get('/probe');
"#
        ));
        assert_eq!(
            facts,
            ["GET /probe"],
            "{method} must be refused (Async-suffixed method-name verb, a \
             stated ceiling), not silently captured with the wrong verb, and \
             the file was genuinely scanned: {facts:?}"
        );
    }
}

// ── 2. The shared negative-case fixture contract (S-340, [FR-WS-08]) ────────

/// Shared negative case **1** — a same-shaped non-HTTP receiver call. Defined
/// once in [FR-WS-08]'s "Shared negative-case fixture contract" section; this
/// story only proves PHP's half of it: the file carries **no** `GuzzleHttp`
/// reference, so the arm's ledger gate never scans it and `$cache->get("k")`
/// cannot fabricate anything.
#[test]
fn a_non_client_file_is_never_scanned_for_client_calls() {
    assert!(
        client_calls_raw(
            r#"<?php
class Calls {
    private $cache;
    function lookup() {
        return $this->cache->get('k');
    }
    function routeShapedKey() {
        return $this->cache->get('/admin/users');
    }
}
"#
        )
        .is_empty(),
        "no GuzzleHttp reference ⇒ the file is never scanned, so neither the \
         plain key nor the route-shaped one becomes a cross-service edge"
    );
}

/// **Stated over-capture ceiling** — the ledger gate is *file*-grained, so the
/// negative case above holds only across files. Inside a file that already
/// references `GuzzleHttp`, a same-shaped collection call with a route-shaped
/// key still captures — inherited from every other language's arm and pinned
/// here rather than left to prose (see `HTTP_METHODS`'s rustdoc).
#[test]
fn a_route_shaped_property_get_inside_a_client_file_is_a_stated_ceiling() {
    assert_eq!(
        client_calls(
            r#"
class Calls {
    private $client;
    private $perms;
    function notACall() {
        return $this->perms->get('/admin/users');
    }
}
"#
        ),
        ["GET /admin/users"],
        "file-grained gate: a route-shaped property-call key inside a client \
         file is still captured — the documented ADR-54 accuracy ceiling"
    );
}

/// Shared negative case **2** — `base-url-runtime`. A concatenated path and an
/// interpolated one (both PHP spellings: `"{$base}/users"` and
/// `"$base/users"`) each emit **no** reference. The classification itself is
/// generic and already fixture-pinned in
/// `resolve::http_client_call::classify_client_call`; what this asserts is
/// that PHP's query fills the interpreter's slots such that the refusal
/// fires.
#[test]
fn a_runtime_composed_path_emits_no_reference() {
    assert!(
        client_calls(
            r#"
$client = new Client();
$base = 'https://example.test';
$client->get($base . '/users');
$client->get("{$base}/users");
$client->get("$base/users");
$client->get($base);
"#
        )
        .is_empty(),
        "a concatenation, both PHP interpolation spellings, and a bare \
         variable are each base-url-runtime — no reference, no ledger entry, \
         no approximate bind"
    );
}

/// Shared negative case **3** — `path-not-composed`. A static, absolute
/// literal that does not positionally normalize is refused rather than
/// approximated.
#[test]
fn an_absolute_but_non_normalizing_path_emits_no_reference() {
    assert!(
        client_calls(
            r#"
$client = new Client();
$client->get('/files/**');
"#
        )
        .is_empty(),
        "a catch-all template is honestly unbound, never approximately matched"
    );
}

// ── 3. The registration-vs-client trap (this story's own AC) ────────────────

/// The single most dangerous collision in this story: Slim's
/// `$app->get('/x', $handler)` is syntactically identical to
/// `$client->get('/x')` — a `member_call_expression` on a bare `$var`
/// receiver, differing only in that a registration always carries a SECOND
/// argument (the handler). Requiring exactly one argument is the structural
/// discriminator; no text predicate on `$app` vs `$client` is involved.
#[test]
fn a_slim_route_registration_is_not_read_as_an_outbound_call() {
    for (label, handler) in [
        ("a bound closure", "function() {}"),
        ("a callable string", "'indexAction'"),
        (
            "a callable array (class-constant + method name)",
            "[Controller::class, 'index']",
        ),
    ] {
        let facts = client_calls(&format!(
            r#"
$app = null;
$app->get('/x', {handler});
$client = new Client();
$client->get('/probe');
"#
        ));
        // `$client->get('/probe')` is the positive control: it proves the
        // file WAS scanned, so `$app->get(...)`'s absence is the query's
        // discriminator refusing it — not a closed ledger gate.
        assert_eq!(
            facts,
            ["GET /probe"],
            "{label}: a Slim registration must never be read as an outbound \
             call, and the file was genuinely scanned: {facts:?}"
        );
    }
}

/// Laravel's `Route::get('/x', $handler)` is a `scoped_call_expression`
/// (`Route::`), never a `member_call_expression` — a different node kind at
/// the grammar level, so it is excluded structurally with no arity check
/// standing behind that exclusion at all.
#[test]
fn a_laravel_route_facade_call_is_not_read_as_an_outbound_call() {
    let facts = client_calls(
        r#"
Route::get('/x', 'Controller@index');
$client = new Client();
$client->get('/probe');
"#,
    );
    assert_eq!(
        facts,
        ["GET /probe"],
        "a static facade call is never an outbound client call, and the file \
         was genuinely scanned: {facts:?}"
    );
}

/// The fluent-chain twin: `Route::middleware(['api'])->get('/x', $handler)`
/// puts the `->get(...)` on the OUTSIDE, so its `object:` is itself a
/// `scoped_call_expression` rather than a bare variable — still excluded by
/// the same receiver-shape discriminator, and doubly excluded by arity.
#[test]
fn a_laravel_fluent_middleware_chain_is_not_read_as_an_outbound_call() {
    let facts = client_calls(
        r#"
Route::middleware(['api'])->get('/x', 'Controller@index');
$client = new Client();
$client->get('/probe');
"#,
    );
    assert_eq!(
        facts,
        ["GET /probe"],
        "a fluent facade chain is never an outbound client call, and the file \
         was genuinely scanned: {facts:?}"
    );
}

// ── 4. HTML-interleaved parity ([FR-PL-07]) ──────────────────────────────────

/// The acceptance criterion a query written only against pure-PHP fixtures
/// silently misses: PHP binds tree-sitter-php's **full** `php` grammar so an
/// HTML-interleaved template still parses its embedded `<?php … ?>` islands,
/// and a Guzzle call inside one must capture IDENTICALLY to the same call in
/// a pure-PHP file — same target, same count.
#[test]
fn a_call_inside_an_html_interleaved_template_captures_identically_to_pure_php() {
    let pure_php = client_calls_raw(&format!(
        "<?php\n{CLIENT_IMPORT}$client = new Client();\n$client->get('/users');\n"
    ));
    let html_interleaved = client_calls_raw(&format!(
        "<!DOCTYPE html>\n<html><body>\n<h1>Users</h1>\n<?php\n{CLIENT_IMPORT}$client = new Client();\necho $client->get('/users')->getBody();\n?>\n</body></html>\n"
    ));
    assert_eq!(pure_php, ["GET /users"]);
    assert_eq!(
        html_interleaved, pure_php,
        "the HTML markup around the `<?php … ?>` island must not change what \
         the arm captures"
    );
}

// ── 5. Stated coverage ceilings ([ADR-54]) ───────────────────────────────────

/// Guzzle's optional `array $options` second argument
/// (`$client->get('/p', ['query' => [...]])`) is a stated ceiling: the
/// exactly-one-argument arity gate that keeps a Slim/Laravel registration out
/// also excludes this. Widening it needs a discriminator narrow enough to
/// admit an associative options array while still excluding a positional
/// `[Controller::class, 'method']` callable array (see
/// `a_slim_route_registration_is_not_read_as_an_outbound_call`) — deliberately
/// deferred rather than invented under this story's scope.
#[test]
fn a_verb_method_call_with_an_options_array_second_argument_is_a_stated_ceiling() {
    assert!(
        client_calls(
            r#"
$client = new Client();
$client->get('/users', ['query' => ['page' => 1]]);
"#
        )
        .is_empty(),
        "the options-array second argument is a stated ceiling, not a capture"
    );
}

/// `$client->send($request)` / `$client->sendAsync($request)` — the verb and
/// path live on the `Request` object that built `$request`, and joining them
/// needs dataflow the extractor does not have (the same ceiling as Go's
/// `client.Do(req)`).
#[test]
fn send_with_a_prebuilt_request_object_is_a_stated_ceiling() {
    assert!(
        client_calls(
            r#"
$client = new Client();
$request = new Request('GET', '/users');
$client->send($request);
$client->sendAsync($request);
"#
        )
        .is_empty(),
        "the verb/path live on the Request object, not this call site — for \
         both send() and sendAsync()"
    );
}
