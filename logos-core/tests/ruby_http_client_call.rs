//! Ruby HTTP client-call capture (S-347, [CR-108], [FR-WS-08], [ADR-54],
//! [NFR-RA-05]).
//!
//! Drives the **real** pipeline — the compiled `tree-sitter-ruby` grammar, the
//! plugin's `invocations` query, the generic dispatch's HTTP-verb and
//! static-literal decisions, and `resolve::http_client_call`'s normalizer —
//! over one fixture per idiom in [FR-WS-08]'s normative Ruby row (Net::HTTP,
//! Faraday), so a query that compiles but captures nothing cannot pass
//! ([CR-101]'s lesson).
//!
//! It pins three things a reader has to be able to trust:
//!
//! 1. **What is captured** — the receiver-method idiom in both its
//!    parenthesised and parenthesis-free spellings (the same key, not a
//!    near-miss), the `Net::HTTP.get(URI(...))` constructor-wrapped path, and a
//!    symbol-keyed hash call.
//! 2. **What is refused** — [FR-WS-08]'s shared negative-case fixture contract
//!    (S-340), referenced rather than re-invented here.
//! 3. **What is a stated ceiling** — the Ruby idioms the arm's capture
//!    vocabulary provably cannot express, asserted as *zero* references
//!    ([ADR-54]).
//!
//! No real Ruby estate is enrolled for measurement (`pec-services` is a
//! Java/Spring workspace, zero `.rb` files) — the "measured capture count"
//! this story's Testing & Verification asks for is therefore the fixture
//! suite's own count, recorded in the S-347 implementation notes rather than
//! against a real codebase.
//!
//! Gated on the Ruby grammar so a build excluding it does not run it.
//!
//! [CR-101]: ../../docs/requests/CR-101-jvm-spring-route-extraction.md
//! [CR-108]: ../../docs/requests/CR-108-per-language-http-client-call-capture.md
//! [FR-WS-08]: ../../docs/specs/requirements/FR-WS-08.md
//! [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
//! [ADR-54]: ../../docs/specs/architecture/decisions/ADR-54.md
#![cfg(feature = "lang-ruby")]

use logos_core::extract::{extract, Facts, FileInput, SymbolContext};
use logos_core::model::ArtifactRelation;
use logos_core::plugin::LanguageRegistry;

/// Extract one in-memory Ruby source through the embedded Ruby plugin.
fn extract_ruby(source: &str) -> Facts {
    let tmp = tempfile::tempdir().expect("tempdir");
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
    let plugin = reg.for_extension("rb").expect("ruby grammar");
    extract(
        &FileInput::new("client.rb", source),
        plugin,
        &SymbolContext::default(),
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

// ── 1. The receiver-method idiom — parenthesised and parenthesis-free ──────

/// `conn.get("/users")` — Faraday, parenthesised.
#[test]
fn a_faraday_parenthesised_call_yields_one_reference() {
    let facts = extract_ruby(
        r#"require "faraday"

def list_users(conn)
  conn.get("/users")
end
"#,
    );
    assert_eq!(client_call_targets(&facts), vec!["GET /users".to_string()]);
}

/// `conn.get "/users"` — Faraday, parenthesis-free. **The story's decision,
/// proven**: `command_call` aliases to the same `call` node type as the
/// parenthesised form, so this renders the identical key — not a near-miss.
#[test]
fn a_faraday_parenthesis_free_call_captures_identically_to_the_parenthesised_form() {
    let parens = extract_ruby(
        r#"require "faraday"

def list_users(conn)
  conn.get("/users")
end
"#,
    );
    let no_parens = extract_ruby(
        r#"require "faraday"

def list_users(conn)
  conn.get "/users"
end
"#,
    );
    assert_eq!(
        client_call_targets(&parens),
        client_call_targets(&no_parens),
        "parenthesised and parenthesis-free calls must render the same key"
    );
    assert_eq!(
        client_call_targets(&no_parens),
        vec!["GET /users".to_string()]
    );
}

/// `Faraday.get("/users")` — the module-level free-function spelling, same
/// anchor (a `constant` receiver is accepted like any other).
#[test]
fn a_faraday_module_level_call_is_captured() {
    let facts = extract_ruby(
        r#"require "faraday"

def ping
  Faraday.get("/users")
end
"#,
    );
    assert_eq!(client_call_targets(&facts), vec!["GET /users".to_string()]);
}

// ── 2. `Net::HTTP.get(URI(...))` — the constructor-wrapped path ────────────

/// **The story's decision, proven.** `Net::HTTP.get(URI("/users"))` yields
/// exactly one `GET /users` reference: the wrapper is seen through to the
/// static literal inside it, not silently refused for the wrong reason.
#[test]
fn a_net_http_uri_wrapped_call_yields_one_reference() {
    let facts = extract_ruby(
        r#"require "net/http"
require "uri"

def list_users
  Net::HTTP.get(URI("/users"))
end
"#,
    );
    assert_eq!(client_call_targets(&facts), vec!["GET /users".to_string()]);
}

/// The same idiom with a bare literal (no `URI(...)` wrapper) captures through
/// the plain receiver-method pattern — proof the wrapper is an *additional*
/// idiom, not a replacement for the bare-literal case.
#[test]
fn a_net_http_bare_literal_call_is_also_captured() {
    let facts = extract_ruby(
        r#"require "net/http"

def list_users
  Net::HTTP.get("/users")
end
"#,
    );
    assert_eq!(client_call_targets(&facts), vec!["GET /users".to_string()]);
}

// ── 3. A symbol-keyed hash call ─────────────────────────────────────────────

/// `conn.get(path: "/users")` — captured, not silently dropped: the
/// keyword-argument `pair` is unwrapped to its `path:` value directly.
#[test]
fn a_symbol_keyed_hash_call_yields_one_reference() {
    let facts = extract_ruby(
        r#"require "faraday"

def list_users(conn)
  conn.get(path: "/users")
end
"#,
    );
    assert_eq!(client_call_targets(&facts), vec!["GET /users".to_string()]);
}

/// **Ceiling.** The hash-ROCKET spelling (`"path" => "/users"`) is a different
/// grammar shape — its `pair` key is a `(string)` node, not the
/// `hash_key_symbol` pattern 3 requires — so it falls through to the generic
/// pattern, which binds the whole `pair` as the argument and refuses it as
/// `base-url-runtime`: refused explicitly, never silently dropped, but for a
/// classification that is not quite the real reason.
#[test]
fn a_hash_rocket_keyed_call_is_a_stated_ceiling() {
    let facts = extract_ruby(
        r#"require "faraday"

def list_users(conn)
  conn.get("path" => "/users")
end
"#,
    );
    assert!(
        client_call_targets(&facts).is_empty(),
        "the hash-rocket form is not unwrapped by pattern 3: {:?}",
        client_call_targets(&facts)
    );
}

// ── 2. The shared negative-case fixture contract (S-340, [FR-WS-08]) ────────

/// Shared negative case **1** — a same-shaped non-HTTP receiver call. Defined
/// once in [FR-WS-08]'s "Shared negative-case fixture contract" section; this
/// story only proves Ruby's half of it: the file carries no `require` of a
/// known HTTP-client package, so the arm's ledger gate never scans it and
/// `cache.get("k")` cannot fabricate anything — nor can a route-shaped key.
#[test]
fn a_non_client_file_is_never_scanned_for_client_calls() {
    let facts = extract_ruby(
        r#"def lookup(cache)
  cache.get("k")
end

def route_shaped_key(cache)
  cache.get("/admin/users")
end
"#,
    );
    assert!(
        client_call_targets(&facts).is_empty(),
        "no HTTP-client require ⇒ the file is never scanned, so neither the \
         plain key nor the route-shaped one becomes a cross-service edge"
    );
}

/// **Stated over-capture ceiling** — the ledger gate is *file*-grained, so the
/// negative case above holds only across files. Inside a file that already
/// requires a client package, a same-shaped collection call with a
/// route-shaped key still captures — the same residual every other language's
/// arm documents (Java's
/// `a_route_shaped_collection_get_inside_a_client_file_is_a_stated_ceiling`,
/// Go's `a_route_shaped_get_outside_a_net_http_file_is_not_captured`'s
/// positive control).
#[test]
fn a_route_shaped_collection_get_inside_a_client_file_is_a_stated_ceiling() {
    let facts = extract_ruby(
        r#"require "faraday"

def notarget(perms)
  perms.get("/admin/users")
end
"#,
    );
    assert_eq!(
        client_call_targets(&facts),
        vec!["GET /admin/users".to_string()],
        "file-grained gate: a route-shaped collection key inside a client file \
         is still captured — the documented ADR-54 accuracy ceiling"
    );
}

/// Shared negative case **2** — `base-url-runtime`. A bare variable, an
/// interpolated string, and a relative literal each emit **no** reference. The
/// classification itself is generic and already fixture-pinned in
/// `resolve::http_client_call::classify_client_call`; this proves Ruby's query
/// fills the interpreter's slots such that the refusal fires.
#[test]
fn a_runtime_composed_path_emits_no_reference() {
    for body in [
        r#"conn.get(path)"#,
        r##"conn.get("#{base}/users")"##,
        r#"conn.get("users/me")"#,
    ] {
        let src = format!(
            r#"require "faraday"

def call(conn, path, base)
  {body}
end
"#
        );
        let facts = extract_ruby(&src);
        assert!(
            client_call_targets(&facts).is_empty(),
            "a bare variable, an interpolated string, and a relative literal \
             are each base-url-runtime — got {:?} for {body:?}",
            client_call_targets(&facts)
        );
    }
}

/// String interpolation on the `Faraday.get`/`Net::HTTP` spelling directly,
/// pinned as its own test per the story's own acceptance criterion.
#[test]
fn an_interpolated_path_emits_no_reference() {
    let facts = extract_ruby(
        r##"require "faraday"

def call(conn, base)
  conn.get("#{base}/users")
end
"##,
    );
    assert!(
        client_call_targets(&facts).is_empty(),
        "an interpolated path is base-url-runtime, not approximately matched: {:?}",
        client_call_targets(&facts)
    );
}

/// Shared negative case **3** — `path-not-composed`. A static, absolute
/// literal that does not positionally normalize is refused rather than
/// approximated.
#[test]
fn an_absolute_but_non_normalizing_path_emits_no_reference() {
    let facts = extract_ruby(
        r#"require "faraday"

def call(conn)
  conn.get("/files/**")
end
"#,
    );
    assert!(
        client_call_targets(&facts).is_empty(),
        "a catch-all template is honestly unbound, never approximately matched"
    );
}

// ── Provider registrations must never be read as outbound calls ────────────

/// Sinatra's `get '/users' do … end` is a **registration**, syntactically the
/// same shape as an outbound call — and must never be captured as one, even
/// inside a file that also requires a genuine HTTP client (a Sinatra app
/// calling out to another service is an entirely ordinary shape). The
/// structural discriminator is `receiver: (_)`: Sinatra's DSL methods run
/// against an implicit `self`, so the registration call has **no receiver
/// field at all** and the pattern never matches it — no text predicate
/// involved.
#[test]
fn a_sinatra_route_registration_is_not_read_as_an_outbound_call() {
    let facts = extract_ruby(
        r#"require "faraday"

class App < Sinatra::Base
  get '/users' do
    'ok'
  end

  post '/users' do
    conn = Faraday.new(url: 'http://upstream')
    conn.get('/probe')
    'ok'
  end
end
"#,
    );
    // `conn.get('/probe')` is the positive control, proving the file was
    // genuinely scanned: the two registrations above are absent because the
    // query's receiver requirement rejected them, not because the ledger gate
    // happened to be closed.
    assert_eq!(
        client_call_targets(&facts),
        vec!["GET /probe".to_string()],
        "a Sinatra route registration is never an outbound call, and the file \
         was genuinely scanned: {:?}",
        client_call_targets(&facts)
    );
}

/// A Rails `routes.draw { get "/users", to: "users#index" }` block is the
/// same trap in the framework this plugin's own ratified set (`references.scm`
/// / `frameworks.scm`) targets. Same discriminator: the route verb call is
/// receiver-less.
#[test]
fn a_rails_route_registration_is_not_read_as_an_outbound_call() {
    let facts = extract_ruby(
        r#"require "net/http"

Rails.application.routes.draw do
  get "/users", to: "users#index"
  post "/users", to: "users#create"
end

def probe
  Net::HTTP.get("/probe")
end
"#,
    );
    assert_eq!(
        client_call_targets(&facts),
        vec!["GET /probe".to_string()],
        "a Rails route registration is never an outbound call, and the file \
         was genuinely scanned: {:?}",
        client_call_targets(&facts)
    );
}

// ── Stated coverage ceilings ([ADR-54]) ─────────────────────────────────────

/// **Ceiling.** `URI(...)` is anchored on its exact callee name — a
/// receiver-typed constructor (`URI.parse(...)`) is a different shape
/// (`method:` would be an `identifier` under an explicit `receiver:`, not a
/// receiver-less `constant`), so it is not unwrapped and the site is refused.
#[test]
fn only_bare_uri_unwraps_a_constructor_wrapped_path() {
    let facts = extract_ruby(
        r#"require "net/http"
require "uri"

def list_users
  Net::HTTP.get(URI.parse("/internal/{id}"))
end
"#,
    );
    assert!(
        client_call_targets(&facts).is_empty(),
        "`URI.parse(...)` is not the bare `URI(...)` Kernel function — no path \
         is unwrapped: {:?}",
        client_call_targets(&facts)
    );
}

/// **Ceiling.** An unrelated receiver-less constructor named something other
/// than `URI` is not mistaken for the wrapper, however similarly shaped.
#[test]
fn only_uri_named_wrapper_unwraps_a_constructor_wrapped_path() {
    let facts = extract_ruby(
        r#"require "net/http"

def list_users
  Net::HTTP.get(MyFactory("/internal/{id}"))
end
"#,
    );
    assert!(
        client_call_targets(&facts).is_empty(),
        "`MyFactory(...)` is not `URI(...)` — no path is unwrapped: {:?}",
        client_call_targets(&facts)
    );
}

/// **Ceiling.** `Net::HTTP::Get.new("/users")` — the `Net::HTTPGenericRequest`
/// class-constructor idiom — carries its verb in the CONSTANT name (`Get`),
/// not in the called method: the actual method text is `new`, which
/// `is_http_method` correctly rejects (it is not an HTTP verb). Lifting this
/// needs a per-verb constant-name table reading the *receiver's* last segment.
/// S-346's `[invocation_methods]` table
/// (same sprint) does not reach it: that table normalizes a captured text, and
/// the only text captured here is `new`. A table plus a pattern, deliberately
/// deferred — the same position Go's `http.MethodGet` ceiling is in.
#[test]
fn the_generic_request_class_constructor_idiom_is_a_stated_ceiling() {
    let facts = extract_ruby(
        r#"require "net/http"

def build_request
  Net::HTTP::Get.new("/users")
end
"#,
    );
    assert!(
        client_call_targets(&facts).is_empty(),
        "the verb rides the constant name `Get`, not a bound method call: {:?}",
        client_call_targets(&facts)
    );
}

/// **Ceiling.** A block-form Faraday request whose path is built *inside* the
/// block (`req.url "/users"`) rather than passed as a call argument has no
/// `arguments:` field at all to anchor on: `conn.get { |req| … }` matches the
/// grammar's block-only `call` variant (`receiver` + `block`, no `argument_list`
/// — confirmed against the vendored `tree-sitter-ruby` grammar), so pattern 1
/// never matches this call in the first place. Not attempted, not worked
/// around.
#[test]
fn a_block_form_request_with_no_path_argument_is_a_stated_ceiling() {
    let facts = extract_ruby(
        r#"require "faraday"

def list_users(conn)
  conn.get { |req| req.url "/users" }
end
"#,
    );
    assert!(
        client_call_targets(&facts).is_empty(),
        "the path is built inside the block, not passed as an argument — there \
         is no `arguments:` field to anchor on: {:?}",
        client_call_targets(&facts)
    );
}
