//! Go HTTP client-call capture (S-345, [CR-108], [FR-WS-08], [ADR-54],
//! [NFR-RA-05]).
//!
//! Two levels, because the story has two obligations:
//!
//! 1. **Capture** — the `invocations.scm` query populates the interpreter's
//!    `invoke.http.method` / `invoke.http.arg` slots correctly for each `net/http`
//!    idiom in [FR-WS-08]'s normative Go row. Driven through the public
//!    [`extract`] entry point, asserting the `HttpClientCall` reference targets
//!    directly — the same grain the Rust arm's fixtures use.
//! 2. **Cross-member bind** — those references actually bind a Go provider's
//!    routes in **another** workspace member, through the real `Engine` +
//!    `ContractBridge` path, across `{id}`/`:id` syntax drift.
//!
//! The three shared negative cases are [FR-WS-08]'s "Shared negative-case
//! fixture contract" (S-340) — referenced, not re-invented:
//!
//! - **case 1** (same-shaped non-HTTP receiver call) is per-language and gated by
//!   the descriptor's `http_client_detectors` row (`plugins/go/plugin.toml`, read
//!   by `extract::capture_http_client_call_arm`):
//!   [`a_route_shaped_get_outside_a_net_http_file_is_not_captured`];
//! - **case 2** (`base-url-runtime`) : [`a_runtime_composed_path_is_not_captured`];
//! - **case 3** (`path-not-composed`): [`an_absolute_but_non_normalizing_path_is_not_captured`].
//!
//! Cases 2 and 3 are classified generically in
//! `resolve::http_client_call::classify_client_call` and fixture-pinned there;
//! what these prove is only that the Go query routes its idioms into the slots
//! that reach that classifier.
//!
//! [CR-108]: ../../docs/requests/CR-108-per-language-http-client-call-capture.md
//! [FR-WS-08]: ../../docs/specs/requirements/FR-WS-08.md
//! [ADR-54]: ../../docs/specs/architecture/decisions/ADR-54.md
//! [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
#![cfg(feature = "lang-go")]

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

/// Extract one in-memory Go source through the embedded Go plugin.
fn extract_go(source: &str) -> Facts {
    let tmp = tempfile::tempdir().expect("tempdir");
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
    let plugin = reg.for_extension("go").expect("go grammar");
    extract(
        &FileInput::new("client.go", source),
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

// ── AC1: verb-as-method-name (`http.Get` / `http.Post`) ─────────────────────

/// `http.Get("/users")` and `http.Post("/users", …)` each yield exactly one
/// reference, with the verb taken from the **function name** and upper-cased.
#[test]
fn a_verb_named_free_function_call_takes_its_verb_from_the_name() {
    let facts = extract_go(
        r#"package client

import "net/http"

func ListUsers() { http.Get("/users") }
func CreateUser() { http.Post("/users", "application/json", nil) }
"#,
    );
    // Canonical ledger order — `(source, target, …)`, so `CreateUser` precedes
    // `ListUsers` regardless of source order ([NFR-RA-06]).
    assert_eq!(
        client_call_targets(&facts),
        vec!["POST /users".to_string(), "GET /users".to_string()],
        "one reference each, verb from the function name"
    );
}

/// The same shape on a `*http.Client` receiver (`c.Get("/users")`) captures
/// identically — the query anchors on the selector's field, so the free-function
/// and receiver-method forms are one pattern, not two near-misses.
#[test]
fn a_receiver_method_call_captures_identically_to_the_free_function_form() {
    let facts = extract_go(
        r#"package client

import "net/http"

func ListUsers(c *http.Client) { c.Get("/users") }
"#,
    );
    assert_eq!(client_call_targets(&facts), vec!["GET /users".to_string()]);
}

/// A backtick raw string literal is a static path like any other — the same
/// content-child capture reads it, so it is not silently refused.
#[test]
fn a_raw_string_path_literal_is_captured() {
    let facts = extract_go(
        r#"package client

import "net/http"

func ListUsers() { http.Get(`/users`) }
"#,
    );
    assert_eq!(client_call_targets(&facts), vec!["GET /users".to_string()]);
}

// ── AC2: the constructor-argument anchor (`http.NewRequest`) ────────────────

/// **The story's decision, proven.** `http.NewRequest("GET", "/users", nil)`
/// yields exactly one `GET /users` reference: the verb is read from the *first
/// argument*, not from the method name. The anchor is expressible in the
/// existing two-name capture vocabulary — the capture simply lands on the string
/// literal's content child so its text is the bare verb rather than `"GET"` with
/// its quotes.
///
/// The trailing `client.Do(req)` deliberately contributes nothing: `Do` is not an
/// HTTP verb and carries neither slot.
#[test]
fn a_constructor_argument_call_takes_its_verb_from_the_first_argument() {
    let facts = extract_go(
        r#"package client

import "net/http"

func GetUser(c *http.Client) {
	req, _ := http.NewRequest("GET", "/users/{id}", nil)
	c.Do(req)
}
"#,
    );
    assert_eq!(
        client_call_targets(&facts),
        vec!["GET /users/{id}".to_string()],
        "exactly one reference, verb from the constructor's first argument"
    );
}

/// `http.NewRequestWithContext(ctx, "DELETE", "/orders/{id}", body)` — the same
/// verb/path pair one position further along — captures through the same
/// pattern, because the pair is matched wherever it sits in the argument list.
#[test]
fn the_with_context_constructor_captures_through_the_same_anchor() {
    let facts = extract_go(
        r#"package client

import (
	"context"
	"net/http"
)

func DeleteOrder(ctx context.Context, c *http.Client) {
	req, _ := http.NewRequestWithContext(ctx, "DELETE", "/orders/{id}", nil)
	c.Do(req)
}
"#,
    );
    assert_eq!(
        client_call_targets(&facts),
        vec!["DELETE /orders/{id}".to_string()]
    );
}

/// A lower-cased verb argument (`http.NewRequest("get", …)`) keys equal to the
/// upper-cased one — the drift the arm's normalizer erases, exercised through the
/// constructor-argument anchor rather than only through a method name.
#[test]
fn a_lower_cased_verb_argument_normalizes_to_the_same_key() {
    let facts = extract_go(
        r#"package client

import "net/http"

func GetUser(c *http.Client) {
	req, _ := http.NewRequest("get", "/users", nil)
	c.Do(req)
}
"#,
    );
    assert_eq!(client_call_targets(&facts), vec!["GET /users".to_string()]);
}

/// A non-verb first argument is not a client call, however constructor-shaped:
/// `http.Post("/users", "application/json", nil)` has an adjacent
/// (string, string) pair whose first element is `/users`, and it must be read as
/// the verb-named `POST /users` **once** — never additionally as a call whose
/// "verb" is `/users`.
#[test]
fn an_adjacent_string_pair_whose_first_is_not_a_verb_yields_no_extra_reference() {
    let facts = extract_go(
        r#"package client

import "net/http"

func CreateUser() { http.Post("/users", "application/json", nil) }
"#,
    );
    assert_eq!(
        client_call_targets(&facts),
        vec!["POST /users".to_string()],
        "the content-type argument never becomes a second reference"
    );

    // …and with the response bound, which is the far more common spelling. (An
    // earlier draft justified this fixture by the constructor-argument pattern
    // being "live in a value position" — it is not: that pattern is pinned by
    // `#eq? @_pkg "http"` + `#any-of? @_fn "NewRequest" …`, so `http.Post` never
    // reaches it in any position. The value-position discriminator was the
    // ABANDONED draft; see the query header.)
    let bound = extract_go(
        r#"package client

import "net/http"

func CreateUser() {
	resp, _ := http.Post("/users", "application/json", nil)
	_ = resp
}
"#,
    );
    assert_eq!(
        client_call_targets(&bound),
        vec!["POST /users".to_string()],
        "still exactly one reference when the response is bound"
    );
}

// ── AC3 / shared negative case 2: base-url-runtime ──────────────────────────

/// A `NewRequest` (or `Get`) whose URL is a bare variable, a `fmt.Sprintf`
/// result, or a concatenation emits **no** reference — the path is composed at
/// runtime, so there is nothing static to bind ([NFR-RA-05]). This is
/// [FR-WS-08]'s shared negative case 2; the `base-url-runtime` reason itself is
/// classified generically by `classify_client_call`.
#[test]
fn a_runtime_composed_path_is_not_captured() {
    for body in [
        r#"req, _ := http.NewRequest("GET", url, nil)
	c.Do(req)"#,
        r#"req, _ := http.NewRequest("GET", fmt.Sprintf("%s/users", url), nil)
	c.Do(req)"#,
        r#"req, _ := http.NewRequest("GET", url+"/users", nil)
	c.Do(req)"#,
        r#"http.Get(url)"#,
        r#"http.Get(fmt.Sprintf("%s/users", url))"#,
    ] {
        let src = format!(
            r#"package client

import (
	"fmt"
	"net/http"
)

var _ = fmt.Sprintf

func Fetch(c *http.Client, url string) {{
	{body}
}}
"#
        );
        let facts = extract_go(&src);
        assert!(
            client_call_targets(&facts).is_empty(),
            "a runtime-composed path is base-url-runtime, got {:?} for {body:?}",
            client_call_targets(&facts)
        );
    }
}

/// A relative literal (`"users"`) has no absolute route prefix — its base URL is
/// composed elsewhere — so it is likewise refused, not approximated.
#[test]
fn a_relative_path_literal_is_not_captured() {
    let facts = extract_go(
        r#"package client

import "net/http"

func ListUsers() { http.Get("users/{id}") }
"#,
    );
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
    let facts = extract_go(
        r#"package client

import "net/http"

func ListUsers() { http.Get("/v{version}/users") }
"#,
    );
    assert!(
        client_call_targets(&facts).is_empty(),
        "a mixed literal/parameter segment is path-not-composed: {:?}",
        client_call_targets(&facts)
    );
}

// ── Shared negative case 1: same-shaped non-HTTP receiver call ──────────────

/// A `/`-shaped-key `.Get(...)` in a file that references **no** Go HTTP-client
/// package is never scanned for client calls at all, so it can never fabricate a
/// cross-service edge ([FR-WS-08] shared negative case 1, [NFR-RA-05]).
#[test]
fn a_route_shaped_get_outside_a_net_http_file_is_not_captured() {
    const BODY: &str = r#"type Perms struct{}

func (p Perms) Get(k string) bool { return false }

func Authorize(p Perms) { p.Get("/admin/users") }
"#;

    let facts = extract_go(&format!("package authz\n\n{BODY}"));
    assert!(
        client_call_targets(&facts).is_empty(),
        "a non-client file never emits an outbound-call ref: {:?}",
        client_call_targets(&facts)
    );

    // Positive control, and the ADR-54 ceiling in the same fixture. The source
    // is byte-identical but for the `net/http` import, so the emptiness above is
    // attributable to `capture_http_client_call_arm`'s ledger gate and to
    // nothing else — without this, the test would still pass with the gate
    // deleted if some other filter happened to reject `p.Get`.
    //
    // What it captures is the documented ADR-54 accuracy ceiling: the gate is
    // FILE-grained, so this same incidental call INSIDE a genuine client file
    // does capture. Under-capture is safe, over-capture is not, and the residual
    // is stated rather than worked around — the Java arm pins the identical
    // ceiling in `a_route_shaped_collection_get_inside_a_client_file_is_a_stated_ceiling`.
    let gated = extract_go(&format!(
        "package authz\n\nimport \"net/http\"\n\nvar _ = http.StatusOK\n\n{BODY}"
    ));
    assert_eq!(
        client_call_targets(&gated),
        vec!["GET /admin/users".to_string()],
        "the same source with the client import DOES capture — the ledger gate \
         is the only difference between these two cases, and the residual is \
         ADR-54's stated file-grained ceiling"
    );
}

/// Inside a genuine `net/http` file, a same-shaped call whose method is not an
/// HTTP verb (a cache `Fetch`, a `client.Do`) is still never captured — the
/// verb filter narrows the broad anchor.
#[test]
fn a_non_verb_method_call_inside_a_client_file_is_not_captured() {
    let facts = extract_go(
        r#"package client

import "net/http"

type Cache struct{}

func (c Cache) Fetch(k string) string { return k }

func Warm(cache Cache, c *http.Client) {
	_ = cache.Fetch("/admin/users")
	http.Get("/health")
}
"#,
    );
    assert_eq!(
        client_call_targets(&facts),
        vec!["GET /health".to_string()],
        "only the HTTP-verb call is captured, not `Fetch`"
    );
}

// ── Cross-member bind against a Go provider ─────────────────────────────────

/// A Go consumer exercising both anchor shapes: `http.Get` (verb-as-method-name)
/// and `http.NewRequest` (verb-as-constructor-argument), plus one runtime-composed
/// call that must bind nothing even though a matching route exists.
const GO_CLIENT: &str = r#"package client

import (
	"fmt"
	"net/http"
)

func ListUsers() {
	http.Get("/users")
}

func GetOrder(c *http.Client) {
	req, _ := http.NewRequest("GET", "/orders/{id}", nil)
	c.Do(req)
}

func GetOrderDynamic(c *http.Client, base string) {
	req, _ := http.NewRequest("GET", fmt.Sprintf("%s/reports", base), nil)
	c.Do(req)
}
"#;

/// A Gin router registering exact-method routes — deliberately NOT
/// `hermodr-mirror`'s own shape: that repo registers with `http.HandleFunc`,
/// which `[framework_methods]` maps to `ANY`, and an `ANY` provider cannot bind
/// a `GET` consumer until [S-349] lands the wildcard-method index. `:id` drifts
/// from the consumer's `{id}`; the positional `route_key` erases the drift.
/// `/reports` is registered too, so the runtime-composed call has a provider it
/// would bind to if the arm ever approximated — it must not.
///
/// [S-349]: ../../docs/planning/journal.md#s-349-wildcard-method-route-matching-with-exact-method-precedence
const GO_PROVIDER: &str = r#"package main

import "github.com/gin-gonic/gin"

func listUsers(c *gin.Context)  {}
func getOrder(c *gin.Context)   {}
func listReports(c *gin.Context) {}

func main() {
	r := gin.Default()
	r.GET("/users", listUsers)
	r.GET("/orders/:id", getOrder)
	r.GET("/reports", listReports)
	r.Run()
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
        name: "go-shop".to_string(),
        root: root.to_path_buf(),
        members,
        default: None,
        links: Vec::new(),
        governance: Default::default(),
        warm_concurrency: None,
    }
}

/// End-to-end: both Go anchor shapes bind real Go routes in **another** member,
/// and the runtime-composed call binds nothing even though `/reports` exists.
#[test]
fn go_client_calls_bind_go_routes_in_another_member() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    let web = root.join("web");
    let api = root.join("api");
    write(&web, "client.go", GO_CLIENT);
    write(&api, "main.go", GO_PROVIDER);
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
        "both static Go calls bind, the composed one does not: {edges:?}"
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
        "the verb-as-method-name call bound its route: {bound:?}"
    );
    assert!(
        bound.iter().any(|s| s.contains("/orders")),
        "the constructor-argument call bound its route: {bound:?}"
    );
    assert!(
        !bound.iter().any(|s| s.contains("reports")),
        "the fmt.Sprintf-composed call must bind NOTHING even though /reports \
         exists as a provider: {bound:?}"
    );

    let coverage = cross_service_coverage(&registry);
    assert_eq!(coverage.bound, 2, "both static calls are bound");
    assert_eq!(coverage.ambiguous, 0);
    // Pin the whole census, not just the bound bucket: a phantom third
    // reference landing in `unbound` (or a real one silently reclassified into
    // `no_provider_in_workspace`) would otherwise go unremarked while
    // `bound == 2` still held.
    assert_eq!(
        coverage.references.len(),
        2,
        "exactly two client-call references reach the bridge — the \
         fmt.Sprintf-composed call is refused before it becomes one: {:?}",
        coverage.references
    );
    assert_eq!(coverage.unbound, 0, "neither reference is left unbound");
    assert_eq!(
        coverage.no_provider_in_workspace, 0,
        "both routes exist in member `api`"
    );
}

// ── Stated ceiling: the named-constant verb ([ADR-54]) ──────────────────────

/// **Recorded ceiling, not a bug.** `http.NewRequest(http.MethodGet, "/users",
/// nil)` captures nothing, and that is the honest outcome of the *pure-data*
/// budget ([NFR-MA-01]).
///
/// The constructor-argument anchor this story settles reads the verb from a node's
/// TEXT. Go's `http.MethodGet` parses as
///
/// ```text
/// argument_list
///   selector_expression [http.MethodGet]
///     identifier      [http]
///     field_identifier[MethodGet]
///   interpreted_string_literal ["/users"]
/// ```
///
/// so the only texts a capture can offer are `http.MethodGet` and `MethodGet` —
/// neither of which is an HTTP verb to `extract::is_http_method`, whose vocabulary
/// is the bare verbs.
///
/// **The table this test was written against has since landed, and this test did
/// not notice.** S-346 shipped `[invocation_methods]` for C#'s `HttpMethod.Get`
/// — the same shape — as per-plugin descriptor data read by
/// `extract::normalize_invocation_method`, so the original "needs a logos-core
/// change" framing is spent, and the original promise below ("if that table ever
/// lands, this test fails loudly") was not kept: the table is per-plugin and
/// `plugins/go/plugin.toml` declares no rows, so Go's behaviour is byte-identical
/// and this stayed green. Recorded rather than quietly corrected, because a
/// ceiling that outlives its stated reason is the failure mode [ADR-54] exists to
/// prevent.
///
/// What actually holds the ceiling now is narrower and structural: the
/// constructor pattern binds `@invoke.http.method` to an
/// `interpreted_string_literal_content`, while `http.MethodGet` is a
/// `selector_expression` — no text reaches the table for a row to normalize — so
/// lifting it is a table PLUS a query pattern, a Go-side decision. Declaring any
/// row would also opt Go into the table's filter half, costing `http.Get`/`c.Head`
/// identity rows the pass-through gives them free.
///
/// Cost of leaving it, measured on `pec-services`' `hermodr-mirror`: 23 of its 24
/// `http.NewRequest` sites carry a **runtime variable** as the URL, so every one
/// is refused as `base-url-runtime` on the path regardless of whether the verb
/// resolves. The ceiling costs nothing measurable today.
///
/// If a Go `[invocation_methods]` row plus a matching pattern ever land, this
/// test fails loudly — which is the point: the ceiling is lifted deliberately,
/// not drifted past.
///
/// [ADR-54]: ../../docs/specs/architecture/decisions/ADR-54.md
/// [NFR-MA-01]: ../../docs/specs/requirements/NFR-MA-01.md
/// [S-346]: ../../docs/planning/journal.md#s-346-c-http-client-call-capture
#[test]
fn a_named_constant_verb_is_a_stated_ceiling_not_a_capture() {
    let facts = extract_go(
        r#"package client

import "net/http"

func GetUser(c *http.Client) {
	req, _ := http.NewRequest(http.MethodGet, "/users", nil)
	c.Do(req)
}
"#,
    );
    assert!(
        client_call_targets(&facts).is_empty(),
        "a named-constant verb is honestly uncaptured, never guessed: {:?}",
        client_call_targets(&facts)
    );
}

// ── Provider registrations must never be read as outbound calls ─────────────

/// A Gin service that also imports `net/http` (for `http.StatusOK`) is a
/// *provider*. Its own route registrations — `r.GET("/users", h)`,
/// `r.Handle("GET", "/users", h)` — are the same shape as a client call and must
/// NOT be captured as outbound: doing so would turn every Gin service's provider
/// surface into phantom consumer calls that bind other members' routes
/// ([NFR-RA-05]).
#[test]
fn a_route_registration_in_a_net_http_file_is_not_read_as_an_outbound_call() {
    let facts = extract_go(
        r#"package main

import (
	"net/http"

	"github.com/gin-gonic/gin"
)

func listUsers(c *gin.Context) { c.JSON(http.StatusOK, nil) }

func main() {
	r := gin.Default()
	r.GET("/users", listUsers)
	r.POST("/users", listUsers)
	r.Handle("GET", "/orders/{id}", listUsers)
	r.GET("/legacy", func(c *gin.Context) {})
	http.Get("/probe")
}
"#,
    );
    // The `http.Get` is the positive control: it proves the file WAS scanned, so
    // the four registrations above are empty because the query's discriminators
    // rejected them — not because a later tightening of `http_client_detectors`
    // quietly closed the gate on the whole fixture.
    assert_eq!(
        client_call_targets(&facts),
        vec!["GET /probe".to_string()],
        "a provider's own registrations are never outbound calls, and the file \
         was genuinely scanned: {:?}",
        client_call_targets(&facts)
    );
}

/// The BOUND-RESULT twin of the test above, and the one an
/// "a registration discards its result" discriminator gets wrong. Gin's
/// `Handle` returns `gin.IRoutes` and Echo's `Add` returns `*echo.Route`, so
/// naming or returning a route is idiomatic — and a value-position check alone
/// would read every one of these as an outbound call ([NFR-RA-05]).
#[test]
fn a_route_registration_bound_to_a_value_is_not_read_as_an_outbound_call() {
    let facts = extract_go(
        r#"package main

import (
	"net/http"

	"github.com/gin-gonic/gin"
)

func listUsers(c *gin.Context) { c.JSON(http.StatusOK, nil) }

func mount(r *gin.Engine) gin.IRoutes {
	routes := r.Handle("GET", "/orders/{id}", listUsers)
	_ = routes
	return r.Handle("POST", "/orders", listUsers)
}

func probe() { http.Get("/probe") }
"#,
    );
    // Positive control — see the sibling test above.
    assert_eq!(
        client_call_targets(&facts),
        vec!["GET /probe".to_string()],
        "a registration whose result is bound or returned is still not an \
         outbound call, and the file was genuinely scanned: {:?}",
        client_call_targets(&facts)
    );
}

/// `httptest.NewRequest` builds an **inbound** request for testing a handler
/// in-process — it is a provider's own test idiom, not an outbound call.
/// It is named `NewRequest`, sits in a value position, and lives in a file that
/// imports `net/http/httptest`, so only pinning the callee's PACKAGE keeps it
/// out. Capturing it would make every provider's handler tests bind another
/// member's matching route ([NFR-RA-05]).
#[test]
fn an_httptest_new_request_is_not_read_as_an_outbound_call() {
    let facts = extract_go(
        r#"package handlers

import (
	"net/http"
	"net/http/httptest"
	"testing"
)

func ListUsers(w http.ResponseWriter, r *http.Request) {}

func TestListUsers(t *testing.T) {
	req := httptest.NewRequest("GET", "/users", nil)
	rr := httptest.NewRecorder()
	ListUsers(rr, req)
	http.Get("/probe")
}
"#,
    );
    // Positive control — see `a_route_registration_in_a_net_http_file_…`.
    assert_eq!(
        client_call_targets(&facts),
        vec!["GET /probe".to_string()],
        "a server-side test request is never an outbound call, and the file was \
         genuinely scanned: {:?}",
        client_call_targets(&facts)
    );
}

/// An adjacent `("VERB", "/path")` string pair in an unrelated call is not a
/// request constructor. Without the callee predicate the constructor pattern
/// slides across every adjacent argument pair, so `exec.Command` captured a
/// phantom `GET /v1/ping`.
#[test]
fn an_adjacent_verb_path_pair_in_an_unrelated_call_is_not_captured() {
    let facts = extract_go(
        r#"package client

import (
	"net/http"
	"os/exec"
)

func Probe(c *http.Client) {
	cmd := exec.Command("curl", "-X", "GET", "/v1/ping")
	_ = cmd
	http.Get("/probe")
}
"#,
    );
    // Positive control — see `a_route_registration_in_a_net_http_file_…`.
    assert_eq!(
        client_call_targets(&facts),
        vec!["GET /probe".to_string()],
        "only http.NewRequest/NewRequestWithContext anchor the constructor \
         form, and the file was genuinely scanned: {:?}",
        client_call_targets(&facts)
    );
}

/// The candidacy gate is scoped to `net::http` at a segment boundary, so a
/// sibling `net/url` import does NOT make a file a client candidate. This is
/// [FR-WS-08] shared negative case 1 at its real boundary: the earlier `net`
/// root admitted every `net/*` importer, and a plain `cache.Get("/admin/users")`
/// there cleared the verb and static-absolute-path filters and was captured.
#[test]
fn a_sibling_net_package_import_does_not_make_a_file_a_client_candidate() {
    let facts = extract_go(
        r#"package cfg

import "net/url"

type Cache struct{}

func (c Cache) Get(k string) string { return k }

func Load(cache Cache) {
	_ = url.QueryEscape("x")
	_ = cache.Get("/admin/users")
}
"#,
    );
    assert!(
        client_call_targets(&facts).is_empty(),
        "only a net/http reference opens the arm, not any net/* package: {:?}",
        client_call_targets(&facts)
    );
}

/// A path literal ending in a quote or `#` keeps those characters. The literal
/// node is handed to `static_string_literal` whole for exactly this reason —
/// passing its already-unquoted content child instead took a fallback that
/// trims those characters, silently turning `/tag/#` into `/tag/`: a WRONG
/// template, not an absent one.
#[test]
fn a_path_literal_ending_in_a_trimmed_character_is_not_corrupted() {
    let facts = extract_go(
        r#"package client

import "net/http"

func Tags() { http.Get("/tag/#") }
"#,
    );
    assert_eq!(
        client_call_targets(&facts),
        vec!["GET /tag/#".to_string()],
        "the trailing `#` survives — a trimmed path would be a different route"
    );
}

/// The constructor form reads a raw-string path too, not only the interpreted
/// form the other constructor tests use.
#[test]
fn the_constructor_form_reads_a_raw_string_path() {
    let facts = extract_go(
        r#"package client

import "net/http"

func GetUser(c *http.Client) {
	req, _ := http.NewRequest("GET", `/users/{id}`, nil)
	c.Do(req)
}
"#,
    );
    assert_eq!(
        client_call_targets(&facts),
        vec!["GET /users/{id}".to_string()]
    );
}
