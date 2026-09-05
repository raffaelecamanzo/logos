; Python HTTP client-call capture (S-344, capability = "invocations",
; FR-WS-08, CR-108). `requests` and `httpx` — FR-WS-08's normative Python row.
;
; Capture vocabulary (interpreted by extract::collect_invocation_sites):
;
;   @invoke.http.method — a node whose TEXT is the verb. Kept only when it is
;                          one of the HTTP verbs (case-insensitive).
;   @invoke.http.arg    — the request path. Kept as a `"METHOD /template"`
;                          reference only when it is a static string literal;
;                          an f-string, a `%`-formatted or `.format()`-composed
;                          string, or a bare variable is refused as
;                          base-url-runtime, never approximately matched
;                          (NFR-RA-05).
;
; The free-function anchor question this story consumes rather than re-derives
; (S-343, CR-108): the generic dispatch is receiver-agnostic — it reads capture
; names, not call shape — so a module-qualified free function
; (`requests.get(…)`) needs no structural support beyond an ordinary
; `<object>.<attr>(…)` pattern. Python is the only language in this sprint
; exercising BOTH anchor shapes at once: the free-function form below and the
; session/client receiver form.
;
; ── Why every pattern is anchored to a NAMED client ──────────────────────────
;
; Rust's `invocations.scm` ships one broad `<receiver>.<method>(<arg>)` anchor
; with no receiver-name restriction, relying on the file-level
; `http_client_detectors` gate plus `is_http_method` to bound it. That anchor is
; deliberately NOT used here, for two reasons specific to Python:
;
;   1. Python's own most common false-positive shape is a same-shaped
;      dict/cache lookup (`cache.get("k")`, [FR-WS-08]'s shared negative-case
;      1) — an unnamed receiver anchor would need the file-level gate alone to
;      keep it out, and the gate is file-, not receiver-, grained.
;   2. FAR more load-bearing: FastAPI and (since Flask 2.0) Flask both ship a
;      route-REGISTRATION API whose shape is indistinguishable from a client
;      CALL — `@app.get("/users")` and `@bp.get("/users")` decorators parse to
;      exactly the `(call function: (attribute … attribute: (identifier)
;      "get")) arguments: (argument_list (string)))` shape a
;      `session.get("/users")` client call does; only the surrounding
;      `decorator` node differs, and a tree-sitter pattern anchored on the call
;      alone cannot see that ancestor. A microservice handler routinely calls
;      an upstream over `requests`/`httpx` from inside a file that ALSO
;      registers routes with `app.get`/`app.post`, so the two shapes coexist
;      inside one client-gated file far more often than not — this is exactly
;      the "registration is syntactically a client call" trap S-345 hit on the
;      Go provider side (Gin's `r.GET(…)`), reopened here on the consumer side
;      if the receiver is left unconstrained.
;
; Anchoring the free-function form to the exact module names (`requests`,
; `httpx`) and the receiver form to a `session`/`client` naming convention
; sidesteps both: neither `app`, `bp`, `router` nor a generic `cache`/`d`
; receiver ever matches, so the FastAPI/Flask decorator and the dict lookup are
; excluded by construction, with no structural "not inside a decorator"
; predicate required. Pinned by
; `a_fastapi_route_decorator_is_never_captured_as_a_client_call`,
; `a_flask_blueprint_route_decorator_is_never_captured_as_a_client_call` and
; `a_route_shaped_dict_get_inside_a_client_file_is_still_refused` in
; `tests/python_invocations.rs`.
;
; Ceiling (stated, not worked around): a renamed/aliased import
; (`import requests as req`) or a session/client instance bound through a
; receiver name outside the convention (`api = requests.Session()`) stays
; honestly uncaptured — the same posture `references.scm` already states for
; import aliasing, and the same "conventional NAME only" ceiling the
; `typescript`/`tsx` `axios` anchor accepts. A renamed receiver never widens the
; anchor to every identifier in the file, which is the false-positive class
; this file exists not to reopen (NFR-RA-05).
;
; Droppable on disk at `.logos/plugins/python/queries/invocations.scm`
; (FR-PL-04, FR-PL-05).

; ── requests/httpx, module-level free-function form ─────────────────────────
; `requests.get("/users")`, `httpx.post("/users", json=body)`. The receiver is
; the imported module name itself — an exact match, not a boundary regex,
; since neither name is ever a legitimate local variable for anything else.
((call
   function: (attribute
     object: (identifier) @_module
     attribute: (identifier) @invoke.http.method)
   arguments: (argument_list
     .
     (_) @invoke.http.arg))
  (#any-of? @_module "requests" "httpx"))

; ── session/client receiver form ─────────────────────────────────────────────
; `session.get("/users")`, `self.client.post("/users", json=body)`,
; `await client.get("/users")` — an `await` wrapper leaves the `call` node
; itself unchanged, so no separate async pattern is needed.
;
; The name rule is a BOUNDARY rule, never a substring test. Python's
; convention is snake_case, not camelCase, so the boundary is an underscore or
; a dot rather than a case change:
;
;   * ATTACHED PREFIX — the segment STARTS with `session`/`Session`/`client`/
;     `Client`: `session`, `session_pool`, `client_wrapper`. No separator is
;     required after the word, mirroring the `axios` anchor's own attached-
;     prefix half (`axiosClient`).
;   * SEPARATED SUFFIX — the segment ENDS with `_session`/`.session`/
;     `_client`/`.client`, the `_`/`.` immediately before it standing in for
;     the case-change boundary a camelCase language would use: `http_session`,
;     `api_client`, `self.client`.
;
; A bare attached suffix with NO separator (`notasession`) satisfies neither
; half — "session" is not a leading substring, and nothing but a plain letter
; precedes its trailing occurrence — so it is excluded, the same
; false-positive class the `axios` boundary rule exists not to reopen
; (NFR-RA-05). An all-caps `SESSION`/`CLIENT` is deliberately outside the set,
; matching the `axios` anchor's posture.
;
; Stated ceiling, not worked around: Flask's own test client
; (`client = app.test_client(); client.get("/users")`) matches this anchor even
; though it drives an INBOUND request against the app under test, not an
; outbound one — the receiver name convention that correctly excludes
; `app`/`bp` gives no signal here, since the test client is idiomatically named
; `client` too. No query can distinguish the two without type information this
; extractor does not have; a test file rarely also becomes a bridge-bound
; workspace member, so the residual risk is low ([ADR-54]).
((call
   function: (attribute
     object: [(identifier) (attribute)] @_recv
     attribute: (identifier) @invoke.http.method)
   arguments: (argument_list
     .
     (_) @invoke.http.arg))
  (#match? @_recv "^([Ss]ession|[Cc]lient)[A-Za-z0-9_]*$|(^|[_.])([Ss]ession|[Cc]lient)$"))

; ── inline construction-and-call chain ───────────────────────────────────────
; `httpx.Client().get("/users")`, `httpx.AsyncClient().get("/users")`,
; `requests.Session().get("/users")` — the receiver is itself a `call`
; (the constructor), so the boundary regex above (anchored to an identifier or
; attribute node's own text) cannot reach it: a separate pattern applies the
; same boundary rule to the constructor's callee instead of the whole
; `object` node's text (which would carry a trailing `()` the regex could never
; anchor past).
((call
   function: (attribute
     object: (call
       function: [(identifier) (attribute)] @_ctor)
     attribute: (identifier) @invoke.http.method)
   arguments: (argument_list
     .
     (_) @invoke.http.arg))
  (#match? @_ctor "(^|\\.)([Ss]ession|[Cc]lient|Async[Cc]lient)$"))
