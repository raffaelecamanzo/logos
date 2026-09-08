//! Python HTTP client-call capture (S-344, [CR-108], [FR-WS-08], [ADR-54],
//! [NFR-RA-05]).
//!
//! Python is the only [CR-108] language exercising **both** anchor shapes in
//! one story: the module-level free-function form (`requests.get("/p")`,
//! S-343's finding applied rather than re-derived) and a session/client
//! receiver form (`session.get("/p")`, `httpx.Client().get("/p")`).
//!
//! Driven through the public [`extract`] entry point, asserting the
//! `HttpClientCall` reference targets directly — the same grain the Go arm's
//! fixtures use ([`tests/go_invocations.rs`](go_invocations.rs)).
//!
//! Three things this file pins:
//!
//! 1. **What is captured** — the free-function and receiver forms in
//!    [FR-WS-08]'s normative Python row, including the sync/async httpx
//!    client split ([`AC2`](a_httpx_async_client_capture_needs_no_await_specific_pattern)).
//! 2. **What is refused** — [FR-WS-08]'s shared negative-case fixture contract
//!    (S-340), referenced rather than re-invented.
//! 3. **The registration-vs-client trap** — FastAPI's and (Flask 2+'s) route
//!    decorators are syntactically identical to a client call
//!    (`@app.get("/users")` vs. `session.get("/users")`); this story closes it
//!    by anchoring every pattern to a named client rather than S-345's
//!    argument-shape discriminators, since Python's grammar gives no structural
//!    "not inside a decorator" predicate to write instead.
//!
//! Gated on the Python grammar so a build excluding it does not run it.
//!
//! [CR-108]: ../../docs/requests/CR-108-per-language-http-client-call-capture.md
//! [FR-WS-08]: ../../docs/specs/requirements/FR-WS-08.md
//! [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
//! [ADR-54]: ../../docs/specs/architecture/decisions/ADR-54.md
#![cfg(feature = "lang-python")]

use logos_core::extract::{extract, Facts, FileInput, SymbolContext};
use logos_core::model::ArtifactRelation;
use logos_core::plugin::LanguageRegistry;

/// Extract one in-memory Python source through the embedded Python plugin.
fn extract_py(source: &str) -> Facts {
    let tmp = tempfile::tempdir().expect("tempdir");
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
    let plugin = reg.for_extension("py").expect("python grammar");
    extract(
        &FileInput::new("client.py", source),
        plugin,
        &SymbolContext::default(),
    )
}

/// The `HttpClientCall` reference targets captured from a source, sorted.
fn client_call_targets(facts: &Facts) -> Vec<String> {
    let mut targets: Vec<String> = facts
        .refs
        .iter()
        .filter(|r| r.relation == Some(ArtifactRelation::HttpClientCall))
        .map(|r| r.target.clone())
        .collect();
    targets.sort();
    targets
}

// ── AC1: module-level free-function form (`requests`/`httpx`) ───────────────

/// `requests.get("/users")` and `httpx.post("/orders")` each yield one
/// reference — the free-function anchor S-343 already proved needs no
/// structural dispatch support, applied rather than re-derived here.
#[test]
fn a_module_level_free_function_call_yields_one_reference() {
    let facts = extract_py(
        r#"
import requests

def list_users():
    return requests.get("/users")
"#,
    );
    assert_eq!(client_call_targets(&facts), vec!["GET /users".to_string()]);

    let facts = extract_py(
        r#"
import httpx

def create_order():
    return httpx.post("/orders")
"#,
    );
    assert_eq!(client_call_targets(&facts), vec!["POST /orders".to_string()]);
}

// ── AC2: session/client receiver form ────────────────────────────────────────

/// `session.get("/users")` — a `requests.Session()` instance bound to a
/// conventionally-named local variable.
#[test]
fn a_session_receiver_call_yields_one_reference() {
    let facts = extract_py(
        r#"
import requests

def list_users():
    session = requests.Session()
    return session.get("/users")
"#,
    );
    assert_eq!(client_call_targets(&facts), vec!["GET /users".to_string()]);
}

/// `self.client.post("/orders", json=body)` — an attribute-qualified receiver,
/// the same posture the `axios`/`Axios` boundary anchor takes for `this.axios`.
#[test]
fn a_member_qualified_client_receiver_call_yields_one_reference() {
    let facts = extract_py(
        r#"
import httpx

class OrderService:
    def __init__(self):
        self.client = httpx.Client()

    def create(self, body):
        return self.client.post("/orders", json=body)
"#,
    );
    assert_eq!(client_call_targets(&facts), vec!["POST /orders".to_string()]);
}

/// `httpx.Client().get("/users")` — the inline construction-and-call chain:
/// the receiver is itself a constructor `call`, not a plain identifier, so it
/// exercises the dedicated chain pattern rather than the plain receiver one.
#[test]
fn an_inline_client_construction_chain_yields_one_reference() {
    let facts = extract_py(
        r#"
import httpx

def list_users():
    return httpx.Client().get("/users")
"#,
    );
    assert_eq!(client_call_targets(&facts), vec!["GET /users".to_string()]);
}

/// `httpx` sync and async client forms both capture, since an `await` wrapper
/// changes the call's node shape (it becomes the sole child of an `await`
/// node) — this proves the receiver pattern needs no `await`-specific variant,
/// because tree-sitter matches the `call` node regardless of its parent.
#[test]
fn a_httpx_async_client_capture_needs_no_await_specific_pattern() {
    let facts = extract_py(
        r#"
import httpx

async def list_users():
    client = httpx.AsyncClient()
    return await client.get("/users")
"#,
    );
    assert_eq!(client_call_targets(&facts), vec!["GET /users".to_string()]);

    // The inline chain form under `await` too — `await httpx.AsyncClient().get(…)`.
    let facts = extract_py(
        r#"
import httpx

async def list_users():
    return await httpx.AsyncClient().get("/users")
"#,
    );
    assert_eq!(client_call_targets(&facts), vec!["GET /users".to_string()]);
}

// ── Boundary precision: a named-client anchor, not a substring test ─────────

/// A receiver whose name merely *contains* `session`/`client` as a substring,
/// rather than starting or ending with it, is not captured — the same
/// boundary discipline the `typescript`/`tsx` `axios` anchor states
/// (`notaxiosCache` stays excluded). `notasession` fails both the
/// starts-with and ends-with halves of the rule.
#[test]
fn a_substring_only_match_on_the_receiver_name_is_not_captured() {
    let facts = extract_py(
        r#"
import requests

def lookup(notasession):
    return notasession.get("/cache/key")
"#,
    );
    assert_eq!(client_call_targets(&facts), Vec::<String>::new());
}

// ── The registration-vs-client trap ([FR-WS-08], NFR-RA-05) ─────────────────

/// A FastAPI route decorator (`@app.get("/users")`) is syntactically the same
/// `<receiver>.<verb>("<path>")` shape a client call is — differing only by
/// the surrounding `decorator` node a tree-sitter pattern anchored on the call
/// cannot see. The file also imports `requests` (a service that calls
/// upstream from inside its own handlers is the common case that makes the
/// two shapes coexist), so the ledger gate is open; only the named-client
/// receiver anchor keeps `app.get` from being read as an outbound call.
#[test]
fn a_fastapi_route_decorator_is_never_captured_as_a_client_call() {
    let facts = extract_py(
        r#"
import requests
from fastapi import FastAPI

app = FastAPI()

@app.get("/users")
def list_users():
    return requests.get("/upstream/users")
"#,
    );
    assert_eq!(
        client_call_targets(&facts),
        vec!["GET /upstream/users".to_string()],
        "the FastAPI registration contributes no HttpClientCall reference; \
         only the genuine outbound `requests.get` inside the handler does"
    );
}

/// Flask 2+ ships the same `@app.get(...)`/`@bp.get(...)` decorator shorthand
/// as FastAPI — a second, independently-named framework proving the anchor
/// choice, not a shape S-344 invented for FastAPI alone.
#[test]
fn a_flask_blueprint_route_decorator_is_never_captured_as_a_client_call() {
    let facts = extract_py(
        r#"
import requests
from flask import Blueprint

bp = Blueprint("users", __name__)

@bp.get("/users")
def list_users():
    return requests.get("/upstream/users")
"#,
    );
    assert_eq!(
        client_call_targets(&facts),
        vec!["GET /upstream/users".to_string()]
    );
}

// ── Shared negative-case fixture contract (S-340, [FR-WS-08]) ───────────────

/// Shared negative case **1** — a same-shaped non-HTTP receiver call.
/// `dict.get("k")` and a route-shaped `dict.get("/admin/users")` are refused
/// in a file with no `requests`/`httpx` import, since the arm's ledger gate
/// never scans it.
#[test]
fn a_non_client_file_is_never_scanned_for_client_calls() {
    let facts = extract_py(
        r#"
def lookup(cache):
    return cache.get("k")

def route_shaped(cache):
    return cache.get("/admin/users")
"#,
    );
    assert_eq!(client_call_targets(&facts), Vec::<String>::new());
}

/// Unlike the Rust, Go, Kotlin, Ruby and PHP arms' broad
/// `<receiver>.<method>(<arg>)` anchor, Python's named-client anchor refuses
/// `dict.get("k")` even **inside** a client file — there is no file-grained
/// ceiling to state here, because the receiver name itself (`cache`) never
/// matches the `session`/`client` boundary, independent of the ledger gate. It
/// is stricter than the ADR-54 ceiling those arms accept, and a direct
/// consequence of the registration-vs-client trap forcing a named anchor rather
/// than a broad one. Java adopted the same posture in S-375 (CR-120), citing
/// this test as the precedent it followed.
#[test]
fn a_route_shaped_dict_get_inside_a_client_file_is_still_refused() {
    let facts = extract_py(
        r#"
import requests

def route_shaped(cache):
    return cache.get("/admin/users")
"#,
    );
    assert_eq!(client_call_targets(&facts), Vec::<String>::new());
}

/// Shared negative case **2** — `base-url-runtime`. An f-string, a
/// `%`-formatted string, a `.format()`-composed string and a bare variable
/// each emit no reference. The classification itself is generic and already
/// fixture-pinned in `resolve::http_client_call::classify_client_call`; what
/// this proves is that Python's query fills the interpreter's slots such that
/// the refusal fires — which for Python also proves the `static_string_literal`
/// core fix this story required (see the implementation notes): Python's
/// grammar names its quote/prefix tokens as separate `string_start`/
/// `string_end` children, unlike every other supported grammar's `string`
/// node, so an f-string's `interpolation` child is what disqualifies it here,
/// not a structural quirk of the delimiters.
#[test]
fn a_runtime_composed_path_emits_no_reference() {
    let facts = extract_py(
        r#"
import requests

def bare_variable(url):
    return requests.get(url)

def fstring_composed(base):
    return requests.get(f"{base}/users")

def percent_composed(user_id):
    return requests.get("/users/%s" % user_id)

def dot_format_composed(user_id):
    return requests.get("/users/{}".format(user_id))
"#,
    );
    assert_eq!(
        client_call_targets(&facts),
        Vec::<String>::new(),
        "a bare variable, an f-string, a `%`-formatted string and a \
         `.format()`-composed string are each base-url-runtime — no \
         reference, no ledger entry, no approximate bind"
    );
}

/// A real-world shape measured on the `pec-services` sample workspace's one
/// genuine `requests`-importing Python file
/// (`hermodr-mirror/documentation/tutorials/5.2-ccflow/python/test_ccflow.py`):
/// both of its call sites (`requests.post`/`requests.get`) build the path from
/// an f-string over a runtime `endpoint` parameter. Reproduced verbatim in
/// shape here rather than only asserted in prose, so the real-world negative
/// case that motivated this test is pinned, not just claimed.
#[test]
fn the_pec_services_ccflow_shape_is_base_url_runtime() {
    let facts = extract_py(
        r#"
import requests

def call_headers(endpoint):
    r = requests.post(f"{endpoint}/token", data={})
    r.raise_for_status()
    rh = requests.get(f"{endpoint}/headers", headers={})
    return rh.json()
"#,
    );
    assert_eq!(client_call_targets(&facts), Vec::<String>::new());
}

/// Shared negative case **3** — `path-not-composed`. A static, absolute
/// literal that does not positionally normalize is refused rather than
/// approximated.
#[test]
fn an_absolute_but_non_normalizing_path_emits_no_reference() {
    let facts = extract_py(
        r#"
import requests

def catch_all():
    return requests.get("/files/{*rest}")
"#,
    );
    assert_eq!(client_call_targets(&facts), Vec::<String>::new());
}

// ── A static literal is captured verbatim, quote/prefix stripped ───────────

/// A plain literal's content is captured with neither its quotes nor an
/// f-string's `f` prefix retained — proving the `static_string_literal` core
/// fix strips Python's named `string_start`/`string_end` delimiter children
/// without corrupting the path.
#[test]
fn a_static_literal_is_captured_with_quotes_and_prefix_stripped() {
    let facts = extract_py(
        r#"
import requests

def list_users():
    return requests.get('/users/{id}')
"#,
    );
    assert_eq!(
        client_call_targets(&facts),
        vec!["GET /users/{id}".to_string()]
    );
}
